-- Apps that count as studying (focus measurement, stage 1-5). Apps are identified by bundle id /
-- normalized executable path only; the foreground window title is never stored. `kind` lists its
-- final value set now ('site' is reserved for the browser-extension judgement); the Rust
-- `EntryKind` matches it (drift test in `focus::tests`). The list starts empty on purpose: the
-- user picks study apps on first run, and an empty list is surfaced as a warning.
CREATE TABLE study_apps (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    app_id        TEXT NOT NULL CHECK (length(app_id) > 0),
    kind          TEXT NOT NULL CHECK (kind IN ('app', 'site')),
    enabled       INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    UNIQUE (kind, app_id)
);
