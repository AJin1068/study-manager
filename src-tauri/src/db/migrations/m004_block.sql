-- Block list and block-attempt log. Neither table has a column for window titles or URLs: apps are
-- identified by bundle id / normalized executable path, sites (stage 1-4) by domain only.
--
-- `kind`, `source`, `target_kind`, and `action` list their final value sets now, so stage 1-4 can
-- record site and extension events without a migration. The Rust enums in `enforce` match these
-- lists (drift test); values not yet used by code are marked there.
CREATE TABLE block_entries (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    kind          TEXT NOT NULL CHECK (kind IN ('app', 'site')),
    pattern       TEXT NOT NULL CHECK (length(pattern) > 0),
    source        TEXT NOT NULL CHECK (source IN ('user', 'preset')),
    category      TEXT,
    enabled       INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    UNIQUE (kind, pattern)
);

-- Append-only. `mode` is the enforcement mode in force ('dryRun' or 'enforce'); `detail_code` is a
-- short machine code (never free text).
CREATE TABLE block_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms       INTEGER NOT NULL,
    run_id      INTEGER,
    segment_id  INTEGER,
    target_kind TEXT NOT NULL CHECK (target_kind IN ('app', 'site', 'extension')),
    target_id   TEXT NOT NULL,
    action      TEXT NOT NULL CHECK (action IN (
        'would_terminate', 'terminate_requested', 'terminated', 'force_killed', 'failed',
        'skipped_protected', 'skipped_pass', 'site_blocked', 'site_allowed_pass',
        'extension_disconnected', 'extension_connected')),
    mode        TEXT NOT NULL CHECK (mode IN ('dryRun', 'enforce')),
    detail_code TEXT
);
CREATE INDEX block_events_at ON block_events (at_ms);
CREATE TRIGGER block_events_no_update BEFORE UPDATE ON block_events
BEGIN SELECT RAISE(ABORT, 'block_events rows are append-only'); END;
CREATE TRIGGER block_events_no_delete BEFORE DELETE ON block_events
BEGIN SELECT RAISE(ABORT, 'block_events rows are append-only'); END;
