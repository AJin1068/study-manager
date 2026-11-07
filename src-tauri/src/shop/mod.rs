//! The character shop (PLAN 12): clothes in four slots, hut furniture, and extra characters,
//! bought with points. A purchase is a negative `point_ledger` row keyed `buy:<item>:<n>` written
//! in the same Immediate transaction as the inventory row, so the balance stays the sum of the
//! ledger and a retry cannot charge twice (an owned item is never charged again). Achievement
//! items are never sold: they are granted once the study records show the achievement.
//!
//! What a friend may see of the inventory is `mirror::inventory_public`: item ids of what is
//! worn, nothing else.

pub mod items;
pub mod mirror;
#[cfg(test)]
mod tests;

use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;

use crate::character::{self, CharacterView, Equipped};
use crate::error::{AppError, AppResult};
use crate::points::rewards::REASON_SHOP_PURCHASE;
use crate::store;
use crate::store::character as char_repo;
use crate::store::shop as repo;
use items::{Achievement, Item, ItemKind, Slot, Source};

/// Days in a row at the streak threshold for the scarf, and right answers for the mortarboard.
pub const STREAK_ACHIEVEMENT_DAYS: i64 = 7;
pub const QUIZ_ACHIEVEMENT_CORRECT: i64 = 100;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PurchaseOutcome {
    Purchased {
        balance: i64,
    },
    AlreadyOwned,
    InsufficientPoints {
        balance: i64,
        price: i64,
    },
    /// Starter and achievement items are not for sale.
    NotForSale,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShopItemView {
    pub id: String,
    pub kind: ItemKind,
    pub slot: Option<Slot>,
    pub label: String,
    pub price: i64,
    pub source: Source,
    pub giftable: bool,
    /// What earns an achievement item.
    pub achievement: Option<String>,
    pub owned: bool,
    pub equipped: bool,
    pub affordable: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShopView {
    pub balance: i64,
    pub items: Vec<ShopItemView>,
}

/// Which achievements the records show. Pure.
pub fn earned_achievements(streak_days: i64, quiz_correct: i64) -> Vec<Achievement> {
    let mut out = Vec::new();
    if streak_days >= STREAK_ACHIEVEMENT_DAYS {
        out.push(Achievement::Streak7);
    }
    if quiz_correct >= QUIZ_ACHIEVEMENT_CORRECT {
        out.push(Achievement::Quiz100);
    }
    out
}

/// Grants the items of every earned achievement not owned yet; returns the new item ids. Once
/// granted an item stays, even if the streak later breaks.
pub fn grant_achievements(
    conn: &Connection,
    streak_days: i64,
    quiz_correct: i64,
    now_ms: i64,
) -> AppResult<Vec<String>> {
    let earned = earned_achievements(streak_days, quiz_correct);
    let mut granted = Vec::new();
    for item in &items::catalog()?.items {
        let Some(achievement) = item.achievement else {
            continue;
        };
        if earned.contains(&achievement)
            && repo::insert_inventory(conn, &item.id, Source::Achievement.as_str(), now_ms, None)?
        {
            granted.push(item.id.clone());
        }
    }
    Ok(granted)
}

/// Reads the streak and the right-answer count from the records and grants what they earn.
pub fn grant_from_records(conn: &mut Connection, now_ms: i64) -> AppResult<Vec<String>> {
    let streak = crate::dashboard::today(conn, &chrono::Local, now_ms)?.streak_days;
    let (_, correct) = store::growth::quiz_counts(conn)?;
    grant_achievements(conn, streak, correct, now_ms)
}

/// Whether an owned character item unlocks `species`.
pub fn owns_species(conn: &Connection, species: &str) -> AppResult<bool> {
    for row in repo::list_owned(conn)? {
        if items::item(&row.item_id)?
            .is_some_and(|i| i.kind == ItemKind::Character && i.species.as_deref() == Some(species))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn purchase(conn: &mut Connection, item_id: &str, now_ms: i64) -> AppResult<PurchaseOutcome> {
    let item = items::item(item_id)?.ok_or_else(|| AppError::Invalid("unknown item".into()))?;
    if item.source != Source::Shop || item.price <= 0 {
        return Ok(PurchaseOutcome::NotForSale);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if repo::is_owned(&tx, &item.id)? {
        return Ok(PurchaseOutcome::AlreadyOwned);
    }
    let (balance, _) = store::ledger_sums(&tx)?;
    if balance < item.price {
        return Ok(PurchaseOutcome::InsufficientPoints {
            balance,
            price: item.price,
        });
    }
    let n = repo::purchase_count(&tx, &item.id)? + 1;
    let ref_key = format!("buy:{}:{n}", item.id);
    if !store::insert_ledger_entry(&tx, now_ms, -item.price, REASON_SHOP_PURCHASE, &ref_key)? {
        return Err(AppError::Internal(
            "purchase ledger key already used".into(),
        ));
    }
    repo::insert_inventory(&tx, &item.id, Source::Shop.as_str(), now_ms, Some(&ref_key))?;
    tx.commit()?;
    tracing::info!(event = "shop_purchase", price = item.price);
    Ok(PurchaseOutcome::Purchased {
        balance: balance - item.price,
    })
}

pub fn shop_view(conn: &Connection) -> AppResult<ShopView> {
    let (balance, _) = store::ledger_sums(conn)?;
    let owned: Vec<String> = repo::list_owned(conn)?
        .into_iter()
        .map(|r| r.item_id)
        .collect();
    let equipped = Equipped::from_rows(&char_repo::wardrobe(conn)?);
    let worn: Vec<&str> = Slot::ALL.iter().filter_map(|s| equipped.get(*s)).collect();
    let items = items::catalog()?
        .items
        .iter()
        .map(|i: &Item| ShopItemView {
            id: i.id.clone(),
            kind: i.kind,
            slot: i.slot,
            label: i.label.clone(),
            price: i.price,
            source: i.source,
            giftable: i.giftable(),
            achievement: i.achievement.map(|a| a.describe().to_string()),
            owned: owned.contains(&i.id),
            equipped: worn.contains(&i.id.as_str()),
            affordable: i.source == Source::Shop && balance >= i.price,
        })
        .collect();
    Ok(ShopView { balance, items })
}

fn need_character(conn: &Connection) -> AppResult<CharacterView> {
    character::view(conn)?.ok_or_else(|| AppError::Invalid("choose a character first".into()))
}

/// Wears an owned clothing item (replacing the slot's item), or switches to an owned character.
/// Furniture is placed in the hut, not worn.
pub fn equip(conn: &mut Connection, item_id: &str) -> AppResult<CharacterView> {
    let item = items::item(item_id)?.ok_or_else(|| AppError::Invalid("unknown item".into()))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    need_character(&tx)?;
    if !repo::is_owned(&tx, &item.id)? {
        return Err(AppError::Invalid("item is not owned".into()));
    }
    match (item.kind, item.slot) {
        (ItemKind::Clothing, Some(slot)) => char_repo::set_wardrobe(&tx, slot.as_str(), &item.id)?,
        (ItemKind::Character, _) => {
            let species = item
                .species
                .as_deref()
                .ok_or_else(|| AppError::Internal("character item has no species".into()))?;
            char_repo::set_species(&tx, species)?;
        }
        _ => return Err(AppError::Invalid("item cannot be worn".into())),
    }
    tx.commit()?;
    need_character(conn)
}

pub fn unequip(conn: &mut Connection, slot: Slot) -> AppResult<CharacterView> {
    need_character(conn)?;
    char_repo::clear_wardrobe(conn, slot.as_str())?;
    need_character(conn)
}
