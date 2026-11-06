//! Outcome codes of the phone RPCs and the errors of a call. `code()` is the only text that
//! reaches the UI or a log.

use serde::{Deserialize, Serialize};

use crate::http::TransportError;
use crate::session::AuthError;

/// Every outcome code of `fixtures/phone/rpc-contract.json` (a code the phone does not know is a
/// decode error, never guessed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhoneCode {
    Ok,
    NotSignedIn,
    InvalidInput,
    AnonymousNotAllowed,
    AlreadyPaired,
    RateLimited,
    InvalidCode,
    Used,
    Expired,
    CannotSelf,
    PhoneAlreadyPaired,
    NotFound,
    NotPaired,
    StaleSeq,
    LeaseTooLong,
}

impl PhoneCode {
    pub fn parse(raw: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PhoneCode::Ok => "ok",
            PhoneCode::NotSignedIn => "not_signed_in",
            PhoneCode::InvalidInput => "invalid_input",
            PhoneCode::AnonymousNotAllowed => "anonymous_not_allowed",
            PhoneCode::AlreadyPaired => "already_paired",
            PhoneCode::RateLimited => "rate_limited",
            PhoneCode::InvalidCode => "invalid_code",
            PhoneCode::Used => "used",
            PhoneCode::Expired => "expired",
            PhoneCode::CannotSelf => "cannot_self",
            PhoneCode::PhoneAlreadyPaired => "phone_already_paired",
            PhoneCode::NotFound => "not_found",
            PhoneCode::NotPaired => "not_paired",
            PhoneCode::StaleSeq => "stale_seq",
            PhoneCode::LeaseTooLong => "lease_too_long",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PhoneError {
    #[error("{0}")]
    Auth(AuthError),
    #[error("{0}")]
    Transport(TransportError),
    #[error("http_{status}")]
    Http { status: u16 },
    #[error("decode")]
    Decode,
    /// The server answered and refused.
    #[error("{}", .0.as_str())]
    Rejected(PhoneCode),
}

impl PhoneError {
    pub fn code(self) -> &'static str {
        match self {
            PhoneError::Auth(err) => err.code(),
            PhoneError::Transport(TransportError::Timeout) => "timeout",
            PhoneError::Transport(TransportError::Network) => "network",
            PhoneError::Http { .. } => "http",
            PhoneError::Decode => "decode",
            PhoneError::Rejected(code) => code.as_str(),
        }
    }
}

impl From<AuthError> for PhoneError {
    fn from(err: AuthError) -> Self {
        PhoneError::Auth(err)
    }
}

/// What a successful claim gives the phone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedPair {
    pub pair_id: String,
    pub owner_user_id: String,
    pub server_now_ms: i64,
}

/// A lease read by the phone with the server's clock reading that anchors it. `lease` is `None`
/// until the desktop app published one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseFetch {
    pub lease: Option<link_protocol::LockLease>,
    pub server_now_ms: i64,
}
