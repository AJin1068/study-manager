//! `session_retro`: the user's retrospective of a run and where its merge into the wiki stands.
//! `retro_note` is local only; reads return it to the service that builds the merge prompt.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::error::{AppError, AppResult};

/// `undone` is the state after the user restored the page from the backup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetroStatus {
    Pending,
    Merged,
    Skipped,
    Failed,
    Undone,
}

impl RetroStatus {
    pub const ALL: [RetroStatus; 5] = [
        RetroStatus::Pending,
        RetroStatus::Merged,
        RetroStatus::Skipped,
        RetroStatus::Failed,
        RetroStatus::Undone,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RetroStatus::Pending => "pending",
            RetroStatus::Merged => "merged",
            RetroStatus::Skipped => "skipped",
            RetroStatus::Failed => "failed",
            RetroStatus::Undone => "undone",
        }
    }

    fn parse(raw: &str) -> AppResult<Self> {
        Self::ALL
            .into_iter()
            .find(|s| s.as_str() == raw)
            .ok_or_else(|| AppError::Internal("session_retro holds an unknown status".into()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetroRow {
    pub run_id: i64,
    pub note: String,
    pub status: RetroStatus,
    pub subject_id: Option<i64>,
    pub page_name: Option<String>,
    pub backup_name: Option<String>,
    pub attempts: i64,
    pub last_code: Option<String>,
    pub created_at_ms: i64,
    pub merged_at_ms: Option<i64>,
}

const COLUMNS: &str = "run_id, retro_note, status, subject_id, page_name, backup_name, attempts, last_code, created_at_ms, merged_at_ms";

fn from_row(row: &Row<'_>) -> rusqlite::Result<(RetroRow, String)> {
    let status: String = row.get(2)?;
    Ok((
        RetroRow {
            run_id: row.get(0)?,
            note: row.get(1)?,
            status: RetroStatus::Pending,
            subject_id: row.get(3)?,
            page_name: row.get(4)?,
            backup_name: row.get(5)?,
            attempts: row.get(6)?,
            last_code: row.get(7)?,
            created_at_ms: row.get(8)?,
            merged_at_ms: row.get(9)?,
        },
        status,
    ))
}

fn finish((mut row, status): (RetroRow, String)) -> AppResult<RetroRow> {
    row.status = RetroStatus::parse(&status)?;
    Ok(row)
}

pub fn get(conn: &Connection, run_id: i64) -> AppResult<Option<RetroRow>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM session_retro WHERE run_id = ?1"),
        params![run_id],
        from_row,
    )
    .optional()?
    .map(finish)
    .transpose()
}

/// Rows that still wait for a merge (or failed and can be retried), newest first.
pub fn list_open(conn: &Connection) -> AppResult<Vec<RetroRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM session_retro WHERE status IN ('pending', 'failed') ORDER BY created_at_ms DESC, run_id DESC"
    ))?;
    let rows = stmt.query_map([], from_row)?;
    rows.collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(finish)
        .collect()
}

/// Stores the note for a run, replacing an earlier one that is not merged yet.
pub fn upsert_pending(
    conn: &Connection,
    run_id: i64,
    note: &str,
    subject_id: Option<i64>,
    page_name: Option<&str>,
    at_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO session_retro (run_id, retro_note, status, subject_id, page_name, created_at_ms)
         VALUES (?1, ?2, 'pending', ?3, ?4, ?5)
         ON CONFLICT(run_id) DO UPDATE SET retro_note = excluded.retro_note, status = 'pending',
           subject_id = excluded.subject_id, page_name = excluded.page_name, attempts = 0, last_code = NULL
         WHERE session_retro.status IN ('pending', 'failed', 'skipped', 'undone')",
        params![run_id, note, subject_id, page_name, at_ms],
    )?;
    Ok(())
}

pub fn record_attempt(
    conn: &Connection,
    run_id: i64,
    status: RetroStatus,
    last_code: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE session_retro SET status = ?2, attempts = attempts + 1, last_code = ?3 WHERE run_id = ?1",
        params![run_id, status.as_str(), last_code],
    )?;
    Ok(())
}

pub fn mark_merged(
    conn: &Connection,
    run_id: i64,
    page_name: &str,
    backup_name: Option<&str>,
    at_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "UPDATE session_retro SET status = 'merged', page_name = ?2, backup_name = ?3, merged_at_ms = ?4, last_code = NULL,
           page_id = (SELECT p.id FROM wiki_pages p WHERE p.subject_id = session_retro.subject_id AND p.page_name = ?2)
         WHERE run_id = ?1",
        params![run_id, page_name, backup_name, at_ms],
    )?;
    Ok(())
}

pub fn mark_skipped(conn: &Connection, run_id: i64) -> AppResult<()> {
    conn.execute("UPDATE session_retro SET status = 'skipped' WHERE run_id = ?1 AND status IN ('pending', 'failed')", params![run_id])?;
    Ok(())
}

pub fn mark_undone(conn: &Connection, run_id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE session_retro SET status = 'undone', backup_name = NULL, merged_at_ms = NULL WHERE run_id = ?1 AND status = 'merged'",
        params![run_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn statuses_match_the_table_check() {
        let conn = open_in_memory().expect("db");
        for status in RetroStatus::ALL {
            conn.execute("DELETE FROM session_retro", [])
                .expect("clear");
            conn.execute(
                "INSERT INTO session_retro (run_id, retro_note, status, created_at_ms) VALUES (1, 'n', ?1, 1)",
                params![status.as_str()],
            )
            .unwrap_or_else(|err| panic!("{status:?} rejected: {err}"));
            assert_eq!(get(&conn, 1).expect("get").map(|r| r.status), Some(status));
        }
        assert!(conn.execute("INSERT INTO session_retro (run_id, retro_note, status, created_at_ms) VALUES (2, 'n', 'other', 1)", []).is_err());
    }

    #[test]
    fn a_merged_retro_is_not_overwritten_by_a_new_note() {
        let conn = open_in_memory().expect("db");
        upsert_pending(&conn, 1, "first", None, None, 1).expect("insert");
        mark_merged(&conn, 1, "page", Some("page.1.md"), 2).expect("merge");
        upsert_pending(&conn, 1, "second", None, None, 3).expect("noop");
        let row = get(&conn, 1).expect("get").expect("row");
        assert_eq!(
            (row.note.as_str(), row.status),
            ("first", RetroStatus::Merged)
        );
    }
}
