-- Stage 3-14 (slug: chat). A plan draft the chatbot made, kept until the user applies or discards it.
-- Nothing in the plan tables changes before `applied_at_ms` is set; applying runs the stored
-- operations through the normal plan functions, so `draft_json` is the only thing the UI can name
-- (by id) and cannot be forged. The conversation itself is never stored.
CREATE TABLE chat_drafts (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at_ms INTEGER NOT NULL,
    draft_json    TEXT NOT NULL CHECK (length(draft_json) > 0),
    applied_at_ms INTEGER,
    batch_id      TEXT,
    CHECK ((applied_at_ms IS NULL) = (batch_id IS NULL))
);
