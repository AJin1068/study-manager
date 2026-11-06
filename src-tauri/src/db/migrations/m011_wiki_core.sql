-- Stage 3-2 (slug: wiki_core). The markdown files under the wiki folder are the truth; these
-- tables are a rebuildable index (wiki::index::reindex) plus the subject list and source log.
-- `source_ref` is a label the user typed (a file name or a note about a link). It is never
-- fetched and never sent anywhere.
CREATE TABLE wiki_subjects (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    slug           TEXT NOT NULL UNIQUE CHECK (length(slug) > 0),
    name           TEXT NOT NULL CHECK (length(name) > 0),
    archived_at_ms INTEGER,
    created_at_ms  INTEGER NOT NULL
);

CREATE TABLE wiki_pages (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_id    INTEGER NOT NULL REFERENCES wiki_subjects(id) ON DELETE CASCADE,
    page_name     TEXT NOT NULL CHECK (length(page_name) > 0),
    rel_path      TEXT NOT NULL CHECK (length(rel_path) > 0),
    content_hash  TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    quiz_total    INTEGER NOT NULL DEFAULT 0 CHECK (quiz_total >= 0),
    quiz_correct  INTEGER NOT NULL DEFAULT 0 CHECK (quiz_correct >= 0 AND quiz_correct <= quiz_total),
    UNIQUE (subject_id, page_name)
);

CREATE TABLE wiki_sources (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_id     INTEGER NOT NULL REFERENCES wiki_subjects(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL CHECK (kind IN ('pdf', 'note', 'link', 'paste')),
    source_ref     TEXT NOT NULL,
    imported_at_ms INTEGER NOT NULL,
    status         TEXT NOT NULL CHECK (length(status) > 0)
);
CREATE INDEX wiki_sources_subject ON wiki_sources (subject_id, imported_at_ms);
