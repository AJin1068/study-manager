//! Shop commands. Opening the shop first grants the achievement items the records show.

use tauri::{AppHandle, State};

use super::{lock_db, AppState};
use crate::character::{self, CharacterView};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::shop::items::Slot;
use crate::shop::{self, PurchaseOutcome, ShopView};

#[tauri::command]
pub fn get_shop(state: State<'_, AppState>) -> AppResult<ShopView> {
    let mut conn = lock_db(&state.db)?;
    shop::grant_from_records(&mut conn, now_ms()?)?;
    shop::shop_view(&conn)
}

#[tauri::command]
pub fn purchase_item(state: State<'_, AppState>, item_id: String) -> AppResult<PurchaseOutcome> {
    shop::purchase(&mut *lock_db(&state.db)?, &item_id, now_ms()?)
}

#[tauri::command]
pub fn equip_item(app: AppHandle, state: State<'_, AppState>, item_id: String) -> AppResult<CharacterView> {
    let view = shop::equip(&mut *lock_db(&state.db)?, &item_id)?;
    character::refresh_tray(&app, &state.db);
    Ok(view)
}

#[tauri::command]
pub fn unequip_slot(state: State<'_, AppState>, slot: Slot) -> AppResult<CharacterView> {
    shop::unequip(&mut *lock_db(&state.db)?, slot)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::shop::items::{ItemKind, Slot, Source};
    use crate::shop::{PurchaseOutcome, ShopItemView, ShopView};

    #[test]
    fn shop_wire_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/shop-contract.json");
        let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let shop = ShopView {
            balance: 60,
            items: vec![
                ShopItemView {
                    id: "hat_beanie".into(),
                    kind: ItemKind::Clothing,
                    slot: Some(Slot::Hat),
                    label: "비니".into(),
                    price: 40,
                    source: Source::Shop,
                    giftable: true,
                    achievement: None,
                    owned: true,
                    equipped: true,
                    affordable: true,
                },
                ShopItemView {
                    id: "acc_knit_scarf".into(),
                    kind: ItemKind::Clothing,
                    slot: Some(Slot::Accessory),
                    label: "뜨개 목도리".into(),
                    price: 0,
                    source: Source::Achievement,
                    giftable: false,
                    achievement: Some("7일 연속 달성".into()),
                    owned: false,
                    equipped: false,
                    affordable: false,
                },
            ],
        };
        assert_eq!(serde_json::to_value(shop).expect("shop"), fixture["shop"]);
        let outcomes = serde_json::json!([
            PurchaseOutcome::Purchased { balance: 60 },
            PurchaseOutcome::AlreadyOwned,
            PurchaseOutcome::InsufficientPoints { balance: 3, price: 40 },
            PurchaseOutcome::NotForSale,
        ]);
        assert_eq!(outcomes, fixture["outcomes"]);
    }
}
