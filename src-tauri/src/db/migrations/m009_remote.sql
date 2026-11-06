-- Stage 2-1 (slug: remote). Server-link bookkeeping plus the setting_history vocabulary for
-- secrets and privacy tightening.
--
-- setting_history is rebuilt because its CHECK lists were closed in m001 and a shipped file is
-- never edited. Existing rows keep their ids, so ordering and the AUTOINCREMENT counter survive.
-- New reasons: `secret_set` / `secret_cleared` record only that a secret changed, never its value
-- or a hash of it. New outcome: `applied_privacy_tighten` marks a change that narrows what leaves
-- the device and is therefore allowed during a planned run (settings lock exception).
CREATE TABLE setting_history_v2 (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms   INTEGER NOT NULL,
    key     TEXT NOT NULL,
    before  TEXT,
    after   TEXT NOT NULL,
    reason  TEXT NOT NULL CHECK (reason IN ('user', 'default', 'reconcile', 'test_toggle', 'secret_set', 'secret_cleared')),
    outcome TEXT NOT NULL CHECK (outcome IN ('applied', 'rejected_locked', 'applied_privacy_tighten'))
);
INSERT INTO setting_history_v2 (id, at_ms, key, before, after, reason, outcome)
SELECT id, at_ms, key, before, after, reason, outcome FROM setting_history;
DROP TABLE setting_history;
ALTER TABLE setting_history_v2 RENAME TO setting_history;
CREATE TRIGGER setting_history_no_update BEFORE UPDATE ON setting_history
BEGIN SELECT RAISE(ABORT, 'setting_history is append-only'); END;
CREATE TRIGGER setting_history_no_delete BEFORE DELETE ON setting_history
BEGIN SELECT RAISE(ABORT, 'setting_history is append-only'); END;

-- Per-stream push position (stage 2-4 reads and advances it).
CREATE TABLE sync_cursor (
    name           TEXT PRIMARY KEY CHECK (length(name) > 0),
    last_local_id  INTEGER NOT NULL DEFAULT 0 CHECK (last_local_id >= 0),
    updated_at_ms  INTEGER NOT NULL
);

-- What left (or tried to leave) the device. No content: only kind, outcome, status, byte count.
-- `port` and `outcome` are supersets reserved for the later stages (drift-tested in Rust).
CREATE TABLE remote_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms       INTEGER NOT NULL,
    port        TEXT NOT NULL CHECK (port IN ('supabase', 'notify', 'llm', 'push', 'phone_bridge')),
    kind        TEXT NOT NULL CHECK (length(kind) > 0),
    outcome     TEXT NOT NULL CHECK (outcome IN ('ok', 'http_error', 'transport_error', 'host_denied', 'rate_limited')),
    http_status INTEGER,
    bytes       INTEGER NOT NULL DEFAULT 0 CHECK (bytes >= 0),
    detail_code TEXT
);
CREATE INDEX remote_events_at ON remote_events (at_ms);
CREATE TRIGGER remote_events_no_update BEFORE UPDATE ON remote_events
BEGIN SELECT RAISE(ABORT, 'remote_events is append-only'); END;
CREATE TRIGGER remote_events_no_delete BEFORE DELETE ON remote_events
BEGIN SELECT RAISE(ABORT, 'remote_events is append-only'); END;
