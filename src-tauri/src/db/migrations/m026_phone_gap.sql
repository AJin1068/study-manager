-- Stage 5-3 (slug: phone, owner S5.3). Stretches of a locked study segment in which the phone was
-- not under the lock, and the publisher's bookkeeping.
--
-- phone_gap is append-only: a row is written once, when the stretch is over. `kind` says why:
--   unreachable      the phone sent no heartbeat for 90 s or more
--   lease_expired    the desktop app had a lock to hold but no published lease covered the phone
--   service_disabled the phone reported its blocking service switched off
-- Times are on this PC's clock. `run_id` is the plan_runs id (no foreign key: the row is a record
-- of what happened and must survive whatever happens to the run's rows). It holds counts of time
-- only: no app names, no usage.
CREATE TABLE phone_gap (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id   INTEGER NOT NULL,
    start_ms INTEGER NOT NULL,
    end_ms   INTEGER NOT NULL,
    kind     TEXT NOT NULL CHECK (kind IN ('unreachable', 'lease_expired', 'service_disabled')),
    CHECK (end_ms >= start_ms),
    UNIQUE (run_id, kind, start_ms)
);
CREATE INDEX phone_gap_start ON phone_gap (start_ms);
CREATE TRIGGER phone_gap_no_update BEFORE UPDATE ON phone_gap
BEGIN SELECT RAISE(ABORT, 'phone_gap rows are append-only'); END;
CREATE TRIGGER phone_gap_no_delete BEFORE DELETE ON phone_gap
BEGIN SELECT RAISE(ABORT, 'phone_gap rows are append-only'); END;

-- One row: the highest sequence number this PC ever published and the last state it put on the
-- server, so a restart neither reuses a number nor publishes the same state twice. Mutable by
-- design (it is a cursor, not a record).
CREATE TABLE phone_publish (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    seq         INTEGER NOT NULL DEFAULT 0 CHECK (seq >= 0),
    last_key    TEXT,
    last_state  TEXT CHECK (last_state IN ('locked', 'free')),
    last_cause  TEXT,
    last_valid_until_ms INTEGER,
    updated_at_ms INTEGER NOT NULL DEFAULT 0
);
INSERT INTO phone_publish (id) VALUES (1);
