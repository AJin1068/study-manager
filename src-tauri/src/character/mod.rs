//! The study character (PLAN 15). First run (or settings) asks for MBTI, birth year, and gender,
//! every one skippable; rules turn them into 3-4 candidates (`compose`), the user picks one or
//! rolls again, and the pick is stored locally in `character_profile`.
//!
//! Privacy: MBTI, birth year, and gender never leave this module's table. They are not part of
//! any view, any LLM prompt (which carries part labels only), or any server payload. Friends see
//! the look and level only.
//!
//! - `catalog`: the part catalog (data file) and the protected-name lint.
//! - `compose`: pure candidate rules, built-in names, MBTI tone default.
//! - `render`: placeholder SVG with body-aligned clothing anchors.
//! - `service`: candidate names through the Llm port, with the built-in fallback.
//!
//! Choosing does not touch anything else the user owns: a re-roll only produces candidates, and
//! the wardrobe and inventory survive a new character (clothes are shared by all species).

pub mod catalog;
pub mod compose;
pub mod render;
pub mod service;
#[cfg(test)]
mod tests;

use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::messages::config::TONE;
use crate::points;
use crate::points::rewards::Stage;
use crate::settings::{self, SettingValue, UpdateOutcome};
use crate::shop::items::{self, ItemKind, Slot, Source};
use crate::store;
use crate::store::character as repo;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gender {
    Male,
    Female,
    Other,
}

impl Gender {
    pub fn as_str(self) -> &'static str {
        match self {
            Gender::Male => "male",
            Gender::Female => "female",
            Gender::Other => "other",
        }
    }
}

/// What the user typed in the setup form. Every field may be left out.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SetupInput {
    pub mbti: Option<String>,
    pub birth_year: Option<i32>,
    pub gender: Option<Gender>,
}

/// The input after validation, reduced to what the rules read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Traits {
    pub mbti: Option<String>,
    pub birth_year: Option<i32>,
    pub gender: Option<Gender>,
    /// `Some(true)` for E, `Some(false)` for I.
    pub extravert: Option<bool>,
    /// `Some(true)` for T, `Some(false)` for F.
    pub thinking: Option<bool>,
    pub zodiac: Option<usize>,
}

impl Traits {
    /// Stable text of the inputs that shape the look (not gender, which only picks the starter
    /// top): the seed of the candidate rules mixes it in.
    pub fn key(&self) -> String {
        format!(
            "{}|{}",
            self.mbti.as_deref().unwrap_or("-"),
            self.birth_year.map_or("-".to_string(), |y| y.to_string())
        )
    }
}

impl SetupInput {
    /// Validates and normalizes. A blank MBTI counts as skipped; anything else must be a real
    /// MBTI. The birth year must be between 1900 and `current_year`.
    pub fn normalize(&self, current_year: i32) -> AppResult<Traits> {
        let mbti = match self
            .mbti
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            None => None,
            Some(raw) => {
                if compose::tone_for_mbti(raw).is_none() {
                    return Err(AppError::Invalid(
                        "mbti must be four letters like INFP".into(),
                    ));
                }
                Some(raw.to_uppercase())
            }
        };
        if let Some(year) = self.birth_year {
            if !(1900..=current_year).contains(&year) {
                return Err(AppError::Invalid("birth year is out of range".into()));
            }
        }
        let letters: Vec<char> = mbti.as_deref().unwrap_or("").chars().collect();
        Ok(Traits {
            extravert: letters.first().map(|c| *c == 'E'),
            thinking: letters.get(2).map(|c| *c == 'T'),
            zodiac: self.birth_year.map(compose::zodiac_index),
            birth_year: self.birth_year,
            gender: self.gender,
            mbti,
        })
    }
}

/// The part ids of a look.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    pub species: String,
    pub palette: String,
    pub expression: String,
    pub prop: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub look: Look,
    pub starter_top: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LookLabels {
    pub species: String,
    pub palette: String,
    pub expression: String,
    pub prop: String,
}

pub fn labels(look: &Look) -> AppResult<LookLabels> {
    let cat = catalog::catalog()?;
    let miss = || AppError::Internal("character look names an unknown part".into());
    Ok(LookLabels {
        species: cat.species(&look.species).ok_or_else(miss)?.label.clone(),
        palette: cat.palette(&look.palette).ok_or_else(miss)?.label.clone(),
        expression: cat
            .expression(&look.expression)
            .ok_or_else(miss)?
            .label
            .clone(),
        prop: cat.prop(&look.prop).ok_or_else(miss)?.label.clone(),
    })
}

/// Worn items by slot (item ids).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Equipped {
    pub hat: Option<String>,
    pub top: Option<String>,
    pub accessory: Option<String>,
    pub hand: Option<String>,
}

impl Equipped {
    pub fn get(&self, slot: Slot) -> Option<&str> {
        match slot {
            Slot::Hat => self.hat.as_deref(),
            Slot::Top => self.top.as_deref(),
            Slot::Accessory => self.accessory.as_deref(),
            Slot::Hand => self.hand.as_deref(),
        }
    }

    pub fn from_rows(rows: &[(String, String)]) -> Self {
        let mut out = Self::default();
        for (slot, id) in rows {
            match Slot::parse(slot) {
                Some(Slot::Hat) => out.hat = Some(id.clone()),
                Some(Slot::Top) => out.top = Some(id.clone()),
                Some(Slot::Accessory) => out.accessory = Some(id.clone()),
                Some(Slot::Hand) => out.hand = Some(id.clone()),
                None => {}
            }
        }
        out
    }
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NameSource {
    Llm,
    Builtin,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateView {
    pub look: Look,
    pub labels: LookLabels,
    pub starter_top: String,
    pub name: String,
    pub blurb: String,
    pub name_source: NameSource,
    pub svg: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateSet {
    pub seed: u32,
    pub candidates: Vec<CandidateView>,
    /// Why the names are built-in: a stable code (`llm_disabled`, ...) or `bad_answer`; `None`
    /// when the model named every candidate.
    pub fallback_reason: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CharacterView {
    pub name: String,
    pub blurb: String,
    pub look: Look,
    pub labels: LookLabels,
    pub stage: Stage,
    pub stage_label: String,
    pub level: i64,
    pub equipped: Equipped,
    pub svg: String,
    pub chosen_at_ms: i64,
}

/// The saved character at its current level, or `None` before the first pick.
pub fn view(conn: &Connection) -> AppResult<Option<CharacterView>> {
    let Some(profile) = repo::get_profile(conn)? else {
        return Ok(None);
    };
    let look = Look {
        species: profile.species,
        palette: profile.palette,
        expression: profile.expression,
        prop: profile.prop,
    };
    let equipped = Equipped::from_rows(&repo::wardrobe(conn)?);
    let status = points::status(conn)?;
    Ok(Some(CharacterView {
        name: profile.name,
        blurb: profile.blurb,
        labels: labels(&look)?,
        svg: render::render(&look, status.stage, &equipped)?,
        look,
        stage: status.stage,
        stage_label: status.stage_label,
        level: status.level,
        equipped,
        chosen_at_ms: profile.chosen_at_ms,
    }))
}

/// The menu-bar line: stage glyph, name, level.
pub fn tray_line(view: &CharacterView) -> String {
    let glyph = match view.stage {
        Stage::Seed => "🌰",
        Stage::Sprout => "🌱",
        Stage::SmallTree => "🌳",
        Stage::ForestSpirit => "✨",
    };
    format!(
        "{glyph} {} · Lv.{} {}",
        view.name, view.level, view.stage_label
    )
}

/// Updates the menu-bar line from the database. Best effort: the tray is not worth failing for.
pub fn refresh_tray(app: &tauri::AppHandle, db: &std::sync::Mutex<Connection>) {
    let line = crate::commands::lock_db(db)
        .and_then(|conn| view(&conn))
        .map(|v| v.map(|v| tray_line(&v)));
    match line {
        Ok(line) => crate::tray::set_character_line(app, line.as_deref()),
        Err(err) => tracing::warn!(error = %err, "character line for the tray unavailable"),
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChooseRequest {
    pub look: Look,
    pub starter_top: String,
    pub name: String,
    pub blurb: String,
    #[serde(default)]
    pub input: SetupInput,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChooseOutcome {
    pub character: CharacterView,
    /// The message tone the MBTI set as the starting value, only the first time and only when the
    /// user had no tone of their own.
    pub tone_applied: Option<String>,
}

/// Saves the pick. Validates every id against the catalogs, keeps the wardrobe, grants and wears
/// the free starter top when the top slot is empty, and (once) sets the message tone from the MBTI.
pub fn choose(
    conn: &mut Connection,
    req: &ChooseRequest,
    now_ms: i64,
    current_year: i32,
) -> AppResult<ChooseOutcome> {
    let traits = req.input.normalize(current_year)?;
    let cat = catalog::catalog()?;
    let invalid = |what: &str| AppError::Invalid(format!("unknown or unavailable {what}"));
    let species = cat
        .species(&req.look.species)
        .ok_or_else(|| invalid("species"))?;
    if species.shop_only && !crate::shop::owns_species(conn, &species.id)? {
        return Err(invalid("species"));
    }
    cat.palette(&req.look.palette)
        .ok_or_else(|| invalid("palette"))?;
    cat.expression(&req.look.expression)
        .ok_or_else(|| invalid("expression"))?;
    cat.prop(&req.look.prop).ok_or_else(|| invalid("prop"))?;
    let starter = items::item(&req.starter_top)?
        .filter(|i| {
            i.source == Source::Starter && i.kind == ItemKind::Clothing && i.slot == Some(Slot::Top)
        })
        .ok_or_else(|| invalid("starter top"))?;
    let name = compose::clean_name(&req.name)
        .ok_or_else(|| AppError::Invalid("name must be 1-8 characters".into()))?;
    let blurb = compose::clean_intro(&req.blurb)
        .ok_or_else(|| AppError::Invalid("intro must be 1-40 characters".into()))?;

    {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        repo::save_profile(
            &tx,
            &repo::ProfileRow {
                species: req.look.species.clone(),
                palette: req.look.palette.clone(),
                expression: req.look.expression.clone(),
                prop: req.look.prop.clone(),
                name,
                blurb,
                mbti: traits.mbti.clone(),
                birth_year: traits.birth_year,
                gender: traits.gender.map(|g| g.as_str().to_string()),
                chosen_at_ms: now_ms,
            },
        )?;
        crate::store::shop::insert_inventory(
            &tx,
            &starter.id,
            Source::Starter.as_str(),
            now_ms,
            None,
        )?;
        if !repo::wardrobe(&tx)?
            .iter()
            .any(|(slot, _)| slot == Slot::Top.as_str())
        {
            repo::set_wardrobe(&tx, Slot::Top.as_str(), &starter.id)?;
        }
        tx.commit()?;
    }

    let tone_applied = apply_default_tone(conn, traits.mbti.as_deref(), now_ms)?;
    let character =
        view(conn)?.ok_or_else(|| AppError::Internal("saved character is missing".into()))?;
    Ok(ChooseOutcome {
        character,
        tone_applied,
    })
}

/// Sets `messages.tone` from the MBTI when no tone row exists yet. A user's own value (any row,
/// even one equal to the default) is never overwritten, and a locked setting is simply skipped.
fn apply_default_tone(
    conn: &mut Connection,
    mbti: Option<&str>,
    now_ms: i64,
) -> AppResult<Option<String>> {
    let Some(tone) = mbti.and_then(compose::tone_for_mbti) else {
        return Ok(None);
    };
    if store::get_setting(conn, TONE.name)?.is_some() {
        return Ok(None);
    }
    let outcome = settings::apply_setting(
        conn,
        &TONE,
        &SettingValue::Text(tone.as_str().to_string()),
        now_ms,
        || Ok(()),
    )?;
    Ok(matches!(outcome, UpdateOutcome::Applied { .. }).then(|| tone.as_str().to_string()))
}
