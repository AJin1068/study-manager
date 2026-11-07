//! Set and run summaries. A set ends when its study segment closes; the summary settles the
//! time actually studied: the segment minus any recorded downtime (sleep, app off) inside it.
//! Distraction attempts come from `block_events` (see `store::count_attempts`).

use rusqlite::Connection;
use serde::Serialize;

use crate::downtime::Interval;
use crate::error::AppResult;
use crate::focus;
use crate::plan::BlockKind;
use crate::store;

use super::model::{SegmentRow, TimerState};

pub const SET_SUMMARY_EVENT: &str = "set_summary";
pub const RUN_SUMMARY_EVENT: &str = "run_summary";

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetSummary {
    pub run_id: i64,
    /// 1-based, counting study blocks.
    pub set_index: i64,
    pub set_studied_ms: i64,
    pub today_studied_ms: i64,
    /// Block attempts during this set (app or site, dry-run or enforce).
    pub attempt_count: i64,
    /// Recognized study time: study app in front and input not idle (see `focus`).
    pub focus_credited_ms: i64,
    /// Recognized over studied time, 0 to 100; `None` when nothing was studied.
    pub focus_score: Option<i64>,
    /// Study start to the first switch to a non-study app; `None` when it never happened.
    pub first_away_after_ms: Option<i64>,
    /// First such switch to the return to a study app; `None` when it never happened.
    pub return_after_ms: Option<i64>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub run_id: i64,
    pub sets_total: i64,
    pub run_studied_ms: i64,
    pub today_studied_ms: i64,
    /// Block attempts during the whole run.
    pub attempt_count: i64,
    pub focus_credited_ms: i64,
    pub focus_score: Option<i64>,
    /// `completed`, `aborted`, or another terminal `plan_runs.status`.
    pub status: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Settled {
    pub sets: Vec<SetSummary>,
    pub run: Option<RunSummary>,
}

/// Length of `seg` that is not covered by downtime. Overlapping downtime rows count once.
pub fn studied_ms(seg: &SegmentRow, downtime: &[Interval]) -> i64 {
    let Some(end) = seg.ended_at_ms else { return 0 };
    let start = seg.started_at_ms;
    if end <= start {
        return 0;
    }
    let mut clipped: Vec<(i64, i64)> = downtime
        .iter()
        .map(|d| (d.start.max(start), d.end.min(end)))
        .filter(|(s, e)| s < e)
        .collect();
    clipped.sort_unstable();
    let mut covered = 0;
    let mut reach = start;
    for (s, e) in clipped {
        let s = s.max(reach);
        if e > s {
            covered += e - s;
            reach = e;
        }
    }
    (end - start) - covered
}

fn closed_study(segments: &[SegmentRow]) -> impl Iterator<Item = &SegmentRow> {
    segments
        .iter()
        .filter(|s| s.kind == BlockKind::Study && s.ended_at_ms.is_some())
}

fn total_studied(segments: &[SegmentRow], downtime: &[Interval]) -> i64 {
    closed_study(segments)
        .map(|s| studied_ms(s, downtime))
        .sum()
}

fn day_studied(conn: &Connection, date: &str) -> AppResult<i64> {
    let mut all = Vec::new();
    for run in store::list_runs_on_date(conn, date)? {
        all.extend(store::list_segments(conn, run.id)?);
    }
    let from = all.iter().map(|s| s.started_at_ms).min().unwrap_or(0);
    let to = all.iter().filter_map(|s| s.ended_at_ms).max().unwrap_or(0);
    let downtime = store::list_downtime_intervals(conn, from, to)?;
    Ok(total_studied(&all, &downtime))
}

/// Summaries for what ended between two consecutive engine states: every study segment of the
/// previous run that closed since, and the run itself if it is no longer the current one.
/// Several can end at once when the machine slept through them.
pub fn settle(conn: &Connection, prev: &TimerState, cur: &TimerState) -> AppResult<Settled> {
    let (Some(run_id), Some(prev_segment)) = (prev.run_id, prev.segment_id) else {
        return Ok(Settled::default());
    };
    let run_over = cur.run_id != Some(run_id);
    let Some(run) = store::get_run(conn, run_id)? else {
        return Ok(Settled::default());
    };
    let segments = store::list_segments(conn, run_id)?;
    let current_segment = cur.segment_id.unwrap_or(i64::MAX);
    let ended: Vec<&SegmentRow> = closed_study(&segments)
        .filter(|s| s.id >= prev_segment && (run_over || s.id < current_segment))
        .collect();
    if ended.is_empty() && !run_over {
        return Ok(Settled::default());
    }
    let today = day_studied(conn, &run.date)?;
    let mut out = Settled::default();
    for seg in ended {
        let set_index = study_index(&segments, seg);
        let downtime = store::list_downtime_intervals(
            conn,
            seg.started_at_ms,
            seg.ended_at_ms.unwrap_or(seg.started_at_ms),
        )?;
        let set_studied_ms = studied_ms(seg, &downtime);
        let focus = focus::segment_focus(conn, seg, set_studied_ms)?;
        out.sets.push(SetSummary {
            run_id,
            set_index,
            set_studied_ms,
            today_studied_ms: today,
            // Block attempts only: focus `away` events are a different measure and never added here.
            attempt_count: store::count_attempts(conn, run_id, Some(seg.id))?,
            focus_credited_ms: focus.credited_ms,
            focus_score: focus.score,
            first_away_after_ms: focus.first_away_after_ms,
            return_after_ms: focus.return_after_ms,
        });
    }
    if run_over {
        let from = segments.iter().map(|s| s.started_at_ms).min().unwrap_or(0);
        let to = segments
            .iter()
            .filter_map(|s| s.ended_at_ms)
            .max()
            .unwrap_or(0);
        let downtime = store::list_downtime_intervals(conn, from, to)?;
        let run_studied_ms = total_studied(&segments, &downtime);
        let (focus_credited_ms, focus_score) = focus::run_focus(conn, run_id, run_studied_ms)?;
        out.run = Some(RunSummary {
            run_id,
            sets_total: run.sets_total,
            run_studied_ms,
            today_studied_ms: today,
            attempt_count: store::count_attempts(conn, run_id, None)?,
            focus_credited_ms,
            focus_score,
            status: run.status.as_str().to_owned(),
        });
    }
    Ok(out)
}

fn study_index(segments: &[SegmentRow], seg: &SegmentRow) -> i64 {
    let before = segments
        .iter()
        .filter(|s| s.kind == BlockKind::Study && s.seq <= seg.seq)
        .count();
    i64::try_from(before).unwrap_or(i64::MAX)
}

/// `1시간 15분`, `25분`, `1분 미만`.
pub fn format_duration_ko(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, 0) => "1분 미만".to_owned(),
        (0, m) => format!("{m}분"),
        (h, 0) => format!("{h}시간"),
        (h, m) => format!("{h}시간 {m}분"),
    }
}

/// Title and body of the notification for a finished set. No subject: it can show on a lock screen.
pub fn set_notification(summary: &SetSummary) -> (String, String) {
    (
        format!("{}세트 끝", summary.set_index),
        format!(
            "이번 세트 {} · 오늘 {}",
            format_duration_ko(summary.set_studied_ms),
            format_duration_ko(summary.today_studied_ms)
        ),
    )
}
