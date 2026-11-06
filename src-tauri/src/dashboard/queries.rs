//! SQL for the dashboard. Everything here is read-only; the folding into numbers is in `mod.rs`.
//! Only app ids, plan names, subjects, counts, and times are read: no titles or URLs exist in the schema.

use std::collections::HashSet;

use rusqlite::{params, Connection};

use super::{AppCount, PostponedPlan};
use crate::downtime::Interval;
use crate::enforce::model::BlockAction;
use crate::error::AppResult;
use crate::store;
use crate::timer::summary::studied_ms;

/// One plan run reduced to what the aggregates need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunFact {
    pub date: String,
    pub plan_id: i64,
    pub subject: String,
    /// Study time the run planned: sets times the study length frozen on the run.
    pub planned_ms: i64,
    /// Recognized time of every segment, including one still running.
    pub credited_ms: i64,
    /// Recognized time of closed study segments only; pairs with `studied_ms` for the score.
    pub closed_credited_ms: i64,
    /// Closed study segments minus downtime, the denominator of the focus score.
    pub studied_ms: i64,
}

/// Runs whose local date is in `from..=to` (date strings compare in calendar order). A run that
/// crosses midnight belongs to the date it was planned for.
pub fn run_facts(conn: &Connection, from: &str, to: &str) -> AppResult<Vec<RunFact>> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.date, r.plan_id, p.subject, r.sets_total * r.study_min * 60000,
                COALESCE((SELECT SUM(m.credited_ms) FROM focus_minutes m
                          JOIN timer_segments s ON s.id = m.segment_id WHERE s.run_id = r.id), 0),
                COALESCE((SELECT SUM(m.credited_ms) FROM focus_minutes m
                          JOIN timer_segments s ON s.id = m.segment_id
                          WHERE s.run_id = r.id AND s.kind = 'study' AND s.ended_at_ms IS NOT NULL), 0)
         FROM plan_runs r JOIN plans p ON p.id = r.plan_id
         WHERE r.date >= ?1 AND r.date <= ?2 ORDER BY r.planned_start_ms, r.id",
    )?;
    let rows = stmt.query_map(params![from, to], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            RunFact {
                date: row.get(1)?,
                plan_id: row.get(2)?,
                subject: row.get(3)?,
                planned_ms: row.get(4)?,
                credited_ms: row.get(5)?,
                closed_credited_ms: row.get(6)?,
                studied_ms: 0,
            },
        ))
    })?;
    let mut facts: Vec<(i64, RunFact)> = rows.collect::<Result<_, _>>()?;

    let mut segments = Vec::new();
    for (run_id, _) in &facts {
        segments.push(store::list_segments(conn, *run_id)?);
    }
    let from_ms = segments.iter().flatten().map(|s| s.started_at_ms).min();
    let to_ms = segments.iter().flatten().filter_map(|s| s.ended_at_ms).max();
    let downtime = match (from_ms, to_ms) {
        (Some(from_ms), Some(to_ms)) => store::list_downtime_intervals(conn, from_ms, to_ms)?,
        _ => Vec::new(),
    };
    for ((_, fact), segs) in facts.iter_mut().zip(&segments) {
        fact.studied_ms = segs
            .iter()
            .filter(|s| s.kind == crate::plan::BlockKind::Study)
            .map(|s| studied_ms(s, &downtime))
            .sum();
    }
    Ok(facts.into_iter().map(|(_, fact)| fact).collect())
}

/// `(plan id, date)` of every run in the range: occurrences with a run are already counted.
pub fn run_keys(conn: &Connection, from: &str, to: &str) -> AppResult<HashSet<(i64, String)>> {
    let mut stmt = conn.prepare("SELECT plan_id, date FROM plan_runs WHERE date >= ?1 AND date <= ?2")?;
    let rows = stmt.query_map(params![from, to], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Planned windows of runs that overlap `[from_ms, to_ms)`.
pub fn planned_windows(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<Vec<Interval>> {
    let mut stmt = conn.prepare(
        "SELECT planned_start_ms, planned_end_ms FROM plan_runs
         WHERE planned_end_ms > ?1 AND planned_start_ms < ?2 ORDER BY planned_start_ms",
    )?;
    let rows =
        stmt.query_map(params![from_ms, to_ms], |row| Ok(Interval { start: row.get(0)?, end: row.get(1)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Apps ranked by distraction attempts (a quit request, or a dry-run hit): the same events the
/// point rules count. Sites are not ranked; their ids are hosts and stay out of the ranking.
pub fn deviation_apps(conn: &Connection, from_ms: i64, to_ms: i64, limit: i64) -> AppResult<Vec<AppCount>> {
    let mut stmt = conn.prepare(
        "SELECT target_id, COUNT(*) AS n FROM block_events
         WHERE target_kind = 'app' AND action IN (?3, ?4) AND at_ms >= ?1 AND at_ms < ?2
         GROUP BY target_id ORDER BY n DESC, target_id LIMIT ?5",
    )?;
    let rows = stmt.query_map(
        params![
            from_ms,
            to_ms,
            BlockAction::TerminateRequested.as_str(),
            BlockAction::WouldTerminate.as_str(),
            limit
        ],
        |row| Ok(AppCount { app_id: row.get(0)?, count: row.get(1)? }),
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Times of every deviation in the window: switches to a non-study app and distraction attempts
/// (app or site).
pub fn deviation_times(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT at_ms FROM focus_events WHERE kind = 'away' AND at_ms >= ?1 AND at_ms < ?2
         UNION ALL
         SELECT at_ms FROM block_events WHERE action IN (?3, ?4, ?5) AND at_ms >= ?1 AND at_ms < ?2",
    )?;
    let rows = stmt.query_map(
        params![
            from_ms,
            to_ms,
            BlockAction::TerminateRequested.as_str(),
            BlockAction::WouldTerminate.as_str(),
            BlockAction::SiteBlocked.as_str()
        ],
        |row| row.get(0),
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Ids and start times of study segments that began in the window.
pub fn study_segment_starts(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT id, started_at_ms FROM timer_segments
         WHERE kind = 'study' AND started_at_ms >= ?1 AND started_at_ms < ?2 ORDER BY started_at_ms",
    )?;
    let rows = stmt.query_map(params![from_ms, to_ms], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Plans postponed at least `min_count` times in the window (applied changes only).
pub fn postponed_plans(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
    min_count: i64,
    limit: i64,
) -> AppResult<Vec<PostponedPlan>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.plan_name, p.subject, COUNT(*) AS n FROM plan_history h JOIN plans p ON p.id = h.plan_id
         WHERE h.reason_code = 'postponed' AND h.outcome = 'applied' AND h.at_ms >= ?1 AND h.at_ms < ?2
         GROUP BY p.id HAVING n >= ?3 ORDER BY n DESC, p.id LIMIT ?4",
    )?;
    let rows = stmt.query_map(params![from_ms, to_ms, min_count, limit], |row| {
        Ok(PostponedPlan { plan_id: row.get(0)?, name: row.get(1)?, subject: row.get(2)?, count: row.get(3)? })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Counts of `study_passes` by kind, plus aborted runs, in the window.
pub fn release_counts(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<(i64, i64, i64, i64)> {
    let by_kind = |kind: &str| -> AppResult<i64> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM study_passes WHERE kind = ?3 AND at_ms >= ?1 AND at_ms < ?2",
            params![from_ms, to_ms, kind],
            |row| row.get(0),
        )?)
    };
    let aborted: i64 = conn.query_row(
        "SELECT COUNT(*) FROM plan_runs WHERE status = 'aborted' AND planned_start_ms >= ?1 AND planned_start_ms < ?2",
        params![from_ms, to_ms],
        |row| row.get(0),
    )?;
    Ok((by_kind("emergency_pass")?, by_kind("skip_today")?, by_kind("early_release")?, aborted))
}
