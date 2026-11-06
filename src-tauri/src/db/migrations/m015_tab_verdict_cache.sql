-- Stage 3-10 (slug: tab_verdict_cache). What the model said about a tab, kept so the same page is
-- not asked about twice. `key_hash` is a hash of the domain and the normalized tab label; the
-- label and the address themselves are never stored, only the domain the verdict applies to.
CREATE TABLE tab_verdict_cache (
    key_hash      TEXT PRIMARY KEY CHECK (length(key_hash) = 16),
    domain        TEXT NOT NULL CHECK (length(domain) > 0 AND length(domain) <= 253),
    verdict       TEXT NOT NULL CHECK (verdict IN ('study', 'distraction')),
    decided_at_ms INTEGER NOT NULL
);
CREATE INDEX tab_verdict_cache_decided ON tab_verdict_cache (decided_at_ms);
