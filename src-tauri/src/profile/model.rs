//! Types of the profile layer: the codes the server answers with, the sections and scopes, the
//! decoded server answers (`Deserialize`, snake_case on the wire) and what the UI receives
//! (camelCase). No type here can hold a window title, URL, app name, typed text, an attempt
//! count, or the character inputs.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::AppError;
use crate::remote::supabase::RemoteError;

/// Every outcome code of the profile and gift RPCs (`fixtures/profile/rpc-contract.json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileCode {
    Ok,
    NotSignedIn,
    InvalidInput,
    NotShared,
    NotFound,
    NotMember,
    CannotSelf,
    NotFriends,
    Closed,
    RateLimited,
    KeyConflict,
    NotGiftable,
    DailyLimit,
    AlreadyResolved,
    /// Local refusals (never sent by the server).
    NameRequired,
    InsufficientPoints,
    AlreadyOwned,
    UnknownItem,
    TooMany,
}

impl ProfileCode {
    pub fn parse(raw: &str) -> Option<Self> {
        serde_json::from_value(Value::String(raw.to_string())).ok()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ProfileCode::Ok => "ok",
            ProfileCode::NotSignedIn => "not_signed_in",
            ProfileCode::InvalidInput => "invalid_input",
            ProfileCode::NotShared => "not_shared",
            ProfileCode::NotFound => "not_found",
            ProfileCode::NotMember => "not_member",
            ProfileCode::CannotSelf => "cannot_self",
            ProfileCode::NotFriends => "not_friends",
            ProfileCode::Closed => "closed",
            ProfileCode::RateLimited => "rate_limited",
            ProfileCode::KeyConflict => "key_conflict",
            ProfileCode::NotGiftable => "not_giftable",
            ProfileCode::DailyLimit => "daily_limit",
            ProfileCode::AlreadyResolved => "already_resolved",
            ProfileCode::NameRequired => "name_required",
            ProfileCode::InsufficientPoints => "insufficient_points",
            ProfileCode::AlreadyOwned => "already_owned",
            ProfileCode::UnknownItem => "unknown_item",
            ProfileCode::TooMany => "too_many",
        }
    }
}

/// Why a profile call did not complete. `code()` is the only text that reaches the UI or a log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("{0}")]
    Remote(RemoteError),
    /// The server answered and refused.
    #[error("{}", .0.as_str())]
    Rejected(ProfileCode),
}

impl ProfileError {
    pub fn code(self) -> &'static str {
        match self {
            ProfileError::Remote(err) => err.code(),
            ProfileError::Rejected(code) => code.as_str(),
        }
    }

    /// Whether the server certainly did not keep the request: it answered with a refusal, or the
    /// request never left (not signed in, body not encodable), or a 4xx other than a timeout or
    /// throttle. A timeout, lost connection, 5xx or unreadable answer is NOT certain: the server
    /// may have processed the request and only the answer was lost.
    pub fn is_certain_refusal(self) -> bool {
        match self {
            ProfileError::Rejected(_) => true,
            ProfileError::Remote(RemoteError::NotSignedIn | RemoteError::AuthExpired | RemoteError::Encode) => true,
            ProfileError::Remote(RemoteError::Http { status }) => (400..500).contains(&status) && status != 408 && status != 429,
            ProfileError::Remote(RemoteError::Transport(_) | RemoteError::Decode) => false,
        }
    }
}

impl From<RemoteError> for ProfileError {
    fn from(err: RemoteError) -> Self {
        ProfileError::Remote(err)
    }
}

impl From<ProfileError> for AppError {
    fn from(err: ProfileError) -> Self {
        AppError::Remote(err.code().to_string())
    }
}

/// The profile sections. Matches `profile_visibility.section` and the SQL `vis_*` columns
/// (drift-tested).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Room,
    Grass,
    Level,
    Badges,
    Guestbook,
    Visitors,
    Wiki,
    Subject,
    Gifts,
}

impl Section {
    pub const ALL: [Section; 9] = [
        Section::Room,
        Section::Grass,
        Section::Level,
        Section::Badges,
        Section::Guestbook,
        Section::Visitors,
        Section::Wiki,
        Section::Subject,
        Section::Gifts,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Section::Room => "room",
            Section::Grass => "grass",
            Section::Level => "level",
            Section::Badges => "badges",
            Section::Guestbook => "guestbook",
            Section::Visitors => "visitors",
            Section::Wiki => "wiki",
            Section::Subject => "subject",
            Section::Gifts => "gifts",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// Who may see a section. Matches `profile_visibility.scope` (drift-tested). `Private` is the
/// default of every section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Public,
    Friends,
    Private,
}

impl Scope {
    pub const ALL: [Scope; 3] = [Scope::Public, Scope::Friends, Scope::Private];

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Public => "public",
            Scope::Friends => "friends",
            Scope::Private => "private",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

// ---- decoded server answers ----

/// A profile as the server answered: only the sections the viewer may see are keys of `sections`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct WireProfile {
    pub name: String,
    #[serde(default)]
    pub slug: Option<String>,
    pub sections: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomLook {
    pub species: Option<String>,
    pub hat: Option<String>,
    pub top: Option<String>,
    pub accessory: Option<String>,
    pub hand: Option<String>,
    #[serde(default)]
    pub furniture: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrassDay {
    pub day: String,
    #[serde(alias = "study_min")]
    pub study_min: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestEntry {
    pub id: i64,
    #[serde(alias = "author_name")]
    pub author_name: String,
    pub body: String,
    #[serde(alias = "at_ms")]
    pub at_ms: i64,
    pub mine: bool,
    #[serde(default)]
    pub reported: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Visitors {
    pub total: i64,
    pub today: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GiftedItem {
    #[serde(alias = "item_id")]
    pub item_id: String,
    #[serde(alias = "sender_name")]
    pub sender_name: String,
    pub message: String,
    #[serde(alias = "at_ms")]
    pub at_ms: Option<i64>,
}

/// What the profile screen draws. A section the viewer may not see is `None` and absent from
/// `visible`, which is how the UI tells "hidden" from "empty".
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    pub code: ProfileCode,
    pub name: Option<String>,
    pub slug: Option<String>,
    pub visible: Vec<Section>,
    pub room: Option<RoomLook>,
    pub grass: Option<Vec<GrassDay>>,
    pub level: Option<i64>,
    pub badges: Option<Vec<String>>,
    pub guestbook: Option<Vec<GuestEntry>>,
    pub visitors: Option<Visitors>,
    pub wiki: Option<Vec<String>>,
    pub subject: Option<String>,
    pub gifts: Option<Vec<GiftedItem>>,
}

impl ProfileView {
    pub fn empty(code: ProfileCode) -> Self {
        Self {
            code,
            name: None,
            slug: None,
            visible: Vec::new(),
            room: None,
            grass: None,
            level: None,
            badges: None,
            guestbook: None,
            visitors: None,
            wiki: None,
            subject: None,
            gifts: None,
        }
    }

    /// Decodes a server answer. An unreadable section is a decode error, not a silent blank.
    pub fn from_wire(wire: WireProfile) -> Result<Self, ProfileError> {
        fn part<T: serde::de::DeserializeOwned>(sections: &Map<String, Value>, key: &str) -> Result<Option<T>, ProfileError> {
            match sections.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => serde_json::from_value(value.clone()).map(Some).map_err(|_| ProfileError::Remote(RemoteError::Decode)),
            }
        }
        let s = &wire.sections;
        let visible = Section::ALL.into_iter().filter(|section| s.contains_key(section.as_str())).collect();
        Ok(Self {
            code: ProfileCode::Ok,
            name: Some(wire.name.clone()),
            slug: wire.slug.clone(),
            visible,
            room: part(s, "room")?,
            grass: part(s, "grass")?,
            level: part(s, "level")?,
            badges: part(s, "badges")?,
            guestbook: part(s, "guestbook")?,
            visitors: part(s, "visitors")?,
            wiki: part(s, "wiki")?,
            subject: part(s, "subject")?,
            gifts: part(s, "gifts")?,
        })
    }
}

/// One gift waiting in (or accepted from) the box.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxGift {
    #[serde(alias = "gift_id")]
    pub gift_id: String,
    #[serde(alias = "item_id")]
    pub item_id: String,
    #[serde(alias = "sender_name")]
    pub sender_name: String,
    pub message: String,
    pub status: String,
    #[serde(alias = "at_ms")]
    pub at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GiftState {
    None,
    Pending,
    Accepted,
    Declined,
}

impl GiftState {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "none" => Some(GiftState::None),
            "pending" => Some(GiftState::Pending),
            "accepted" => Some(GiftState::Accepted),
            "declined" => Some(GiftState::Declined),
            _ => None,
        }
    }
}
