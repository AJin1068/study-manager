//! Types of the pairing layer: the codes the server answers with, the decoded answers and the
//! views the UI receives (camelCase). Nothing here can hold an app name, a window title, a URL or
//! typed text: a phone reports counts and ids only.

use link_protocol::{LeaseState, LockLease, ReleaseCause};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AppError;
use crate::remote::supabase::RemoteError;

/// Every outcome code of the phone RPCs (`fixtures/phone/rpc-contract.json`) and the local refusals.
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
    /// Local refusal (never sent by the server): the pairing may not be ended during a study run.
    RunLocked,
    /// Local refusal: the scanned text is not a pairing offer.
    BadOffer,
}

impl PhoneCode {
    pub fn parse(raw: &str) -> Option<Self> {
        serde_json::from_value(Value::String(raw.to_string())).ok()
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
            PhoneCode::RunLocked => "run_locked",
            PhoneCode::BadOffer => "bad_offer",
        }
    }
}

/// Why a phone call did not complete. `code()` is the only text that reaches the UI or a log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PhoneError {
    #[error("{0}")]
    Remote(RemoteError),
    /// The server answered and refused.
    #[error("{}", .0.as_str())]
    Rejected(PhoneCode),
    /// `stale_seq` with the sequence number the server holds, so the publisher can catch up.
    #[error("stale_seq")]
    Stale { current_seq: i64 },
}

impl PhoneError {
    pub fn code(self) -> &'static str {
        match self {
            PhoneError::Remote(err) => err.code(),
            PhoneError::Rejected(code) => code.as_str(),
            PhoneError::Stale { .. } => PhoneCode::StaleSeq.as_str(),
        }
    }
}

impl From<RemoteError> for PhoneError {
    fn from(err: RemoteError) -> Self {
        PhoneError::Remote(err)
    }
}

impl From<PhoneError> for AppError {
    fn from(err: PhoneError) -> Self {
        AppError::Remote(err.code().to_string())
    }
}

pub fn state_text(state: LeaseState) -> &'static str {
    match state {
        LeaseState::Locked => "locked",
        LeaseState::Free => "free",
    }
}

pub fn cause_text(cause: ReleaseCause) -> &'static str {
    match cause {
        ReleaseCause::RunEnded => "run_ended",
        ReleaseCause::EmergencyPass => "emergency_pass",
        ReleaseCause::SkipToday => "skip_today",
        ReleaseCause::EarlyRelease => "early_release",
        ReleaseCause::PairRevoked => "pair_revoked",
    }
}

/// A fresh one-time code as the server made it.
#[derive(Clone, PartialEq, Eq)]
pub struct NewPairCode {
    pub pair_code: String,
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for NewPairCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NewPairCode { .. }")
    }
}

/// What a successful claim gives the phone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedPair {
    pub pair_id: String,
    pub owner_user_id: String,
    pub server_now_ms: i64,
}

/// The owner's view of the pairing from `phone_status`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireStatus {
    pub paired: bool,
    #[serde(default)]
    pub paired_at_ms: Option<i64>,
    #[serde(default)]
    pub lease_seq: i64,
    #[serde(default)]
    pub last_beat_ms: Option<i64>,
    #[serde(default)]
    pub service_enabled: Option<bool>,
    /// The server's clock reading of this answer; the desktop turns the server's beat time into
    /// its own clock with it.
    #[serde(default)]
    pub server_now_ms: Option<i64>,
}

/// One pick-up report as the server holds it: a segment key and a cumulative count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickupRow {
    pub segment_id: String,
    pub count: i64,
}

/// A lease read by the phone, with the server's clock reading that anchors it. `lease` is `None`
/// until the desktop app published one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseFetch {
    pub lease: Option<LockLease>,
    pub server_now_ms: i64,
}

/// What the server says about a publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublishAck {
    pub seq: i64,
    pub replayed: bool,
    pub server_now_ms: i64,
}

// ---- UI views ----

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairStart {
    pub code: PhoneCode,
    /// `ABCDE-FGHJK`, to read aloud or type when scanning is not possible.
    pub display_code: Option<String>,
    pub expires_at_ms: Option<i64>,
    pub qr_svg: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairStatusView {
    pub paired: bool,
    pub paired_at_ms: Option<i64>,
    pub last_beat_ms: Option<i64>,
    /// `Some(false)` when the phone reported its blocking service switched off.
    pub service_enabled: Option<bool>,
    /// Whether the server could be reached for this refresh; when not, the rest is the last known.
    pub online: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PhoneAck {
    pub code: PhoneCode,
}
