//! Polling feed (O-12): while a room screen is open its snapshot is fetched every 5 seconds, and
//! while no room is open only the room list (and, if the user opted in, friends' distraction
//! notices) every 30 seconds. A failed poll keeps the last good snapshot and reports a degraded
//! code instead of blanking the screen. Realtime is an optional later adapter behind the same
//! `RoomFeed` port.
//!
//! The feed is a state machine driven by `tick(now_ms, source)`; the thread that calls it lives in
//! `service`. The lock is never held across a network call.

use std::sync::{Arc, Mutex, MutexGuard};

use super::model::{RoomError, RoomSummary, WireDistraction, WireSnapshot};
use super::port::RoomApi;

pub const OPEN_POLL_MS: i64 = 5_000;
pub const CLOSED_POLL_MS: i64 = 30_000;

pub fn poll_interval_ms(room_open: bool) -> i64 {
    if room_open { OPEN_POLL_MS } else { CLOSED_POLL_MS }
}

pub trait FeedSource: Send + Sync {
    /// `None` when signed out or not configured.
    fn api(&self) -> Option<Arc<dyn RoomApi>>;
    /// Whether this user turned friends' distraction notices on.
    fn receive_enabled(&self) -> bool;
    /// A fresh room list, to be cached locally.
    fn on_rooms(&self, rooms: &[RoomSummary], now_ms: i64);
}

#[derive(Default)]
struct Inner {
    open_room: Option<String>,
    snapshot: Option<(WireSnapshot, i64)>,
    degraded: Option<&'static str>,
    last_ok_ms: Option<i64>,
    next_due_ms: i64,
    distraction_cursor: i64,
}

/// What `view` returns: the data as last fetched, and the health of the latest poll.
pub struct FeedState {
    pub snapshot: Option<(WireSnapshot, i64)>,
    pub degraded_code: Option<&'static str>,
    pub last_ok_ms: Option<i64>,
    pub poll_interval_ms: i64,
}

#[derive(Default)]
pub struct PollingFeed {
    inner: Mutex<Inner>,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct TickResult {
    /// A room snapshot or room list was requested this tick.
    pub polled: bool,
    pub notices: Vec<WireDistraction>,
}

impl PollingFeed {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// The room screen opened (`Some`) or closed (`None`). Opening asks for an immediate poll.
    pub fn set_open(&self, room_id: Option<String>, now_ms: i64) {
        let mut inner = self.lock();
        if inner.open_room != room_id {
            inner.snapshot = None;
            inner.degraded = None;
        }
        if room_id.is_some() {
            inner.next_due_ms = now_ms;
        }
        inner.open_room = room_id;
    }

    /// A realtime notice arrived: the next tick polls at once instead of waiting for the schedule.
    pub fn poll_now(&self, now_ms: i64) {
        self.lock().next_due_ms = now_ms;
    }

    pub fn open_room(&self) -> Option<String> {
        self.lock().open_room.clone()
    }

    pub fn state(&self) -> FeedState {
        let inner = self.lock();
        FeedState {
            snapshot: inner.snapshot.clone(),
            degraded_code: inner.degraded,
            last_ok_ms: inner.last_ok_ms,
            poll_interval_ms: poll_interval_ms(inner.open_room.is_some()),
        }
    }

    pub fn tick(&self, now_ms: i64, source: &dyn FeedSource) -> TickResult {
        let open = {
            let inner = self.lock();
            if now_ms < inner.next_due_ms {
                return TickResult::default();
            }
            inner.open_room.clone()
        };
        let Some(api) = source.api() else {
            let mut inner = self.lock();
            inner.next_due_ms = now_ms + poll_interval_ms(open.is_some());
            return TickResult::default();
        };

        let mut result = TickResult { polled: true, notices: Vec::new() };
        match &open {
            Some(room_id) => match api.snapshot(room_id) {
                Ok(snapshot) => {
                    let mut inner = self.lock();
                    if inner.open_room.as_deref() == Some(room_id.as_str()) {
                        inner.snapshot = Some((snapshot, now_ms));
                        inner.degraded = None;
                        inner.last_ok_ms = Some(now_ms);
                    }
                }
                Err(err) => self.fail(err),
            },
            None => match api.list_rooms() {
                Ok(rooms) => {
                    source.on_rooms(&rooms, now_ms);
                    let mut inner = self.lock();
                    inner.degraded = None;
                    inner.last_ok_ms = Some(now_ms);
                }
                Err(err) => self.fail(err),
            },
        }

        if source.receive_enabled() {
            let since = {
                let mut inner = self.lock();
                if inner.distraction_cursor == 0 {
                    // Start from now: notices from before the user turned this on are not replayed.
                    inner.distraction_cursor = now_ms;
                }
                inner.distraction_cursor
            };
            if let Ok(events) = api.poll_distractions(since) {
                let newest = events.iter().map(|e| e.at_ms).max();
                if let Some(newest) = newest {
                    self.lock().distraction_cursor = newest;
                }
                result.notices = events;
            }
        } else {
            self.lock().distraction_cursor = 0;
        }

        self.lock().next_due_ms = now_ms + poll_interval_ms(open.is_some());
        result
    }

    fn fail(&self, err: RoomError) {
        let mut inner = self.lock();
        inner.degraded = Some(err.code());
        if matches!(err, RoomError::Rejected(super::model::RoomCode::NotMember)) {
            // The member was removed: the open room no longer exists for this user.
            inner.open_room = None;
            inner.snapshot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::rooms::port::{RoomFeed, RoomReporter, RoomService};
    use crate::rooms::prefs::FriendSettings;
    use crate::rooms::testkit::{Person, World, T0};
    use crate::remote::Clock;
    use crate::rooms::view::{presence, OFFLINE_AFTER_SEC};
    use crate::remote::payload::StatusReportArgs;
    use crate::rooms::model::{PresenceKind, RoomCode};

    struct Source {
        api: Option<Arc<dyn RoomApi>>,
        receive: bool,
        listed: AtomicUsize,
    }

    impl FeedSource for Source {
        fn api(&self) -> Option<Arc<dyn RoomApi>> {
            self.api.clone()
        }
        fn receive_enabled(&self) -> bool {
            self.receive
        }
        fn on_rooms(&self, _rooms: &[RoomSummary], _now_ms: i64) {
            self.listed.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn source(person: &Person, receive: bool) -> Source {
        let api: Arc<dyn RoomApi> = person.rooms.clone();
        Source { api: Some(api), receive, listed: AtomicUsize::new(0) }
    }

    fn studying(remaining: i64) -> StatusReportArgs {
        StatusReportArgs { state: "studying", remaining_sec: remaining, level: 3, attempts_today: None }
    }

    /// Two people in one room; returns the room id.
    fn pair(world: &World) -> (Person, Person, String) {
        let alice = world.person("a@example.test");
        let bob = world.person("b@example.test");
        let room = alice.rooms.create_room("스터디", "앨리스").expect("room");
        let invite = alice.rooms.create_invite(&room, 24, 10).expect("invite");
        bob.rooms.join_room(&invite.invite_code, "밥").expect("join");
        (alice, bob, room)
    }

    #[test]
    fn an_open_room_is_polled_every_five_seconds_and_a_closed_one_every_thirty() {
        let world = World::new();
        let (alice, _bob, room) = pair(&world);
        let src = source(&alice, false);
        let feed = PollingFeed::new();
        feed.set_open(Some(room), T0);

        let calls = |world: &World| world.recorder.bodies_of("room_snapshot").len();
        let mut polls = 0;
        for second in 0..=20 {
            if feed.tick(T0 + second * 1_000, &src).polled {
                polls += 1;
            }
        }
        assert_eq!(polls, 5, "t = 0, 5, 10, 15, 20 s");
        assert_eq!(calls(&world), 5);

        feed.set_open(None, T0 + 20_000);
        let before = calls(&world);
        let mut closed_polls = 0;
        for second in 21..=80 {
            if feed.tick(T0 + second * 1_000, &src).polled {
                closed_polls += 1;
            }
        }
        assert_eq!(calls(&world), before, "no snapshot is requested while no room is open");
        assert_eq!(closed_polls, 2, "the room list every 30 s: at 25 s and 55 s after the last open poll");
        assert_eq!(src.listed.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_friends_status_change_shows_within_one_poll_interval_plus_a_second() {
        let world = World::new();
        let (alice, bob, room) = pair(&world);
        let src = source(&alice, false);
        let feed = PollingFeed::new();
        feed.set_open(Some(room), T0);
        feed.tick(T0, &src);

        // Bob starts studying right after Alice's poll; the server stamps its own time.
        world.clock.set(T0 + 1_000);
        bob.rooms.report_status(&studying(1_800)).expect("status");

        let mut seen_at = None;
        for second in 1..=10 {
            let now = T0 + second * 1_000;
            world.clock.set(now);
            feed.tick(now, &src);
            let snapshot = feed.state().snapshot.expect("snapshot").0;
            let bob_view = snapshot.members.iter().find(|m| m.user_id == bob.user_id).expect("bob");
            if bob_view.status.is_some() {
                seen_at = Some(now - (T0 + 1_000));
                break;
            }
        }
        let latency = seen_at.expect("visible");
        assert!(latency <= OPEN_POLL_MS + 1_000, "latency {latency} ms");
    }

    #[test]
    fn a_member_with_no_report_for_ninety_seconds_is_offline() {
        let world = World::new();
        let (alice, bob, room) = pair(&world);
        bob.rooms.report_status(&studying(3_000)).expect("status");
        let src = source(&alice, false);
        let feed = PollingFeed::new();
        feed.set_open(Some(room), T0);

        for (advance_ms, expect) in [(0, PresenceKind::Studying), (60_000, PresenceKind::Studying), (31_000, PresenceKind::Offline)] {
            world.clock.advance(advance_ms);
            let now = world.clock.now_ms().expect("now");
            feed.set_open(feed.open_room(), now);
            feed.tick(now, &src);
            let (snapshot, fetched) = feed.state().snapshot.expect("snapshot");
            let status = snapshot.members.iter().find(|m| m.user_id == bob.user_id).and_then(|m| m.status.clone());
            assert_eq!(presence(status.as_ref(), now - fetched).kind, expect, "after +{advance_ms} ms");
        }
        assert_eq!(OFFLINE_AFTER_SEC, 90);
    }

    #[test]
    fn a_failed_poll_keeps_the_last_good_data_and_reports_degraded_until_it_recovers() {
        let world = World::new();
        let (alice, _bob, room) = pair(&world);
        let src = source(&alice, false);
        let feed = PollingFeed::new();
        feed.set_open(Some(room), T0);
        feed.tick(T0, &src);
        assert!(feed.state().snapshot.is_some() && feed.state().degraded_code.is_none());

        world.backend.set_outage(true);
        feed.tick(T0 + 5_000, &src);
        let state = feed.state();
        assert!(state.snapshot.is_some(), "the last good snapshot stays");
        assert!(state.degraded_code.is_some());
        assert_eq!(state.last_ok_ms, Some(T0));

        world.backend.set_outage(false);
        feed.tick(T0 + 10_000, &src);
        let state = feed.state();
        assert_eq!((state.degraded_code, state.last_ok_ms), (None, Some(T0 + 10_000)));
    }

    #[test]
    fn being_removed_from_the_open_room_closes_it() {
        let world = World::new();
        let (alice, bob, room) = pair(&world);
        let src = source(&bob, false);
        let feed = PollingFeed::new();
        feed.set_open(Some(room.clone()), T0);
        feed.tick(T0, &src);
        alice.rooms.kick_member(&room, &bob.user_id).expect("kick");
        feed.tick(T0 + 5_000, &src);
        let state = feed.state();
        assert_eq!(state.degraded_code, Some(RoomCode::NotMember.as_str()));
        assert!(state.snapshot.is_none() && feed.open_room().is_none());
    }

    #[test]
    fn distraction_notices_arrive_only_for_a_receiver_who_opted_in_and_are_not_replayed() {
        let world = World::new();
        let (alice, bob, _room) = pair(&world);
        let on = FriendSettings {
            share_level: true,
            share_status: true,
            share_week_time: false,
            share_attempts: false,
            distraction_publish: true,
            distraction_receive: true,
        };
        bob.rooms.set_prefs(&on.as_args()).expect("bob prefs");
        alice.rooms.set_prefs(&FriendSettings { distraction_publish: false, ..on }.as_args()).expect("alice prefs");

        let feed = PollingFeed::new();
        let alice_src = source(&alice, true);
        feed.tick(T0, &alice_src);
        world.clock.set(T0 + 2_000);
        bob.rooms.report_distraction().expect("event");

        world.clock.set(T0 + 31_000);
        let got = feed.tick(T0 + 31_000, &alice_src);
        assert_eq!(got.notices.len(), 1);
        assert_eq!(got.notices[0].nickname.as_deref(), Some("밥"));
        let again = feed.tick(T0 + 61_000, &alice_src);
        assert!(again.notices.is_empty(), "the cursor moved past the event");

        // Alice turned receiving off: even though Bob publishes, nothing is delivered.
        alice.rooms.set_prefs(&FriendSettings { distraction_receive: false, ..on }.as_args()).expect("prefs");
        world.clock.set(T0 + 70_000);
        bob.rooms.report_distraction().expect("event");
        assert!(alice.rooms.poll_distractions(0).expect("poll").is_empty());
    }
}
