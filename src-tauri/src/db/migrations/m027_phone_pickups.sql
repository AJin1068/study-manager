-- Stage 5-5 (slug: phone, owner S5.5). How often the phone's screen was switched on or unlocked
-- during a study segment: a count and the segment id, nothing else (no app names, no usage).
--
-- Append-only and idempotent per segment: the phone reports a cumulative count, and a row is added
-- only when the count is higher than every row the segment already has, so a repeated report adds
-- nothing and the segment's count is the largest one. `reported_at_ms` is when this PC first saw
-- that count. Display only: it never changes a focus score or a point (decision O-23).
CREATE TABLE phone_pickups (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    segment_id     INTEGER NOT NULL,
    count          INTEGER NOT NULL CHECK (count >= 0),
    reported_at_ms INTEGER NOT NULL,
    UNIQUE (segment_id, count)
);
CREATE INDEX phone_pickups_reported ON phone_pickups (reported_at_ms);
CREATE TRIGGER phone_pickups_no_update BEFORE UPDATE ON phone_pickups
BEGIN SELECT RAISE(ABORT, 'phone_pickups rows are append-only'); END;
CREATE TRIGGER phone_pickups_no_delete BEFORE DELETE ON phone_pickups
BEGIN SELECT RAISE(ABORT, 'phone_pickups rows are append-only'); END;
