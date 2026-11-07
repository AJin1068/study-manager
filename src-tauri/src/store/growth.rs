//! Totals for the growth view. Read-only.

use rusqlite::Connection;

use crate::error::AppResult;

/// Recognized study time over the app's whole life.
pub fn total_credited_ms(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(credited_ms), 0) FROM focus_minutes",
        [],
        |r| r.get(0),
    )?)
}

/// Pages in subjects that are not archived.
pub fn active_page_count(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM wiki_pages p JOIN wiki_subjects s ON s.id = p.subject_id WHERE s.archived_at_ms IS NULL",
        [],
        |r| r.get(0),
    )?)
}

/// `(answered, correct)` over every quiz attempt that was answered (skipped ones do not count).
pub fn quiz_counts(conn: &Connection) -> AppResult<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(correct), 0) FROM quiz_attempts WHERE skipped = 0",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}
