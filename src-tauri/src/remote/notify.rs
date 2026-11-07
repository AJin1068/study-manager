//! Phone-notification client of the desktop (the "send a test" button). The server sends the real
//! missed-plan alerts; this client only proves the channel works.
//!
//! * `NtfyClient` posts the text to `{server}/{topic}` itself.
//! * Telegram's bot token lives only in the Edge Function, so `ServerTest` asks the function to
//!   send the test message to the signed-in user's own channel.
//! * `RecordingNotify` is the mock: it keeps the messages in memory and sends nothing.
//!
//! The wire shapes are the ones in `fixtures/notify/contract.json`, the same file the Edge
//! Function's tests read. Messages never name a subject or a plan: they show on a lock screen.

use std::sync::{Arc, Mutex};

use super::http::{HttpRequest, HttpTransport, Method, RequestTag};
use super::payload::{Body, NoticeText, NotifyTestRequest};
use super::supabase::{RemoteError, SupabaseRest};

pub const TEST_MESSAGE: &str = "테스트 알림이에요. 잘 보이나요?";
/// ASCII on purpose: an HTTP header carries no UTF-8.
const NTFY_TITLE: &str = "Study Manager";

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct NotifyError(pub RemoteError);

impl NotifyError {
    pub fn code(self) -> &'static str {
        self.0.code()
    }
}

pub trait NotifyClient: Send + Sync {
    fn send_test(&self) -> Result<(), NotifyError>;
}

/// Mock adapter: nothing leaves the device.
#[derive(Default)]
pub struct RecordingNotify {
    sent: Mutex<Vec<String>>,
}

impl RecordingNotify {
    pub fn sent(&self) -> Vec<String> {
        match self.sent.lock() {
            Ok(sent) => sent.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl NotifyClient for RecordingNotify {
    fn send_test(&self) -> Result<(), NotifyError> {
        match self.sent.lock() {
            Ok(mut sent) => sent.push(TEST_MESSAGE.to_string()),
            Err(poisoned) => poisoned.into_inner().push(TEST_MESSAGE.to_string()),
        }
        Ok(())
    }
}

/// Real ntfy adapter. The topic is part of the URL path, which the audit log never records.
pub struct NtfyClient {
    server: String,
    topic: String,
    transport: Arc<dyn HttpTransport>,
}

impl NtfyClient {
    pub fn new(server: &str, topic: &str, transport: Arc<dyn HttpTransport>) -> Self {
        Self {
            server: server.trim_end_matches('/').to_string(),
            topic: topic.to_string(),
            transport,
        }
    }

    fn send_text(&self, text: &str) -> Result<(), NotifyError> {
        let request = HttpRequest::new(
            RequestTag {
                port: "notify",
                kind: "ntfy_test",
            },
            Method::Post,
            format!("{}/{}", self.server, self.topic),
        )
        .header("title", NTFY_TITLE)
        .with_body(Body::from_notice(&NoticeText::new(text)));
        let response = self
            .transport
            .send(request)
            .map_err(|err| NotifyError(RemoteError::Transport(err)))?;
        if response.is_success() {
            Ok(())
        } else {
            Err(NotifyError(RemoteError::Http {
                status: response.status,
            }))
        }
    }
}

impl NotifyClient for NtfyClient {
    fn send_test(&self) -> Result<(), NotifyError> {
        self.send_text(TEST_MESSAGE)
    }
}

/// Telegram (and any channel the server owns the credentials for): the Edge Function sends it.
pub struct ServerTest {
    rest: Arc<SupabaseRest>,
}

impl ServerTest {
    pub fn new(rest: Arc<SupabaseRest>) -> Self {
        Self { rest }
    }
}

impl NotifyClient for ServerTest {
    fn send_test(&self) -> Result<(), NotifyError> {
        self.rest
            .function(
                "notify_test",
                "missed-plan",
                &NotifyTestRequest { action: "test" },
            )
            .map(|_| ())
            .map_err(NotifyError)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::remote::fake::{FAKE_ANON_KEY, FAKE_ENDPOINT, FakeBackend};
    use crate::remote::http::{HostPolicy, HttpResponse, ScriptedTransport, TransportError};

    fn contract() -> Value {
        serde_json::from_str(include_str!("../../../fixtures/notify/contract.json"))
            .expect("contract")
    }

    #[test]
    fn test_message_matches_the_shared_contract_and_names_nothing() {
        let contract = contract();
        assert_eq!(contract["messages"]["test"], TEST_MESSAGE);
        for word in ["수학", "영어", "math", "Math", "계획 이름"] {
            assert!(!TEST_MESSAGE.contains(word));
        }
    }

    #[test]
    fn ntfy_requests_match_every_case_in_the_shared_contract() {
        let contract = contract();
        let cases = contract["ntfy"]["requests"].as_array().expect("cases");
        assert!(!cases.is_empty());
        for case in cases {
            let name = case["name"].as_str().expect("name");
            let transport = Arc::new(ScriptedTransport::new(
                HostPolicy::configured(["ntfy.sh", "ntfy.example.org"]),
                vec![Ok(HttpResponse::ok_json(200, "{}"))],
            ));
            let client = NtfyClient::new(
                case["server"].as_str().expect("server"),
                case["topic"].as_str().expect("topic"),
                transport.clone(),
            );
            client
                .send_text(case["message"].as_str().expect("message"))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let seen = transport.requests();
            let expect = &case["expect"];
            assert_eq!(seen.len(), 1, "{name}");
            assert_eq!(
                seen[0].method.as_str(),
                expect["method"].as_str().expect("method"),
                "{name}"
            );
            assert_eq!(seen[0].url, expect["url"].as_str().expect("url"), "{name}");
            assert_eq!(
                seen[0].content_type,
                expect["contentType"].as_str(),
                "{name}"
            );
            assert_eq!(
                String::from_utf8_lossy(&seen[0].body),
                expect["body"].as_str().expect("body"),
                "{name}"
            );
            for (header, value) in expect["headers"].as_object().expect("headers") {
                let got = seen[0]
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(header))
                    .map(|(_, v)| v.as_str());
                assert_eq!(got, value.as_str(), "{name}: header {header}");
            }
        }
    }

    #[test]
    fn ntfy_failures_surface_as_stable_codes() {
        let policy = HostPolicy::configured(["ntfy.sh"]);
        let refused = Arc::new(ScriptedTransport::new(
            policy.clone(),
            vec![
                Ok(HttpResponse::ok_json(403, "no")),
                Err(TransportError::Timeout),
            ],
        ));
        let client = NtfyClient::new("https://ntfy.sh", "abcdefghijklmnopqrstuvwx", refused);
        assert_eq!(client.send_test().expect_err("403").code(), "http_error");
        assert_eq!(client.send_test().expect_err("timeout").code(), "timeout");
        // A server other than the configured one is refused before any request exists.
        let strict = Arc::new(ScriptedTransport::new(policy, vec![]));
        let elsewhere = NtfyClient::new("https://other.example", "t", strict.clone());
        assert_eq!(
            elsewhere.send_test().expect_err("denied").code(),
            "host_denied"
        );
        assert!(strict.requests().is_empty());
    }

    #[test]
    fn the_mock_records_each_test_and_sends_nothing() {
        let mock = RecordingNotify::default();
        assert!(mock.sent().is_empty());
        mock.send_test().expect("mock");
        assert_eq!(mock.sent(), [TEST_MESSAGE]);
    }

    #[test]
    fn server_test_posts_the_contract_body_to_the_function_route() {
        let contract = contract();
        let route = contract["serverTest"]["route"]
            .as_str()
            .expect("route")
            .to_string();
        let transport = Arc::new(ScriptedTransport::new(
            HostPolicy::configured(["proj.supabase.co"]),
            vec![Ok(HttpResponse::ok_json(
                200,
                &contract["serverTest"]["responseBody"].to_string(),
            ))],
        ));
        let tokens = Arc::new(StaticToken);
        let rest = Arc::new(SupabaseRest::new(
            "https://proj.supabase.co",
            "anon-public-key",
            transport.clone(),
            tokens,
        ));
        ServerTest::new(rest).send_test().expect("test");
        let seen = transport.requests();
        assert_eq!(seen[0].url, format!("https://proj.supabase.co{route}"));
        let body: Value = serde_json::from_slice(&seen[0].body).expect("json");
        assert_eq!(body, contract["serverTest"]["requestBody"]);
    }

    struct StaticToken;

    impl crate::remote::auth::AccessTokenSource for StaticToken {
        fn access_token(&self) -> Result<String, crate::remote::auth::AuthError> {
            Ok("user-token".into())
        }
        fn force_refresh(&self) -> Result<String, crate::remote::auth::AuthError> {
            Ok("user-token".into())
        }
    }

    #[test]
    fn unknown_function_route_on_the_fake_is_an_http_error_not_a_panic() {
        let clock = Arc::new(crate::remote::ManualClock::new(1));
        let backend = Arc::new(FakeBackend::new(clock));
        let rest = Arc::new(SupabaseRest::new(
            FAKE_ENDPOINT,
            FAKE_ANON_KEY,
            backend,
            Arc::new(StaticToken),
        ));
        // The fake has no token for "user-token": it answers 401 and the adapter reports it.
        assert!(ServerTest::new(rest).send_test().is_err());
    }
}
