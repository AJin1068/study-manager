//! Codes and wire types of the shared wiki. A body is another member's text: `Debug` never prints it.

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::remote::supabase::RemoteError;

/// Largest page body, in characters, the server accepts (and the app sends).
pub const MAX_BODY_CHARS: usize = 20_000;

/// The `code` of a shared-wiki RPC answer (see `fixtures/shared_wiki/rpc-contract.json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedCode {
    Ok,
    NotSignedIn,
    NotMember,
    InvalidInput,
    TooLarge,
    TooMany,
    NotFound,
    NotShared,
    NotConsented,
}

impl SharedCode {
    pub fn parse(raw: &str) -> Option<SharedCode> {
        serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SharedCode::Ok => "ok",
            SharedCode::NotSignedIn => "not_signed_in",
            SharedCode::NotMember => "not_member",
            SharedCode::InvalidInput => "invalid_input",
            SharedCode::TooLarge => "too_large",
            SharedCode::TooMany => "too_many",
            SharedCode::NotFound => "not_found",
            SharedCode::NotShared => "not_shared",
            SharedCode::NotConsented => "not_consented",
        }
    }
}

/// Why a shared-wiki call did not complete. `code()` is the only text that reaches the UI or a log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SharedError {
    #[error("{0}")]
    Remote(RemoteError),
    /// The server answered and refused.
    #[error("{}", .0.as_str())]
    Rejected(SharedCode),
    /// The app refused before anything was sent (no share mark, nothing consented, too large).
    #[error("{0}")]
    Local(&'static str),
}

impl SharedError {
    pub fn code(self) -> &'static str {
        match self {
            SharedError::Remote(err) => err.code(),
            SharedError::Rejected(code) => code.as_str(),
            SharedError::Local(code) => code,
        }
    }
}

impl From<RemoteError> for SharedError {
    fn from(err: RemoteError) -> Self {
        SharedError::Remote(err)
    }
}

impl From<SharedError> for AppError {
    fn from(err: SharedError) -> Self {
        AppError::Remote(err.code().to_string())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionKind {
    Edit,
    Merge,
}

impl std::fmt::Debug for RevisionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RevisionKind::Edit => "Edit",
            RevisionKind::Merge => "Merge",
        })
    }
}

/// One revision as the server lists it: every member's newest edit of a page, and the newest merge.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct WireRevision {
    pub revision_id: String,
    pub kind: RevisionKind,
    /// The author's nickname in this room.
    pub author: String,
    pub is_mine: bool,
    pub body: String,
    pub allow_llm_merge: bool,
    /// Stored next to a newer revision instead of replacing it (a concurrent edit).
    pub branched: bool,
    pub base_revision_id: Option<String>,
    /// Nicknames of everyone whose text is in this revision, in join order.
    pub contributors: Vec<String>,
    pub created_at_ms: i64,
}

impl std::fmt::Debug for WireRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WireRevision({}, {:?})", self.revision_id, self.kind)
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct WirePage {
    pub page_id: String,
    pub title: String,
    pub revisions: Vec<WireRevision>,
}

impl std::fmt::Debug for WirePage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WirePage({}, {} revisions)", self.page_id, self.revisions.len())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Published {
    pub revision_id: String,
    pub branched: bool,
}
