//! The part catalog (`assets/character/parts.json`): species, fur colors, expressions, props.
//! Replacing the placeholder shapes with real art means editing that file (docs/art-assets.md);
//! no code reads a shape. Names and labels are original: `is_banned` is the lint (and also
//! screens model-written names).

use std::sync::OnceLock;

use serde::Deserialize;

use crate::error::{AppError, AppResult};

const RAW: &str = include_str!("../../assets/character/parts.json");

/// Names of existing film, comic, and game characters (and studios) that must not appear.
const BANNED: &[&str] = &[
    "지브리",
    "토토로",
    "가오나시",
    "치히로",
    "하울",
    "포뇨",
    "라퓨타",
    "나우시카",
    "모노노케",
    "피카츄",
    "포켓몬",
    "도라에몽",
    "짱구",
    "뽀로로",
    "미키",
    "미니마우스",
    "푸우",
    "카카오프렌즈",
    "라이언",
    "어피치",
    "스누피",
    "헬로키티",
    "둘리",
    "산리오",
    "디즈니",
    "마리오",
];

/// Whether `text` names a protected character.
pub fn is_banned(text: &str) -> bool {
    let lower = text.to_lowercase();
    BANNED.iter().any(|b| lower.contains(b))
}

#[derive(Clone, Debug, Deserialize)]
pub struct Body {
    pub head_cy: f64,
    pub head_r: f64,
    pub torso_w: f64,
    pub torso_h: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Species {
    pub id: String,
    pub label: String,
    pub shop_only: bool,
    pub body: Body,
    pub svg: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Temperature {
    Warm,
    Cool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Palette {
    pub id: String,
    pub label: String,
    pub hex: String,
    pub temperature: Temperature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Energy {
    High,
    Low,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Expression {
    pub id: String,
    pub label: String,
    pub energy: Energy,
    pub svg: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mind {
    Thinking,
    Feeling,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Prop {
    pub id: String,
    pub label: String,
    pub mind: Mind,
    pub svg: String,
}

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub species: Vec<Species>,
    pub palettes: Vec<Palette>,
    pub expressions: Vec<Expression>,
    pub props: Vec<Prop>,
}

impl Catalog {
    pub fn species(&self, id: &str) -> Option<&Species> {
        self.species.iter().find(|s| s.id == id)
    }
    pub fn palette(&self, id: &str) -> Option<&Palette> {
        self.palettes.iter().find(|p| p.id == id)
    }
    pub fn expression(&self, id: &str) -> Option<&Expression> {
        self.expressions.iter().find(|e| e.id == id)
    }
    pub fn prop(&self, id: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.id == id)
    }
}

static CATALOG: OnceLock<Result<Catalog, String>> = OnceLock::new();

pub fn catalog() -> AppResult<&'static Catalog> {
    CATALOG
        .get_or_init(|| serde_json::from_str::<Catalog>(RAW).map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| AppError::Internal(format!("part catalog is unreadable: {e}")))
}
