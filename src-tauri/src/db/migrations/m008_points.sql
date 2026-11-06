-- Point ledger (stage 1-5 story 3). Append-only: a correction is a new row, never an edit.
-- `ref_key` names the event a row pays for (`focus30:run:12:win:3`, `start:run:12`, `pass:4`,
-- `abort:run:12`), so recomputing or restarting can never pay the same event twice: the UNIQUE
-- index turns the second insert into a no-op. `reason_code` is a Rust constant (points::rewards),
-- deliberately without a CHECK so a new reward type needs no migration. The balance is the sum of
-- `delta`; the level reads the sum of the positive deltas (points earned), not the balance.
CREATE TABLE point_ledger (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms       INTEGER NOT NULL,
    delta       INTEGER NOT NULL CHECK (delta <> 0),
    reason_code TEXT NOT NULL CHECK (length(reason_code) > 0),
    ref_key     TEXT NOT NULL UNIQUE CHECK (length(ref_key) > 0)
);
CREATE INDEX point_ledger_at ON point_ledger (at_ms);
CREATE TRIGGER point_ledger_no_update BEFORE UPDATE ON point_ledger
BEGIN SELECT RAISE(ABORT, 'point_ledger rows are append-only'); END;
CREATE TRIGGER point_ledger_no_delete BEFORE DELETE ON point_ledger
BEGIN SELECT RAISE(ABORT, 'point_ledger rows are append-only'); END;
