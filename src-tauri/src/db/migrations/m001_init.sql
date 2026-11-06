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
-- The column is named `studying` for history; its meaning is `lock_held`: 1 while a planned
-- study run (study plus the breaks between its sets) holds the settings/exit/plan lock.
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
