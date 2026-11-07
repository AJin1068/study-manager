//! `tab_verdict_cache`: the model's verdict per (domain, tab label) hash. Holds the domain only;
//! neither the label nor the address is stored.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachedVerdict {
    Study,
    Distraction,
}

impl CachedVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            CachedVerdict::Study => "study",
            CachedVerdict::Distraction => "distraction",
        }
    }
}

/// Most rows kept; the oldest go first.
pub const MAX_ROWS: i64 = 2_000;

pub fn get(
    conn: &Connection,
    key_hash: &str,
    not_before_ms: i64,
) -> AppResult<Option<CachedVerdict>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT verdict FROM tab_verdict_cache WHERE key_hash = ?1 AND decided_at_ms >= ?2",
            params![key_hash, not_before_ms],
            |r| r.get(0),
        )
        .optional()?;
    match raw.as_deref() {
        None => Ok(None),
        Some("study") => Ok(Some(CachedVerdict::Study)),
        Some("distraction") => Ok(Some(CachedVerdict::Distraction)),
        Some(_) => Err(AppError::Internal(
            "tab_verdict_cache holds an unknown verdict".into(),
        )),
    }
}

pub fn put(
    conn: &Connection,
    key_hash: &str,
    domain: &str,
    verdict: CachedVerdict,
    at_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO tab_verdict_cache (key_hash, domain, verdict, decided_at_ms) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(key_hash) DO UPDATE SET verdict = excluded.verdict, decided_at_ms = excluded.decided_at_ms",
        params![key_hash, domain, verdict.as_str(), at_ms],
    )?;
    conn.execute(
        "DELETE FROM tab_verdict_cache WHERE key_hash IN
           (SELECT key_hash FROM tab_verdict_cache ORDER BY decided_at_ms DESC LIMIT -1 OFFSET ?1)",
        params![MAX_ROWS],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn verdicts_round_trip_expire_and_stay_bounded() {
        let conn = open_in_memory().expect("db");
        put(
            &conn,
            "0123456789abcdef",
            "a.test",
            CachedVerdict::Study,
            100,
        )
        .expect("put");
        assert_eq!(
            get(&conn, "0123456789abcdef", 0).expect("get"),
            Some(CachedVerdict::Study)
        );
        assert_eq!(get(&conn, "0123456789abcdef", 101).expect("expired"), None);
        put(
            &conn,
            "0123456789abcdef",
            "a.test",
            CachedVerdict::Distraction,
            200,
        )
        .expect("update");
        assert_eq!(
            get(&conn, "0123456789abcdef", 150).expect("get"),
            Some(CachedVerdict::Distraction)
        );
        for i in 0..(MAX_ROWS + 5) {
            put(
                &conn,
                &format!("{i:016x}"),
                "b.test",
                CachedVerdict::Study,
                1_000 + i,
            )
            .expect("bulk");
        }
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM tab_verdict_cache", [], |r| r.get(0))
            .expect("count");
        assert_eq!(rows, MAX_ROWS);
    }
}
