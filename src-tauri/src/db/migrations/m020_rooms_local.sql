-- Stage 4 (slug: rooms_local). What the app remembers about study rooms on this PC.
-- The server is the truth for membership; these rows are a cache so the room list still shows
-- (marked stale) while offline, plus two local-only facts. No invite code, no other member's data
-- and nothing about distractions is stored here.
--
-- room_membership_cache  last known list from the server, replaced as a whole on each refresh.
-- room_prefs             the nickname this user uses in a room and when the "what the room can
--                        see" notice was acknowledged (the notice is shown before joining).
-- together_imports       which items of a room's shared timetable were already copied into the
--                        user's own plan, so a second tap cannot add the same plan twice.
CREATE TABLE room_membership_cache (
    room_id       TEXT PRIMARY KEY CHECK (length(room_id) BETWEEN 8 AND 64),
    room_name     TEXT NOT NULL CHECK (length(room_name) BETWEEN 1 AND 80),
    role          TEXT NOT NULL CHECK (role IN ('owner', 'member')),
    member_count  INTEGER NOT NULL CHECK (member_count >= 0),
    capacity      INTEGER NOT NULL CHECK (capacity >= 2),
    refreshed_at_ms INTEGER NOT NULL
);

CREATE TABLE room_prefs (
    room_id         TEXT PRIMARY KEY CHECK (length(room_id) BETWEEN 8 AND 64),
    nickname        TEXT NOT NULL CHECK (length(nickname) BETWEEN 1 AND 20),
    notice_ack_ms   INTEGER NOT NULL
);

CREATE TABLE together_imports (
    room_id    TEXT NOT NULL CHECK (length(room_id) BETWEEN 8 AND 64),
    item_key   TEXT NOT NULL CHECK (length(item_key) BETWEEN 1 AND 80),
    plan_id    INTEGER NOT NULL,
    at_ms      INTEGER NOT NULL,
    PRIMARY KEY (room_id, item_key)
);
