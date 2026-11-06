-- Stage 4-8 (slug: shop). Items the user owns. A purchase is a negative `point_ledger` row
-- (`ref_key = buy:<item>:<n>`, reason `shop_purchase`) written in the same transaction as the
-- inventory row; the balance stays the sum of the ledger. `source` is a superset reserved for the
-- gift story (drift-tested against shop::items::Source plus `gift`). A shop item must name the
-- ledger row that paid for it.
CREATE TABLE inventory_local (
    item_id        TEXT PRIMARY KEY CHECK (length(item_id) > 0),
    source         TEXT NOT NULL CHECK (source IN ('starter', 'shop', 'achievement', 'gift')),
    acquired_at_ms INTEGER NOT NULL,
    ledger_ref_key TEXT REFERENCES point_ledger (ref_key),
    CHECK (source <> 'shop' OR ledger_ref_key IS NOT NULL)
);
