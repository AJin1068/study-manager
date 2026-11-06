-- Stage 3-1 (slug: llm_usage). One append-only row per LLM call that reached an adapter (the
-- network or the mock): purpose, model, token counts, latency, result code. Never the prompt, the
-- answer, or any document text. `purpose` is a superset reserved for the later 3rd-stage stories
-- (drift-tested against llm::Purpose).
CREATE TABLE llm_usage (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms       INTEGER NOT NULL,
    purpose     TEXT NOT NULL CHECK (purpose IN (
        'tab_judge', 'window_classify', 'wiki_ingest', 'wiki_ask', 'wiki_merge', 'wiki_lint',
        'quiz_gen', 'message', 'plan_chat', 'plan_adjust', 'chat_name', 'insight', 'probe')),
    model       TEXT NOT NULL,
    mode        TEXT NOT NULL CHECK (mode IN ('mock', 'live')),
    in_tokens   INTEGER NOT NULL DEFAULT 0 CHECK (in_tokens >= 0),
    out_tokens  INTEGER NOT NULL DEFAULT 0 CHECK (out_tokens >= 0),
    latency_ms  INTEGER NOT NULL DEFAULT 0 CHECK (latency_ms >= 0),
    cached      INTEGER NOT NULL DEFAULT 0 CHECK (cached IN (0, 1)),
    result_code TEXT NOT NULL CHECK (length(result_code) > 0)
);
CREATE INDEX llm_usage_at ON llm_usage (at_ms);
CREATE TRIGGER llm_usage_no_update BEFORE UPDATE ON llm_usage
BEGIN SELECT RAISE(ABORT, 'llm_usage is append-only'); END;
CREATE TRIGGER llm_usage_no_delete BEFORE DELETE ON llm_usage
BEGIN SELECT RAISE(ABORT, 'llm_usage is append-only'); END;
