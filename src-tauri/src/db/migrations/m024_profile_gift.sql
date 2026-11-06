-- Stage 4-9/4-10/4-11 (slugs: profile_local, gift). What this PC remembers about the user's
-- profile and gifts. Nothing here is a copy of anybody else's profile.
--
-- profile_local: one row. `display_name` is typed by the user for the profile (not the character
-- name and not an account name). `slug` is the server-issued profile link id. `web_base_url` is
-- where the static web bundle (web/) is hosted; empty until the user deploys it.
CREATE TABLE profile_local (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    display_name     TEXT NOT NULL DEFAULT '' CHECK (length(display_name) <= 20),
    slug             TEXT CHECK (slug IS NULL OR length(slug) = 16),
    web_base_url     TEXT CHECK (web_base_url IS NULL OR length(web_base_url) <= 200),
    updated_at_ms    INTEGER NOT NULL DEFAULT 0
);
INSERT INTO profile_local (id) VALUES (1);

-- Per-section scope. A missing row means private (the default for every section). The section and
-- scope lists are drift-tested against profile::model::{Section, Scope}.
CREATE TABLE profile_visibility (
    section       TEXT PRIMARY KEY CHECK (section IN ('room', 'grass', 'level', 'badges', 'guestbook', 'visitors', 'wiki', 'subject', 'gifts')),
    scope         TEXT NOT NULL CHECK (scope IN ('public', 'friends', 'private')),
    updated_at_ms INTEGER NOT NULL
);

-- Wiki page names the user chose to show on the profile (names only, never page text).
CREATE TABLE profile_wiki_public (
    page_name TEXT PRIMARY KEY CHECK (length(page_name) BETWEEN 1 AND 60)
);

-- A gift being sent. The points were spent when the row was written (ledger `gift:<gift_id>`);
-- `state` moves pending -> sent (the server has it) or pending -> refunded (the server refused or
-- certainly never got it; ledger `gift_refund:<gift_id>`). An unclear failure stays pending.
CREATE TABLE gift_outbox (
    gift_id          TEXT PRIMARY KEY CHECK (length(gift_id) = 36),
    recipient_user   TEXT NOT NULL CHECK (length(recipient_user) > 0),
    recipient_name   TEXT NOT NULL CHECK (length(recipient_name) BETWEEN 1 AND 20),
    item_id          TEXT NOT NULL CHECK (length(item_id) > 0),
    message          TEXT NOT NULL DEFAULT '' CHECK (length(message) <= 60),
    price            INTEGER NOT NULL CHECK (price > 0),
    state            TEXT NOT NULL CHECK (state IN ('pending', 'sent', 'refunded')),
    last_code        TEXT,
    attempts         INTEGER NOT NULL DEFAULT 0,
    created_at_ms    INTEGER NOT NULL,
    last_attempt_ms  INTEGER,
    resolved_at_ms   INTEGER
);
CREATE INDEX gift_outbox_state ON gift_outbox (state);

-- Gifts this user accepted or declined (so a re-listed accepted gift is applied once).
CREATE TABLE gift_received (
    gift_id        TEXT PRIMARY KEY,
    item_id        TEXT NOT NULL,
    sender_name    TEXT NOT NULL,
    message        TEXT NOT NULL DEFAULT '',
    outcome        TEXT NOT NULL CHECK (outcome IN ('accepted', 'declined')),
    resolved_at_ms INTEGER NOT NULL
);
