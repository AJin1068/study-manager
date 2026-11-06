-- Calendar plans. A plan is a start time plus a set count; the end is derived from the mode
-- length. `plan_name` (not `title`) keeps the privacy schema guard meaningful.
-- Dates are local 'YYYY-MM-DD', times local 'HH:MM'; instants elsewhere are UTC epoch ms.
-- repeat_weekdays is a bitmask, bit 0 = Monday .. bit 6 = Sunday; NULL = single occurrence.
CREATE TABLE plans (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id           INTEGER NOT NULL,
    plan_name           TEXT NOT NULL,
    subject             TEXT NOT NULL,
    start_date          TEXT NOT NULL CHECK (start_date GLOB '[0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]'),
    start_time          TEXT NOT NULL CHECK (start_time GLOB '[0-2][0-9]:[0-5][0-9]'),
    sets                INTEGER NOT NULL CHECK (sets BETWEEN 1 AND 24),
    repeat_weekdays     INTEGER CHECK (repeat_weekdays IS NULL OR repeat_weekdays BETWEEN 1 AND 127),
    repeat_until        TEXT CHECK (repeat_until IS NULL OR repeat_until GLOB '[0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]'),
    effective_from_date TEXT NOT NULL CHECK (effective_from_date GLOB '[0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]'),
    created_at_ms       INTEGER NOT NULL,
    deleted_at_ms       INTEGER
);
CREATE INDEX plans_series ON plans (series_id);

-- One row per edited occurrence, keyed by the occurrence's original date.
CREATE TABLE plan_exceptions (
    plan_id       INTEGER NOT NULL REFERENCES plans(id),
    date          TEXT NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('moved', 'cancelled')),
    new_date      TEXT,
    new_time      TEXT,
    sets_override INTEGER CHECK (sets_override IS NULL OR sets_override BETWEEN 1 AND 24),
    PRIMARY KEY (plan_id, date),
    CHECK (
        (kind = 'moved' AND new_date IS NOT NULL AND new_time IS NOT NULL)
        OR (kind = 'cancelled' AND new_date IS NULL AND new_time IS NULL AND sets_override IS NULL)
    )
);

-- action and reason_code reserve the 3rd-stage chatbot / LLM values now so a later release
-- never has to rewrite this table; the Rust enums match this list (drift test in plan.rs).
-- plan_id is NULL only for a rejected create, where no plan row exists.
CREATE TABLE plan_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id     INTEGER REFERENCES plans(id),
    at_ms       INTEGER NOT NULL,
    action      TEXT NOT NULL CHECK (action IN (
        'create', 'move', 'retime', 'resize_sets', 'delete', 'repeat_change',
        'mode_shift', 'undo', 'chatbot_create', 'llm_adjust')),
    reason_code TEXT NOT NULL CHECK (reason_code IN (
        'created', 'postponed', 'sick', 'custom', 'mode_shift', 'undo', 'chatbot', 'llm_adjust')),
    reason_text TEXT,
    before_json TEXT,
    after_json  TEXT,
    outcome     TEXT NOT NULL CHECK (outcome IN ('applied', 'rejected_locked')),
    batch_id    TEXT
);
CREATE INDEX plan_history_plan_at ON plan_history (plan_id, at_ms);
CREATE TRIGGER plan_history_no_update BEFORE UPDATE ON plan_history
BEGIN SELECT RAISE(ABORT, 'plan_history is append-only'); END;
CREATE TRIGGER plan_history_no_delete BEFORE DELETE ON plan_history
BEGIN SELECT RAISE(ABORT, 'plan_history is append-only'); END;
