//! SQLite schema and migrations. Append-only tables are guarded by triggers that reject
//! UPDATE and DELETE through normal SQL. This is a recording guarantee, not tamper-proofing:
//! anyone with file access can DROP TRIGGER or edit the file (out of scope for 1-1).

use rusqlite::Connection;

use crate::error::AppResult;

const MIGRATION_1: &str = r#"
CREATE TABLE settings (
    key           TEXT PRIMARY KEY,
    category      TEXT NOT NULL,
    value         TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE setting_history (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms   INTEGER NOT NULL,
    key     TEXT NOT NULL,
    before  TEXT,
    after   TEXT NOT NULL,
    reason  TEXT NOT NULL CHECK (reason IN ('user', 'default', 'reconcile', 'test_toggle')),
    outcome TEXT NOT NULL CHECK (outcome IN ('applied', 'rejected_locked'))
);
CREATE TRIGGER setting_history_no_update BEFORE UPDATE ON setting_history
BEGIN SELECT RAISE(ABORT, 'setting_history is append-only'); END;
CREATE TRIGGER setting_history_no_delete BEFORE DELETE ON setting_history
BEGIN SELECT RAISE(ABORT, 'setting_history is append-only'); END;

-- Seeded in the same transaction as the schema, so a crash cannot leave a schema with no lock
-- row. changed_at_ms = 0 means "never changed". A missing row is an error on every later read.
CREATE TABLE study_state (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    studying      INTEGER NOT NULL CHECK (studying IN (0, 1)),
    changed_at_ms INTEGER NOT NULL
);
INSERT INTO study_state (id, studying, changed_at_ms) VALUES (1, 0, 0);

CREATE TABLE daily_mode (
    date           TEXT PRIMARY KEY,
    level          TEXT NOT NULL CHECK (level IN ('easy', 'medium', 'hard')),
    selected_at_ms INTEGER NOT NULL
);

CREATE TABLE study_mode_config (
    level                 TEXT PRIMARY KEY CHECK (level IN ('easy', 'medium', 'hard')),
    study_min             INTEGER NOT NULL,
    break_min             INTEGER NOT NULL,
    long_break_min        INTEGER,
    sets_before_long_break INTEGER
);
INSERT INTO study_mode_config VALUES ('easy', 25, 5, 20, 4);
INSERT INTO study_mode_config VALUES ('medium', 50, 15, NULL, NULL);
INSERT INTO study_mode_config VALUES ('hard', 90, 10, NULL, NULL);

-- clean_exit: 1 = normal exit via RunEvent::ExitRequested; 0 = unclean exit already
-- recorded as a forced_quit downtime on the next launch; NULL = session still open.
CREATE TABLE sessions (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at_ms     INTEGER NOT NULL,
    last_heartbeat_ms INTEGER NOT NULL,
    clean_exit        INTEGER CHECK (clean_exit IN (0, 1))
);

CREATE TABLE downtime_records (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL CHECK (kind IN ('sleep', 'forced_quit')),
    start_at_ms INTEGER NOT NULL,
    end_at_ms   INTEGER NOT NULL CHECK (end_at_ms >= start_at_ms),
    estimated   INTEGER NOT NULL DEFAULT 0 CHECK (estimated IN (0, 1))
);
CREATE TRIGGER downtime_no_update BEFORE UPDATE ON downtime_records
BEGIN SELECT RAISE(ABORT, 'downtime_records is append-only'); END;
CREATE TRIGGER downtime_no_delete BEFORE DELETE ON downtime_records
BEGIN SELECT RAISE(ABORT, 'downtime_records is append-only'); END;
"#;

const MIGRATIONS: &[&str] = &[MIGRATION_1];

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
}
