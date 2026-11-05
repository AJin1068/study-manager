//! Repository layer: all SQL lives here. Domain modules call these functions instead of
//! writing SQL, so the storage backend can be swapped later without touching callers.

use rusqlite::{params, Connection, OptionalExtension};

use crate::downtime::{DowntimeKind, DowntimeRecord, Interval};
use crate::error::{AppError, AppResult};
use crate::settings::{HistoryEntry, Outcome, Reason};
use crate::study::{ModeConfig, StudyLevel};

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

/// Reads the study lock. A missing row is an error, never "not studying": every lock check
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
        Some((studying, changed)) => Ok((studying != 0, changed)),
        None => Err(AppError::Internal("study_state row is missing; lock state is unknown".into())),
    }
}

/// Updates the lock row. UPDATE, not upsert: writing to a missing row must fail rather than
/// recreate it, so the lock cannot be reset by writing to it.
pub fn put_study_state(conn: &Connection, studying: bool, at_ms: i64) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE study_state SET studying = ?1, changed_at_ms = ?2 WHERE id = 1",
        params![i64::from(studying), at_ms],
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
        "UPDATE sessions SET last_heartbeat_ms = ?2 WHERE id = ?1",
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
