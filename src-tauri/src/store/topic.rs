//! `topic_assignment`: which wiki page a plan occurrence should cover. Rows are insert-only; the
//! newest row of a `(plan_id, date)` is the current assignment, older rows are its history.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::AppResult;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignmentRow {
    pub plan_id: i64,
    pub date: String,
    pub page_id: i64,
    pub source: String,
    pub reason: String,
    pub assigned_at_ms: i64,
}

fn row_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<AssignmentRow> {
    Ok(AssignmentRow {
        plan_id: row.get(0)?,
        date: row.get(1)?,
        page_id: row.get(2)?,
        source: row.get(3)?,
        reason: row.get(4)?,
        assigned_at_ms: row.get(5)?,
    })
}

const COLUMNS: &str = "plan_id, date, page_id, source, reason, assigned_at_ms";

pub fn insert(conn: &Connection, row: &AssignmentRow) -> AppResult<()> {
    conn.execute(
        "INSERT INTO topic_assignment (plan_id, date, page_id, source, reason, assigned_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![row.plan_id, row.date, row.page_id, row.source, row.reason, row.assigned_at_ms],
    )?;
    Ok(())
}

/// The current assignment of one occurrence.
pub fn current(conn: &Connection, plan_id: i64, date: &str) -> AppResult<Option<AssignmentRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM topic_assignment WHERE plan_id = ?1 AND date = ?2 ORDER BY id DESC LIMIT 1"),
            params![plan_id, date],
            row_from,
        )
        .optional()?)
}

/// Current assignments for dates in `from..=to`, oldest date first.
pub fn current_between(conn: &Connection, from: &str, to: &str) -> AppResult<Vec<AssignmentRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM topic_assignment
         WHERE id IN (SELECT MAX(id) FROM topic_assignment WHERE date >= ?1 AND date <= ?2 GROUP BY plan_id, date)
         ORDER BY date, plan_id"
    ))?;
    let rows = stmt.query_map(params![from, to], row_from)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Every assignment ever made for one occurrence, newest first.
pub fn history(conn: &Connection, plan_id: i64, date: &str) -> AppResult<Vec<AssignmentRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM topic_assignment WHERE plan_id = ?1 AND date = ?2 ORDER BY id DESC"
    ))?;
    let rows = stmt.query_map(params![plan_id, date], row_from)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// `(minute_start_ms, credited_ms, away_ms, idle_ms)` of study minutes since `from_ms`.
pub fn focus_minutes_since(
    conn: &Connection,
    from_ms: i64,
) -> AppResult<Vec<(i64, i64, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT minute_start_ms, credited_ms, away_ms, idle_ms FROM focus_minutes WHERE minute_start_ms >= ?1 ORDER BY minute_start_ms",
    )?;
    let rows = stmt.query_map(params![from_ms], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
