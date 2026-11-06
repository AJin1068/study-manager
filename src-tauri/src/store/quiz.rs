//! Quiz tables: the question bank, the append-only attempt log, reports, and the early-release
//! counters. Answers the user gave are not stored anywhere: an attempt row says right, wrong, or
//! skipped.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::quiz::generate::NewItem;
use crate::quiz::select::Candidate;
use crate::quiz::{ItemKind, QuizItem};

pub struct NewAttempt<'a> {
    pub at_ms: i64,
    pub item_id: i64,
    pub context: &'a str,
    pub release_kind: Option<&'a str>,
    pub run_id: Option<i64>,
    pub segment_id: Option<i64>,
    pub correct: bool,
    pub skipped: bool,
    pub response_ms: i64,
}

/// Counts for the quiz card.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BankStats {
    /// Questions that can be asked now (not reported, subject not archived).
    pub drawable: i64,
    pub reported: i64,
}

const ITEM_COLUMNS: &str = "i.id, i.page_id, i.kind, i.prompt, i.choices_json, i.answer_key";

fn corrupt(what: &str) -> AppError {
    AppError::Internal(format!("quiz bank row is unreadable: {what}"))
}

fn item_from(row: &Row<'_>) -> AppResult<QuizItem> {
    let kind_raw: String = row.get(2)?;
    let choices_raw: String = row.get(4)?;
    Ok(QuizItem {
        id: row.get(0)?,
        page_id: row.get(1)?,
        kind: ItemKind::parse(&kind_raw).ok_or_else(|| corrupt("kind"))?,
        prompt: row.get(3)?,
        choices: serde_json::from_str(&choices_raw).map_err(|_| corrupt("choices"))?,
        answer_key: row.get(5)?,
    })
}

/// Stores a question; `false` when the same prompt already exists for the page.
pub fn insert_item(conn: &Connection, page_id: i64, item: &NewItem, at_ms: i64) -> AppResult<bool> {
    let choices = serde_json::to_string(&item.choices).map_err(|_| corrupt("choices"))?;
    let changed = conn.execute(
        "INSERT OR IGNORE INTO quiz_items (page_id, kind, prompt, choices_json, answer_key, evidence_quote, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![page_id, item.kind.as_str(), item.prompt, choices, item.answer_key, item.evidence_quote, at_ms],
    )?;
    Ok(changed > 0)
}

#[cfg(test)]
pub fn count_for_page(conn: &Connection, page_id: i64) -> AppResult<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM quiz_items WHERE page_id = ?1", params![page_id], |r| r.get(0))?)
}

pub fn get_item(conn: &Connection, id: i64) -> AppResult<Option<QuizItem>> {
    let sql = format!("SELECT {ITEM_COLUMNS} FROM quiz_items i WHERE i.id = ?1");
    conn.query_row(&sql, params![id], |row| Ok(item_from(row))).optional()?.transpose()
}

pub fn bank_stats(conn: &Connection) -> AppResult<BankStats> {
    let drawable = drawable_count(conn)?;
    let reported = conn.query_row("SELECT COUNT(*) FROM quiz_items WHERE disabled_at_ms IS NOT NULL", [], |r| r.get(0))?;
    Ok(BankStats { drawable, reported })
}

/// Questions that can be asked now: not reported, and their subject is not archived.
pub fn drawable_count(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM quiz_items i JOIN wiki_pages p ON p.id = i.page_id JOIN wiki_subjects s ON s.id = p.subject_id
         WHERE i.disabled_at_ms IS NULL AND s.archived_at_ms IS NULL",
        [],
        |r| r.get(0),
    )?)
}

/// Drawable questions with what the draw needs. `subject_id` narrows to one subject.
pub fn candidates(conn: &Connection, subject_id: Option<i64>) -> AppResult<Vec<Candidate>> {
    let sql = format!(
        "SELECT {ITEM_COLUMNS}, p.quiz_total, p.quiz_correct, (SELECT MAX(a.at_ms) FROM quiz_attempts a WHERE a.item_id = i.id)
         FROM quiz_items i JOIN wiki_pages p ON p.id = i.page_id JOIN wiki_subjects s ON s.id = p.subject_id
         WHERE i.disabled_at_ms IS NULL AND s.archived_at_ms IS NULL AND (?1 IS NULL OR s.id = ?1)
         ORDER BY i.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![subject_id], |row| {
        let counts = (row.get::<_, i64>(6)?, row.get::<_, i64>(7)?, row.get::<_, Option<i64>>(8)?);
        Ok((item_from(row), counts))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (item, (total, correct, last)) = row?;
        out.push(Candidate { item: item?, page_total: total, page_correct: correct, last_asked_ms: last });
    }
    Ok(out)
}

/// Appends an attempt and, unless it was skipped, counts it on the page (`wiki_pages` counters).
pub fn insert_attempt(conn: &Connection, a: &NewAttempt<'_>) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO quiz_attempts (at_ms, item_id, context, release_kind, run_id, segment_id, correct, skipped, response_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![a.at_ms, a.item_id, a.context, a.release_kind, a.run_id, a.segment_id, a.correct, a.skipped, a.response_ms.max(0)],
    )?;
    let id = conn.last_insert_rowid();
    if !a.skipped {
        conn.execute(
            "UPDATE wiki_pages SET quiz_total = quiz_total + 1, quiz_correct = quiz_correct + ?2
             WHERE id = (SELECT page_id FROM quiz_items WHERE id = ?1)",
            params![a.item_id, i64::from(a.correct)],
        )?;
    }
    Ok(id)
}

/// Reports a question and removes it from every later draw.
pub fn flag_item(conn: &Connection, item_id: i64, reason: &str, at_ms: i64) -> AppResult<()> {
    if get_item(conn, item_id)?.is_none() {
        return Err(AppError::Invalid(format!("unknown quiz item {item_id}")));
    }
    conn.execute("INSERT INTO quiz_flags (item_id, at_ms, reason_code) VALUES (?1, ?2, ?3)", params![item_id, at_ms, reason])?;
    conn.execute("UPDATE quiz_items SET disabled_at_ms = COALESCE(disabled_at_ms, ?2) WHERE id = ?1", params![item_id, at_ms])?;
    Ok(())
}

/// `(attempts, last_attempt_ms)` of an early-release day.
pub fn release_state(conn: &Connection, date: &str) -> AppResult<Option<(i64, i64)>> {
    Ok(conn
        .query_row("SELECT attempts, last_attempt_ms FROM release_quiz_state WHERE date = ?1", params![date], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?)
}

/// Counts one early-release attempt and starts its cooldown.
pub fn open_release_attempt(conn: &Connection, date: &str, at_ms: i64) -> AppResult<()> {
    conn.execute(
        "INSERT INTO release_quiz_state (date, attempts, last_attempt_ms) VALUES (?1, 1, ?2)
         ON CONFLICT(date) DO UPDATE SET attempts = attempts + 1, last_attempt_ms = excluded.last_attempt_ms",
        params![date, at_ms],
    )?;
    Ok(())
}

/// A failed attempt restarts the cooldown from the moment of failure.
pub fn mark_release_failure(conn: &Connection, date: &str, at_ms: i64) -> AppResult<()> {
    conn.execute("UPDATE release_quiz_state SET last_attempt_ms = MAX(last_attempt_ms, ?2) WHERE date = ?1", params![date, at_ms])?;
    Ok(())
}

/// `(answered, correct, skipped)` of the session-end quiz of a run.
pub fn session_end_stats(conn: &Connection, run_id: i64) -> AppResult<(i64, i64, i64)> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(1 - skipped), 0), COALESCE(SUM(correct), 0), COALESCE(SUM(skipped), 0)
         FROM quiz_attempts WHERE context = 'session_end' AND run_id = ?1",
        params![run_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?)
}

/// Whether the session-end quiz of the run was already answered or skipped.
pub fn session_end_done(conn: &Connection, run_id: i64) -> AppResult<bool> {
    Ok(conn.query_row("SELECT COUNT(*) FROM quiz_attempts WHERE context = 'session_end' AND run_id = ?1", params![run_id], |r| r.get::<_, i64>(0))? > 0)
}

/// Pages that have text indexed but no question yet: `(page_id, subject_id, page_name)`.
pub fn pages_without_items(conn: &Connection, subject_id: i64) -> AppResult<Vec<(i64, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.subject_id, p.page_name FROM wiki_pages p
         WHERE p.subject_id = ?1 AND NOT EXISTS (SELECT 1 FROM quiz_items i WHERE i.page_id = p.id) ORDER BY p.page_name",
    )?;
    let rows = stmt.query_map(params![subject_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Sum of the bonus rows already posted in `[from_ms, to_ms)`, counted by number of rows.
pub fn bonus_rows_between(conn: &Connection, reason: &str, from_ms: i64, to_ms: i64) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM point_ledger WHERE reason_code = ?1 AND at_ms >= ?2 AND at_ms < ?3",
        params![reason, from_ms, to_ms],
        |r| r.get(0),
    )?)
}

/// Id of the newest block event that is a blocked-app or blocked-site attempt (0 when none).
/// Read-only: the quiz only watches the log, it never writes to it.
pub fn latest_attempt_event_id(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(id), 0) FROM block_events WHERE action IN ('would_terminate', 'terminate_requested', 'site_blocked')",
        [],
        |r| r.get(0),
    )?)
}
