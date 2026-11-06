-- Stage 5-2 (slug: phone, owner S5.2). What this PC remembers about its paired phone: one row.
-- The server is the authority (supabase phone_pairs); this is the last answer, so the settings
-- screen still says "paired" while the server is unreachable. Later phone stories add their own
-- files (phone_gap, phone_pickups); nothing here is a copy of the phone's data.
CREATE TABLE phone_pair (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    state         TEXT NOT NULL DEFAULT 'unpaired' CHECK (state IN ('unpaired', 'paired')),
    paired_at_ms  INTEGER,
    updated_at_ms INTEGER NOT NULL DEFAULT 0
);
INSERT INTO phone_pair (id) VALUES (1);
