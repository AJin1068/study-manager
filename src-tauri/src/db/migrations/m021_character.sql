-- Stage 4-2 (slug: character). The study character, local only. `mbti`, `birth_year`, and
-- `gender` are the user's setup answers (all optional): they stay in this table and are never
-- part of any server payload, LLM prompt, or UI view. Part ids are validated against the catalog
-- (src-tauri/assets/character/parts.json) in Rust, so a new part needs no migration.
CREATE TABLE character_profile (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    species      TEXT NOT NULL CHECK (length(species) > 0),
    palette      TEXT NOT NULL CHECK (length(palette) > 0),
    expression   TEXT NOT NULL CHECK (length(expression) > 0),
    prop         TEXT NOT NULL CHECK (length(prop) > 0),
    name         TEXT NOT NULL CHECK (length(name) > 0),
    blurb        TEXT NOT NULL,
    mbti         TEXT CHECK (mbti IS NULL OR (length(mbti) = 4 AND mbti = upper(mbti))),
    birth_year   INTEGER CHECK (birth_year IS NULL OR birth_year BETWEEN 1900 AND 2200),
    gender       TEXT CHECK (gender IS NULL OR gender IN ('male', 'female', 'other')),
    chosen_at_ms INTEGER NOT NULL
);

-- Worn item per slot (the slot set matches shop::items::Slot, drift-tested). Items are shared by
-- all species, so a new character keeps the wardrobe.
CREATE TABLE wardrobe (
    slot    TEXT PRIMARY KEY CHECK (slot IN ('hat', 'top', 'accessory', 'hand')),
    item_id TEXT NOT NULL CHECK (length(item_id) > 0)
);
