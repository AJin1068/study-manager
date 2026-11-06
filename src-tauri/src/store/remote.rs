//! `remote_events`: one append-only row per outbound request. It records that something was sent
//! (kind, outcome, status, byte count), never what.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::AppResult;
use crate::plan::PlanHistoryEntry;

/// `sync_cursor` name of the plan-history push position.
pub const HISTORY_CURSOR: &str = "plan_history";

pub struct NewRemoteEvent<'a> {
    pub at_ms: i64,
    pub port: &'a str,
    pub kind: &'a str,
    pub outcome: &'a str,
    pub http_status: Option<u16>,
    pub bytes: i64,
    pub detail_code: Option<&'a str>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEventRow {
    pub id: i64,
    pub at_ms: i64,
    pub port: String,
    pub kind: String,
    pub outcome: String,
    pub http_status: Option<i64>,
    pub bytes: i64,
    pub detail_code: Option<String>,
}

/// Per kind: how many times and when last (the "what left the device" screen).
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EgressSummaryRow {
    pub port: String,
    pub kind: String,
    pub count: i64,
    pub last_at_ms: i64,
}

pub fn insert_event(conn: &Connection, event: &NewRemoteEvent<'_>) -> AppResult<()> {
    conn.execute(
        "INSERT INTO remote_events (at_ms, port, kind, outcome, http_status, bytes, detail_code)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![event.at_ms, event.port, event.kind, event.outcome, event.http_status, event.bytes, event.detail_code],
    )?;
    Ok(())
}

pub fn list_events(conn: &Connection, limit: i64) -> AppResult<Vec<RemoteEventRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, at_ms, port, kind, outcome, http_status, bytes, detail_code
         FROM remote_events ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(RemoteEventRow {
            id: row.get(0)?,
            at_ms: row.get(1)?,
            port: row.get(2)?,
            kind: row.get(3)?,
            outcome: row.get(4)?,
            http_status: row.get(5)?,
            bytes: row.get(6)?,
            detail_code: row.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn egress_summary(conn: &Connection) -> AppResult<Vec<EgressSummaryRow>> {
    let mut stmt = conn.prepare(
        "SELECT port, kind, COUNT(*), MAX(at_ms) FROM remote_events GROUP BY port, kind ORDER BY MAX(at_ms) DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(EgressSummaryRow { port: row.get(0)?, kind: row.get(1)?, count: row.get(2)?, last_at_ms: row.get(3)? })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- plan mirror push position and change detection ----

pub fn get_cursor(conn: &Connection, name: &str) -> AppResult<i64> {
    Ok(conn
        .query_row("SELECT last_local_id FROM sync_cursor WHERE name = ?1", params![name], |row| row.get(0))
        .optional()?
        .unwrap_or(0))
}

pub fn set_cursor(conn: &Connection, name: &str, last_local_id: i64, at_ms: i64) -> AppResult<()> {
    conn.execute(
        "INSERT INTO sync_cursor (name, last_local_id, updated_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET last_local_id = excluded.last_local_id, updated_at_ms = excluded.updated_at_ms",
        params![name, last_local_id, at_ms],
    )?;
    Ok(())
}

/// Forgets a push position so the next push starts over. Sign-out does this: the server rows went
/// away with the device, so the old position would skip history the new account never received.
pub fn reset_cursor(conn: &Connection, name: &str) -> AppResult<()> {
    conn.execute("DELETE FROM sync_cursor WHERE name = ?1", params![name])?;
    Ok(())
}

/// History rows after `after_id`, oldest first. Includes rejected attempts: the mirror is a copy.
pub fn list_plan_history_after(conn: &Connection, after_id: i64, limit: i64) -> AppResult<Vec<PlanHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, plan_id, at_ms, action, reason_code, reason_text, before_json, after_json, outcome, batch_id
         FROM plan_history WHERE id > ?1 ORDER BY id LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![after_id, limit], |row| {
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

/// Cheap change marker for the mirror worker: the newest plan-history row, release pass, and
/// setting-history row. Every plan edit appends a history row, every skip or pass appends a pass
/// row, and every settings change appends a settings row, so an unchanged marker means nothing
/// the mirror shows has changed.
pub fn change_marker(conn: &Connection) -> AppResult<(i64, i64, i64)> {
    let max = |table: &str| -> AppResult<i64> {
        Ok(conn.query_row(&format!("SELECT COALESCE(MAX(id), 0) FROM {table}"), [], |row| row.get(0))?)
    };
    Ok((max("plan_history")?, max("study_passes")?, max("setting_history")?))
}

/// "Skip today" grants on or after `from_date` as (local date, grant time).
pub fn list_skip_passes(conn: &Connection, from_date: &str) -> AppResult<Vec<(String, i64)>> {
    let mut stmt = conn.prepare("SELECT date, at_ms FROM study_passes WHERE kind = 'skip_today' AND date >= ?1 ORDER BY id")?;
    let rows = stmt.query_map(params![from_date], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn cursor_defaults_to_zero_advances_and_resets() {
        let conn = open_in_memory().expect("db");
        assert_eq!(get_cursor(&conn, HISTORY_CURSOR).expect("cursor"), 0);
        set_cursor(&conn, HISTORY_CURSOR, 40, 1).expect("set");
        set_cursor(&conn, HISTORY_CURSOR, 55, 2).expect("advance");
        assert_eq!(get_cursor(&conn, HISTORY_CURSOR).expect("cursor"), 55);
        reset_cursor(&conn, HISTORY_CURSOR).expect("reset");
        assert_eq!(get_cursor(&conn, HISTORY_CURSOR).expect("cursor"), 0);
    }

    #[test]
    fn change_marker_moves_with_each_source() {
        let conn = open_in_memory().expect("db");
        let base = change_marker(&conn).expect("marker");
        crate::store::append_history(&conn, 1, "k", None, "v", crate::settings::Reason::User, crate::settings::Outcome::Applied)
            .expect("history");
        let after = change_marker(&conn).expect("marker");
        assert_eq!((after.0, after.1), (base.0, base.1));
        assert!(after.2 > base.2);
    }

    fn event(at_ms: i64, kind: &'static str) -> NewRemoteEvent<'static> {
        NewRemoteEvent { at_ms, port: "supabase", kind, outcome: "ok", http_status: Some(200), bytes: 10, detail_code: None }
    }

    #[test]
    fn events_are_append_only_and_summarized_by_kind() {
        let conn = open_in_memory().expect("db");
        insert_event(&conn, &event(1, "auth_otp")).expect("insert");
        insert_event(&conn, &event(2, "rpc_heartbeat")).expect("insert");
        insert_event(&conn, &event(3, "rpc_heartbeat")).expect("insert");
        assert!(conn.execute("UPDATE remote_events SET bytes = 0", []).is_err());
        assert!(conn.execute("DELETE FROM remote_events", []).is_err());
        let summary = egress_summary(&conn).expect("summary");
        assert_eq!(summary.len(), 2);
        assert_eq!((summary[0].kind.as_str(), summary[0].count, summary[0].last_at_ms), ("rpc_heartbeat", 2, 3));
        assert_eq!(list_events(&conn, 10).expect("list")[0].at_ms, 3);
    }

    #[test]
    fn unknown_port_or_outcome_is_rejected_by_the_schema() {
        let conn = open_in_memory().expect("db");
        let mut bad = event(1, "x");
        bad.port = "telemetry";
        assert!(insert_event(&conn, &bad).is_err());
        let mut bad = event(1, "x");
        bad.outcome = "weird";
        assert!(insert_event(&conn, &bad).is_err());
    }
}
