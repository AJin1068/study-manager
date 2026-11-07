//! The phone side of the `phone_app` notification channel (S5.7).
//!
//! The server posts missed-plan alerts to the user's ntfy topic (the same topic and server as the
//! `ntfy` channel; see `fixtures/notify/contract.json`, section `phoneApp`). The phone polls the
//! topic through `PushChannel` and shows each message as a local notification through
//! `LocalNotifier`. Adapters: `NtfyPush` (real, over the HTTP port) and `MockPush`/`MockNotifier`
//! (host tests).
//!
//! The notification shows on a lock screen, so the phone never displays server-supplied text as
//! it is: a message is shown only if it is one of the fixed phrases (the test message, or the
//! missed-plan sentence with a clock time). Anything else - a stranger who learned the topic, a
//! future server change - is shown as the generic phrase. A subject or plan name cannot appear.
//!
//! Polling is not streaming: delivery latency and battery-saver behavior on a real device are
//! unverified (V5-8).

use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::remote::http::{HttpRequest, HttpTransport, Method, RequestTag, TransportError};
use crate::remote::notify::TEST_MESSAGE;

pub const GENERIC_BODY: &str = "새 알림이 도착했어요.";
const MISSED_SUFFIX: &str = "인데 아직 안 앉았어! 신호가 없어요.";
/// Lines read from one poll; ntfy returns the backlog since the cursor.
const MAX_MESSAGES_PER_POLL: usize = 100;
const MAX_ID_LEN: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushCursor {
    /// Messages at or after this Unix time (seconds): the first poll after subscribing.
    Time(i64),
    /// Messages after this message id.
    After(String),
}

impl PushCursor {
    fn query_value(&self) -> String {
        match self {
            PushCursor::Time(secs) => secs.to_string(),
            PushCursor::After(id) => id.clone(),
        }
    }
}

/// One message from the topic. The text is what the server sent; it goes through `display_body`
/// before it is shown.
#[derive(Clone, PartialEq, Eq)]
pub struct PushMessage {
    pub id: String,
    pub text: String,
}

impl std::fmt::Debug for PushMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushMessage")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PushError {
    #[error("{0}")]
    Transport(TransportError),
    #[error("http_error")]
    Http { status: u16 },
    #[error("malformed")]
    Malformed,
}

impl PushError {
    pub fn code(self) -> &'static str {
        match self {
            PushError::Transport(err) => err.code(),
            PushError::Http { .. } => "http_error",
            PushError::Malformed => "malformed",
        }
    }
}

pub trait PushChannel: Send + Sync {
    /// Messages after `cursor`, oldest first.
    fn poll(&self, cursor: &PushCursor) -> Result<Vec<PushMessage>, PushError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("notify_failed")]
pub struct NotifyFailed;

/// Shows a notification on this device (the Tauri notification plugin on the phone).
pub trait LocalNotifier: Send + Sync {
    fn show(&self, body: &str) -> Result<(), NotifyFailed>;
}

/// The text to show for a received message: itself when it is a fixed phrase, else the generic one.
pub fn display_body(received: &str) -> &str {
    if received == TEST_MESSAGE || is_missed_sentence(received) {
        received
    } else {
        GENERIC_BODY
    }
}

/// `HH:MM` + the fixed sentence, as `supabase/functions/_shared/detect.ts` words it.
fn is_missed_sentence(text: &str) -> bool {
    let Some(clock) = text.strip_suffix(MISSED_SUFFIX) else {
        return false;
    };
    let bytes = clock.as_bytes();
    if bytes.len() != 5
        || bytes[2] != b':'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 2 || b.is_ascii_digit())
    {
        return false;
    }
    let digit = |i: usize| u32::from(bytes[i] - b'0');
    digit(0) * 10 + digit(1) < 24 && digit(3) * 10 + digit(4) < 60
}

// ---- ntfy ----

pub struct NtfyPush {
    server: String,
    topic: String,
    transport: Arc<dyn HttpTransport>,
}

impl NtfyPush {
    pub fn new(server: &str, topic: &str, transport: Arc<dyn HttpTransport>) -> Self {
        Self {
            server: server.trim_end_matches('/').to_string(),
            topic: topic.to_string(),
            transport,
        }
    }
}

#[derive(Deserialize)]
struct NtfyLine {
    id: Option<String>,
    event: Option<String>,
    message: Option<String>,
}

impl PushChannel for NtfyPush {
    fn poll(&self, cursor: &PushCursor) -> Result<Vec<PushMessage>, PushError> {
        let url = format!(
            "{}/{}/json?poll=1&since={}",
            self.server,
            self.topic,
            cursor.query_value()
        );
        let request = HttpRequest::new(
            RequestTag {
                port: "notify",
                kind: "ntfy_poll",
            },
            Method::Get,
            url,
        );
        let response = self.transport.send(request).map_err(PushError::Transport)?;
        if !response.is_success() {
            return Err(PushError::Http {
                status: response.status,
            });
        }
        let text = std::str::from_utf8(&response.body).map_err(|_| PushError::Malformed)?;
        let mut messages = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let parsed: NtfyLine = serde_json::from_str(line).map_err(|_| PushError::Malformed)?;
            // `open` and `keepalive` lines are the stream's own noise.
            if parsed.event.as_deref() != Some("message") {
                continue;
            }
            let id = parsed
                .id
                .filter(|id| {
                    !id.is_empty()
                        && id.len() <= MAX_ID_LEN
                        && id.bytes().all(|b| b.is_ascii_alphanumeric())
                })
                .ok_or(PushError::Malformed)?;
            messages.push(PushMessage {
                id,
                text: parsed.message.unwrap_or_default(),
            });
            if messages.len() == MAX_MESSAGES_PER_POLL {
                break;
            }
        }
        Ok(messages)
    }
}

// ---- mocks ----

#[derive(Default)]
pub struct MockPush {
    inbox: Mutex<Vec<PushMessage>>,
}

impl MockPush {
    /// A message arrives on the topic.
    pub fn deliver(&self, id: &str, text: &str) {
        let message = PushMessage {
            id: id.to_string(),
            text: text.to_string(),
        };
        match self.inbox.lock() {
            Ok(mut inbox) => inbox.push(message),
            Err(poisoned) => poisoned.into_inner().push(message),
        }
    }
}

impl PushChannel for MockPush {
    /// Messages after `After(id)`; every message for `Time`, which the mock does not date.
    fn poll(&self, cursor: &PushCursor) -> Result<Vec<PushMessage>, PushError> {
        let inbox = match self.inbox.lock() {
            Ok(inbox) => inbox.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        Ok(match cursor {
            PushCursor::Time(_) => inbox,
            PushCursor::After(id) => match inbox.iter().position(|m| &m.id == id) {
                Some(at) => inbox[at + 1..].to_vec(),
                None => inbox,
            },
        })
    }
}

#[derive(Default)]
pub struct MockNotifier {
    shown: Mutex<Vec<String>>,
    fail: Mutex<bool>,
}

impl MockNotifier {
    pub fn shown(&self) -> Vec<String> {
        match self.shown.lock() {
            Ok(shown) => shown.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// The OS refuses notifications (permission revoked).
    pub fn set_failing(&self, failing: bool) {
        match self.fail.lock() {
            Ok(mut fail) => *fail = failing,
            Err(poisoned) => *poisoned.into_inner() = failing,
        }
    }
}

impl LocalNotifier for MockNotifier {
    fn show(&self, body: &str) -> Result<(), NotifyFailed> {
        let failing = match self.fail.lock() {
            Ok(fail) => *fail,
            Err(poisoned) => *poisoned.into_inner(),
        };
        if failing {
            return Err(NotifyFailed);
        }
        match self.shown.lock() {
            Ok(mut shown) => shown.push(body.to_string()),
            Err(poisoned) => poisoned.into_inner().push(body.to_string()),
        }
        Ok(())
    }
}

// ---- relay ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RelayError {
    #[error("{0}")]
    Push(PushError),
    #[error("notify_failed")]
    Notify,
}

/// Pulls new messages and shows each one. The cursor moves past a message only after it was shown,
/// so a notification the OS refused is tried again on the next poll; the network call is made
/// without any lock held.
pub fn relay(
    channel: &dyn PushChannel,
    notifier: &dyn LocalNotifier,
    cursor: &mut PushCursor,
) -> Result<usize, RelayError> {
    let messages = channel.poll(cursor).map_err(RelayError::Push)?;
    let mut shown = 0;
    for message in messages {
        notifier
            .show(display_body(&message.text))
            .map_err(|_| RelayError::Notify)?;
        *cursor = PushCursor::After(message.id);
        shown += 1;
    }
    Ok(shown)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::remote::http::{HostPolicy, HttpResponse, ScriptedTransport};

    fn contract() -> Value {
        serde_json::from_str::<Value>(include_str!("../../../fixtures/notify/contract.json"))
            .expect("contract")["phoneApp"]
            .clone()
    }

    fn cursor_of(raw: &Value) -> PushCursor {
        match (raw["time"].as_i64(), raw["id"].as_str()) {
            (Some(time), _) => PushCursor::Time(time),
            (None, Some(id)) => PushCursor::After(id.to_string()),
            _ => panic!("cursor"),
        }
    }

    fn ndjson(lines: &Value) -> String {
        lines
            .as_array()
            .expect("lines")
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_poll_requests_match_the_shared_contract() {
        for case in contract()["poll"]["requests"].as_array().expect("requests") {
            let name = case["name"].as_str().expect("name");
            let server = case["server"].as_str().expect("server");
            let transport = Arc::new(ScriptedTransport::new(
                HostPolicy::configured(["ntfy.sh", "ntfy.example.org"]),
                vec![Ok(HttpResponse::ok_json(200, ""))],
            ));
            let channel = NtfyPush::new(
                server,
                case["topic"].as_str().expect("topic"),
                transport.clone(),
            );
            assert_eq!(
                channel.poll(&cursor_of(&case["cursor"])),
                Ok(vec![]),
                "{name}"
            );
            let seen = transport.requests();
            assert_eq!(seen.len(), 1, "{name}");
            assert_eq!(seen[0].method, Method::Get, "{name}");
            assert_eq!(
                seen[0].url,
                case["expectUrl"].as_str().expect("url"),
                "{name}"
            );
            assert!(seen[0].body.is_empty(), "{name}");
        }
    }

    #[test]
    fn a_polled_stream_becomes_local_notifications_with_fixed_bodies() {
        let contract = contract();
        let body = ndjson(&contract["poll"]["response"]);
        let transport = Arc::new(ScriptedTransport::new(
            HostPolicy::configured(["ntfy.sh"]),
            vec![Ok(HttpResponse::ok_json(200, &body))],
        ));
        let channel = NtfyPush::new("https://ntfy.sh", "abcdefghijklmnopqrstuvwx", transport);
        let notifier = MockNotifier::default();
        let mut cursor = PushCursor::Time(0);

        assert_eq!(relay(&channel, &notifier, &mut cursor), Ok(3));
        let expected: Vec<&str> = contract["poll"]["expectBodies"]
            .as_array()
            .expect("bodies")
            .iter()
            .map(|b| b.as_str().expect("body"))
            .collect();
        assert_eq!(notifier.shown(), expected);
        assert_eq!(
            cursor,
            PushCursor::After(
                contract["poll"]["expectCursorId"]
                    .as_str()
                    .expect("id")
                    .to_string()
            )
        );
    }

    #[test]
    fn only_fixed_phrases_are_shown_as_received() {
        let bodies = &contract()["bodies"];
        assert_eq!(bodies["generic"], GENERIC_BODY);
        for text in bodies["fixed"].as_array().expect("fixed") {
            let text = text.as_str().expect("text");
            assert_eq!(display_body(text), text);
        }
        for text in bodies["replacedByGeneric"].as_array().expect("replaced") {
            assert_eq!(
                display_body(text.as_str().expect("text")),
                GENERIC_BODY,
                "{text}"
            );
        }
    }

    #[test]
    fn a_subject_in_a_received_message_never_reaches_the_screen() {
        let push = MockPush::default();
        let notifier = MockNotifier::default();
        push.deliver("a1", "수학 숙제 시작할 시간이에요");
        push.deliver("a2", "10:30인데 아직 안 앉았어! 신호가 없어요.");
        let mut cursor = PushCursor::Time(0);
        assert_eq!(relay(&push, &notifier, &mut cursor), Ok(2));
        let shown = notifier.shown();
        assert!(
            shown
                .iter()
                .all(|body| !body.contains("수학") && !body.contains("숙제")),
            "{shown:?}"
        );
        assert_eq!(
            shown,
            [GENERIC_BODY, "10:30인데 아직 안 앉았어! 신호가 없어요."]
        );
    }

    #[test]
    fn a_refused_notification_is_retried_and_nothing_is_shown_twice() {
        let push = MockPush::default();
        let notifier = MockNotifier::default();
        push.deliver("a1", TEST_MESSAGE);
        push.deliver("a2", TEST_MESSAGE);
        let mut cursor = PushCursor::Time(0);

        notifier.set_failing(true);
        assert_eq!(
            relay(&push, &notifier, &mut cursor),
            Err(RelayError::Notify)
        );
        assert_eq!(
            cursor,
            PushCursor::Time(0),
            "not advanced past an unshown message"
        );

        notifier.set_failing(false);
        assert_eq!(relay(&push, &notifier, &mut cursor), Ok(2));
        assert_eq!(relay(&push, &notifier, &mut cursor), Ok(0), "already shown");
        assert_eq!(notifier.shown().len(), 2);
    }

    #[test]
    fn poll_failures_surface_as_stable_codes_and_show_nothing() {
        let notifier = MockNotifier::default();
        let mut cursor = PushCursor::Time(0);
        let cases: Vec<(Result<HttpResponse, TransportError>, &str)> = vec![
            (Ok(HttpResponse::ok_json(403, "no")), "http_error"),
            (Err(TransportError::Timeout), "timeout"),
            (Ok(HttpResponse::ok_json(200, "not json")), "malformed"),
            (
                Ok(HttpResponse::ok_json(
                    200,
                    r#"{"event":"message","message":"x"}"#,
                )),
                "malformed",
            ),
        ];
        for (step, code) in cases {
            let transport = Arc::new(ScriptedTransport::new(
                HostPolicy::configured(["ntfy.sh"]),
                vec![step],
            ));
            let channel = NtfyPush::new("https://ntfy.sh", "abcdefghijklmnopqrstuvwx", transport);
            match relay(&channel, &notifier, &mut cursor) {
                Err(RelayError::Push(err)) => assert_eq!(err.code(), code),
                other => panic!("{code}: {other:?}"),
            }
        }
        let strict = Arc::new(ScriptedTransport::new(
            HostPolicy::configured(["ntfy.sh"]),
            vec![],
        ));
        let elsewhere = NtfyPush::new("https://other.example", "t", strict.clone());
        assert_eq!(
            elsewhere.poll(&cursor).expect_err("denied").code(),
            "host_denied"
        );
        assert!(strict.requests().is_empty());
        assert!(notifier.shown().is_empty());
    }

    #[test]
    fn debug_output_of_a_message_does_not_contain_its_text() {
        let message = PushMessage {
            id: "a1".into(),
            text: "비밀 과목".into(),
        };
        assert!(!format!("{message:?}").contains("비밀"));
    }
}
