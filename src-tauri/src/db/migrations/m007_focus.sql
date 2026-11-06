-- Focus measurement during locked study blocks. No window titles, URLs, or typed text: only app
-- ids, event kinds, and millisecond counts.
--
-- `focus_events` is append-only. `kind` lists its final value set now: 'self_report' is written
-- by the "just got distracted" signal (stage 1-5 story 4) without another migration; the Rust
-- `FocusEventKind` matches this list (drift test in `focus::tests`). `app_id` is the app switched
-- to (away) and NULL for the other kinds.
CREATE TABLE focus_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id     INTEGER NOT NULL REFERENCES plan_runs(id),
    segment_id INTEGER NOT NULL REFERENCES timer_segments(id),
    at_ms      INTEGER NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('away', 'return', 'idle_start', 'idle_end', 'self_report')),
    app_id     TEXT
);
CREATE INDEX focus_events_segment ON focus_events (segment_id, at_ms);
CREATE INDEX focus_events_at ON focus_events (at_ms);
CREATE TRIGGER focus_events_no_update BEFORE UPDATE ON focus_events
BEGIN SELECT RAISE(ABORT, 'focus_events rows are append-only'); END;
CREATE TRIGGER focus_events_no_delete BEFORE DELETE ON focus_events
BEGIN SELECT RAISE(ABORT, 'focus_events rows are append-only'); END;

-- One row per segment and wall-clock minute (never per sample): 8 hours is at most 480 rows.
-- The recorder only adds to the counters; the three together cannot exceed the minute.
CREATE TABLE focus_minutes (
    segment_id      INTEGER NOT NULL REFERENCES timer_segments(id),
    minute_start_ms INTEGER NOT NULL,
    credited_ms     INTEGER NOT NULL DEFAULT 0 CHECK (credited_ms >= 0),
    away_ms         INTEGER NOT NULL DEFAULT 0 CHECK (away_ms >= 0),
    idle_ms         INTEGER NOT NULL DEFAULT 0 CHECK (idle_ms >= 0),
    PRIMARY KEY (segment_id, minute_start_ms),
    CHECK (credited_ms + away_ms + idle_ms <= 60000)
) WITHOUT ROWID;
CREATE TRIGGER focus_minutes_no_delete BEFORE DELETE ON focus_minutes
BEGIN SELECT RAISE(ABORT, 'focus_minutes rows are never deleted'); END;
