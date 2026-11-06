//! Rows and wire types of focus measurement. `FocusEventKind` mirrors the CHECK list in
//! `m007_focus.sql` (drift-tested in `focus::tests`). Rows carry app ids only, never titles.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusEventKind {
    Away,
    Return,
    IdleStart,
    IdleEnd,
    SelfReport,
}

impl FocusEventKind {
    #[cfg(test)]
    pub const ALL: [FocusEventKind; 5] = [
        FocusEventKind::Away,
        FocusEventKind::Return,
        FocusEventKind::IdleStart,
        FocusEventKind::IdleEnd,
        FocusEventKind::SelfReport,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FocusEventKind::Away => "away",
            FocusEventKind::Return => "return",
            FocusEventKind::IdleStart => "idle_start",
            FocusEventKind::IdleEnd => "idle_end",
            FocusEventKind::SelfReport => "self_report",
        }
    }

    pub fn parse(raw: &str) -> Option<FocusEventKind> {
        match raw {
            "away" => Some(FocusEventKind::Away),
            "return" => Some(FocusEventKind::Return),
            "idle_start" => Some(FocusEventKind::IdleStart),
            "idle_end" => Some(FocusEventKind::IdleEnd),
            "self_report" => Some(FocusEventKind::SelfReport),
            _ => None,
        }
    }
}

/// A `focus_events` row to insert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFocusEvent<'a> {
    pub run_id: i64,
    pub segment_id: i64,
    pub at_ms: i64,
    pub kind: FocusEventKind,
    pub app_id: Option<&'a str>,
}

/// A `focus_events` row as read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusEventRow {
    pub at_ms: i64,
    pub kind: FocusEventKind,
    pub app_id: Option<String>,
}

/// Milliseconds added to one `focus_minutes` row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MinuteDelta {
    pub minute_start_ms: i64,
    pub credited_ms: i64,
    pub away_ms: i64,
    pub idle_ms: i64,
}

/// Sum of the minute buckets of a segment or run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FocusTotals {
    pub credited_ms: i64,
    pub away_ms: i64,
    pub idle_ms: i64,
}

/// One entry of the study-app list.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StudyApp {
    pub id: i64,
    pub kind: String,
    /// Bundle id (macOS) or normalized executable path (Windows).
    pub app_id: String,
    pub enabled: bool,
    pub created_at_ms: i64,
}
