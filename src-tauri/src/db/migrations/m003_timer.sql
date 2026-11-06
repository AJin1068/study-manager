-- Timer runs and their segments. A run is one occurrence of a plan that the timer engine saw
-- (or, for `missed`, never saw). Bounds are UTC epoch ms frozen at creation, so a later time
-- zone, DST, or mode-length change cannot move a run that already exists.
--
-- `status` and `end_reason` list their final value sets now (`skipped`, `early_release` are
-- written by stage 1-3); the Rust enums in `timer` match these lists (drift test).
-- The mode columns snapshot the lengths in force at creation. `actual_start_ms` is NULL for a
-- run that never started. `date` is the local date the occurrence happens on.
CREATE TABLE plan_runs (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id                INTEGER NOT NULL REFERENCES plans(id),
    date                   TEXT NOT NULL,
    planned_start_ms       INTEGER NOT NULL,
    planned_end_ms         INTEGER NOT NULL CHECK (planned_end_ms >= planned_start_ms),
    actual_start_ms        INTEGER,
    mode_level             TEXT NOT NULL CHECK (mode_level IN ('easy', 'medium', 'hard')),
    study_min              INTEGER NOT NULL,
    break_min              INTEGER NOT NULL,
    long_break_min         INTEGER,
    sets_before_long_break INTEGER,
    sets_total             INTEGER NOT NULL CHECK (sets_total BETWEEN 1 AND 24),
    status                 TEXT NOT NULL CHECK (status IN ('active', 'completed', 'missed', 'skipped', 'aborted')),
    ended_at_ms            INTEGER,
    detail_code            TEXT,
    UNIQUE (plan_id, date)
);
CREATE INDEX plan_runs_date ON plan_runs (date);

-- One row per study/break block the engine opened. Closing a segment (setting ended_at_ms,
-- end_reason) or marking its notice is the only allowed change, and only while it is open.
CREATE TABLE timer_segments (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id           INTEGER NOT NULL REFERENCES plan_runs(id),
    seq              INTEGER NOT NULL,
    kind             TEXT NOT NULL CHECK (kind IN ('study', 'break', 'long_break')),
    planned_start_ms INTEGER NOT NULL,
    planned_end_ms   INTEGER NOT NULL CHECK (planned_end_ms > planned_start_ms),
    started_at_ms    INTEGER NOT NULL,
    ended_at_ms      INTEGER,
    end_reason       TEXT CHECK (end_reason IN ('elapsed', 'elapsed_while_down', 'skipped', 'aborted', 'early_release')),
    late_join        INTEGER NOT NULL DEFAULT 0 CHECK (late_join IN (0, 1)),
    notice_sent_at_ms INTEGER,
    UNIQUE (run_id, seq),
    CHECK ((ended_at_ms IS NULL) = (end_reason IS NULL))
);
CREATE INDEX timer_segments_run_seq ON timer_segments (run_id, seq);
CREATE TRIGGER timer_segments_no_delete BEFORE DELETE ON timer_segments
BEGIN SELECT RAISE(ABORT, 'timer_segments rows are never deleted'); END;
CREATE TRIGGER timer_segments_closed_immutable BEFORE UPDATE ON timer_segments
WHEN OLD.ended_at_ms IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'a closed timer segment is immutable'); END;
