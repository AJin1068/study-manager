-- Release procedures that cost the user something: emergency pass, skip today, early release.
-- One append-only row per granted release. The typed sentence is never stored: only its length
-- and how long typing took. `penalty_points` is the ledger-ready deduction the point system
-- (stage 1-5) reads; NULL for kinds that carry no penalty. `expires_at_ms` is set only for an
-- emergency pass (enforcement pauses until then). `date` is the local date of the grant.
CREATE TABLE study_passes (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms          INTEGER NOT NULL,
    date           TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('emergency_pass', 'skip_today', 'early_release')),
    run_id         INTEGER REFERENCES plan_runs(id),
    expires_at_ms  INTEGER,
    challenge_len  INTEGER NOT NULL CHECK (challenge_len > 0),
    typing_ms      INTEGER NOT NULL CHECK (typing_ms >= 0),
    penalty_points INTEGER CHECK (penalty_points IS NULL OR penalty_points >= 0),
    CHECK ((kind = 'emergency_pass') = (expires_at_ms IS NOT NULL))
);
CREATE INDEX study_passes_date_kind ON study_passes (date, kind);
CREATE INDEX study_passes_at ON study_passes (at_ms);
CREATE TRIGGER study_passes_no_update BEFORE UPDATE ON study_passes
BEGIN SELECT RAISE(ABORT, 'study_passes rows are append-only'); END;
CREATE TRIGGER study_passes_no_delete BEFORE DELETE ON study_passes
BEGIN SELECT RAISE(ABORT, 'study_passes rows are append-only'); END;
