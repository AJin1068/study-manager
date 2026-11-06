//! Rows and wire types of app blocking. The enums mirror the CHECK lists in `m004_block.sql`
//! (drift-tested in `enforce::tests`). Variants not used until stage 1-4 are marked.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnforceMode {
    DryRun,
    Enforce,
}

impl EnforceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            EnforceMode::DryRun => "dryRun",
            EnforceMode::Enforce => "enforce",
        }
    }

    pub fn parse(raw: &str) -> Option<EnforceMode> {
        match raw {
            "dryRun" => Some(EnforceMode::DryRun),
            "enforce" => Some(EnforceMode::Enforce),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // `Site` entries arrive with stage 1-4; the DB CHECK reserves it now.
pub enum EntryKind {
    App,
    Site,
}

impl EntryKind {
    #[cfg(test)]
    pub const ALL: [EntryKind; 2] = [EntryKind::App, EntryKind::Site];

    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::App => "app",
            EntryKind::Site => "site",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // `Preset` entries arrive with stage 1-4.
pub enum EntrySource {
    User,
    Preset,
}

impl EntrySource {
    #[cfg(test)]
    pub const ALL: [EntrySource; 2] = [EntrySource::User, EntrySource::Preset];

    pub fn as_str(self) -> &'static str {
        match self {
            EntrySource::User => "user",
            EntrySource::Preset => "preset",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // `Site` and `Extension` are recorded by stage 1-4.
pub enum TargetKind {
    App,
    Site,
    Extension,
}

impl TargetKind {
    #[cfg(test)]
    pub const ALL: [TargetKind; 3] = [TargetKind::App, TargetKind::Site, TargetKind::Extension];

    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::App => "app",
            TargetKind::Site => "site",
            TargetKind::Extension => "extension",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // The pass, site, and extension actions are written by stages 1-3 (S4) and 1-4.
pub enum BlockAction {
    WouldTerminate,
    TerminateRequested,
    Terminated,
    ForceKilled,
    Failed,
    SkippedProtected,
    SkippedPass,
    SiteBlocked,
    SiteAllowedPass,
    ExtensionDisconnected,
    ExtensionConnected,
}

impl BlockAction {
    #[cfg(test)]
    pub const ALL: [BlockAction; 11] = [
        BlockAction::WouldTerminate,
        BlockAction::TerminateRequested,
        BlockAction::Terminated,
        BlockAction::ForceKilled,
        BlockAction::Failed,
        BlockAction::SkippedProtected,
        BlockAction::SkippedPass,
        BlockAction::SiteBlocked,
        BlockAction::SiteAllowedPass,
        BlockAction::ExtensionDisconnected,
        BlockAction::ExtensionConnected,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            BlockAction::WouldTerminate => "would_terminate",
            BlockAction::TerminateRequested => "terminate_requested",
            BlockAction::Terminated => "terminated",
            BlockAction::ForceKilled => "force_killed",
            BlockAction::Failed => "failed",
            BlockAction::SkippedProtected => "skipped_protected",
            BlockAction::SkippedPass => "skipped_pass",
            BlockAction::SiteBlocked => "site_blocked",
            BlockAction::SiteAllowedPass => "site_allowed_pass",
            BlockAction::ExtensionDisconnected => "extension_disconnected",
            BlockAction::ExtensionConnected => "extension_connected",
        }
    }
}

/// A `block_entries` row.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlockEntry {
    pub id: i64,
    pub kind: String,
    pub pattern: String,
    pub source: String,
    pub category: Option<String>,
    pub enabled: bool,
    pub created_at_ms: i64,
}

/// A `block_events` row as the UI reads it.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlockEvent {
    pub id: i64,
    pub at_ms: i64,
    pub run_id: Option<i64>,
    pub segment_id: Option<i64>,
    pub target_kind: String,
    pub target_id: String,
    pub action: String,
    pub mode: String,
    pub detail_code: Option<String>,
}

/// A `block_events` row to insert. `target_id` is an app id or a domain, never a title or URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewBlockEvent<'a> {
    pub at_ms: i64,
    pub run_id: Option<i64>,
    pub segment_id: Option<i64>,
    pub target_kind: TargetKind,
    pub target_id: &'a str,
    pub action: BlockAction,
    pub mode: EnforceMode,
    pub detail_code: Option<&'a str>,
}
