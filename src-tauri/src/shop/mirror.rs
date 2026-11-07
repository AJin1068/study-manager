//! What the server mirror (`inventory_public`) gets of the inventory: the species and the item id
//! worn in each slot. No name, no setup answers, no owned-but-unworn list, no prices.

use rusqlite::Connection;

use crate::character::Equipped;
use crate::error::AppResult;
use crate::remote::payload::InventoryPublicArgs;
use crate::store::character as char_repo;

/// `None` before a character exists. The rooms service pushes the result (RPC
/// `set_inventory_public`) after a pick or an equip change.
pub fn inventory_public(conn: &Connection) -> AppResult<Option<InventoryPublicArgs>> {
    let Some(profile) = char_repo::get_profile(conn)? else {
        return Ok(None);
    };
    let worn = Equipped::from_rows(&char_repo::wardrobe(conn)?);
    Ok(Some(InventoryPublicArgs {
        species: profile.species,
        hat: worn.hat,
        top: worn.top,
        accessory: worn.accessory,
        hand: worn.hand,
    }))
}
