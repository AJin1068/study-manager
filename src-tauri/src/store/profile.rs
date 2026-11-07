//! Local profile rows (migration m024): the display name, the profile link id, per-section scopes,
//! the wiki page names chosen for the profile, the gift outbox and the gifts accepted. Everything
//! about other people stays on the server; nothing here is a copy of another profile.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::AppResult;
use crate::profile::model::{Scope, Section};

pub struct LocalRow {
    pub display_name: String,
    pub slug: Option<String>,
    pub web_base_url: Option<String>,
}

pub fn local(conn: &Connection) -> AppResult<LocalRow> {
    Ok(conn.query_row(
        "SELECT display_name, slug, web_base_url FROM profile_local WHERE id = 1",
        [],
        |r| {
            Ok(LocalRow {
                display_name: r.get(0)?,
                slug: r.get(1)?,
                web_base_url: r.get(2)?,
            })
        },
    )?)
}

pub fn save_name(conn: &Connection, name: &str, at_ms: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE profile_local SET display_name = ?1, updated_at_ms = ?2 WHERE id = 1",
        params![name, at_ms],
    )?;
    Ok(())
}

pub fn save_slug(conn: &Connection, slug: &str, at_ms: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE profile_local SET slug = ?1, updated_at_ms = ?2 WHERE id = 1",
        params![slug, at_ms],
    )?;
    Ok(())
}

pub fn save_web_base(conn: &Connection, base: Option<&str>, at_ms: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE profile_local SET web_base_url = ?1, updated_at_ms = ?2 WHERE id = 1",
        params![base, at_ms],
    )?;
    Ok(())
}

/// Every section's scope; a section with no row is private.
pub fn visibility(conn: &Connection) -> AppResult<BTreeMap<Section, Scope>> {
    let mut map: BTreeMap<Section, Scope> = Section::ALL
        .into_iter()
        .map(|s| (s, Scope::Private))
        .collect();
    let mut stmt = conn.prepare("SELECT section, scope FROM profile_visibility")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (section, scope) = row?;
        if let (Some(section), Some(scope)) = (Section::parse(&section), Scope::parse(&scope)) {
            map.insert(section, scope);
        }
    }
    Ok(map)
}

pub fn set_visibility(
    conn: &Connection,
    section: Section,
    scope: Scope,
    at_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO profile_visibility (section, scope, updated_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT (section) DO UPDATE SET scope = excluded.scope, updated_at_ms = excluded.updated_at_ms",
        params![section.as_str(), scope.as_str(), at_ms],
    )?;
    Ok(())
}

pub fn wiki_titles(conn: &Connection) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT page_name FROM profile_wiki_public ORDER BY page_name")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn replace_wiki_titles(conn: &mut Connection, titles: &[String]) -> AppResult<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM profile_wiki_public", [])?;
    for title in titles {
        tx.execute(
            "INSERT OR IGNORE INTO profile_wiki_public (page_name) VALUES (?1)",
            params![title],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Recognized study minutes per local plan date in `from..=to` (date strings compare in calendar
/// order). Dates without a run are absent.
pub fn study_minutes(conn: &Connection, from: &str, to: &str) -> AppResult<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT r.date, COALESCE(SUM(m.credited_ms), 0)
         FROM plan_runs r
         LEFT JOIN timer_segments s ON s.run_id = r.id
         LEFT JOIN focus_minutes m ON m.segment_id = s.id
         WHERE r.date >= ?1 AND r.date <= ?2
         GROUP BY r.date ORDER BY r.date",
    )?;
    let rows = stmt.query_map(params![from, to], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? / 60_000))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The subject of the plan run in progress, if the timer has one.
pub fn run_subject(conn: &Connection, run_id: i64) -> AppResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT p.subject FROM plan_runs r JOIN plans p ON p.id = r.plan_id WHERE r.id = ?1",
            params![run_id],
            |r| r.get(0),
        )
        .optional()?)
}

// ---- gift outbox ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxRow {
    pub gift_id: String,
    pub recipient_user: String,
    pub recipient_name: String,
    pub item_id: String,
    pub message: String,
    pub price: i64,
    pub state: String,
    pub last_code: Option<String>,
    pub attempts: i64,
    pub created_at_ms: i64,
    pub last_attempt_ms: Option<i64>,
}

const OUTBOX_COLUMNS: &str = "gift_id, recipient_user, recipient_name, item_id, message, price, state, last_code, attempts, created_at_ms, last_attempt_ms";

fn outbox_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxRow> {
    Ok(OutboxRow {
        gift_id: r.get(0)?,
        recipient_user: r.get(1)?,
        recipient_name: r.get(2)?,
        item_id: r.get(3)?,
        message: r.get(4)?,
        price: r.get(5)?,
        state: r.get(6)?,
        last_code: r.get(7)?,
        attempts: r.get(8)?,
        created_at_ms: r.get(9)?,
        last_attempt_ms: r.get(10)?,
    })
}

pub fn insert_outbox(conn: &Connection, row: &OutboxRow) -> AppResult<()> {
    conn.execute(
        "INSERT INTO gift_outbox (gift_id, recipient_user, recipient_name, item_id, message, price, state, attempts, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0, ?7)",
        params![row.gift_id, row.recipient_user, row.recipient_name, row.item_id, row.message, row.price, row.created_at_ms],
    )?;
    Ok(())
}

pub fn outbox(conn: &Connection, gift_id: &str) -> AppResult<Option<OutboxRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {OUTBOX_COLUMNS} FROM gift_outbox WHERE gift_id = ?1"),
            params![gift_id],
            outbox_row,
        )
        .optional()?)
}

pub fn pending_outbox(conn: &Connection) -> AppResult<Vec<OutboxRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {OUTBOX_COLUMNS} FROM gift_outbox WHERE state = 'pending' ORDER BY created_at_ms, gift_id"))?;
    let rows = stmt.query_map([], outbox_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn recent_outbox(conn: &Connection, limit: i64) -> AppResult<Vec<OutboxRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {OUTBOX_COLUMNS} FROM gift_outbox ORDER BY created_at_ms DESC, gift_id LIMIT ?1"
    ))?;
    let rows = stmt.query_map(params![limit], outbox_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn note_attempt(conn: &Connection, gift_id: &str, code: &str, at_ms: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE gift_outbox SET attempts = attempts + 1, last_code = ?2, last_attempt_ms = ?3 WHERE gift_id = ?1 AND state = 'pending'",
        params![gift_id, code, at_ms],
    )?;
    Ok(())
}

/// Moves a pending row to `state`. Returns whether this call did it, so a refund or a mark runs
/// exactly once however many times it is retried.
pub fn resolve_outbox(
    conn: &Connection,
    gift_id: &str,
    state: &str,
    at_ms: i64,
) -> AppResult<bool> {
    let changed = conn.execute(
        "UPDATE gift_outbox SET state = ?2, resolved_at_ms = ?3 WHERE gift_id = ?1 AND state = 'pending'",
        params![gift_id, state, at_ms],
    )?;
    Ok(changed == 1)
}

/// Sent gifts today (local count of what this PC sent), so the daily limit is shown before a try.
pub fn sent_since(conn: &Connection, since_ms: i64) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM gift_outbox WHERE created_at_ms >= ?1 AND state <> 'refunded'",
        params![since_ms],
        |r| r.get(0),
    )?)
}

// ---- received ----

pub fn received_outcome(conn: &Connection, gift_id: &str) -> AppResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT outcome FROM gift_received WHERE gift_id = ?1",
            params![gift_id],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn record_received(
    conn: &Connection,
    gift_id: &str,
    item_id: &str,
    sender: &str,
    message: &str,
    outcome: &str,
    at_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO gift_received (gift_id, item_id, sender_name, message, outcome, resolved_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (gift_id) DO NOTHING",
        params![gift_id, item_id, sender, message, outcome, at_ms],
    )?;
    Ok(())
}
