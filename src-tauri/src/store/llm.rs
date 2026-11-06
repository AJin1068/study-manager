//! `llm_usage`: one append-only row per LLM call attempt (purpose, model, token counts, latency,
//! result code). It records that a call happened, never what was in it.

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::AppResult;

pub struct NewUsage<'a> {
    pub at_ms: i64,
    pub purpose: &'a str,
    pub model: &'a str,
    /// `mock` or `live`.
    pub mode: &'a str,
    pub in_tokens: i64,
    pub out_tokens: i64,
    pub latency_ms: i64,
    pub cached: bool,
    pub result_code: &'a str,
}

/// Tokens used per purpose within a window, for the usage screen.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PurposeUsage {
    pub purpose: String,
    pub calls: i64,
    pub tokens: i64,
}

pub fn insert_usage(conn: &Connection, usage: &NewUsage<'_>) -> AppResult<()> {
    conn.execute(
        "INSERT INTO llm_usage (at_ms, purpose, model, mode, in_tokens, out_tokens, latency_ms, cached, result_code)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            usage.at_ms,
            usage.purpose,
            usage.model,
            usage.mode,
            usage.in_tokens,
            usage.out_tokens,
            usage.latency_ms,
            usage.cached,
            usage.result_code
        ],
    )?;
    Ok(())
}

/// Input plus output tokens of every call in `[from_ms, to_ms)`. Mock calls count too, so practice
/// mode shows the same budget behavior as the real thing.
pub fn tokens_between(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(in_tokens + out_tokens), 0) FROM llm_usage WHERE at_ms >= ?1 AND at_ms < ?2",
        params![from_ms, to_ms],
        |row| row.get(0),
    )?)
}

pub fn usage_by_purpose(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<Vec<PurposeUsage>> {
    let mut stmt = conn.prepare(
        "SELECT purpose, COUNT(*), COALESCE(SUM(in_tokens + out_tokens), 0) FROM llm_usage
         WHERE at_ms >= ?1 AND at_ms < ?2 GROUP BY purpose ORDER BY purpose",
    )?;
    let rows = stmt.query_map(params![from_ms, to_ms], |row| {
        Ok(PurposeUsage { purpose: row.get(0)?, calls: row.get(1)?, tokens: row.get(2)? })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn usage(at_ms: i64, mode: &'static str, tokens: i64) -> NewUsage<'static> {
        NewUsage { at_ms, purpose: "wiki_ingest", model: "m", mode, in_tokens: tokens, out_tokens: 1, latency_ms: 5, cached: false, result_code: "ok" }
    }

    #[test]
    fn usage_is_append_only_and_every_mode_counts_toward_the_budget() {
        let conn = open_in_memory().expect("db");
        insert_usage(&conn, &usage(10, "live", 100)).expect("live");
        insert_usage(&conn, &usage(11, "mock", 5_000)).expect("mock");
        insert_usage(&conn, &usage(500, "live", 7)).expect("later");
        assert!(conn.execute("UPDATE llm_usage SET in_tokens = 0", []).is_err());
        assert!(conn.execute("DELETE FROM llm_usage", []).is_err());
        assert_eq!(tokens_between(&conn, 0, 100).expect("sum"), 5_102);
        assert_eq!(tokens_between(&conn, 100, 1000).expect("sum"), 8);
        let by = usage_by_purpose(&conn, 0, 1000).expect("by purpose");
        assert_eq!(by, [PurposeUsage { purpose: "wiki_ingest".into(), calls: 3, tokens: 5_110 }]);
    }

    #[test]
    fn unknown_purpose_or_mode_is_rejected_by_the_schema() {
        let conn = open_in_memory().expect("db");
        let mut bad = usage(1, "live", 1);
        bad.purpose = "telemetry";
        assert!(insert_usage(&conn, &bad).is_err());
        assert!(insert_usage(&conn, &usage(1, "dev", 1)).is_err());
    }
}
