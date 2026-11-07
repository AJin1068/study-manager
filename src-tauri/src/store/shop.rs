//! `inventory_local`: the items the user owns. Purchases also write `point_ledger` (see `shop`).

use rusqlite::{Connection, params};

use crate::error::AppResult;

pub struct OwnedRow {
    pub item_id: String,
}

/// Adds an item unless it is owned. Returns whether a row was added.
pub fn insert_inventory(
    conn: &Connection,
    item_id: &str,
    source: &str,
    at_ms: i64,
    ledger_ref_key: Option<&str>,
) -> AppResult<bool> {
    let added = conn.execute(
        "INSERT INTO inventory_local (item_id, source, acquired_at_ms, ledger_ref_key) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (item_id) DO NOTHING",
        params![item_id, source, at_ms, ledger_ref_key],
    )?;
    Ok(added == 1)
}

pub fn list_owned(conn: &Connection) -> AppResult<Vec<OwnedRow>> {
    let mut stmt =
        conn.prepare("SELECT item_id FROM inventory_local ORDER BY acquired_at_ms, item_id")?;
    let rows = stmt.query_map([], |r| Ok(OwnedRow { item_id: r.get(0)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn is_owned(conn: &Connection, item_id: &str) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM inventory_local WHERE item_id = ?1",
        params![item_id],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

/// Ledger rows already named `buy:<item>:<n>`: the next `n` is this plus one.
pub fn purchase_count(conn: &Connection, item_id: &str) -> AppResult<i64> {
    let prefix = format!("buy:{item_id}:");
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM point_ledger WHERE substr(ref_key, 1, length(?1)) = ?1",
        params![prefix],
        |r| r.get(0),
    )?)
}
