//! The "break ends in one minute" notice. Due when an open break has at most a minute left, was
//! longer than a minute to begin with, and has not been announced yet. The sent mark lives on the
//! segment row, so a restart cannot repeat it and a stale clock cannot send it after the end.

use rusqlite::Connection;
use serde::Serialize;

use crate::error::AppResult;
use crate::plan::BlockKind;
use crate::store;

use super::model::{Phase, SegmentRow, TimerState};

pub const BREAK_ENDING_SOON_EVENT: &str = "break_ending_soon";
pub const WARN_BEFORE_MS: i64 = 60_000;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BreakEndingSoon {
    pub run_id: i64,
    pub segment_id: i64,
    pub planned_end_ms: i64,
    pub remaining_ms: i64,
}

pub fn is_due(seg: &SegmentRow, now_ms: i64) -> bool {
    let remaining = seg.planned_end_ms - now_ms;
    matches!(seg.kind, BlockKind::Break | BlockKind::LongBreak)
        && seg.ended_at_ms.is_none()
        && seg.notice_sent_at_ms.is_none()
        && seg.planned_end_ms - seg.planned_start_ms > WARN_BEFORE_MS
        && remaining > 0
        && remaining <= WARN_BEFORE_MS
}

/// Claims the notice for the break `state` is in, if due. The claim is the send decision: the
/// caller notifies only when this returns `Some`, so a failed notification is not retried.
pub fn claim_due(
    conn: &Connection,
    state: &TimerState,
    now_ms: i64,
) -> AppResult<Option<BreakEndingSoon>> {
    if !matches!(state.phase, Phase::Break | Phase::LongBreak) {
        return Ok(None);
    }
    let (Some(run_id), Some(segment_id)) = (state.run_id, state.segment_id) else {
        return Ok(None);
    };
    let Some(seg) = store::get_segment(conn, segment_id)? else {
        return Ok(None);
    };
    if !is_due(&seg, now_ms) || !store::claim_segment_notice(conn, segment_id, now_ms)? {
        return Ok(None);
    }
    Ok(Some(BreakEndingSoon {
        run_id,
        segment_id,
        planned_end_ms: seg.planned_end_ms,
        remaining_ms: seg.planned_end_ms - now_ms,
    }))
}

pub fn notification() -> (&'static str, &'static str) {
    ("곧 공부 시작", "1분 뒤 공부가 다시 시작됩니다.")
}
