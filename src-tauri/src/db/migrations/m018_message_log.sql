-- Stage 3-12 (slug: messages). A record that a cheer/scold message was sent, for the hourly cap
-- and the random interval. The text is never stored (it can name a wiki page). Append-only.
CREATE TABLE message_log (
    id     INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms  INTEGER NOT NULL,
    kind   TEXT NOT NULL CHECK (kind IN ('distraction', 'streak', 'set_end', 'random')),
    tone   TEXT NOT NULL CHECK (tone IN ('gentle', 'warm', 'normal', 'firm', 'harsh')),
    source TEXT NOT NULL CHECK (source IN ('template', 'wiki', 'llm')),
    run_id INTEGER REFERENCES plan_runs(id)
);
CREATE INDEX message_log_at ON message_log (at_ms);
CREATE TRIGGER message_log_no_update BEFORE UPDATE ON message_log
BEGIN SELECT RAISE(ABORT, 'message_log rows are append-only'); END;
CREATE TRIGGER message_log_no_delete BEFORE DELETE ON message_log
BEGIN SELECT RAISE(ABORT, 'message_log rows are append-only'); END;
