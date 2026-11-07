//! `study_context`: short labels of what was open while studying (opt-in, 30 days).

use rusqlite::{Connection, params};

use crate::error::AppResult;

pub struct ContextRow {
    pub segment_id: i64,
    pub at_ms: i64,
    pub app_id: String,
    pub context_label: String,
}

pub fn last_label(conn: &Connection, segment_id: i64) -> AppResult<Option<(String, String)>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT app_id, context_label FROM study_context WHERE segment_id = ?1 ORDER BY id DESC LIMIT 1",
            params![segment_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

pub fn insert(conn: &Connection, row: &ContextRow) -> AppResult<()> {
    conn.execute(
        "INSERT INTO study_context (segment_id, at_ms, app_id, context_label) VALUES (?1, ?2, ?3, ?4)",
        params![row.segment_id, row.at_ms, row.app_id, row.context_label],
    )?;
    Ok(())
}

/// Removes rows older than `before_ms`; returns how many went.
pub fn prune_before(conn: &Connection, before_ms: i64) -> AppResult<usize> {
    Ok(conn.execute(
        "DELETE FROM study_context WHERE at_ms < ?1",
        params![before_ms],
    )?)
}

pub fn delete_all(conn: &Connection) -> AppResult<usize> {
    Ok(conn.execute("DELETE FROM study_context", [])?)
}

pub fn count(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM study_context", [], |r| r.get(0))?)
}

/// Distinct labels of the segments of one run, newest first.
pub fn labels_for_run(conn: &Connection, run_id: i64, limit: i64) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT c.context_label FROM study_context c JOIN timer_segments s ON s.id = c.segment_id
         WHERE s.run_id = ?1 GROUP BY c.context_label ORDER BY MAX(c.id) DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![run_id, limit], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}
