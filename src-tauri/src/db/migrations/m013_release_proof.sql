-- Stage 3-6 (slug: release_proof). How an early release was proven: by typing the long sentence
-- (every row before this migration, emergency passes, skip-today) or by answering wiki questions.
-- SQLite cannot change the existing NOT NULL / CHECK columns and rebuilding the table would touch
-- the append-only triggers and shipped rows, so this adds one column with a non-NULL default
-- (existing rows become 'typing'; the UPDATE trigger does not fire for ADD COLUMN).
-- For a 'quiz' row the older columns are reused: `challenge_len` is the sum of the character
-- counts of the asked prompts (> 0) and `typing_ms` is the total time spent answering (>= 0).
ALTER TABLE study_passes ADD COLUMN proof_kind TEXT NOT NULL DEFAULT 'typing' CHECK (proof_kind IN ('typing', 'quiz'));
-- A quiz proof exists only for early release; the other kinds always keep the typing path.
CREATE TRIGGER study_passes_proof_guard BEFORE INSERT ON study_passes
WHEN NEW.proof_kind = 'quiz' AND NEW.kind <> 'early_release'
BEGIN SELECT RAISE(ABORT, 'quiz proof is early_release only'); END;

-- Early-release quiz attempts of one local day. Only early-release attempts are counted: skip
-- today and emergency pass never read or change it. `last_attempt_ms` starts the cooldown.
CREATE TABLE release_quiz_state (
    date            TEXT PRIMARY KEY,
    attempts        INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_attempt_ms INTEGER NOT NULL
);
