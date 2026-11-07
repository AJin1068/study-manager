//! The phone side of the server contract: every call is one Supabase RPC
//! (`POST /rest/v1/rpc/<name>`, `fixtures/phone/rpc-contract.json`). Answers are `{"code": ...}`
//! objects; a code other than `ok` becomes `PhoneError::Rejected`. Request bodies are built here
//! from fixed parameter names: a lease receipt (sequence number and two booleans), a pick-up count
//! with a segment id, or the one-time pairing code. There is no parameter for an app name, a
//! package name, usage or a URL.

use std::sync::Arc;

use link_protocol::{LockLease, PhoneBeat, PickupReport};
use serde_json::{Value, json};

use crate::http::{HttpRequest, HttpResponse, HttpTransport, Method};
use crate::model::{ClaimedPair, LeaseFetch, PhoneCode, PhoneError};
use crate::session::PhoneSession;

/// The phone's port to the server.
pub trait PhoneLink: Send + Sync {
    /// Binds this (anonymous) phone to the owner that made the code.
    fn claim(&self, code: &str) -> Result<ClaimedPair, PhoneError>;
    /// The lease in force and the server's clock reading. `NotPaired` once the pairing is revoked.
    fn fetch_lease(&self) -> Result<LeaseFetch, PhoneError>;
    fn beat(&self, beat: &PhoneBeat) -> Result<(), PhoneError>;
    fn report_pickups(&self, report: &PickupReport) -> Result<(), PhoneError>;
    /// The phone ends the pairing from its side.
    fn leave(&self) -> Result<(), PhoneError>;
}

pub struct RestPhoneLink {
    transport: Arc<dyn HttpTransport>,
    session: Arc<PhoneSession>,
}

impl RestPhoneLink {
    pub fn new(transport: Arc<dyn HttpTransport>, session: Arc<PhoneSession>) -> Self {
        Self { transport, session }
    }

    fn attempt(&self, name: &str, body: &[u8], bearer: &str) -> Result<HttpResponse, PhoneError> {
        let request = HttpRequest::new(
            Method::Post,
            format!("{}/rest/v1/rpc/{name}", self.session.endpoint()),
        )
        .header("apikey", self.session.anon_key())
        .header("authorization", &format!("Bearer {bearer}"))
        .json_body(body.to_vec());
        self.transport.send(request).map_err(PhoneError::Transport)
    }

    /// Sends with the current token; a 401 renews the token once and repeats the request.
    fn call(&self, name: &str, args: &Value) -> Result<Value, PhoneError> {
        let body = serde_json::to_vec(args).map_err(|_| PhoneError::Decode)?;
        let mut response = self.attempt(name, &body, &self.session.access_token()?)?;
        if response.status == 401 {
            response = self.attempt(name, &body, &self.session.force_refresh()?)?;
        }
        if !response.is_success() {
            return Err(match response.status {
                401 => PhoneError::Auth(crate::session::AuthError::Expired),
                status => PhoneError::Http { status },
            });
        }
        let answer: Value =
            serde_json::from_slice(&response.body).map_err(|_| PhoneError::Decode)?;
        match answer
            .get("code")
            .and_then(Value::as_str)
            .and_then(PhoneCode::parse)
        {
            Some(PhoneCode::Ok) => Ok(answer),
            Some(code) => Err(PhoneError::Rejected(code)),
            None => Err(PhoneError::Decode),
        }
    }
}

fn text(answer: &Value, field: &str) -> Result<String, PhoneError> {
    answer
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(PhoneError::Decode)
}

fn number(answer: &Value, field: &str) -> Result<i64, PhoneError> {
    answer
        .get(field)
        .and_then(Value::as_i64)
        .ok_or(PhoneError::Decode)
}

impl PhoneLink for RestPhoneLink {
    fn claim(&self, code: &str) -> Result<ClaimedPair, PhoneError> {
        let answer = self.call("claim_pair", &json!({ "p_code": code }))?;
        Ok(ClaimedPair {
            pair_id: text(&answer, "pair_id")?,
            owner_user_id: text(&answer, "owner_user_id")?,
            server_now_ms: number(&answer, "server_now_ms")?,
        })
    }

    fn fetch_lease(&self) -> Result<LeaseFetch, PhoneError> {
        let answer = self.call("phone_lease", &json!({}))?;
        let server_now_ms = number(&answer, "server_now_ms")?;
        let lease = match answer.get("lease") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                serde_json::from_value::<LockLease>(value.clone())
                    .map_err(|_| PhoneError::Decode)?,
            ),
        };
        Ok(LeaseFetch {
            lease,
            server_now_ms,
        })
    }

    fn beat(&self, beat: &PhoneBeat) -> Result<(), PhoneError> {
        let args = json!({ "p_lease_seq": beat.lease_seq, "p_locked": beat.locked, "p_service_enabled": beat.service_enabled });
        self.call("phone_beat", &args).map(|_| ())
    }

    fn report_pickups(&self, report: &PickupReport) -> Result<(), PhoneError> {
        report
            .validate()
            .map_err(|_| PhoneError::Rejected(PhoneCode::InvalidInput))?;
        self.call(
            "report_phone_pickups",
            &json!({ "p_segment_id": report.segment_id, "p_count": report.count }),
        )
        .map(|_| ())
    }

    fn leave(&self) -> Result<(), PhoneError> {
        self.call("revoke_pair", &json!({})).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ScriptedTransport, TestClock, json as reply};

    const TOKENS: &str = r#"{"access_token":"access-1","expires_in":3600,"refresh_token":"refresh-1","user":{"id":"phone-user"}}"#;
    const RENEWED: &str = r#"{"access_token":"access-2","expires_in":3600,"refresh_token":"refresh-2","user":{"id":"phone-user"}}"#;

    fn link(
        replies: Vec<Result<HttpResponse, crate::http::TransportError>>,
    ) -> (RestPhoneLink, Arc<ScriptedTransport>, Arc<PhoneSession>) {
        let transport = Arc::new(ScriptedTransport::new(replies));
        let clock = Arc::new(TestClock::new(0, 1_000_000));
        let session = Arc::new(PhoneSession::new(
            "https://proj.supabase.co/",
            "anon-public",
            transport.clone(),
            clock,
        ));
        (
            RestPhoneLink::new(transport.clone(), session.clone()),
            transport,
            session,
        )
    }

    fn signed_in(
        extra: Vec<Result<HttpResponse, crate::http::TransportError>>,
    ) -> (RestPhoneLink, Arc<ScriptedTransport>, Arc<PhoneSession>) {
        let mut replies = vec![reply(200, TOKENS)];
        replies.extend(extra);
        let (link, transport, session) = link(replies);
        session.sign_in().expect("sign in");
        (link, transport, session)
    }

    #[test]
    fn anonymous_sign_in_posts_an_empty_object_with_only_the_public_key() {
        let (_, transport, session) = signed_in(vec![]);
        let seen = transport.seen();
        assert_eq!(seen[0].url, "https://proj.supabase.co/auth/v1/signup");
        assert_eq!(seen[0].body.as_deref(), Some(b"{}".as_slice()));
        assert_eq!(seen[0].header_value("apikey"), Some("anon-public"));
        assert_eq!(seen[0].header_value("authorization"), None);
        assert_eq!(session.user_id().as_deref(), Some("phone-user"));
    }

    #[test]
    fn claim_pair_sends_the_code_with_the_bearer_and_reads_the_pair() {
        let (link, transport, _) = signed_in(vec![reply(
            200,
            r#"{"code":"ok","pair_id":"p1","owner_user_id":"o1","server_now_ms":123}"#,
        )]);
        let claimed = link.claim("ABCDEFGHJK").expect("claim");
        assert_eq!(
            (
                claimed.pair_id.as_str(),
                claimed.owner_user_id.as_str(),
                claimed.server_now_ms
            ),
            ("p1", "o1", 123)
        );
        let request = &transport.seen()[1];
        assert_eq!(
            request.url,
            "https://proj.supabase.co/rest/v1/rpc/claim_pair"
        );
        assert_eq!(
            request.header_value("authorization"),
            Some("Bearer access-1")
        );
        assert_eq!(
            serde_json::from_slice::<Value>(request.body.as_deref().expect("body")).expect("json"),
            json!({ "p_code": "ABCDEFGHJK" })
        );
    }

    #[test]
    fn every_code_the_contract_lets_a_phone_rpc_return_is_understood() {
        let contract: Value =
            serde_json::from_str(include_str!("../../../fixtures/phone/rpc-contract.json"))
                .expect("fixture");
        let mut checked = 0;
        for rpc in contract["rpcs"].as_array().expect("rpcs") {
            let name = rpc["name"].as_str().expect("name");
            if !matches!(
                name,
                "claim_pair"
                    | "phone_lease"
                    | "phone_beat"
                    | "report_phone_pickups"
                    | "revoke_pair"
            ) {
                continue;
            }
            for code in rpc["returns"].as_array().expect("returns") {
                let code = code.as_str().expect("code");
                let parsed =
                    PhoneCode::parse(code).unwrap_or_else(|| panic!("{name}: unknown code {code}"));
                assert_eq!(parsed.as_str(), code);
                checked += 1;
            }
        }
        assert!(checked >= 20, "checked {checked}");
    }

    #[test]
    fn a_refusal_and_an_unknown_code_are_told_apart() {
        let (link, _, _) = signed_in(vec![
            reply(200, r#"{"code":"invalid_code"}"#),
            reply(200, r#"{"code":"rate_limited"}"#),
            reply(200, r#"{"code":"something_new"}"#),
            reply(200, "not json"),
            reply(503, "down"),
        ]);
        assert_eq!(
            link.claim("ABCDEFGHJK"),
            Err(PhoneError::Rejected(PhoneCode::InvalidCode))
        );
        assert_eq!(
            link.claim("ABCDEFGHJK"),
            Err(PhoneError::Rejected(PhoneCode::RateLimited))
        );
        assert_eq!(link.claim("ABCDEFGHJK"), Err(PhoneError::Decode));
        assert_eq!(link.claim("ABCDEFGHJK"), Err(PhoneError::Decode));
        assert_eq!(
            link.claim("ABCDEFGHJK"),
            Err(PhoneError::Http { status: 503 })
        );
    }

    #[test]
    fn a_401_renews_the_token_once_and_repeats_the_request() {
        let (link, transport, session) = signed_in(vec![
            reply(401, r#"{"code":"PGRST301","message":"JWT expired"}"#),
            reply(200, RENEWED),
            reply(200, r#"{"code":"ok","lease":null,"server_now_ms":5}"#),
        ]);
        assert_eq!(
            link.fetch_lease().expect("lease"),
            LeaseFetch {
                lease: None,
                server_now_ms: 5
            }
        );
        let seen = transport.seen();
        assert_eq!(
            seen[1].header_value("authorization"),
            Some("Bearer access-1")
        );
        assert_eq!(
            seen[2].url,
            "https://proj.supabase.co/auth/v1/token?grant_type=refresh_token"
        );
        assert_eq!(
            seen[3].header_value("authorization"),
            Some("Bearer access-2")
        );
        assert_eq!(
            session.refresh_token().as_deref(),
            Some("refresh-2"),
            "the rotated token is the one to keep"
        );
    }

    #[test]
    fn a_second_401_is_an_expired_identity_not_a_new_sign_in() {
        let (link, transport, _) = signed_in(vec![
            reply(401, "{}"),
            reply(200, RENEWED),
            reply(401, "{}"),
        ]);
        assert_eq!(
            link.fetch_lease(),
            Err(PhoneError::Auth(crate::session::AuthError::Expired))
        );
        assert_eq!(
            transport
                .seen()
                .iter()
                .filter(|r| r.url.ends_with("/auth/v1/signup"))
                .count(),
            1,
            "no second anonymous identity"
        );
    }

    #[test]
    fn a_rejected_refresh_token_is_expired_and_a_429_carries_retry_after() {
        let (_, _, session) = link(vec![
            reply(400, r#"{"error_code":"refresh_token_not_found"}"#),
            Ok(HttpResponse {
                status: 429,
                headers: vec![("Retry-After".into(), "42".into())],
                body: Vec::new(),
            }),
            reply(503, "x"),
            Err(crate::http::TransportError::Timeout),
            reply(200, "not json"),
        ]);
        assert_eq!(
            session.restore("old"),
            Err(crate::session::AuthError::Expired)
        );
        assert_eq!(
            session.restore("old"),
            Err(crate::session::AuthError::RateLimited {
                retry_after_ms: 42_000
            })
        );
        assert_eq!(
            session.restore("old"),
            Err(crate::session::AuthError::Server { status: 503 })
        );
        assert_eq!(
            session.restore("old"),
            Err(crate::session::AuthError::Network(
                crate::http::TransportError::Timeout
            ))
        );
        assert_eq!(
            session.restore("old"),
            Err(crate::session::AuthError::Decode)
        );
        assert_eq!(
            session.access_token(),
            Err(crate::session::AuthError::NotSignedIn)
        );
    }

    #[test]
    fn the_fixture_wire_shapes_decode() {
        let wire: Value =
            serde_json::from_str(include_str!("../../../fixtures/phone/link-protocol.json"))
                .expect("fixture");
        let answer =
            json!({ "code": "ok", "lease": wire["lockedLease"], "server_now_ms": 1_010_000 });
        let (link, _, _) = signed_in(vec![reply(200, &answer.to_string())]);
        let fetch = link.fetch_lease().expect("lease");
        assert_eq!(fetch.lease.expect("lease").seq, 7);
    }

    #[test]
    fn beats_and_pickups_carry_only_their_fields_and_bad_counts_never_leave() {
        let (link, transport, _) = signed_in(vec![
            reply(200, r#"{"code":"ok"}"#),
            reply(200, r#"{"code":"ok"}"#),
        ]);
        link.beat(&PhoneBeat {
            lease_seq: 7,
            locked: true,
            service_enabled: false,
        })
        .expect("beat");
        link.report_pickups(&PickupReport {
            segment_id: "run-1:2".into(),
            count: 3,
        })
        .expect("pickups");
        assert_eq!(
            link.report_pickups(&PickupReport {
                segment_id: "run-1:2".into(),
                count: -1
            }),
            Err(PhoneError::Rejected(PhoneCode::InvalidInput))
        );
        let seen = transport.seen();
        assert_eq!(seen.len(), 3, "the invalid report made no request");
        let body = |i: usize| {
            serde_json::from_slice::<Value>(seen[i].body.as_deref().expect("body")).expect("json")
        };
        assert_eq!(
            body(1),
            json!({ "p_lease_seq": 7, "p_locked": true, "p_service_enabled": false })
        );
        assert_eq!(body(2), json!({ "p_segment_id": "run-1:2", "p_count": 3 }));
    }

    #[test]
    fn a_request_never_shows_its_token_or_code_in_debug_output() {
        let request = HttpRequest::new(Method::Post, "https://x.test".into())
            .header("authorization", "Bearer secret")
            .json_body(b"ABCDEFGHJK".to_vec());
        assert!(!format!("{request:?}").contains("secret"));
        assert!(!format!("{request:?}").contains("ABCDEFGHJK"));
    }
}
