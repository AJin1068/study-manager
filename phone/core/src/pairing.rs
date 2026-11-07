//! Pairing by QR: the phone scans the text the desktop shows (`PairingOffer`), signs in
//! anonymously and claims the one-time code. The refresh token is the only secret that stays on
//! the device; the shell persists it (`StoredPairing`) in the app's private storage.

use std::sync::Arc;

use link_protocol::{OfferError, PairingOffer};
use serde::{Deserialize, Serialize};

use crate::Clock;
use crate::http::HttpTransport;
use crate::model::{ClaimedPair, PhoneError};
use crate::rest::{PhoneLink, RestPhoneLink};
use crate::session::{AuthError, PhoneSession};

/// What the phone keeps between launches. The anon key is public by design.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPairing {
    pub endpoint: String,
    pub anon_key: String,
    pub refresh_token: String,
}

impl std::fmt::Debug for StoredPairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoredPairing { .. }")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PairError {
    /// The scanned text is not a valid offer.
    #[error("bad_offer")]
    BadOffer(OfferError),
    #[error("{0}")]
    Phone(PhoneError),
}

impl PairError {
    pub fn code(self) -> &'static str {
        match self {
            PairError::BadOffer(_) => "bad_offer",
            PairError::Phone(err) => err.code(),
        }
    }
}

pub struct Paired {
    pub link: Arc<RestPhoneLink>,
    pub session: Arc<PhoneSession>,
    pub claimed: ClaimedPair,
    pub stored: StoredPairing,
}

/// Reads the scanned text, signs in anonymously and claims the code. A failed claim leaves
/// nothing to store: the anonymous identity made for it is dropped.
pub fn pair_with_offer(
    text: &str,
    transport: Arc<dyn HttpTransport>,
    clock: Arc<dyn Clock>,
) -> Result<Paired, PairError> {
    let offer = PairingOffer::parse(text).map_err(PairError::BadOffer)?;
    let session = Arc::new(PhoneSession::new(
        &offer.endpoint,
        &offer.anon_key,
        transport.clone(),
        clock,
    ));
    session
        .sign_in()
        .map_err(|err| PairError::Phone(PhoneError::Auth(err)))?;
    let link = Arc::new(RestPhoneLink::new(transport, session.clone()));
    let claimed = link.claim(&offer.code).map_err(PairError::Phone)?;
    let refresh_token = session
        .refresh_token()
        .ok_or(PairError::Phone(PhoneError::Auth(AuthError::NotSignedIn)))?;
    let stored = StoredPairing {
        endpoint: offer.endpoint,
        anon_key: offer.anon_key,
        refresh_token,
    };
    Ok(Paired {
        link,
        session,
        claimed,
        stored,
    })
}

/// A later launch: renews from the stored refresh token. A refusal is `Auth(Expired)`; the caller
/// asks the user to pair again instead of creating a new identity.
pub fn restore(
    stored: &StoredPairing,
    transport: Arc<dyn HttpTransport>,
    clock: Arc<dyn Clock>,
) -> Result<(Arc<RestPhoneLink>, Arc<PhoneSession>), PhoneError> {
    let session = Arc::new(PhoneSession::new(
        &stored.endpoint,
        &stored.anon_key,
        transport.clone(),
        clock,
    ));
    session.restore(&stored.refresh_token)?;
    Ok((
        Arc::new(RestPhoneLink::new(transport, session.clone())),
        session,
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::model::PhoneCode;
    use crate::testing::{ScriptedTransport, TestClock, json};

    const TOKENS: &str =
        r#"{"access_token":"a","expires_in":3600,"refresh_token":"r1","user":{"id":"phone-user"}}"#;

    fn qr() -> String {
        let wire: Value =
            serde_json::from_str(include_str!("../../../fixtures/phone/link-protocol.json"))
                .expect("fixture");
        wire["pairingOfferQrText"]
            .as_str()
            .expect("qr text")
            .to_string()
    }

    #[test]
    fn the_fixture_qr_text_pairs_through_sign_in_then_claim() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            json(200, TOKENS),
            json(
                200,
                r#"{"code":"ok","pair_id":"p","owner_user_id":"o","server_now_ms":1}"#,
            ),
        ]));
        let paired = pair_with_offer(&qr(), transport.clone(), Arc::new(TestClock::new(0, 0)))
            .expect("paired");
        assert_eq!(paired.stored.endpoint, "https://proj.supabase.co");
        assert_eq!(paired.stored.refresh_token, "r1");
        let seen = transport.seen();
        assert_eq!(seen[0].url, "https://proj.supabase.co/auth/v1/signup");
        assert_eq!(
            seen[1].url,
            "https://proj.supabase.co/rest/v1/rpc/claim_pair"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(seen[1].body.as_deref().expect("body")).expect("json")
                ["p_code"],
            "ABCDEFGHJK"
        );
    }

    #[test]
    fn a_bad_scan_makes_no_request() {
        let transport = Arc::new(ScriptedTransport::new(vec![]));
        for text in [
            "",
            "hello",
            "{\"v\":2}",
            &qr()
                .replace("https", "http")
                .replace("proj.supabase.co", "evil.example"),
        ] {
            let err = pair_with_offer(text, transport.clone(), Arc::new(TestClock::new(0, 0)))
                .err()
                .expect("refused");
            assert_eq!(err.code(), "bad_offer", "{text}");
        }
        assert!(transport.seen().is_empty());
    }

    #[test]
    fn a_used_or_expired_code_stores_nothing() {
        for code in ["used", "expired", "invalid_code", "cannot_self"] {
            let transport = Arc::new(ScriptedTransport::new(vec![
                json(200, TOKENS),
                json(200, &format!(r#"{{"code":"{code}"}}"#)),
            ]));
            let err = pair_with_offer(&qr(), transport, Arc::new(TestClock::new(0, 0)))
                .err()
                .expect("refused");
            assert_eq!(err.code(), code);
            assert_eq!(
                err,
                PairError::Phone(PhoneError::Rejected(PhoneCode::parse(code).expect("code")))
            );
        }
    }

    #[test]
    fn restore_renews_from_the_stored_token_and_a_refusal_never_signs_in_again() {
        let stored = StoredPairing {
            endpoint: "https://proj.supabase.co".into(),
            anon_key: "k".into(),
            refresh_token: "r1".into(),
        };
        let ok = Arc::new(ScriptedTransport::new(vec![json(200, TOKENS)]));
        let (_, session) =
            restore(&stored, ok.clone(), Arc::new(TestClock::new(0, 0))).expect("restored");
        assert_eq!(session.user_id().as_deref(), Some("phone-user"));
        assert_eq!(
            ok.seen()[0].url,
            "https://proj.supabase.co/auth/v1/token?grant_type=refresh_token"
        );

        let refused = Arc::new(ScriptedTransport::new(vec![json(
            400,
            r#"{"error_code":"refresh_token_not_found"}"#,
        )]));
        let err = restore(&stored, refused.clone(), Arc::new(TestClock::new(0, 0)))
            .err()
            .expect("refused");
        assert_eq!(err, PhoneError::Auth(AuthError::Expired));
        assert_eq!(refused.seen().len(), 1, "no signup follow-up");
        assert!(!format!("{stored:?}").contains("r1"));
    }
}
