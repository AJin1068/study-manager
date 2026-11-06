//! SQLite schema and migrations. Append-only tables are guarded by triggers that reject
//! UPDATE and DELETE through normal SQL, including `INSERT OR REPLACE` (it deletes the conflicting
//! row, which only fires the delete trigger with `recursive_triggers` on; `init` enables it).
//! Upserts (`ON CONFLICT DO UPDATE`) count as UPDATE, so they must not target a table guarded by
//! an update trigger. This is a recording guarantee, not tamper-proofing:
//! anyone with file access can DROP TRIGGER or edit the file (out of scope for 1-1).

use rusqlite::Connection;

use crate::error::AppResult;

/// Migrations in order; the version stored in `user_version` is the array position + 1.
/// Never edit a shipped file: append a new `mNNN_*.sql` and add it here.
const MIGRATIONS: &[&str] = &[
    include_str!("migrations/m001_init.sql"),
    include_str!("migrations/m002_plans.sql"),
    include_str!("migrations/m003_timer.sql"),
    include_str!("migrations/m004_block.sql"),
    include_str!("migrations/m005_study_passes.sql"),
    include_str!("migrations/m006_study_apps.sql"),
    include_str!("migrations/m007_focus.sql"),
    include_str!("migrations/m008_points.sql"),
    include_str!("migrations/m009_remote.sql"),
    include_str!("migrations/m010_llm_usage.sql"),
    include_str!("migrations/m011_wiki_core.sql"),
    include_str!("migrations/m012_quiz.sql"),
    include_str!("migrations/m013_release_proof.sql"),
    include_str!("migrations/m014_session_retro.sql"),
    include_str!("migrations/m015_tab_verdict_cache.sql"),
    include_str!("migrations/m016_study_context.sql"),
    include_str!("migrations/m017_topic_assignment.sql"),
    include_str!("migrations/m018_message_log.sql"),
    include_str!("migrations/m019_chat_drafts.sql"),
];

/// Opens (creating if needed) the database file and applies pending migrations. The lock row is
/// created by the first migration, so it cannot be re-seeded later: a missing row is an error.
pub fn open(path: &std::path::Path) -> AppResult<Connection> {
    init(Connection::open(path)?)
}

/// Opens an in-memory database with the same schema and seeded lock row as a fresh app.
#[cfg(test)]
pub fn open_in_memory() -> AppResult<Connection> {
    init(Connection::open_in_memory()?)
}

fn init(conn: Connection) -> AppResult<Connection> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Without this, REPLACE-conflict deletes skip DELETE triggers and bypass the append-only guard.
    conn.pragma_update(None, "recursive_triggers", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> AppResult<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let version = i64::try_from(index + 1)
            .map_err(|_| crate::error::AppError::Internal("migration index overflows user_version".into()))?;
        if version <= current {
            continue;
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_history_rejects_update_and_delete() {
        let conn = open_in_memory().expect("open");
        conn.execute(
            "INSERT INTO setting_history (at_ms, key, before, after, reason, outcome) VALUES (1, 'autostart', NULL, 'true', 'default', 'applied')",
            [],
        )
        .expect("insert");
        assert!(conn.execute("UPDATE setting_history SET after = 'x'", []).is_err());
        assert!(conn.execute("DELETE FROM setting_history", []).is_err());
    }

    #[test]
    fn downtime_rejects_update_and_delete() {
        let conn = open_in_memory().expect("open");
        conn.execute(
            "INSERT INTO downtime_records (kind, start_at_ms, end_at_ms, estimated) VALUES ('sleep', 1, 2, 0)",
            [],
        )
        .expect("insert");
        assert!(conn.execute("UPDATE downtime_records SET estimated = 1", []).is_err());
        assert!(conn.execute("DELETE FROM downtime_records", []).is_err());
    }

    #[test]
    fn insert_or_replace_cannot_bypass_append_only_triggers() {
        let conn = open_in_memory().expect("open");
        let on: i64 = conn.pragma_query_value(None, "recursive_triggers", |row| row.get(0)).expect("pragma");
        assert_eq!(on, 1);
        conn.execute("INSERT INTO point_ledger (at_ms, delta, reason_code, ref_key) VALUES (1, 5, 'test', 'k1')", [])
            .expect("insert");
        let replaced =
            conn.execute("INSERT OR REPLACE INTO point_ledger (at_ms, delta, reason_code, ref_key) VALUES (2, 999, 'test', 'k1')", []);
        assert!(replaced.is_err());
        let (delta, rows): (i64, i64) = conn
            .query_row("SELECT COALESCE(SUM(delta), 0), COUNT(*) FROM point_ledger", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("sum");
        assert_eq!((delta, rows), (5, 1));
    }

    #[test]
    fn mode_config_matches_plan_values() {
        let conn = open_in_memory().expect("open");
        let row: (i64, i64, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT study_min, break_min, long_break_min, sets_before_long_break FROM study_mode_config WHERE level = 'easy'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .expect("easy row");
        assert_eq!(row, (25, 5, Some(20), Some(4)));
    }

    #[test]
    fn m009_keeps_history_rows_and_widens_the_vocabulary_without_losing_the_guard() {
        let conn = Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "foreign_keys", "ON").expect("fk");
        conn.pragma_update(None, "recursive_triggers", "ON").expect("triggers");
        for sql in &MIGRATIONS[..8] {
            conn.execute_batch(sql).expect("migration");
        }
        conn.pragma_update(None, "user_version", 8).expect("version");
        conn.execute(
            "INSERT INTO setting_history (id, at_ms, key, before, after, reason, outcome) VALUES (41, 1, 'autostart', NULL, 'true', 'default', 'applied')",
            [],
        )
        .expect("old row");
        assert!(
            conn.execute("INSERT INTO setting_history (at_ms, key, after, reason, outcome) VALUES (2, 'k', 'v', 'secret_set', 'applied')", []).is_err(),
            "the old vocabulary is closed before m009"
        );

        migrate(&conn).expect("m009");

        let kept: (i64, String) = conn
            .query_row("SELECT id, key FROM setting_history WHERE id = 41", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("kept");
        assert_eq!(kept, (41, "autostart".to_string()));
        conn.execute("INSERT INTO setting_history (at_ms, key, after, reason, outcome) VALUES (3, 'k', 'v', 'secret_cleared', 'applied_privacy_tighten')", [])
            .expect("new vocabulary");
        let next: i64 = conn.query_row("SELECT MAX(id) FROM setting_history", [], |r| r.get(0)).expect("max");
        assert!(next > 41, "ids keep increasing after the rebuild");
        assert!(conn.execute("UPDATE setting_history SET after = 'x'", []).is_err());
        assert!(conn.execute("DELETE FROM setting_history", []).is_err());
        assert!(conn.execute("INSERT INTO setting_history (at_ms, key, after, reason, outcome) VALUES (4, 'k', 'v', 'whatever', 'applied')", []).is_err());
    }

    #[test]
    fn m013_marks_old_passes_as_typing_and_keeps_the_append_only_guard() {
        let conn = Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "foreign_keys", "ON").expect("fk");
        conn.pragma_update(None, "recursive_triggers", "ON").expect("triggers");
        let before = MIGRATIONS.iter().position(|sql| sql.contains("proof_kind")).expect("the proof migration");
        for sql in &MIGRATIONS[..before] {
            conn.execute_batch(sql).expect("migration");
        }
        conn.pragma_update(None, "user_version", i64::try_from(before).expect("version")).expect("version");
        conn.execute(
            "INSERT INTO study_passes (at_ms, date, kind, challenge_len, typing_ms, penalty_points) VALUES (1, '2026-03-09', 'skip_today', 90, 4000, NULL)",
            [],
        )
        .expect("shipped row");

        migrate(&conn).expect("m013");

        let kept: (String, i64, i64) = conn
            .query_row("SELECT proof_kind, challenge_len, typing_ms FROM study_passes WHERE at_ms = 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .expect("kept");
        assert_eq!(kept, ("typing".to_string(), 90, 4000));
        assert!(conn.execute("UPDATE study_passes SET proof_kind = 'quiz'", []).is_err(), "append-only trigger survived");
        assert!(conn.execute("DELETE FROM study_passes", []).is_err());
        assert!(
            conn.execute("INSERT INTO study_passes (at_ms, date, kind, challenge_len, typing_ms, proof_kind) VALUES (2, 'd', 'skip_today', 5, 0, 'quiz')", []).is_err(),
            "the guard trigger is in place"
        );
    }

    /// Shared privacy guard (AGENTS.md section 9): no table or column may be named like data we
    /// promised never to store. Every later migration must keep this passing.
    #[test]
    fn privacy_schema_guard() {
        let conn = open_in_memory().expect("open");
        let mut stmt = conn
            .prepare(
                "SELECT m.name, p.name FROM sqlite_master m, pragma_table_info(m.name) p \
                 WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%'",
            )
            .expect("prepare");
        let pairs: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert!(!pairs.is_empty(), "guard must inspect at least one column");
        let names = tables.iter().chain(pairs.iter().map(|(_, column)| column));
        for name in names {
            let lower = name.to_ascii_lowercase();
            let forbidden = lower == "title"
                || lower == "window_title"
                || lower == "url"
                || lower.starts_with("typed")
                || lower.starts_with("sentence");
            assert!(!forbidden, "privacy: forbidden schema name `{name}`");
        }
    }
}
