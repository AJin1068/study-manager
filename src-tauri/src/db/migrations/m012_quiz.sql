-- Stage 3-5 (slug: quiz). The question bank is generated ahead of time from wiki pages, so no LLM
-- call is needed when a question is shown. `answer_key` and `choices_json` never leave Rust: the
-- UI gets the prompt and the choices only. `evidence_quote` is a substring of the page text it
-- came from (checked at generation). What the user answered is never stored: only whether it was
-- right (`correct`), whether the question was skipped, and how long it took.
CREATE TABLE quiz_items (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    page_id        INTEGER NOT NULL REFERENCES wiki_pages(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL CHECK (kind IN ('mcq', 'cloze')),
    prompt         TEXT NOT NULL CHECK (length(prompt) > 0),
    choices_json   TEXT NOT NULL,
    answer_key     TEXT NOT NULL CHECK (length(answer_key) > 0),
    evidence_quote TEXT NOT NULL CHECK (length(evidence_quote) > 0),
    created_at_ms  INTEGER NOT NULL,
    disabled_at_ms INTEGER,
    UNIQUE (page_id, kind, prompt)
);
CREATE INDEX quiz_items_page ON quiz_items (page_id);

-- Append-only. `release_kind` is set only for the early-release proof (O-8); `skipped` rows
-- (a session-end quiz the user passed on) are recorded and excluded from accuracy.
CREATE TABLE quiz_attempts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms        INTEGER NOT NULL,
    item_id      INTEGER NOT NULL REFERENCES quiz_items(id),
    context      TEXT NOT NULL CHECK (context IN ('distraction', 'release', 'session_end', 'checkin', 'review')),
    release_kind TEXT CHECK (release_kind IS NULL OR release_kind = 'early_release'),
    run_id       INTEGER REFERENCES plan_runs(id),
    segment_id   INTEGER,
    correct      INTEGER NOT NULL CHECK (correct IN (0, 1)),
    skipped      INTEGER NOT NULL DEFAULT 0 CHECK (skipped IN (0, 1)),
    response_ms  INTEGER NOT NULL DEFAULT 0 CHECK (response_ms >= 0),
    CHECK (skipped = 0 OR correct = 0),
    CHECK (release_kind IS NULL OR context = 'release')
);
CREATE INDEX quiz_attempts_item ON quiz_attempts (item_id, at_ms);
CREATE INDEX quiz_attempts_context ON quiz_attempts (context, at_ms);
CREATE TRIGGER quiz_attempts_no_update BEFORE UPDATE ON quiz_attempts
BEGIN SELECT RAISE(ABORT, 'quiz_attempts rows are append-only'); END;
CREATE TRIGGER quiz_attempts_no_delete BEFORE DELETE ON quiz_attempts
BEGIN SELECT RAISE(ABORT, 'quiz_attempts rows are append-only'); END;

-- A reported question is excluded from every later draw (quiz_items.disabled_at_ms is set too).
CREATE TABLE quiz_flags (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id     INTEGER NOT NULL REFERENCES quiz_items(id),
    at_ms       INTEGER NOT NULL,
    reason_code TEXT NOT NULL CHECK (reason_code IN ('wrong_answer', 'unclear', 'off_topic', 'other'))
);
CREATE INDEX quiz_flags_item ON quiz_flags (item_id);
