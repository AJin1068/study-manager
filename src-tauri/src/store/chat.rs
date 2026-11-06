//! Chatbot drafts and the plan-history reads its focus summary needs.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppResult;

pub struct DraftRow {
    pub id: i64,
    pub draft_json: String,
    pub applied: bool,
}

pub fn insert_draft(conn: &Connection, draft_json: &str, at_ms: i64) -> AppResult<i64> {
    conn.execute("INSERT INTO chat_drafts (created_at_ms, draft_json) VALUES (?1, ?2)", params![at_ms, draft_json])?;
    Ok(conn.last_insert_rowid())
}

pub fn get_draft(conn: &Connection, id: i64) -> AppResult<Option<DraftRow>> {
    Ok(conn
        .query_row("SELECT id, draft_json, applied_at_ms FROM chat_drafts WHERE id = ?1", params![id], |row| {
            Ok(DraftRow { id: row.get(0)?, draft_json: row.get(1)?, applied: row.get::<_, Option<i64>>(2)?.is_some() })
        })
        .optional()?)
}

pub fn mark_applied(conn: &Connection, id: i64, at_ms: i64, batch_id: &str) -> AppResult<()> {
    conn.execute("UPDATE chat_drafts SET applied_at_ms = ?2, batch_id = ?3 WHERE id = ?1 AND applied_at_ms IS NULL", params![id, at_ms, batch_id])?;
    Ok(())
}

/// Removes a draft that was not applied. `false` when it does not exist or was already applied.
pub fn discard_draft(conn: &Connection, id: i64) -> AppResult<bool> {
    Ok(conn.execute("DELETE FROM chat_drafts WHERE id = ?1 AND applied_at_ms IS NULL", params![id])? > 0)
}

/// Unapplied drafts older than `before_ms` are dropped.
pub fn prune_drafts(conn: &Connection, before_ms: i64) -> AppResult<usize> {
    Ok(conn.execute("DELETE FROM chat_drafts WHERE applied_at_ms IS NULL AND created_at_ms < ?1", params![before_ms])?)
}

/// The "before" JSON of applied `postponed` changes since `since_ms` (it holds the original date).
pub fn postponed_before_json(conn: &Connection, since_ms: i64) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT before_json FROM plan_history
         WHERE reason_code = 'postponed' AND outcome = 'applied' AND at_ms >= ?1 AND before_json IS NOT NULL",
    )?;
    let rows = stmt.query_map(params![since_ms], |row| row.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Distinct subjects of plans that still exist.
pub fn plan_subjects(conn: &Connection) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT subject FROM plans WHERE deleted_at_ms IS NULL ORDER BY subject")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}
