-- Stage 4 (slug: shared_wiki_local). What this PC remembers about wiki pages it shared with a room.
-- A page is private unless it has a row here: the row IS the explicit share mark (S4.7).
--
-- shared_page_marks    page_id -> room, when it was shared, the hash of the text last pushed (so an
--                      unchanged page is not pushed again), the id of the revision that push created
--                      (the next push builds on it, so a concurrent edit by someone else is stored as
--                      a branch), and allow_llm_merge: whether others may
--                      send this page's text to an LLM when merging a room page. Off by default.
-- shared_concept_seen  page names (concepts) of friends' shared pages already announced as "new concept", so the
--                      same notice is not shown twice.
CREATE TABLE shared_page_marks (
    page_id          INTEGER NOT NULL REFERENCES wiki_pages(id) ON DELETE CASCADE,
    room_id          TEXT NOT NULL CHECK (length(room_id) BETWEEN 8 AND 64),
    shared_at_ms     INTEGER NOT NULL,
    last_pushed_hash TEXT NOT NULL DEFAULT '' CHECK (length(last_pushed_hash) <= 64),
    last_revision_id TEXT NOT NULL DEFAULT '' CHECK (length(last_revision_id) <= 64),
    allow_llm_merge  INTEGER NOT NULL DEFAULT 0 CHECK (allow_llm_merge IN (0, 1)),
    PRIMARY KEY (page_id, room_id)
);

CREATE TABLE shared_concept_seen (
    room_id  TEXT NOT NULL CHECK (length(room_id) BETWEEN 8 AND 64),
    concept  TEXT NOT NULL CHECK (length(concept) BETWEEN 1 AND 80),
    seen_ms  INTEGER NOT NULL,
    PRIMARY KEY (room_id, concept)
);
