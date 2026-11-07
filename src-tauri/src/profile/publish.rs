//! Builds what leaves the PC for the profile from the local facts and the user's scopes, and sends
//! it. The scope is applied here first (a private section is replaced by an empty value before the
//! payload exists) and again on the server.

use std::collections::BTreeMap;

use chrono::{Duration, TimeZone};
use rusqlite::Connection;

use super::model::{ProfileCode, ProfileError, Scope, Section};
use super::payload::{GrassEntry, SetProfileArgs};
use super::port::ProfileApi;
use crate::error::{AppError, AppResult};
use crate::points;
use crate::shop::items::{self, ItemKind};
use crate::store;
use crate::store::profile as repo;
use crate::timer::{self, model::Phase};

/// Days of grass kept (a year) and the most furniture ids shown.
pub const GRASS_DAYS: i64 = 365;
pub const MAX_FURNITURE: usize = 12;
pub const MAX_WIKI_TITLES: usize = 10;

/// Everything a profile could show, read once from the local database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalFacts {
    pub name: String,
    pub scopes: BTreeMap<Section, Scope>,
    pub level: i64,
    pub badges: Vec<String>,
    pub furniture: Vec<String>,
    pub wiki_titles: Vec<String>,
    pub subject: Option<String>,
    pub grass: Vec<GrassEntry>,
}

pub fn collect<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<LocalFacts> {
    let today = tz
        .timestamp_millis_opt(now_ms)
        .single()
        .ok_or_else(|| AppError::Internal("timestamp has no local date".into()))?
        .date_naive();
    let date = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();

    let mut badges = Vec::new();
    let mut furniture = Vec::new();
    for item in &items::catalog()?.items {
        if !store::shop::is_owned(conn, &item.id)? {
            continue;
        }
        if let Some(achievement) = item.achievement {
            if let Ok(serde_json::Value::String(id)) = serde_json::to_value(achievement) {
                badges.push(id);
            }
        }
        if item.kind == ItemKind::Furniture && furniture.len() < MAX_FURNITURE {
            furniture.push(item.id.clone());
        }
    }

    let state = timer::current_state(conn, tz, now_ms)?;
    let subject = match (state.phase, state.run_id) {
        (Phase::Study, Some(run)) => repo::run_subject(conn, run)?,
        _ => None,
    };

    let grass = repo::study_minutes(
        conn,
        &date(today - Duration::days(GRASS_DAYS - 1)),
        &date(today),
    )?
    .into_iter()
    .filter(|(_, minutes)| *minutes > 0)
    .map(|(day, study_min)| GrassEntry {
        day,
        study_min: study_min.min(1440),
    })
    .collect();

    Ok(LocalFacts {
        name: repo::local(conn)?.display_name,
        scopes: repo::visibility(conn)?,
        level: points::status(conn)?.level.clamp(1, 999),
        badges,
        furniture,
        wiki_titles: repo::wiki_titles(conn)?,
        subject,
        grass,
    })
}

fn scope_of(facts: &LocalFacts, section: Section) -> Scope {
    facts
        .scopes
        .get(&section)
        .copied()
        .unwrap_or(Scope::Private)
}

fn shown(facts: &LocalFacts, section: Section) -> bool {
    scope_of(facts, section) != Scope::Private
}

/// The settings payload: a section kept private carries nothing, whatever the local data holds.
pub fn settings_args(facts: &LocalFacts) -> SetProfileArgs {
    let scope = |s: Section| scope_of(facts, s).as_str();
    SetProfileArgs {
        display_name: facts.name.trim().to_string(),
        vis_room: scope(Section::Room),
        vis_grass: scope(Section::Grass),
        vis_level: scope(Section::Level),
        vis_badges: scope(Section::Badges),
        vis_guestbook: scope(Section::Guestbook),
        vis_visitors: scope(Section::Visitors),
        vis_wiki: scope(Section::Wiki),
        vis_subject: scope(Section::Subject),
        vis_gifts: scope(Section::Gifts),
        level: shown(facts, Section::Level).then_some(facts.level),
        badges: if shown(facts, Section::Badges) {
            facts.badges.clone()
        } else {
            Vec::new()
        },
        furniture: if shown(facts, Section::Room) {
            facts.furniture.clone()
        } else {
            Vec::new()
        },
        wiki_titles: if shown(facts, Section::Wiki) {
            facts
                .wiki_titles
                .iter()
                .take(MAX_WIKI_TITLES)
                .cloned()
                .collect()
        } else {
            Vec::new()
        },
        subject_now: if shown(facts, Section::Subject) {
            facts.subject.clone()
        } else {
            None
        },
    }
}

/// The grass days to send: none while the grass is private.
pub fn grass_entries(facts: &LocalFacts) -> Vec<GrassEntry> {
    if shown(facts, Section::Grass) {
        facts.grass.clone()
    } else {
        Vec::new()
    }
}

/// Sends the settings, then the grass. Returns the profile link id.
pub fn publish(api: &dyn ProfileApi, facts: &LocalFacts) -> Result<String, ProfileError> {
    if facts.name.trim().is_empty() {
        return Err(ProfileError::Rejected(ProfileCode::NameRequired));
    }
    let slug = api.set_settings(&settings_args(facts))?;
    let grass = grass_entries(facts);
    if !grass.is_empty() {
        api.report_grass(&grass)?;
    }
    Ok(slug)
}
