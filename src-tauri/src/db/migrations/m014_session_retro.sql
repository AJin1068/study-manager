-- Stage 3-9 (slug: retro). One row per finished run the user wrote a retrospective for. The note is
-- local only: it is read back to build a merge proposal and never leaves the device except inside
-- that merge prompt (purpose wiki_merge, which needs llm.enabled). `backup_name` is the file name
-- (inside the subject's .versions folder) of the page text that existed before the merge, so one
-- click can restore it. `undone` joins the four states of the plan so an undo is visible.
CREATE TABLE session_retro (
    run_id        INTEGER PRIMARY KEY,
    retro_note    TEXT NOT NULL CHECK (length(retro_note) > 0 AND length(retro_note) <= 4000),
    status        TEXT NOT NULL CHECK (status IN ('pending', 'merged', 'skipped', 'failed', 'undone')),
    subject_id    INTEGER REFERENCES wiki_subjects(id) ON DELETE SET NULL,
    page_name     TEXT,
    page_id       INTEGER REFERENCES wiki_pages(id) ON DELETE SET NULL,
    backup_name   TEXT,
    attempts      INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_code     TEXT,
    created_at_ms INTEGER NOT NULL,
    merged_at_ms  INTEGER
);
