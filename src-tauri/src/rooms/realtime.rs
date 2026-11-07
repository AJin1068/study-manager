//! Optional Realtime adapter (S4.12): a transport layer behind the same room feed.
//!
//! Polling (`feed`) stays the default and the source of truth. Realtime only says "something in
//! this room changed"; on such a message the feed polls at once through the ordinary RPCs, so row
//! contents that arrive over the socket are never read, and RLS on the RPCs still decides what the
//! user sees. If the socket is down the feed keeps polling on its normal schedule.
//!
//! This module holds the Phoenix-channel wire codec (`Frame`), the session state machine
//! (`RealtimeSession`) and the `RealtimeTransport` port. No websocket crate is part of the project,
//! so the only transports are the in-memory pair used by the tests; a socket-backed transport is a
//! later addition behind the same trait (V4-5: a real connection is not verified).

// Nothing in the app constructs a session yet (no socket transport exists), so the items are only
// reached from tests until one is added.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Value, json};

use super::feed::PollingFeed;

/// Frames larger than this are dropped: a changed-row notice is small.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// Phoenix expects a heartbeat about every 30 seconds.
pub const HEARTBEAT_MS: i64 = 30_000;
/// No frame at all for this long: the connection is treated as lost.
pub const SILENCE_LIMIT_MS: i64 = 65_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RealtimeError {
    /// The frame is not a Phoenix message this client understands.
    Malformed,
    TooLarge,
    /// The transport is closed.
    Closed,
}

/// One Phoenix v1 message: `{"topic", "event", "payload", "ref"}`.
#[derive(Clone, PartialEq)]
pub struct Frame {
    pub topic: String,
    pub event: String,
    pub payload: Value,
    pub msg_ref: Option<String>,
}

/// The payload can carry the access token, so it is never printed.
impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}, {})", self.topic, self.event)
    }
}

impl Frame {
    pub fn encode(&self) -> String {
        json!({ "topic": self.topic, "event": self.event, "payload": self.payload, "ref": self.msg_ref }).to_string()
    }

    pub fn decode(text: &str) -> Result<Frame, RealtimeError> {
        if text.len() > MAX_FRAME_BYTES {
            return Err(RealtimeError::TooLarge);
        }
        let value: Value = serde_json::from_str(text).map_err(|_| RealtimeError::Malformed)?;
        let object = value.as_object().ok_or(RealtimeError::Malformed)?;
        let text_field = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or(RealtimeError::Malformed)
        };
        let msg_ref = match object.get("ref") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(RealtimeError::Malformed),
        };
        Ok(Frame {
            topic: text_field("topic")?,
            event: text_field("event")?,
            payload: object.get("payload").cloned().unwrap_or(Value::Null),
            msg_ref,
        })
    }
}

/// The channel topic of a room.
pub fn room_topic(room_id: &str) -> String {
    format!("realtime:room:{room_id}")
}

fn valid_room_id(room_id: &str) -> bool {
    (8..=64).contains(&room_id.len())
        && room_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Joins a room's channel. Subscribes to row changes of the member list (filtered to the room) and
/// of member status (the server's row policy limits it to people the user may see).
pub fn join_frame(
    room_id: &str,
    access_token: &str,
    msg_ref: &str,
) -> Result<Frame, RealtimeError> {
    if !valid_room_id(room_id) {
        return Err(RealtimeError::Malformed);
    }
    let payload = json!({
        "config": {
            "broadcast": { "self": false },
            "presence": { "key": "" },
            "postgres_changes": [
                { "event": "*", "schema": "public", "table": "room_members", "filter": format!("room_id=eq.{room_id}") },
                { "event": "*", "schema": "public", "table": "member_status" },
            ],
        },
        "access_token": access_token,
    });
    Ok(Frame {
        topic: room_topic(room_id),
        event: "phx_join".into(),
        payload,
        msg_ref: Some(msg_ref.into()),
    })
}

pub fn leave_frame(room_id: &str, msg_ref: &str) -> Frame {
    Frame {
        topic: room_topic(room_id),
        event: "phx_leave".into(),
        payload: json!({}),
        msg_ref: Some(msg_ref.into()),
    }
}

pub fn heartbeat_frame(msg_ref: &str) -> Frame {
    Frame {
        topic: "phoenix".into(),
        event: "heartbeat".into(),
        payload: json!({}),
        msg_ref: Some(msg_ref.into()),
    }
}

/// What a session reports to the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RealtimeEvent {
    /// The channel was accepted by the server.
    Joined,
    /// A row in the room changed: fetch the room again.
    RoomChanged,
    /// The connection or channel is gone; polling carries on alone.
    Lost,
}

/// A text-frame connection. `receive` never blocks: `Ok(None)` means nothing arrived yet.
pub trait RealtimeTransport: Send + Sync {
    fn send(&self, text: &str) -> Result<(), RealtimeError>;
    fn receive(&self) -> Result<Option<String>, RealtimeError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Joining,
    Joined,
    Lost,
}

pub struct RealtimeSession<T: RealtimeTransport> {
    transport: T,
    state: SessionState,
    room_id: Option<String>,
    counter: u64,
    join_ref: Option<String>,
    last_heard_ms: i64,
    last_heartbeat_ms: i64,
}

impl<T: RealtimeTransport> RealtimeSession<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            state: SessionState::Idle,
            room_id: None,
            counter: 0,
            join_ref: None,
            last_heard_ms: 0,
            last_heartbeat_ms: 0,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    fn next_ref(&mut self) -> String {
        self.counter += 1;
        self.counter.to_string()
    }

    fn lose(&mut self, events: &mut Vec<RealtimeEvent>) {
        if self.state != SessionState::Lost {
            self.state = SessionState::Lost;
            events.push(RealtimeEvent::Lost);
        }
    }

    /// Joins `room_id`'s channel (leaving the previous one). `access_token` is the user's session
    /// token; it goes only into the join frame.
    pub fn open(
        &mut self,
        room_id: &str,
        access_token: &str,
        now_ms: i64,
    ) -> Result<(), RealtimeError> {
        self.close();
        let msg_ref = self.next_ref();
        let frame = join_frame(room_id, access_token, &msg_ref)?;
        self.transport.send(&frame.encode())?;
        self.join_ref = Some(msg_ref);
        self.room_id = Some(room_id.to_string());
        self.state = SessionState::Joining;
        self.last_heard_ms = now_ms;
        self.last_heartbeat_ms = now_ms;
        Ok(())
    }

    /// Leaves the channel (best effort).
    pub fn close(&mut self) {
        if let Some(room_id) = self.room_id.take() {
            let msg_ref = self.next_ref();
            let _ = self
                .transport
                .send(&leave_frame(&room_id, &msg_ref).encode());
        }
        self.join_ref = None;
        self.state = SessionState::Idle;
    }

    /// Reads what arrived, answers the heartbeat schedule, and reports what happened.
    pub fn pump(&mut self, now_ms: i64) -> Vec<RealtimeEvent> {
        let mut events = Vec::new();
        if matches!(self.state, SessionState::Idle | SessionState::Lost) {
            return events;
        }
        loop {
            match self.transport.receive() {
                Ok(Some(text)) => {
                    self.last_heard_ms = now_ms;
                    // A bad frame is skipped: one odd message must not end the session.
                    if let Ok(frame) = Frame::decode(&text) {
                        self.handle(&frame, &mut events);
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    self.lose(&mut events);
                    return events;
                }
            }
        }
        if self.state == SessionState::Lost {
            return events;
        }
        if now_ms - self.last_heard_ms > SILENCE_LIMIT_MS {
            self.lose(&mut events);
        } else if now_ms - self.last_heartbeat_ms >= HEARTBEAT_MS {
            self.last_heartbeat_ms = now_ms;
            let msg_ref = self.next_ref();
            if self
                .transport
                .send(&heartbeat_frame(&msg_ref).encode())
                .is_err()
            {
                self.lose(&mut events);
            }
        }
        events
    }

    fn handle(&mut self, frame: &Frame, events: &mut Vec<RealtimeEvent>) {
        let Some(room_id) = self.room_id.clone() else {
            return;
        };
        if frame.topic != room_topic(&room_id) {
            return; // heartbeat replies and other channels
        }
        match frame.event.as_str() {
            "phx_reply" if frame.msg_ref.is_some() && frame.msg_ref == self.join_ref => {
                if frame.payload.get("status").and_then(Value::as_str) == Some("ok") {
                    self.state = SessionState::Joined;
                    events.push(RealtimeEvent::Joined);
                } else {
                    self.lose(events);
                }
            }
            "phx_close" | "phx_error" => self.lose(events),
            // The row contents are ignored on purpose; the feed refetches through the RPCs.
            "postgres_changes" | "broadcast" | "presence_diff"
                if self.state == SessionState::Joined =>
            {
                events.push(RealtimeEvent::RoomChanged)
            }
            _ => {}
        }
    }
}

/// Applies session events to the polling feed: a change makes the next tick poll immediately.
pub fn apply(events: &[RealtimeEvent], feed: &PollingFeed, now_ms: i64) {
    if events.contains(&RealtimeEvent::RoomChanged) {
        feed.poll_now(now_ms);
    }
}

// ---- in-memory transport (tests and mock mode) ----

#[derive(Default)]
struct Pipe {
    to_client: VecDeque<String>,
    to_server: VecDeque<String>,
    closed: bool,
}

fn lock(pipe: &Mutex<Pipe>) -> MutexGuard<'_, Pipe> {
    match pipe.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The client end of an in-memory connection.
#[derive(Clone)]
pub struct MemoryTransport(Arc<Mutex<Pipe>>);

/// The server end: reads what the client sent and pushes frames to it.
#[derive(Clone)]
pub struct MemoryServer(Arc<Mutex<Pipe>>);

pub fn memory_pair() -> (MemoryTransport, MemoryServer) {
    let pipe = Arc::new(Mutex::new(Pipe::default()));
    (MemoryTransport(Arc::clone(&pipe)), MemoryServer(pipe))
}

impl RealtimeTransport for MemoryTransport {
    fn send(&self, text: &str) -> Result<(), RealtimeError> {
        let mut pipe = lock(&self.0);
        if pipe.closed {
            return Err(RealtimeError::Closed);
        }
        pipe.to_server.push_back(text.to_string());
        Ok(())
    }

    fn receive(&self) -> Result<Option<String>, RealtimeError> {
        let mut pipe = lock(&self.0);
        match pipe.to_client.pop_front() {
            Some(text) => Ok(Some(text)),
            None if pipe.closed => Err(RealtimeError::Closed),
            None => Ok(None),
        }
    }
}

impl MemoryServer {
    /// Frames the client sent since the last call.
    pub fn take_received(&self) -> Vec<Frame> {
        lock(&self.0)
            .to_server
            .drain(..)
            .filter_map(|t| Frame::decode(&t).ok())
            .collect()
    }

    pub fn push(&self, frame: &Frame) {
        self.push_raw(&frame.encode());
    }

    pub fn push_raw(&self, text: &str) {
        lock(&self.0).to_client.push_back(text.to_string());
    }

    pub fn close(&self) {
        lock(&self.0).closed = true;
    }

    /// Answers a join the way the server does.
    pub fn accept_join(&self, join: &Frame) {
        self.push(&Frame {
            topic: join.topic.clone(),
            event: "phx_reply".into(),
            payload: json!({"status": "ok", "response": {}}),
            msg_ref: join.msg_ref.clone(),
        });
    }

    pub fn notify_change(&self, room_id: &str) {
        self.push(&Frame {
            topic: room_topic(room_id),
            event: "postgres_changes".into(),
            payload: json!({"data": {"table": "member_status", "type": "UPDATE", "record": {"state": "studying"}}}),
            msg_ref: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::feed::FeedSource;
    use crate::rooms::port::{RoomApi, RoomService};
    use crate::rooms::testkit::{T0, World};

    const ROOM: &str = "11111111-2222-4333-8444-555555555555";

    struct Live(Arc<dyn RoomApi>);
    impl FeedSource for Live {
        fn api(&self) -> Option<Arc<dyn RoomApi>> {
            Some(Arc::clone(&self.0))
        }
        fn receive_enabled(&self) -> bool {
            false
        }
        fn on_rooms(&self, _rooms: &[crate::rooms::model::RoomSummary], _now_ms: i64) {}
    }

    fn joined() -> (RealtimeSession<MemoryTransport>, MemoryServer) {
        let (client, server) = memory_pair();
        let mut session = RealtimeSession::new(client);
        session.open(ROOM, "token-secret", T0).expect("open");
        let join = server.take_received().remove(0);
        server.accept_join(&join);
        assert_eq!(session.pump(T0), vec![RealtimeEvent::Joined]);
        (session, server)
    }

    #[test]
    fn frames_round_trip_and_reject_malformed_input() {
        let frame = Frame {
            topic: "t".into(),
            event: "e".into(),
            payload: json!({"a": 1}),
            msg_ref: Some("7".into()),
        };
        assert_eq!(Frame::decode(&frame.encode()), Ok(frame));
        let no_ref = Frame::decode(r#"{"topic":"t","event":"e","payload":{},"ref":null}"#)
            .expect("null ref");
        assert_eq!(no_ref.msg_ref, None);
        for bad in [
            "",
            "[]",
            "{}",
            r#"{"topic":1,"event":"e"}"#,
            r#"{"topic":"t","event":"e","ref":5}"#,
            "not json",
        ] {
            assert_eq!(Frame::decode(bad), Err(RealtimeError::Malformed), "{bad}");
        }
        assert_eq!(
            Frame::decode(&" ".repeat(MAX_FRAME_BYTES + 1)),
            Err(RealtimeError::TooLarge)
        );
    }

    #[test]
    fn the_join_frame_names_the_room_filter_and_hides_the_token_from_debug() {
        let frame = join_frame(ROOM, "token-secret", "1").expect("join");
        assert_eq!(frame.topic, format!("realtime:room:{ROOM}"));
        let changes = &frame.payload["config"]["postgres_changes"];
        assert_eq!(changes[0]["table"], "room_members");
        assert_eq!(changes[0]["filter"], format!("room_id=eq.{ROOM}"));
        assert_eq!(frame.payload["access_token"], "token-secret");
        assert!(!format!("{frame:?}").contains("token-secret"));
        assert_eq!(
            join_frame("../x", "t", "1").map(|_| ()),
            Err(RealtimeError::Malformed)
        );
        assert_eq!(
            join_frame("a b c d e f g h", "t", "1").map(|_| ()),
            Err(RealtimeError::Malformed)
        );
    }

    #[test]
    fn a_change_notice_after_the_join_makes_the_feed_poll_now_and_before_it_does_not() {
        let world = World::new();
        let alice = world.person("a@example.test");
        let room = alice.rooms.create_room("스터디", "앨리스").expect("room");
        let api: Arc<dyn RoomApi> = alice.rooms.clone();
        let source = Live(api);

        let (client, server) = memory_pair();
        let mut session = RealtimeSession::new(client);
        session.open(&room, "t", T0).expect("open");
        server.notify_change(&room);
        assert!(session.pump(T0).is_empty(), "not joined yet");
        let join = server.take_received().remove(0);
        server.accept_join(&join);
        assert_eq!(session.pump(T0), vec![RealtimeEvent::Joined]);
        server.notify_change(&room);
        let events = session.pump(T0 + 1);
        assert_eq!(events, vec![RealtimeEvent::RoomChanged]);

        let feed = PollingFeed::new();
        feed.set_open(Some(room), T0);
        assert!(feed.tick(T0, &source).polled);
        assert!(!feed.tick(T0 + 1, &source).polled, "the schedule says wait");
        apply(&events, &feed, T0 + 1);
        assert!(
            feed.tick(T0 + 1, &source).polled,
            "the notice made the poll due"
        );
        assert!(feed.state().snapshot.is_some());
        // Without a change notice the schedule stays.
        apply(&[RealtimeEvent::Joined, RealtimeEvent::Lost], &feed, T0 + 2);
        assert!(!feed.tick(T0 + 2, &source).polled);
    }

    #[test]
    fn a_notice_for_another_room_or_a_garbled_frame_is_ignored() {
        let (mut session, server) = joined();
        server.notify_change("99999999-2222-4333-8444-555555555555");
        server.push_raw("garbage");
        server.push(&Frame {
            topic: "phoenix".into(),
            event: "phx_reply".into(),
            payload: json!({"status": "ok"}),
            msg_ref: Some("9".into()),
        });
        assert!(session.pump(T0 + 10).is_empty());
        assert_eq!(session.state(), SessionState::Joined);
    }

    #[test]
    fn a_refused_join_or_a_channel_close_ends_the_session_once() {
        let (client, server) = memory_pair();
        let mut session = RealtimeSession::new(client);
        session.open(ROOM, "t", T0).expect("open");
        let join = server.take_received().remove(0);
        server.push(&Frame {
            topic: join.topic.clone(),
            event: "phx_reply".into(),
            payload: json!({"status": "error"}),
            msg_ref: join.msg_ref.clone(),
        });
        assert_eq!(session.pump(T0), vec![RealtimeEvent::Lost]);
        assert!(session.pump(T0 + 1).is_empty());

        let (mut session, server) = joined();
        server.push(&Frame {
            topic: room_topic(ROOM),
            event: "phx_close".into(),
            payload: json!({}),
            msg_ref: None,
        });
        assert_eq!(session.pump(T0 + 1), vec![RealtimeEvent::Lost]);
        assert_eq!(session.state(), SessionState::Lost);
    }

    #[test]
    fn heartbeats_go_out_every_thirty_seconds_and_silence_ends_the_session() {
        let (mut session, server) = joined();
        assert!(session.pump(T0 + HEARTBEAT_MS - 1).is_empty());
        assert!(server.take_received().is_empty());
        session.pump(T0 + HEARTBEAT_MS);
        let sent = server.take_received();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            (sent[0].topic.as_str(), sent[0].event.as_str()),
            ("phoenix", "heartbeat")
        );
        assert_eq!(
            session.pump(T0 + SILENCE_LIMIT_MS + 1),
            vec![RealtimeEvent::Lost]
        );
    }

    #[test]
    fn a_closed_connection_is_lost_and_polling_is_unaffected() {
        let (mut session, server) = joined();
        server.close();
        assert_eq!(session.pump(T0 + 1), vec![RealtimeEvent::Lost]);
        let feed = PollingFeed::new();
        feed.set_open(Some(ROOM.to_string()), T0);
        apply(&[RealtimeEvent::Lost], &feed, T0 + 1);
        assert_eq!(feed.open_room().as_deref(), Some(ROOM));
    }

    #[test]
    fn switching_rooms_leaves_the_old_channel() {
        let (mut session, server) = joined();
        let other = "22222222-2222-4333-8444-555555555555";
        session.open(other, "t", T0 + 5).expect("open");
        let sent = server.take_received();
        assert_eq!(sent[0].event, "phx_leave");
        assert_eq!(sent[0].topic, room_topic(ROOM));
        assert_eq!(sent[1].event, "phx_join");
        assert_eq!(sent[1].topic, room_topic(other));
    }
}
