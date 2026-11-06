//! Rows and wire types of the timer. Enums mirror the CHECK lists in `m003_timer.sql`
//! (drift-tested in `timer::tests`).

use serde::Serialize;

use crate::plan::BlockKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Active,
    Completed,
    Missed,
    Skipped,
    Aborted,
}

impl RunStatus {
    #[cfg(test)]
    pub const ALL: [RunStatus; 5] =
        [RunStatus::Active, RunStatus::Completed, RunStatus::Missed, RunStatus::Skipped, RunStatus::Aborted];

    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Active => "active",
            RunStatus::Completed => "completed",
            RunStatus::Missed => "missed",
            RunStatus::Skipped => "skipped",
            RunStatus::Aborted => "aborted",
        }
    }

    pub fn parse(raw: &str) -> Option<RunStatus> {
        match raw {
            "active" => Some(RunStatus::Active),
            "completed" => Some(RunStatus::Completed),
            "missed" => Some(RunStatus::Missed),
            "skipped" => Some(RunStatus::Skipped),
            "aborted" => Some(RunStatus::Aborted),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    Elapsed,
    ElapsedWhileDown,
    Skipped,
    Aborted,
    EarlyRelease,
}

impl EndReason {
    #[cfg(test)]
    pub const ALL: [EndReason; 5] = [
        EndReason::Elapsed,
        EndReason::ElapsedWhileDown,
        EndReason::Skipped,
        EndReason::Aborted,
        EndReason::EarlyRelease,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EndReason::Elapsed => "elapsed",
            EndReason::ElapsedWhileDown => "elapsed_while_down",
            EndReason::Skipped => "skipped",
            EndReason::Aborted => "aborted",
            EndReason::EarlyRelease => "early_release",
        }
    }

    pub fn parse(raw: &str) -> Option<EndReason> {
        match raw {
            "elapsed" => Some(EndReason::Elapsed),
            "elapsed_while_down" => Some(EndReason::ElapsedWhileDown),
            "skipped" => Some(EndReason::Skipped),
            "aborted" => Some(EndReason::Aborted),
            "early_release" => Some(EndReason::EarlyRelease),
            _ => None,
        }
    }
}

pub fn kind_str(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Study => "study",
        BlockKind::Break => "break",
        BlockKind::LongBreak => "long_break",
    }
}

pub fn parse_kind(raw: &str) -> Option<BlockKind> {
    match raw {
        "study" => Some(BlockKind::Study),
        "break" => Some(BlockKind::Break),
        "long_break" => Some(BlockKind::LongBreak),
        _ => None,
    }
}

/// A `plan_runs` row. `date` is the local date of the occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRow {
    pub id: i64,
    pub plan_id: i64,
    pub date: String,
    pub planned_start_ms: i64,
    pub planned_end_ms: i64,
    pub actual_start_ms: Option<i64>,
    pub mode_level: String,
    pub study_min: i64,
    pub break_min: i64,
    pub long_break_min: Option<i64>,
    pub sets_before_long_break: Option<i64>,
    pub sets_total: i64,
    pub status: RunStatus,
    pub ended_at_ms: Option<i64>,
    pub detail_code: Option<String>,
}

/// A `timer_segments` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentRow {
    pub id: i64,
    pub run_id: i64,
    pub seq: i64,
    pub kind: BlockKind,
    pub planned_start_ms: i64,
    pub planned_end_ms: i64,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub end_reason: Option<EndReason>,
    pub late_join: bool,
    pub notice_sent_at_ms: Option<i64>,
}

/// A run to insert; the id is assigned by the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRun {
    pub plan_id: i64,
    pub date: String,
    pub planned_start_ms: i64,
    pub planned_end_ms: i64,
    pub actual_start_ms: Option<i64>,
    pub mode_level: String,
    pub study_min: i64,
    pub break_min: i64,
    pub long_break_min: Option<i64>,
    pub sets_before_long_break: Option<i64>,
    pub sets_total: i64,
    pub status: RunStatus,
    pub ended_at_ms: Option<i64>,
    pub detail_code: Option<&'static str>,
}

/// A segment to insert. Already-elapsed segments arrive closed (`ended_at_ms`/`end_reason` set).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSegment {
    pub seq: i64,
    pub kind: BlockKind,
    pub planned_start_ms: i64,
    pub planned_end_ms: i64,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub end_reason: Option<EndReason>,
    pub late_join: bool,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Idle,
    Study,
    Break,
    LongBreak,
}

impl Phase {
    pub fn of(kind: BlockKind) -> Phase {
        match kind {
            BlockKind::Study => Phase::Study,
            BlockKind::Break => Phase::Break,
            BlockKind::LongBreak => Phase::LongBreak,
        }
    }
}

/// The next occurrence that has not started yet (display only).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NextPlan {
    pub plan_id: i64,
    pub name: String,
    pub subject: String,
    pub date: String,
    pub sets: i64,
    pub planned_start_ms: i64,
    pub planned_end_ms: i64,
}

/// What the UI shows and what `timer_changed` carries. `run_id`/`segment_id` are `None` in a
/// read-only projection of something the engine has not written yet.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TimerState {
    pub phase: Phase,
    pub run_id: Option<i64>,
    pub segment_id: Option<i64>,
    /// 1-based set the current block belongs to; 0 when idle.
    pub set_index: i64,
    pub sets_total: i64,
    pub planned_end_ms: Option<i64>,
    pub lock_held: bool,
    /// The effective clock (never behind the highest time already seen) the state was derived at.
    pub now_ms: i64,
    pub next: Option<NextPlan>,
}
