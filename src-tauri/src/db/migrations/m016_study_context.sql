-- Stage 3-11 (slug: study_context). "What was I studying": the study app and a short label (a file
-- name or a summary of the window caption), kept 30 days and only while the user has switched the
-- feature on. The raw caption is never stored; `context_label` is the sanitized, shortened form.
CREATE TABLE study_context (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    segment_id    INTEGER NOT NULL,
    at_ms         INTEGER NOT NULL,
    app_id        TEXT NOT NULL CHECK (length(app_id) > 0),
    context_label TEXT NOT NULL CHECK (length(context_label) > 0 AND length(context_label) <= 80)
);
CREATE INDEX study_context_at ON study_context (at_ms);
CREATE INDEX study_context_segment ON study_context (segment_id, id);
