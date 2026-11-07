//! Everything that may leave the device is a type listed in this file (or in `llm::egress`).
//!
//! `EgressPayload` is sealed: its supertrait lives in a crate-private module, so code outside
//! this crate cannot implement it, and code inside the crate can only do so through
//! `egress_payload!` next to a reviewed struct. A request body is built with
//! `Body::from_payload`, which accepts only `EgressPayload` types, so an arbitrary `Serialize`
//! value (a window title, a URL) has no way into a request. The structs have no field for window
//! titles, URLs, or typed text at all.

use std::fmt;

use serde::Serialize;

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// A reviewed, serializable outbound body. Not implementable outside this crate.
pub trait EgressPayload: sealed::Sealed + Serialize {}

/// Implements `EgressPayload` and a `Debug` that prints no field values, so a payload formatted
/// by mistake (logs, panics, assertion messages) leaks nothing.
macro_rules! egress_payload {
    ($ty:ty) => {
        impl $crate::remote::payload::sealed::Sealed for $ty {}
        impl $crate::remote::payload::EgressPayload for $ty {}
        impl ::std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(concat!(stringify!($ty), " { .. }"))
            }
        }
    };
}

// `llm::egress` reviews its own payload structs and attaches them with the same macro.
pub(crate) use egress_payload;

/// A request body that was built from an `EgressPayload`. There is no other constructor.
///
/// ```compile_fail,E0277
/// // An arbitrary Serialize type cannot be turned into a body: EgressPayload is sealed.
/// use study_manager_lib::remote::payload::{Body, EgressPayload};
/// #[derive(serde::Serialize)]
/// struct Rogue { window_title: String }
/// impl EgressPayload for Rogue {}
/// ```
///
/// ```compile_fail,E0277
/// use study_manager_lib::remote::payload::Body;
/// #[derive(serde::Serialize)]
/// struct Rogue { window_title: String }
/// let rogue = Rogue { window_title: String::new() };
/// let _ = Body::from_payload(&rogue);
/// ```
///
/// ```compile_fail,E0451
/// // Fields are private: a body cannot be assembled from raw bytes either.
/// use study_manager_lib::remote::payload::Body;
/// let _ = Body { bytes: Vec::new(), content_type: None };
/// ```
pub struct Body {
    bytes: Vec<u8>,
    content_type: Option<&'static str>,
}

impl Body {
    pub fn from_payload<P: EgressPayload>(payload: &P) -> Result<Self, serde_json::Error> {
        Ok(Self {
            bytes: serde_json::to_vec(payload)?,
            content_type: Some("application/json"),
        })
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Plain-text body for a phone notification (ntfy takes the text as the message). Only the
    /// fixed-phrase `NoticeText` can become one, so no free text can ride along.
    pub fn from_notice(notice: &NoticeText) -> Self {
        Self {
            bytes: notice.text.clone().into_bytes(),
            content_type: Some("text/plain; charset=utf-8"),
        }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn into_parts(self) -> (Vec<u8>, Option<&'static str>) {
        (self.bytes, self.content_type)
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Body({} bytes)", self.bytes.len())
    }
}

// ---- auth (GoTrue) ----

/// `POST /auth/v1/otp`: asks for a 6-digit code by mail.
#[derive(Serialize)]
pub struct OtpRequest {
    pub email: String,
    pub create_user: bool,
}
egress_payload!(OtpRequest);

/// `POST /auth/v1/verify`.
#[derive(Serialize)]
pub struct OtpVerify {
    pub email: String,
    pub token: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
}
egress_payload!(OtpVerify);

/// `POST /auth/v1/token?grant_type=refresh_token`.
#[derive(Serialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}
egress_payload!(RefreshRequest);

// ---- PostgREST / RPC ----

/// Row inserted into `devices` when a signed-in app first talks to the server. `user_id` is
/// filled by the server default (`auth.uid()`), never sent.
#[derive(Serialize)]
pub struct DeviceRegistration {
    pub id: String,
    pub app_version: String,
    pub platform: &'static str,
}
egress_payload!(DeviceRegistration);

/// Arguments of the `heartbeat` RPC (stage 2-5 sends it every 60 seconds). Only the lock state
/// as a boolean, never what the user is doing.
#[derive(Serialize)]
pub struct PresenceBeat {
    #[serde(rename = "p_device_id")]
    pub device_id: String,
    #[serde(rename = "p_app_version")]
    pub app_version: String,
    #[serde(rename = "p_studying")]
    pub studying: bool,
}
egress_payload!(PresenceBeat);

/// One expanded plan occurrence in the mirror snapshot. The desktop expands repeats; the server
/// never interprets a repeat rule. Plan name and subject are allowed on the server (AGENTS section 6);
/// there is no field for anything else.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct OccurrenceRow {
    pub occurrence_key: String,
    pub start_at_utc_ms: i64,
    pub plan_name: String,
    pub subject: String,
    pub sets: i64,
    pub status: &'static str,
}

/// Arguments of the `replace_plan_snapshot` RPC.
#[derive(Serialize)]
pub struct SnapshotArgs {
    #[serde(rename = "p_device_id")]
    pub device_id: String,
    #[serde(rename = "p_snapshot_hash")]
    pub snapshot_hash: String,
    #[serde(rename = "p_generated_at_ms")]
    pub generated_at_ms: i64,
    #[serde(rename = "p_horizon_days")]
    pub horizon_days: i64,
    #[serde(rename = "p_occurrences")]
    pub occurrences: Vec<OccurrenceRow>,
}
egress_payload!(SnapshotArgs);

/// One `plan_history` row for the server mirror. `(device_id, local_id)` is the idempotency key.
#[derive(Serialize, Clone)]
pub struct PlanHistoryRow {
    pub device_id: String,
    pub local_id: i64,
    pub plan_local_id: Option<i64>,
    pub at_ms: i64,
    pub action: String,
    pub reason_code: String,
    pub reason_text: Option<String>,
    pub before_json: Option<String>,
    pub after_json: Option<String>,
    pub outcome: String,
    pub batch_id: Option<String>,
}

/// A batch of history rows, sent as a JSON array (one PostgREST bulk insert).
#[derive(Serialize)]
#[serde(transparent)]
pub struct PlanHistoryBatch(pub Vec<PlanHistoryRow>);
egress_payload!(PlanHistoryBatch);

/// The user's notification channel as the server needs it to send a missed-plan alert. The ntfy
/// topic works like a password (whoever knows it reads the alerts); the Telegram bot token is
/// not here, it lives only in the Edge Function's secrets. `utc_offset_min` lets the server print
/// the plan's local clock time.
#[derive(Serialize)]
pub struct NotifyChannelRow {
    pub channel: String,
    pub ntfy_topic: Option<String>,
    pub ntfy_server: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub missed_grace_min: i64,
    pub utc_offset_min: i64,
}
egress_payload!(NotifyChannelRow);

/// A phone notification body. Fixed phrases only (no subject or plan names: it shows on a lock
/// screen). Built by `remote::notify`.
pub struct NoticeText {
    text: String,
}

impl NoticeText {
    pub(crate) fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl Serialize for NoticeText {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("NoticeText", 1)?;
        state.serialize_field("message", &self.text)?;
        state.end()
    }
}
egress_payload!(NoticeText);

/// Body of the Edge Function's "send me a test notification" route (Telegram: the bot token is
/// server-side, so the app asks the server to send it).
#[derive(Serialize)]
pub struct NotifyTestRequest {
    pub action: &'static str,
}
egress_payload!(NotifyTestRequest);

// ---- study rooms (stage 4) ----
//
// Argument structs of the room RPCs. None has a field for a window title, URL, app name, typed
// text, MBTI, birth year, or gender. Field names are the RPC parameter names.

/// Arguments of an RPC that takes none (`list_my_rooms`, `report_distraction`).
#[derive(Serialize)]
pub struct NoArgs {}
egress_payload!(NoArgs);

/// `create_room`: a room name and the nickname the creator uses inside it.
#[derive(Serialize)]
pub struct CreateRoomArgs {
    #[serde(rename = "p_name")]
    pub name: String,
    #[serde(rename = "p_nickname")]
    pub nickname: String,
}
egress_payload!(CreateRoomArgs);

#[derive(Serialize)]
pub struct CreateInviteArgs {
    #[serde(rename = "p_room_id")]
    pub room_id: String,
    #[serde(rename = "p_ttl_hours")]
    pub ttl_hours: i64,
    #[serde(rename = "p_max_uses")]
    pub max_uses: i64,
}
egress_payload!(CreateInviteArgs);

#[derive(Serialize)]
pub struct RevokeInviteArgs {
    #[serde(rename = "p_room_id")]
    pub room_id: String,
    #[serde(rename = "p_invite_id")]
    pub invite_id: String,
}
egress_payload!(RevokeInviteArgs);

/// `join_room`. The code is a credential: it is held only for this call and `Debug` hides it.
#[derive(Serialize)]
pub struct JoinRoomArgs {
    #[serde(rename = "p_code")]
    pub code: String,
    #[serde(rename = "p_nickname")]
    pub nickname: String,
}
egress_payload!(JoinRoomArgs);

/// `leave_room` and `room_snapshot`.
#[derive(Serialize)]
pub struct RoomIdArgs {
    #[serde(rename = "p_room_id")]
    pub room_id: String,
}
egress_payload!(RoomIdArgs);

#[derive(Serialize)]
pub struct KickMemberArgs {
    #[serde(rename = "p_room_id")]
    pub room_id: String,
    #[serde(rename = "p_target_user")]
    pub target_user: String,
}
egress_payload!(KickMemberArgs);

/// `report_status`: the lock phase, time left in it, level, and (only when the user shares it)
/// today's distraction count.
#[derive(Serialize, Clone, PartialEq, Eq)]
pub struct StatusReportArgs {
    #[serde(rename = "p_state")]
    pub state: &'static str,
    #[serde(rename = "p_remaining_sec")]
    pub remaining_sec: i64,
    #[serde(rename = "p_level")]
    pub level: i64,
    #[serde(rename = "p_attempts_today")]
    pub attempts_today: Option<i64>,
}
egress_payload!(StatusReportArgs);

/// `report_daily`: minutes studied and planned on the user's local date `day` (YYYY-MM-DD).
#[derive(Serialize, Clone, PartialEq, Eq)]
pub struct DailyReportArgs {
    #[serde(rename = "p_day")]
    pub day: String,
    #[serde(rename = "p_study_min")]
    pub study_min: i64,
    #[serde(rename = "p_plan_min")]
    pub plan_min: i64,
}
egress_payload!(DailyReportArgs);

/// `set_friend_prefs`: what the room may see and which distraction notices flow.
#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
pub struct FriendPrefsArgs {
    #[serde(rename = "p_share_level")]
    pub share_level: bool,
    #[serde(rename = "p_share_status")]
    pub share_status: bool,
    #[serde(rename = "p_share_week_time")]
    pub share_week_time: bool,
    #[serde(rename = "p_share_attempts")]
    pub share_attempts: bool,
    #[serde(rename = "p_distraction_publish")]
    pub distraction_publish: bool,
    #[serde(rename = "p_distraction_receive")]
    pub distraction_receive: bool,
}
egress_payload!(FriendPrefsArgs);

/// One block of the timetable the room owner shares.
#[derive(Serialize, Clone, PartialEq, Eq)]
pub struct ScheduleItemRow {
    pub start_at_utc_ms: i64,
    pub name: String,
    pub subject: String,
    pub sets: i64,
}

#[derive(Serialize)]
pub struct ShareScheduleArgs {
    #[serde(rename = "p_room_id")]
    pub room_id: String,
    #[serde(rename = "p_items")]
    pub items: Vec<ScheduleItemRow>,
}
egress_payload!(ShareScheduleArgs);

#[derive(Serialize)]
pub struct PollDistractionArgs {
    #[serde(rename = "p_since_ms")]
    pub since_ms: i64,
}
egress_payload!(PollDistractionArgs);

/// Arguments of the `set_inventory_public` RPC (4th stage, shop): the species and the item id
/// worn per slot, so friends see the character. Ids only: no name, no setup answers (MBTI, birth
/// year, gender), no owned-but-unworn items, no prices.
#[derive(Serialize, Clone, PartialEq, Eq)]
pub struct InventoryPublicArgs {
    #[serde(rename = "p_species")]
    pub species: String,
    #[serde(rename = "p_hat")]
    pub hat: Option<String>,
    #[serde(rename = "p_top")]
    pub top: Option<String>,
    #[serde(rename = "p_accessory")]
    pub accessory: Option<String>,
    #[serde(rename = "p_hand")]
    pub hand: Option<String>,
}
egress_payload!(InventoryPublicArgs);

#[cfg(test)]
pub(crate) const CANARY_TITLE: &str = "CANARY_TITLE_7f3a";
#[cfg(test)]
pub(crate) const CANARY_URL: &str = "https://canary.example/secret-path";

/// JSON keys no outbound payload may contain (AGENTS section 9), checked recursively.
#[cfg(test)]
pub(crate) fn forbidden_key(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "title"
        || lower == "window_title"
        || lower == "url"
        || lower.starts_with("typed")
        || lower.starts_with("sentence")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every payload type, serialized. A stage that adds an `EgressPayload` adds a sample here;
    /// `payload_samples_are_complete` fails when the list and the macro uses drift apart.
    fn samples() -> Vec<(&'static str, serde_json::Value)> {
        fn json<P: EgressPayload>(p: &P) -> serde_json::Value {
            serde_json::to_value(p).expect("serialize")
        }
        vec![
            (
                "OtpRequest",
                json(&OtpRequest {
                    email: "a@b.test".into(),
                    create_user: true,
                }),
            ),
            (
                "OtpVerify",
                json(&OtpVerify {
                    email: "a@b.test".into(),
                    token: "000000".into(),
                    kind: "email",
                }),
            ),
            (
                "RefreshRequest",
                json(&RefreshRequest {
                    refresh_token: "r".into(),
                }),
            ),
            (
                "DeviceRegistration",
                json(&DeviceRegistration {
                    id: "d".into(),
                    app_version: "0.1.0".into(),
                    platform: "macos",
                }),
            ),
            (
                "PresenceBeat",
                json(&PresenceBeat {
                    device_id: "d".into(),
                    app_version: "0.1.0".into(),
                    studying: true,
                }),
            ),
            (
                "SnapshotArgs",
                json(&SnapshotArgs {
                    device_id: "d".into(),
                    snapshot_hash: "v1-0".into(),
                    generated_at_ms: 1,
                    horizon_days: 14,
                    occurrences: vec![OccurrenceRow {
                        occurrence_key: "s1:2026-06-01".into(),
                        start_at_utc_ms: 1,
                        plan_name: "Math".into(),
                        subject: "math".into(),
                        sets: 2,
                        status: "planned",
                    }],
                }),
            ),
            (
                "PlanHistoryBatch",
                json(&PlanHistoryBatch(vec![PlanHistoryRow {
                    device_id: "d".into(),
                    local_id: 1,
                    plan_local_id: Some(1),
                    at_ms: 1,
                    action: "create".into(),
                    reason_code: "created".into(),
                    reason_text: None,
                    before_json: None,
                    after_json: Some("{}".into()),
                    outcome: "applied".into(),
                    batch_id: None,
                }])),
            ),
            (
                "NotifyChannelRow",
                json(&NotifyChannelRow {
                    channel: "ntfy".into(),
                    ntfy_topic: Some("a".repeat(24)),
                    ntfy_server: Some("https://ntfy.sh".into()),
                    telegram_chat_id: None,
                    missed_grace_min: 3,
                    utc_offset_min: 540,
                }),
            ),
            ("NoticeText", json(&NoticeText::new("test"))),
            (
                "NotifyTestRequest",
                json(&NotifyTestRequest { action: "test" }),
            ),
            ("NoArgs", json(&NoArgs {})),
            (
                "CreateRoomArgs",
                json(&CreateRoomArgs {
                    name: "room".into(),
                    nickname: "nick".into(),
                }),
            ),
            (
                "CreateInviteArgs",
                json(&CreateInviteArgs {
                    room_id: "r".into(),
                    ttl_hours: 24,
                    max_uses: 10,
                }),
            ),
            (
                "RevokeInviteArgs",
                json(&RevokeInviteArgs {
                    room_id: "r".into(),
                    invite_id: "i".into(),
                }),
            ),
            (
                "JoinRoomArgs",
                json(&JoinRoomArgs {
                    code: "0123456789".into(),
                    nickname: "nick".into(),
                }),
            ),
            (
                "RoomIdArgs",
                json(&RoomIdArgs {
                    room_id: "r".into(),
                }),
            ),
            (
                "KickMemberArgs",
                json(&KickMemberArgs {
                    room_id: "r".into(),
                    target_user: "u".into(),
                }),
            ),
            (
                "StatusReportArgs",
                json(&StatusReportArgs {
                    state: "studying",
                    remaining_sec: 60,
                    level: 3,
                    attempts_today: Some(1),
                }),
            ),
            (
                "DailyReportArgs",
                json(&DailyReportArgs {
                    day: "2026-11-06".into(),
                    study_min: 30,
                    plan_min: 60,
                }),
            ),
            (
                "FriendPrefsArgs",
                json(&FriendPrefsArgs {
                    share_level: true,
                    share_status: true,
                    share_week_time: false,
                    share_attempts: false,
                    distraction_publish: false,
                    distraction_receive: false,
                }),
            ),
            (
                "ShareScheduleArgs",
                json(&ShareScheduleArgs {
                    room_id: "r".into(),
                    items: vec![ScheduleItemRow {
                        start_at_utc_ms: 1,
                        name: "Math".into(),
                        subject: "math".into(),
                        sets: 2,
                    }],
                }),
            ),
            (
                "PollDistractionArgs",
                json(&PollDistractionArgs { since_ms: 0 }),
            ),
            (
                "InventoryPublicArgs",
                json(&InventoryPublicArgs {
                    species: "fox".into(),
                    hat: Some("hat_beanie".into()),
                    top: None,
                    accessory: None,
                    hand: None,
                }),
            ),
        ]
    }

    fn keys(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    out.push(k.clone());
                    keys(v, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| keys(v, out)),
            _ => {}
        }
    }

    #[test]
    fn egress_payload_guard_no_forbidden_keys_or_canaries() {
        for (name, value) in samples() {
            let mut found = Vec::new();
            keys(&value, &mut found);
            // `NoArgs` is the empty object on purpose: it carries nothing at all.
            assert!(
                name == "NoArgs" || !found.is_empty(),
                "{name} serialized no fields"
            );
            for key in found {
                assert!(!forbidden_key(&key), "{name} has forbidden key {key}");
            }
            let text = value.to_string();
            assert!(
                !text.contains(CANARY_TITLE) && !text.contains(CANARY_URL),
                "{name} leaked a canary"
            );
        }
    }

    #[test]
    fn payload_samples_are_complete() {
        let source = include_str!("payload.rs");
        let implemented = source
            .lines()
            .filter(|l| l.starts_with("egress_payload!("))
            .count();
        assert_eq!(
            samples().len(),
            implemented,
            "add a sample for every egress_payload! type"
        );
    }

    #[test]
    fn debug_hides_field_values() {
        let verify = OtpVerify {
            email: "person@example.test".into(),
            token: "123456".into(),
            kind: "email",
        };
        let shown = format!("{verify:?}");
        assert!(
            !shown.contains("person") && !shown.contains("123456"),
            "{shown}"
        );
        let body = Body::from_payload(&verify).expect("body");
        assert_eq!(format!("{body:?}"), format!("Body({} bytes)", body.len()));
    }

    #[test]
    fn heartbeat_args_use_rpc_parameter_names() {
        let value = serde_json::to_value(PresenceBeat {
            device_id: "d".into(),
            app_version: "1".into(),
            studying: false,
        })
        .expect("json");
        assert_eq!(
            value,
            serde_json::json!({"p_device_id":"d","p_app_version":"1","p_studying":false})
        );
    }
}
