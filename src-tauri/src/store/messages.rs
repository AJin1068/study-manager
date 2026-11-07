//! `message_log`: that a cheer/scold message was sent (never its text).

use rusqlite::{Connection, params};

use crate::error::AppResult;

pub fn insert(
    conn: &Connection,
    at_ms: i64,
    kind: &str,
    tone: &str,
    source: &str,
    run_id: Option<i64>,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO message_log (at_ms, kind, tone, source, run_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![at_ms, kind, tone, source, run_id],
    )?;
    Ok(())
}

/// Messages sent at or after `since_ms`.
pub fn count_since(conn: &Connection, since_ms: i64) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM message_log WHERE at_ms >= ?1",
        params![since_ms],
        |r| r.get(0),
    )?)
}

/// When the last message of any kind was sent.
pub fn last_at(conn: &Connection) -> AppResult<Option<i64>> {
    Ok(conn.query_row("SELECT MAX(at_ms) FROM message_log", [], |r| r.get(0))?)
}
