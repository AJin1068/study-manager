//! Types of the study-room layer: the codes the server answers with, the decoded server answers
//! (`Wire*`, snake_case, `Deserialize`), and what the UI receives (camelCase, `Serialize`).
//!
//! Nothing here can hold a window title, URL, app name, typed text, or the character inputs
//! (MBTI, birth year, gender): a room member is a nickname, a level, a state, minutes, and
//! equipped item ids.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AppError;
use crate::remote::supabase::RemoteError;

/// Every outcome code of the room RPCs (`fixtures/rooms/rpc-contract.json`). A code other than
/// `Ok` is an expected refusal, shown to the user as text, not an application error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomCode {
    Ok,
    NotSignedIn,
    InvalidInput,
    RoomLimit,
    NotOwner,
    TooManyInvites,
    NotFound,
    RateLimited,
    InvalidCode,
    Revoked,
    Expired,
    Exhausted,
    Banned,
    AlreadyMember,
    RoomFull,
    NotMember,
    CannotKickSelf,
    NotShared,
    SurgeRejected,
    Disabled,
    Throttled,
}

impl RoomCode {
    pub fn parse(raw: &str) -> Option<RoomCode> {
        serde_json::from_value(Value::String(raw.to_string())).ok()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RoomCode::Ok => "ok",
            RoomCode::NotSignedIn => "not_signed_in",
            RoomCode::InvalidInput => "invalid_input",
            RoomCode::RoomLimit => "room_limit",
            RoomCode::NotOwner => "not_owner",
            RoomCode::TooManyInvites => "too_many_invites",
            RoomCode::NotFound => "not_found",
            RoomCode::RateLimited => "rate_limited",
            RoomCode::InvalidCode => "invalid_code",
            RoomCode::Revoked => "revoked",
            RoomCode::Expired => "expired",
            RoomCode::Exhausted => "exhausted",
            RoomCode::Banned => "banned",
            RoomCode::AlreadyMember => "already_member",
            RoomCode::RoomFull => "room_full",
            RoomCode::NotMember => "not_member",
            RoomCode::CannotKickSelf => "cannot_kick_self",
            RoomCode::NotShared => "not_shared",
            RoomCode::SurgeRejected => "surge_rejected",
            RoomCode::Disabled => "disabled",
            RoomCode::Throttled => "throttled",
        }
    }
}

/// Why a room call did not complete. `code()` is the only text that reaches the UI or a log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RoomError {
    #[error("{0}")]
    Remote(RemoteError),
    /// The server answered and refused.
    #[error("{}", .0.as_str())]
    Rejected(RoomCode),
}

impl RoomError {
    pub fn code(self) -> &'static str {
        match self {
            RoomError::Remote(err) => err.code(),
            RoomError::Rejected(code) => code.as_str(),
        }
    }
}

impl From<RemoteError> for RoomError {
    fn from(err: RemoteError) -> Self {
        RoomError::Remote(err)
    }
}

impl From<RoomError> for AppError {
    fn from(err: RoomError) -> Self {
        AppError::Remote(err.code().to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoomRole {
    Owner,
    Member,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberState {
    Studying,
    Break,
    Idle,
}

impl MemberState {
    pub fn as_str(self) -> &'static str {
        match self {
            MemberState::Studying => "studying",
            MemberState::Break => "break",
            MemberState::Idle => "idle",
        }
    }
}

// ---- decoded server answers ----

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireRoom {
    pub room_id: String,
    pub name: String,
    pub role: RoomRole,
    pub capacity: i64,
    pub member_count: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireStatus {
    pub state: MemberState,
    pub remaining_sec: i64,
    /// Seconds between the member's last report and the server's answer.
    pub age_sec: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireDay {
    /// The member's own local date, `YYYY-MM-DD`.
    pub day: String,
    pub study_min: i64,
    pub plan_min: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct WireCharacter {
    pub species: String,
    pub hat: Option<String>,
    pub top: Option<String>,
    pub accessory: Option<String>,
    pub hand: Option<String>,
}

/// One member as the server shows them to the caller. A field the member keeps private is `None`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireMember {
    pub user_id: String,
    pub nickname: String,
    pub role: RoomRole,
    pub is_self: bool,
    pub level: Option<i64>,
    pub status: Option<WireStatus>,
    pub attempts_today: Option<i64>,
    pub days: Option<Vec<WireDay>>,
    pub character: Option<WireCharacter>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireInvite {
    pub invite_id: String,
    pub expires_at_ms: i64,
    pub max_uses: i64,
    pub uses: i64,
    pub revoked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireScheduleItem {
    pub start_at_utc_ms: i64,
    pub name: String,
    pub subject: String,
    pub sets: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireSchedule {
    pub items: Vec<WireScheduleItem>,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireRoomInfo {
    pub room_id: String,
    pub name: String,
    pub capacity: i64,
    pub role: RoomRole,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireSnapshot {
    pub server_now_ms: i64,
    pub room: WireRoomInfo,
    pub members: Vec<WireMember>,
    pub invites: Vec<WireInvite>,
    pub schedule: Option<WireSchedule>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WireDistraction {
    pub event_id: i64,
    pub user_id: String,
    pub nickname: Option<String>,
    pub at_ms: i64,
}

/// What a successful `create_invite` returns. The code exists only here: the server keeps a hash.
#[derive(Clone, PartialEq, Eq)]
pub struct CreatedInvite {
    pub invite_id: String,
    pub invite_code: String,
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for CreatedInvite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CreatedInvite { .. }")
    }
}

// ---- what the UI gets ----

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomSummary {
    pub room_id: String,
    pub name: String,
    pub role: RoomRole,
    pub member_count: i64,
    pub capacity: i64,
}

impl From<WireRoom> for RoomSummary {
    fn from(wire: WireRoom) -> Self {
        Self { room_id: wire.room_id, name: wire.name, role: wire.role, member_count: wire.member_count, capacity: wire.capacity }
    }
}

/// The room list, with whether it came from the server just now or from the local cache.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomList {
    pub rooms: Vec<RoomSummary>,
    /// True when the server could not be reached and these rows are the last known ones.
    pub stale: bool,
    pub refreshed_at_ms: Option<i64>,
}

/// Result of an action that the server may refuse (create, join, leave, kick, revoke).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomAction {
    pub code: RoomCode,
    pub room_id: Option<String>,
}

/// A new invite: the code, and the deep link/QR that also carries the server's public address.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteOutcome {
    pub code: RoomCode,
    pub invite_id: Option<String>,
    /// Shown once in the UI; never logged.
    pub invite_code: Option<String>,
    pub expires_at_ms: Option<i64>,
    pub deep_link: Option<String>,
    pub qr_svg: Option<String>,
}

impl std::fmt::Debug for InviteOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InviteOutcome { .. }")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PresenceKind {
    Studying,
    Break,
    Idle,
    Offline,
    /// The member does not share their state.
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceView {
    pub kind: PresenceKind,
    pub remaining_sec: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterView {
    pub species: String,
    pub hat: Option<String>,
    pub top: Option<String>,
    pub accessory: Option<String>,
    pub hand: Option<String>,
}

impl From<WireCharacter> for CharacterView {
    fn from(c: WireCharacter) -> Self {
        Self { species: c.species, hat: c.hat, top: c.top, accessory: c.accessory, hand: c.hand }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberView {
    pub user_id: String,
    pub nickname: String,
    pub role: RoomRole,
    pub is_self: bool,
    pub level: Option<i64>,
    pub presence: PresenceView,
    pub attempts_today: Option<i64>,
    pub character: Option<CharacterView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankRow {
    pub user_id: String,
    pub nickname: String,
    pub is_self: bool,
    /// `None` when the member does not share study time.
    pub minutes: Option<i64>,
    /// Ties share a rank (1, 1, 3). `None` for a member who does not share.
    pub rank: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteView {
    pub invite_id: String,
    pub expires_at_ms: i64,
    pub max_uses: i64,
    pub uses: i64,
    pub revoked: bool,
    pub expired: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TogetherItem {
    /// Identifies the block inside the room's timetable (its UTC start).
    pub key: String,
    pub name: String,
    pub subject: String,
    pub sets: i64,
    pub start_at_utc_ms: i64,
    /// Local date and time on this PC, `YYYY-MM-DD` / `HH:MM`.
    pub local_date: String,
    pub local_time: String,
    pub already_imported: bool,
    pub in_past: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomView {
    pub room_id: String,
    pub name: String,
    pub role: RoomRole,
    pub capacity: i64,
    pub members: Vec<MemberView>,
    pub today_ranking: Vec<RankRow>,
    pub week_ranking: Vec<RankRow>,
    pub invites: Vec<InviteView>,
    pub schedule_updated_at_ms: Option<i64>,
    pub schedule: Vec<TogetherItem>,
    pub fetched_at_ms: i64,
}

/// What the room screen polls: the last good room data plus whether the latest refresh failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedView {
    pub room: Option<RoomView>,
    /// A code such as `transport_timeout` when the latest refresh failed; the data above is then
    /// the previous good one.
    pub degraded_code: Option<String>,
    pub last_ok_ms: Option<i64>,
    pub poll_interval_ms: u64,
}

/// A friend's distraction notice: who (nickname) and when. No app, site, or count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistractionNotice {
    pub nickname: String,
    pub at_ms: i64,
}
