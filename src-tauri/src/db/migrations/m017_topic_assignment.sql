-- Stage 3-8 (slug: topic). Which wiki page a plan occurrence is meant to cover. The plan itself
-- (name, subject) never changes, so plan_history is untouched; this table is its own history:
-- rows are insert-only and the newest row of a (plan_id, date) is the current assignment.
-- `source` is 'auto' (recommendation) or 'manual' (the user chose it; a later auto run keeps it).
CREATE TABLE topic_assignment (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id        INTEGER NOT NULL REFERENCES plans(id),
    date           TEXT NOT NULL,
    page_id        INTEGER NOT NULL REFERENCES wiki_pages(id) ON DELETE CASCADE,
    source         TEXT NOT NULL CHECK (source IN ('auto', 'manual')),
    reason         TEXT NOT NULL CHECK (reason IN ('empty', 'low_accuracy', 'stale', 'hard', 'review', 'chosen')),
    assigned_at_ms INTEGER NOT NULL
);
CREATE INDEX topic_assignment_slot ON topic_assignment (plan_id, date, id);
CREATE TRIGGER topic_assignment_no_update BEFORE UPDATE ON topic_assignment
BEGIN SELECT RAISE(ABORT, 'topic_assignment rows are insert-only'); END;
