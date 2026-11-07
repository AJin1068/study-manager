//! The item catalog (`assets/character/items.json`): clothes in four slots, hut furniture, and
//! extra characters. The art is placeholder shapes (O-13); each clothing `svg` is a template over
//! the anchor box of its slot (`character::render`), so one item fits every species.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::error::{AppError, AppResult};

const RAW: &str = include_str!("../../assets/character/items.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Clothing,
    Furniture,
    Character,
}

/// The four wardrobe slots. Matches the `wardrobe.slot` CHECK (drift-tested).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    Hat,
    Top,
    Accessory,
    Hand,
}

impl Slot {
    pub const ALL: [Slot; 4] = [Slot::Hat, Slot::Top, Slot::Accessory, Slot::Hand];

    pub fn as_str(self) -> &'static str {
        match self {
            Slot::Hat => "hat",
            Slot::Top => "top",
            Slot::Accessory => "accessory",
            Slot::Hand => "hand",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// How an item is obtained. Matches the `inventory_local.source` CHECK (drift-tested); `gift` is
/// reserved for the gift story.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Starter,
    Shop,
    Achievement,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Starter => "starter",
            Source::Shop => "shop",
            Source::Achievement => "achievement",
        }
    }
}

/// Achievements that unlock an item, derived from the study records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Achievement {
    /// 7 days in a row at the streak threshold.
    #[serde(rename = "streak_7")]
    Streak7,
    /// 100 right quiz answers.
    #[serde(rename = "quiz_100")]
    Quiz100,
}

impl Achievement {
    pub fn describe(self) -> &'static str {
        match self {
            Achievement::Streak7 => "7일 연속 달성",
            Achievement::Quiz100 => "퀴즈 100개 정답",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: ItemKind,
    #[serde(default)]
    pub slot: Option<Slot>,
    pub label: String,
    pub price: i64,
    pub source: Source,
    #[serde(default)]
    pub achievement: Option<Achievement>,
    /// Character items: the species they unlock.
    #[serde(default)]
    pub species: Option<String>,
    pub svg: String,
}

impl Item {
    /// Only shop items can be given away: starter and achievement items cannot, and points never
    /// move between people.
    pub fn giftable(&self) -> bool {
        self.source == Source::Shop
    }
}

#[derive(Deserialize)]
struct Raw {
    items: Vec<Item>,
}

pub struct ItemCatalog {
    pub items: Vec<Item>,
}

impl ItemCatalog {
    pub fn get(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }
}

static CATALOG: OnceLock<Result<ItemCatalog, String>> = OnceLock::new();

pub fn catalog() -> AppResult<&'static ItemCatalog> {
    CATALOG
        .get_or_init(|| {
            serde_json::from_str::<Raw>(RAW)
                .map(|r| ItemCatalog { items: r.items })
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| AppError::Internal(format!("item catalog is unreadable: {e}")))
}

pub fn item(id: &str) -> AppResult<Option<&'static Item>> {
    Ok(catalog()?.get(id))
}
