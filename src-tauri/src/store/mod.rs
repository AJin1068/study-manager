//! Repository layer: all SQL lives here. Domain modules call these functions instead of
//! writing SQL, so the storage backend can be swapped later without touching callers.

pub mod remote;

use rusqlite::{params, Connection, OptionalExtension};

use crate::downtime::{DowntimeKind, DowntimeRecord, Interval};
use crate::enforce::model::{BlockAction, BlockEntry, BlockEvent, EntryKind, EntrySource, NewBlockEvent};
use crate::error::{AppError, AppResult};
use crate::focus::model::{FocusEventKind, FocusEventRow, FocusTotals, MinuteDelta, NewFocusEvent, StudyApp};
use crate::settings::{HistoryEntry, Outcome, Reason};
use crate::study::{ModeConfig, StudyLevel};
use crate::timer::model::{EndReason, NewRun, NewSegment, RunRow, RunStatus, SegmentRow};

// ---- settings ----

pub fn get_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |row| row.get(0))
        .optional()?)
}

pub fn put_setting(conn: &Connection, key: &str, category: &str, value: &str, at_ms: i64) -> AppResult<()> {
    conn.execute(
        "INSERT INTO settings (key, category, value, updated_at_ms) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
        params![key, category, value, at_ms],
    )?;
    Ok(())
}

// ---- setting_history ----

pub fn append_history(
    conn: &Connection,
    at_ms: i64,
    key: &str,
    before: Option<&str>,
    after: &str,
    reason: Reason,
    outcome: Outcome,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO setting_history (at_ms, key, before, after, reason, outcome)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![at_ms, key, before, after, reason.as_str(), outcome.as_str()],
    )?;
    Ok(())
}

pub fn list_history(conn: &Connection, limit: i64) -> AppResult<Vec<HistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, at_ms, key, before, after, reason, outcome
         FROM setting_history ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(HistoryEntry {
            id: row.get(0)?,
            at_ms: row.get(1)?,
            key: row.get(2)?,
            before: row.get(3)?,
            after: row.get(4)?,
            reason: row.get(5)?,
            outcome: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- study_state / daily_mode / study_mode_config ----

/// Reads the lock. Returns `(lock_held, changed_at_ms)`; the `studying` column holds `lock_held`
/// (1 = a planned run holds the lock). A missing row is an error, never "lock free": every lock check
/// goes through here, so a silent default would fail open. The migration seeds the row, so a
/// missing row means the database was corrupted or tampered with.
pub fn read_study_state(conn: &Connection) -> AppResult<(bool, i64)> {
    let row = conn
        .query_row(
            "SELECT studying, changed_at_ms FROM study_state WHERE id = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    match row {
        Some((held, changed)) => Ok((held != 0, changed)),
        None => Err(AppError::Internal("study_state row is missing; lock state is unknown".into())),
    }
}

/// Updates the lock row (`studying` column = `lock_held`). UPDATE, not upsert: writing to a
/// missing row must fail rather than recreate it, so the lock cannot be reset by writing to it.
pub fn put_study_state(conn: &Connection, lock_held: bool, at_ms: i64) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE study_state SET studying = ?1, changed_at_ms = ?2 WHERE id = 1",
        params![i64::from(lock_held), at_ms],
    )?;
    if changed != 1 {
        return Err(AppError::Internal("study_state row is missing; cannot write lock state".into()));
    }
    Ok(())
}

pub fn read_daily_mode(conn: &Connection, date: &str) -> AppResult<Option<StudyLevel>> {
    let raw: Option<String> = conn
        .query_row("SELECT level FROM daily_mode WHERE date = ?1", params![date], |row| row.get(0))
        .optional()?;
    Ok(raw.as_deref().and_then(StudyLevel::parse))
}

pub fn put_daily_mode(conn: &Connection, date: &str, level: StudyLevel, at_ms: i64) -> AppResult<()> {
    conn.execute(
        "INSERT INTO daily_mode (date, level, selected_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(date) DO UPDATE SET level = excluded.level, selected_at_ms = excluded.selected_at_ms",
        params![date, level.as_str(), at_ms],
    )?;
    Ok(())
}

pub fn list_mode_configs(conn: &Connection) -> AppResult<Vec<ModeConfig>> {
    let mut stmt = conn.prepare(
        "SELECT level, study_min, break_min, long_break_min, sets_before_long_break
         FROM study_mode_config ORDER BY CASE level WHEN 'easy' THEN 0 WHEN 'medium' THEN 1 ELSE 2 END",
    )?;
    let rows = stmt.query_map([], |row| {
        let level: String = row.get(0)?;
        Ok((
            level,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
        ))
    })?;
    let mut configs = Vec::new();
    for row in rows {
        let (level, study_min, break_min, long_break_min, sets_before_long_break) = row?;
        let level = StudyLevel::parse(&level)
            .ok_or_else(|| crate::error::AppError::Invalid(format!("unknown mode level {level}")))?;
        configs.push(ModeConfig {
            level,
            study_min,
            break_min,
            long_break_min,
            sets_before_long_break,
        });
    }
    Ok(configs)
}

pub fn put_mode_config(conn: &Connection, config: &ModeConfig) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE study_mode_config
         SET study_min = ?2, break_min = ?3, long_break_min = ?4, sets_before_long_break = ?5
         WHERE level = ?1",
        params![
            config.level.as_str(),
            config.study_min,
            config.break_min,
            config.long_break_min,
            config.sets_before_long_break
        ],
    )?;
    if changed != 1 {
        return Err(AppError::Internal(format!("mode config row missing for {}", config.level.as_str())));
    }
    Ok(())
}

// ---- sessions ----

pub fn insert_session(conn: &Connection, at_ms: i64) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO sessions (started_at_ms, last_heartbeat_ms, clean_exit) VALUES (?1, ?1, NULL)",
        params![at_ms],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn session_last_heartbeat(conn: &Connection, session_id: i64) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT last_heartbeat_ms FROM sessions WHERE id = ?1",
        params![session_id],
        |row| row.get(0),
    )?)
}

pub fn touch_heartbeat(conn: &Connection, session_id: i64, at_ms: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE sessions SET last_heartbeat_ms = MAX(last_heartbeat_ms, ?2) WHERE id = ?1",
        params![session_id, at_ms],
    )?;
    Ok(())
}

pub fn set_clean_exit(conn: &Connection, session_id: i64, clean: bool, at_ms: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE sessions SET clean_exit = ?2, last_heartbeat_ms = ?3 WHERE id = ?1",
        params![session_id, i64::from(clean), at_ms],
    )?;
    Ok(())
}

/// Sessions that never recorded a clean exit and never got their forced quit recorded.
pub fn open_sessions(conn: &Connection) -> AppResult<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT id, last_heartbeat_ms FROM sessions WHERE clean_exit IS NULL ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- downtime_records ----

pub fn insert_downtime(
    conn: &Connection,
    kind: DowntimeKind,
    interval: Interval,
    estimated: bool,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO downtime_records (kind, start_at_ms, end_at_ms, estimated) VALUES (?1, ?2, ?3, ?4)",
        params![kind.as_str(), interval.start, interval.end, i64::from(estimated)],
    )?;
    Ok(())
}

/// Measured (non-estimated) sleep intervals. Estimated gaps are trimmed against these.
pub fn measured_sleep_intervals(conn: &Connection) -> AppResult<Vec<Interval>> {
    let mut stmt = conn.prepare(
        "SELECT start_at_ms, end_at_ms FROM downtime_records
         WHERE kind = 'sleep' AND estimated = 0 ORDER BY start_at_ms",
    )?;
    let rows = stmt.query_map([], |row| Ok(Interval { start: row.get(0)?, end: row.get(1)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn list_downtime(conn: &Connection, limit: i64) -> AppResult<Vec<DowntimeRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, start_at_ms, end_at_ms, estimated FROM downtime_records
         ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        let kind: String = row.get(1)?;
        Ok(DowntimeRecord {
            id: row.get(0)?,
            kind: DowntimeKind::parse(&kind).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    format!("unknown downtime kind {kind}").into(),
                )
            })?,
            start_at_ms: row.get(2)?,
            end_at_ms: row.get(3)?,
            estimated: row.get::<_, i64>(4)? != 0,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- plans / plan_exceptions / plan_history ----

use crate::plan::{ExceptionKind, NewPlanHistory, Plan, PlanException, PlanHistoryEntry, SubjectCount};

const PLAN_COLUMNS: &str = "id, series_id, plan_name, subject, start_date, start_time, sets, \
     repeat_weekdays, repeat_until, effective_from_date, created_at_ms, deleted_at_ms";

fn plan_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Plan> {
    Ok(Plan {
        id: row.get(0)?,
        series_id: row.get(1)?,
        name: row.get(2)?,
        subject: row.get(3)?,
        start_date: row.get(4)?,
        start_time: row.get(5)?,
        sets: row.get(6)?,
        repeat_weekdays: row.get(7)?,
        repeat_until: row.get(8)?,
        effective_from_date: row.get(9)?,
        created_at_ms: row.get(10)?,
        deleted_at_ms: row.get(11)?,
    })
}

/// Inserts a plan. A `series_id` of 0 starts a new series: it becomes the plan's own id.
pub fn insert_plan(conn: &Connection, plan: &Plan) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO plans (series_id, plan_name, subject, start_date, start_time, sets,
                            repeat_weekdays, repeat_until, effective_from_date, created_at_ms, deleted_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            plan.series_id,
            plan.name,
            plan.subject,
            plan.start_date,
            plan.start_time,
            plan.sets,
            plan.repeat_weekdays,
            plan.repeat_until,
            plan.effective_from_date,
            plan.created_at_ms,
            plan.deleted_at_ms,
        ],
    )?;
    let id = conn.last_insert_rowid();
    if plan.series_id == 0 {
        conn.execute("UPDATE plans SET series_id = id WHERE id = ?1", params![id])?;
    }
    Ok(id)
}

pub fn get_plan(conn: &Connection, id: i64) -> AppResult<Option<Plan>> {
    Ok(conn
        .query_row(&format!("SELECT {PLAN_COLUMNS} FROM plans WHERE id = ?1"), params![id], plan_from_row)
        .optional()?)
}

pub fn list_live_plans(conn: &Connection) -> AppResult<Vec<Plan>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PLAN_COLUMNS} FROM plans WHERE deleted_at_ms IS NULL ORDER BY id"
    ))?;
    let rows = stmt.query_map([], plan_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Updates the fields a series edit may change in place. Name and subject are not editable.
pub fn update_plan_fields(conn: &Connection, plan: &Plan) -> AppResult<()> {
    conn.execute(
        "UPDATE plans SET start_time = ?2, sets = ?3, repeat_weekdays = ?4, repeat_until = ?5 WHERE id = ?1",
        params![plan.id, plan.start_time, plan.sets, plan.repeat_weekdays, plan.repeat_until],
    )?;
    Ok(())
}

pub fn set_plan_repeat_until(conn: &Connection, id: i64, until: Option<&str>) -> AppResult<()> {
    conn.execute("UPDATE plans SET repeat_until = ?2 WHERE id = ?1", params![id, until])?;
    Ok(())
}

pub fn soft_delete_plan(conn: &Connection, id: i64, at_ms: i64) -> AppResult<()> {
    conn.execute("UPDATE plans SET deleted_at_ms = ?2 WHERE id = ?1", params![id, at_ms])?;
    Ok(())
}

fn exception_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlanException> {
    let kind: String = row.get(2)?;
    Ok(PlanException {
        plan_id: row.get(0)?,
        date: row.get(1)?,
        kind: ExceptionKind::parse(&kind).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                format!("unknown exception kind {kind}").into(),
            )
        })?,
        new_date: row.get(3)?,
        new_time: row.get(4)?,
        sets_override: row.get(5)?,
    })
}

const EXCEPTION_COLUMNS: &str = "plan_id, date, kind, new_date, new_time, sets_override";

pub fn get_plan_exception(conn: &Connection, plan_id: i64, date: &str) -> AppResult<Option<PlanException>> {
    Ok(conn
        .query_row(
            &format!("SELECT {EXCEPTION_COLUMNS} FROM plan_exceptions WHERE plan_id = ?1 AND date = ?2"),
            params![plan_id, date],
            exception_from_row,
        )
        .optional()?)
}

/// Exceptions of live plans only.
pub fn list_plan_exceptions(conn: &Connection) -> AppResult<Vec<PlanException>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {EXCEPTION_COLUMNS} FROM plan_exceptions
         WHERE plan_id IN (SELECT id FROM plans WHERE deleted_at_ms IS NULL) ORDER BY plan_id, date"
    ))?;
    let rows = stmt.query_map([], exception_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn put_plan_exception(conn: &Connection, ex: &PlanException) -> AppResult<()> {
    conn.execute(
        "INSERT INTO plan_exceptions (plan_id, date, kind, new_date, new_time, sets_override)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(plan_id, date) DO UPDATE SET kind = excluded.kind, new_date = excluded.new_date,
             new_time = excluded.new_time, sets_override = excluded.sets_override",
        params![ex.plan_id, ex.date, ex.kind.as_str(), ex.new_date, ex.new_time, ex.sets_override],
    )?;
    Ok(())
}

pub fn delete_plan_exception(conn: &Connection, plan_id: i64, date: &str) -> AppResult<()> {
    conn.execute(
        "DELETE FROM plan_exceptions WHERE plan_id = ?1 AND date = ?2",
        params![plan_id, date],
    )?;
    Ok(())
}

/// Hands the exceptions keyed on or after `from_date` to another plan row (series split).
pub fn move_plan_exceptions_from(conn: &Connection, from_plan: i64, to_plan: i64, from_date: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE plan_exceptions SET plan_id = ?2 WHERE plan_id = ?1 AND date >= ?3",
        params![from_plan, to_plan, from_date],
    )?;
    Ok(())
}

pub fn list_daily_modes(conn: &Connection) -> AppResult<std::collections::HashMap<String, StudyLevel>> {
    let mut stmt = conn.prepare("SELECT date, level FROM daily_mode")?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (date, level) = row?;
        let level = StudyLevel::parse(&level)
            .ok_or_else(|| AppError::Invalid(format!("unknown mode level {level}")))?;
        map.insert(date, level);
    }
    Ok(map)
}

pub fn append_plan_history(conn: &Connection, row: &NewPlanHistory<'_>) -> AppResult<()> {
    conn.execute(
        "INSERT INTO plan_history (plan_id, at_ms, action, reason_code, reason_text, before_json, after_json, outcome, batch_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            row.plan_id,
            row.at_ms,
            row.action.as_str(),
            row.reason.as_str(),
            row.reason_text,
            row.before_json,
            row.after_json,
            row.outcome.as_str(),
            row.batch_id,
        ],
    )?;
    Ok(())
}

/// Id the next `plan_history` row will get; unique batch ids derive from it.
pub fn next_plan_history_id(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM sqlite_sequence WHERE name = 'plan_history'",
        [],
        |row| row.get(0),
    )?)
}

/// Every row of one batch, oldest first (applied changes, their undo, and rejected attempts).
pub fn list_plan_history_batch(conn: &Connection, batch_id: &str) -> AppResult<Vec<PlanHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, plan_id, at_ms, action, reason_code, reason_text, before_json, after_json, outcome, batch_id
         FROM plan_history WHERE batch_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![batch_id], |row| {
        Ok(PlanHistoryEntry {
            id: row.get(0)?,
            plan_id: row.get(1)?,
            at_ms: row.get(2)?,
            action: row.get(3)?,
            reason_code: row.get(4)?,
            reason_text: row.get(5)?,
            before_json: row.get(6)?,
            after_json: row.get(7)?,
            outcome: row.get(8)?,
            batch_id: row.get(9)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// History newest first. With a plan id, the whole series that plan belongs to (a series edit
/// splits the plan into several rows, but the user sees one timeline).
pub fn list_plan_history(conn: &Connection, plan_id: Option<i64>, limit: i64) -> AppResult<Vec<PlanHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, plan_id, at_ms, action, reason_code, reason_text, before_json, after_json, outcome, batch_id
         FROM plan_history
         WHERE ?1 IS NULL OR plan_id IN (
             SELECT id FROM plans WHERE series_id = (SELECT series_id FROM plans WHERE id = ?1))
         ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![plan_id, limit], |row| {
        Ok(PlanHistoryEntry {
            id: row.get(0)?,
            plan_id: row.get(1)?,
            at_ms: row.get(2)?,
            action: row.get(3)?,
            reason_code: row.get(4)?,
            reason_text: row.get(5)?,
            before_json: row.get(6)?,
            after_json: row.get(7)?,
            outcome: row.get(8)?,
            batch_id: row.get(9)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn count_postponed(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM plan_history
         WHERE reason_code = 'postponed' AND outcome = 'applied' AND at_ms >= ?1 AND at_ms < ?2",
        params![from_ms, to_ms],
        |row| row.get(0),
    )?)
}

pub fn postponed_by_subject(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<Vec<SubjectCount>> {
    let mut stmt = conn.prepare(
        "SELECT p.subject, COUNT(*) AS n FROM plan_history h JOIN plans p ON p.id = h.plan_id
         WHERE h.reason_code = 'postponed' AND h.outcome = 'applied' AND h.at_ms >= ?1 AND h.at_ms < ?2
         GROUP BY p.subject ORDER BY n DESC, p.subject",
    )?;
    let rows = stmt.query_map(params![from_ms, to_ms], |row| {
        Ok(SubjectCount { subject: row.get(0)?, count: row.get(1)? })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- timer: plan_runs / timer_segments ----

/// Highest heartbeat of any session: the persisted floor of the engine's high-water clock.
pub fn max_heartbeat(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("SELECT COALESCE(MAX(last_heartbeat_ms), 0) FROM sessions", [], |row| row.get(0))?)
}

fn bad_value(column: usize, what: &str, raw: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        format!("unknown {what} {raw}").into(),
    )
}

const RUN_COLUMNS: &str = "id, plan_id, date, planned_start_ms, planned_end_ms, actual_start_ms, mode_level, \
     study_min, break_min, long_break_min, sets_before_long_break, sets_total, status, ended_at_ms, detail_code";

fn run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    let status: String = row.get(12)?;
    Ok(RunRow {
        id: row.get(0)?,
        plan_id: row.get(1)?,
        date: row.get(2)?,
        planned_start_ms: row.get(3)?,
        planned_end_ms: row.get(4)?,
        actual_start_ms: row.get(5)?,
        mode_level: row.get(6)?,
        study_min: row.get(7)?,
        break_min: row.get(8)?,
        long_break_min: row.get(9)?,
        sets_before_long_break: row.get(10)?,
        sets_total: row.get(11)?,
        status: RunStatus::parse(&status).ok_or_else(|| bad_value(12, "run status", &status))?,
        ended_at_ms: row.get(13)?,
        detail_code: row.get(14)?,
    })
}

/// Runs on or after `from_date` plus every active run, whatever its date.
pub fn list_runs_since(conn: &Connection, from_date: &str) -> AppResult<Vec<RunRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RUN_COLUMNS} FROM plan_runs WHERE date >= ?1 OR status = 'active'
         ORDER BY planned_start_ms, id"
    ))?;
    let rows = stmt.query_map(params![from_date], run_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn list_active_runs(conn: &Connection) -> AppResult<Vec<RunRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {RUN_COLUMNS} FROM plan_runs WHERE status = 'active' ORDER BY planned_start_ms, id"))?;
    let rows = stmt.query_map([], run_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn get_run(conn: &Connection, run_id: i64) -> AppResult<Option<RunRow>> {
    Ok(conn
        .query_row(&format!("SELECT {RUN_COLUMNS} FROM plan_runs WHERE id = ?1"), params![run_id], run_from_row)
        .optional()?)
}

/// Every run of one local date, whatever its status.
pub fn list_runs_on_date(conn: &Connection, date: &str) -> AppResult<Vec<RunRow>> {
    let mut stmt =
        conn.prepare(&format!("SELECT {RUN_COLUMNS} FROM plan_runs WHERE date = ?1 ORDER BY planned_start_ms, id"))?;
    let rows = stmt.query_map(params![date], run_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn insert_run(conn: &Connection, run: &NewRun) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO plan_runs (plan_id, date, planned_start_ms, planned_end_ms, actual_start_ms, mode_level,
                                study_min, break_min, long_break_min, sets_before_long_break, sets_total,
                                status, ended_at_ms, detail_code)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            run.plan_id,
            run.date,
            run.planned_start_ms,
            run.planned_end_ms,
            run.actual_start_ms,
            run.mode_level,
            run.study_min,
            run.break_min,
            run.long_break_min,
            run.sets_before_long_break,
            run.sets_total,
            run.status.as_str(),
            run.ended_at_ms,
            run.detail_code,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn finish_run(
    conn: &Connection,
    run_id: i64,
    status: RunStatus,
    ended_at_ms: i64,
    detail_code: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE plan_runs SET status = ?2, ended_at_ms = ?3, detail_code = COALESCE(?4, detail_code) WHERE id = ?1",
        params![run_id, status.as_str(), ended_at_ms, detail_code],
    )?;
    Ok(())
}

fn segment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SegmentRow> {
    let kind: String = row.get(3)?;
    let reason: Option<String> = row.get(8)?;
    Ok(SegmentRow {
        id: row.get(0)?,
        run_id: row.get(1)?,
        seq: row.get(2)?,
        kind: crate::timer::model::parse_kind(&kind).ok_or_else(|| bad_value(3, "segment kind", &kind))?,
        planned_start_ms: row.get(4)?,
        planned_end_ms: row.get(5)?,
        started_at_ms: row.get(6)?,
        ended_at_ms: row.get(7)?,
        end_reason: match reason {
            Some(raw) => Some(EndReason::parse(&raw).ok_or_else(|| bad_value(8, "end reason", &raw))?),
            None => None,
        },
        late_join: row.get::<_, i64>(9)? != 0,
        notice_sent_at_ms: row.get(10)?,
    })
}

const SEGMENT_COLUMNS: &str = "id, run_id, seq, kind, planned_start_ms, planned_end_ms, started_at_ms, \
     ended_at_ms, end_reason, late_join, notice_sent_at_ms";

pub fn list_segments(conn: &Connection, run_id: i64) -> AppResult<Vec<SegmentRow>> {
    let mut stmt =
        conn.prepare(&format!("SELECT {SEGMENT_COLUMNS} FROM timer_segments WHERE run_id = ?1 ORDER BY seq"))?;
    let rows = stmt.query_map(params![run_id], segment_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn get_segment(conn: &Connection, segment_id: i64) -> AppResult<Option<SegmentRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {SEGMENT_COLUMNS} FROM timer_segments WHERE id = ?1"),
            params![segment_id],
            segment_from_row,
        )
        .optional()?)
}

/// Marks the "break ends soon" notice as sent. True only for the caller that set it, so a notice
/// is sent at most once even across restarts or racing callers.
pub fn claim_segment_notice(conn: &Connection, segment_id: i64, at_ms: i64) -> AppResult<bool> {
    let changed = conn.execute(
        "UPDATE timer_segments SET notice_sent_at_ms = ?2
         WHERE id = ?1 AND notice_sent_at_ms IS NULL AND ended_at_ms IS NULL",
        params![segment_id, at_ms],
    )?;
    Ok(changed == 1)
}

pub fn insert_segment(conn: &Connection, run_id: i64, seg: &NewSegment) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO timer_segments (run_id, seq, kind, planned_start_ms, planned_end_ms, started_at_ms,
                                     ended_at_ms, end_reason, late_join)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            run_id,
            seg.seq,
            crate::timer::model::kind_str(seg.kind),
            seg.planned_start_ms,
            seg.planned_end_ms,
            seg.started_at_ms,
            seg.ended_at_ms,
            seg.end_reason.map(EndReason::as_str),
            i64::from(seg.late_join),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn close_segment(conn: &Connection, segment_id: i64, ended_at_ms: i64, reason: EndReason) -> AppResult<()> {
    conn.execute(
        "UPDATE timer_segments SET ended_at_ms = ?2, end_reason = ?3 WHERE id = ?1",
        params![segment_id, ended_at_ms, reason.as_str()],
    )?;
    Ok(())
}

/// Spans of segments that elapsed while the app was not watching, overlapping the window, with
/// whether the engine was watching when the segment started. A segment the engine opened and
/// closed in one pass (`late_join` with `started_at == planned_start`) was never watched; any other
/// segment was watched from its `started_at_ms`.
pub fn list_unwatched_spans(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<Vec<(Interval, bool)>> {
    let mut stmt = conn.prepare(
        "SELECT planned_start_ms, started_at_ms, planned_end_ms, late_join FROM timer_segments
         WHERE end_reason = 'elapsed_while_down' AND planned_end_ms > ?1 AND planned_start_ms < ?2
         ORDER BY planned_start_ms",
    )?;
    let rows = stmt.query_map(params![from_ms, to_ms], |row| {
        let (planned_start, started_at, end): (i64, i64, i64) = (row.get(0)?, row.get(1)?, row.get(2)?);
        let never_watched = row.get::<_, i64>(3)? != 0 && started_at == planned_start;
        let start = if never_watched { planned_start } else { started_at };
        Ok((Interval { start, end }, !never_watched))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn list_downtime_intervals(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<Vec<Interval>> {
    let mut stmt = conn.prepare(
        "SELECT start_at_ms, end_at_ms FROM downtime_records
         WHERE end_at_ms > ?1 AND start_at_ms < ?2 ORDER BY start_at_ms",
    )?;
    let rows = stmt.query_map(params![from_ms, to_ms], |row| Ok(Interval { start: row.get(0)?, end: row.get(1)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- block_entries / block_events ----

fn entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BlockEntry> {
    Ok(BlockEntry {
        id: row.get(0)?,
        kind: row.get(1)?,
        pattern: row.get(2)?,
        source: row.get(3)?,
        category: row.get(4)?,
        enabled: row.get::<_, i64>(5)? != 0,
        created_at_ms: row.get(6)?,
    })
}

const ENTRY_COLUMNS: &str = "id, kind, pattern, source, category, enabled, created_at_ms";

pub fn list_block_entries(conn: &Connection) -> AppResult<Vec<BlockEntry>> {
    let mut stmt = conn.prepare(&format!("SELECT {ENTRY_COLUMNS} FROM block_entries ORDER BY id"))?;
    let rows = stmt.query_map([], entry_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn get_block_entry(conn: &Connection, id: i64) -> AppResult<Option<BlockEntry>> {
    Ok(conn
        .query_row(&format!("SELECT {ENTRY_COLUMNS} FROM block_entries WHERE id = ?1"), params![id], entry_from_row)
        .optional()?)
}

/// Patterns of the enabled entries of one kind, in insertion order.
pub fn list_enabled_patterns(conn: &Connection, kind: EntryKind) -> AppResult<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT pattern FROM block_entries WHERE kind = ?1 AND enabled = 1 ORDER BY id")?;
    let rows = stmt.query_map(params![kind.as_str()], |row| row.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Inserts an entry; false when the (kind, pattern) pair already exists.
pub fn insert_block_entry(
    conn: &Connection,
    kind: EntryKind,
    pattern: &str,
    source: EntrySource,
    category: Option<&str>,
    at_ms: i64,
) -> AppResult<bool> {
    let changed = conn.execute(
        "INSERT OR IGNORE INTO block_entries (kind, pattern, source, category, enabled, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, 1, ?5)",
        params![kind.as_str(), pattern, source.as_str(), category, at_ms],
    )?;
    Ok(changed == 1)
}

/// Inserts a preset entry with the category's current on/off state. An existing row for the same
/// (kind, pattern) is left alone, so a user entry is never converted or overwritten.
pub fn insert_preset_entry(
    conn: &Connection,
    kind: EntryKind,
    pattern: &str,
    category: &str,
    enabled: bool,
    at_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO block_entries (kind, pattern, source, category, enabled, created_at_ms)
         VALUES (?1, ?2, 'preset', ?3, ?4, ?5)",
        params![kind.as_str(), pattern, category, i64::from(enabled), at_ms],
    )?;
    Ok(())
}

/// Switches every preset row of a category; user rows never carry a preset category match.
pub fn set_preset_category_enabled(conn: &Connection, category: &str, enabled: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE block_entries SET enabled = ?2 WHERE source = 'preset' AND category = ?1",
        params![category, i64::from(enabled)],
    )?;
    Ok(())
}

pub fn delete_block_entry(conn: &Connection, id: i64) -> AppResult<bool> {
    Ok(conn.execute("DELETE FROM block_entries WHERE id = ?1", params![id])? == 1)
}

pub fn insert_block_event(conn: &Connection, event: &NewBlockEvent<'_>) -> AppResult<()> {
    conn.execute(
        "INSERT INTO block_events (at_ms, run_id, segment_id, target_kind, target_id, action, mode, detail_code)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            event.at_ms,
            event.run_id,
            event.segment_id,
            event.target_kind.as_str(),
            event.target_id,
            event.action.as_str(),
            event.mode.as_str(),
            event.detail_code,
        ],
    )?;
    Ok(())
}

/// Distraction attempts in a run, or in one of its segments: events that show an attempt to leave
/// the study (an app asked to quit or found in dry-run, a blocked site), in either enforcement
/// mode. Protected/pass skips, outcomes of an earlier request (`terminated`, `force_killed`,
/// `failed`) and extension connection events are not attempts.
pub fn count_attempts(conn: &Connection, run_id: i64, segment_id: Option<i64>) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM block_events
         WHERE run_id = ?1 AND (?2 IS NULL OR segment_id = ?2)
           AND action IN (?3, ?4, ?5)",
        params![
            run_id,
            segment_id,
            BlockAction::TerminateRequested.as_str(),
            BlockAction::WouldTerminate.as_str(),
            BlockAction::SiteBlocked.as_str(),
        ],
        |row| row.get(0),
    )?)
}

/// Newest first.
pub fn list_block_events(conn: &Connection, limit: i64) -> AppResult<Vec<BlockEvent>> {
    let mut stmt = conn.prepare(
        "SELECT id, at_ms, run_id, segment_id, target_kind, target_id, action, mode, detail_code
         FROM block_events ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(BlockEvent {
            id: row.get(0)?,
            at_ms: row.get(1)?,
            run_id: row.get(2)?,
            segment_id: row.get(3)?,
            target_kind: row.get(4)?,
            target_id: row.get(5)?,
            action: row.get(6)?,
            mode: row.get(7)?,
            detail_code: row.get(8)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- study_apps / focus_events / focus_minutes ----

fn study_app_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StudyApp> {
    Ok(StudyApp {
        id: row.get(0)?,
        kind: row.get(1)?,
        app_id: row.get(2)?,
        enabled: row.get::<_, i64>(3)? != 0,
        created_at_ms: row.get(4)?,
    })
}

const STUDY_APP_COLUMNS: &str = "id, kind, app_id, enabled, created_at_ms";

pub fn list_study_apps(conn: &Connection) -> AppResult<Vec<StudyApp>> {
    let mut stmt = conn.prepare(&format!("SELECT {STUDY_APP_COLUMNS} FROM study_apps ORDER BY id"))?;
    let rows = stmt.query_map([], study_app_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Ids of the enabled study apps of one kind.
pub fn list_enabled_study_app_ids(conn: &Connection, kind: EntryKind) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT app_id FROM study_apps WHERE kind = ?1 AND enabled = 1 ORDER BY id")?;
    let rows = stmt.query_map(params![kind.as_str()], |row| row.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Inserts a study app; false when the (kind, app id) pair already exists.
pub fn insert_study_app(conn: &Connection, kind: EntryKind, app_id: &str, at_ms: i64) -> AppResult<bool> {
    let changed = conn.execute(
        "INSERT OR IGNORE INTO study_apps (kind, app_id, enabled, created_at_ms) VALUES (?1, ?2, 1, ?3)",
        params![kind.as_str(), app_id, at_ms],
    )?;
    Ok(changed == 1)
}

pub fn delete_study_app(conn: &Connection, id: i64) -> AppResult<bool> {
    Ok(conn.execute("DELETE FROM study_apps WHERE id = ?1", params![id])? == 1)
}

pub fn set_study_app_enabled(conn: &Connection, id: i64, enabled: bool) -> AppResult<bool> {
    Ok(conn.execute("UPDATE study_apps SET enabled = ?2 WHERE id = ?1", params![id, i64::from(enabled)])? == 1)
}

pub fn insert_focus_event(conn: &Connection, event: &NewFocusEvent<'_>) -> AppResult<()> {
    conn.execute(
        "INSERT INTO focus_events (run_id, segment_id, at_ms, kind, app_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![event.run_id, event.segment_id, event.at_ms, event.kind.as_str(), event.app_id],
    )?;
    Ok(())
}

/// Time of the newest event of `kind` in a segment, if any.
pub fn last_focus_event_at(conn: &Connection, segment_id: i64, kind: FocusEventKind) -> AppResult<Option<i64>> {
    Ok(conn.query_row(
        "SELECT MAX(at_ms) FROM focus_events WHERE segment_id = ?1 AND kind = ?2",
        params![segment_id, kind.as_str()],
        |row| row.get(0),
    )?)
}

/// Events of one segment, oldest first.
pub fn list_focus_events(conn: &Connection, segment_id: i64) -> AppResult<Vec<FocusEventRow>> {
    let mut stmt =
        conn.prepare("SELECT at_ms, kind, app_id FROM focus_events WHERE segment_id = ?1 ORDER BY at_ms, id")?;
    let rows = stmt.query_map(params![segment_id], |row| {
        let raw: String = row.get(1)?;
        Ok((row.get::<_, i64>(0)?, raw, row.get::<_, Option<String>>(2)?))
    })?;
    rows.map(|row| {
        let (at_ms, raw, app_id) = row?;
        let kind = FocusEventKind::parse(&raw)
            .ok_or_else(|| AppError::Internal(format!("unknown focus event kind {raw:?}")))?;
        Ok(FocusEventRow { at_ms, kind, app_id })
    })
    .collect()
}

/// Adds to the minute bucket, creating it on first use.
pub fn add_focus_minute(conn: &Connection, segment_id: i64, delta: &MinuteDelta) -> AppResult<()> {
    conn.execute(
        "INSERT INTO focus_minutes (segment_id, minute_start_ms, credited_ms, away_ms, idle_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (segment_id, minute_start_ms) DO UPDATE SET
             credited_ms = credited_ms + excluded.credited_ms,
             away_ms = away_ms + excluded.away_ms,
             idle_ms = idle_ms + excluded.idle_ms",
        params![segment_id, delta.minute_start_ms, delta.credited_ms, delta.away_ms, delta.idle_ms],
    )?;
    Ok(())
}

/// Bucket totals of a run, or of one of its segments.
pub fn focus_totals(conn: &Connection, run_id: i64, segment_id: Option<i64>) -> AppResult<FocusTotals> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(m.credited_ms), 0), COALESCE(SUM(m.away_ms), 0), COALESCE(SUM(m.idle_ms), 0)
         FROM focus_minutes m JOIN timer_segments s ON s.id = m.segment_id
         WHERE s.run_id = ?1 AND (?2 IS NULL OR s.id = ?2)",
        params![run_id, segment_id],
        |row| Ok(FocusTotals { credited_ms: row.get(0)?, away_ms: row.get(1)?, idle_ms: row.get(2)? }),
    )?)
}

// ---- point_ledger and its inputs ----

/// Adds a ledger row unless `ref_key` was already paid. Returns whether a row was added.
pub fn insert_ledger_entry(
    conn: &Connection,
    at_ms: i64,
    delta: i64,
    reason_code: &str,
    ref_key: &str,
) -> AppResult<bool> {
    let added = conn.execute(
        "INSERT INTO point_ledger (at_ms, delta, reason_code, ref_key) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (ref_key) DO NOTHING",
        params![at_ms, delta, reason_code, ref_key],
    )?;
    Ok(added == 1)
}

/// Newest first.
pub fn list_ledger(conn: &Connection, limit: i64) -> AppResult<Vec<crate::points::LedgerEntry>> {
    let mut stmt = conn
        .prepare("SELECT id, at_ms, delta, reason_code, ref_key FROM point_ledger ORDER BY id DESC LIMIT ?1")?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(crate::points::LedgerEntry {
            id: row.get(0)?,
            at_ms: row.get(1)?,
            delta: row.get(2)?,
            reason_code: row.get(3)?,
            ref_key: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// `(balance, earned)`: the sum of every delta, and the sum of the positive ones.
pub fn ledger_sums(conn: &Connection) -> AppResult<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(delta), 0), COALESCE(SUM(CASE WHEN delta > 0 THEN delta ELSE 0 END), 0)
         FROM point_ledger",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

/// Minute buckets of a run in time order (segments never overlap).
pub fn list_run_minutes(conn: &Connection, run_id: i64) -> AppResult<Vec<(i64, MinuteDelta)>> {
    let mut stmt = conn.prepare(
        "SELECT m.minute_start_ms, m.credited_ms, m.away_ms, m.idle_ms
         FROM focus_minutes m JOIN timer_segments s ON s.id = m.segment_id
         WHERE s.run_id = ?1 ORDER BY m.minute_start_ms",
    )?;
    let rows = stmt.query_map(params![run_id], |row| {
        let minute_start_ms: i64 = row.get(0)?;
        Ok((
            minute_start_ms,
            MinuteDelta { minute_start_ms, credited_ms: row.get(1)?, away_ms: row.get(2)?, idle_ms: row.get(3)? },
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Times of the distraction attempts of a run (the same events `count_attempts` counts).
pub fn list_attempt_times(conn: &Connection, run_id: i64) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT at_ms FROM block_events WHERE run_id = ?1 AND action IN (?2, ?3, ?4) ORDER BY at_ms",
    )?;
    let rows = stmt.query_map(
        params![
            run_id,
            BlockAction::TerminateRequested.as_str(),
            BlockAction::WouldTerminate.as_str(),
            BlockAction::SiteBlocked.as_str(),
        ],
        |row| row.get(0),
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// `(pass id, at_ms, points)` of every release that carries a penalty.
pub fn list_pass_penalties(conn: &Connection) -> AppResult<Vec<(i64, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT id, at_ms, penalty_points FROM study_passes WHERE penalty_points > 0 ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}
