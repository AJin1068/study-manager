//! Recommended block-list presets (docs/PLAN.md feature 2). The catalog is versioned bundled data;
//! choosing categories materializes it as `block_entries` rows with `source = 'preset'` and the
//! category name, and switching a category only flips `enabled` on those rows. Entries the user
//! typed are `source = 'user'` rows and are never touched here: a refresh inserts missing preset
//! rows with `INSERT OR IGNORE`, so a user entry with the same pattern stays the user's.
//!
//! Identifier accuracy: none of the app identifiers below could be checked against an installed
//! copy on the development machine (see docs/stage-1-4-verification.md); the user can add the real
//! identifier by hand from the running-apps picker. App identifiers are macOS bundle ids; on
//! Windows an app is matched by executable path, so preset apps are inert there until a path list
//! is added in stage 2.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::enforce::model::EntryKind;
use crate::error::{AppError, AppResult};
use crate::platform::normalize_app_id;
use crate::settings::{Outcome, Reason, UpdateOutcome};
use crate::site_rules::normalize_pattern;
use crate::store;

/// Bump when the catalog below changes; startup then adds the new entries.
pub const PRESET_VERSION: u32 = 1;
/// Settings key of the saved category choice (`{"version":N,"enabled":{"<id>":bool}}`).
pub const SELECTION_KEY: &str = "presetSelection";
const HISTORY_KEY: &str = "presetCategory";
const SETTING_CATEGORY: &str = "browser";

/// One row of the PLAN.md preset table.
pub struct Category {
    pub id: &'static str,
    pub label: &'static str,
    pub default_on: bool,
    /// macOS bundle ids (UNVERIFIED, see module docs).
    pub apps: &'static [&'static str],
    /// `domain` or `domain/path`.
    pub sites: &'static [&'static str],
}

/// The PLAN.md table: messenger, video, SNS, community, and game are on; shopping/webtoon is off.
/// Music apps are not here: they stay allowed by default (`site_rules` music domains).
pub const CATALOG: &[Category] = &[
    Category {
        id: "messenger",
        label: "메신저",
        default_on: true,
        apps: &[
            "com.kakao.KakaoTalkMac",
            "com.hnc.Discord",
            "ru.keepcoder.Telegram",
        ],
        sites: &["web.kakao.com", "discord.com"],
    },
    Category {
        id: "video",
        label: "영상",
        default_on: true,
        apps: &["com.netflix.Netflix"],
        sites: &["youtube.com", "netflix.com", "chzzk.naver.com", "twitch.tv"],
    },
    Category {
        id: "sns",
        label: "SNS",
        default_on: true,
        apps: &[],
        sites: &["instagram.com", "x.com", "threads.net", "facebook.com"],
    },
    Category {
        id: "community",
        label: "커뮤니티",
        default_on: true,
        apps: &[],
        sites: &["dcinside.com", "fmkorea.com", "theqoo.net", "reddit.com"],
    },
    Category {
        id: "game",
        label: "게임",
        default_on: true,
        apps: &[
            "com.valvesoftware.steam",
            "com.riotgames.RiotGames.RiotClient",
            "net.battle.app",
            "com.epicgames.EpicGamesLauncher",
        ],
        sites: &[],
    },
    Category {
        id: "shopping_webtoon",
        label: "쇼핑·웹툰",
        default_on: false,
        apps: &[],
        sites: &["coupang.com", "29cm.co.kr", "comic.naver.com"],
    },
];

fn category(id: &str) -> Option<&'static Category> {
    CATALOG.iter().find(|c| c.id == id)
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
struct Selection {
    version: u32,
    enabled: BTreeMap<String, bool>,
}

fn read_selection(conn: &Connection) -> AppResult<Option<Selection>> {
    match store::get_setting(conn, SELECTION_KEY)? {
        Some(raw) => serde_json::from_str(&raw).map(Some).map_err(|err| {
            AppError::Internal(format!("{SELECTION_KEY} setting is invalid: {err}"))
        }),
        None => Ok(None),
    }
}

/// What the stored choice says about a category, or the PLAN default for one never chosen.
fn is_enabled(selection: Option<&Selection>, cat: &Category) -> bool {
    selection
        .and_then(|s| s.enabled.get(cat.id).copied())
        .unwrap_or(cat.default_on)
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CategoryStatus {
    pub id: &'static str,
    pub label: &'static str,
    pub default_on: bool,
    pub enabled: bool,
    pub apps: Vec<String>,
    pub sites: Vec<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PresetStatus {
    /// The first-run choice has been made. Until then nothing from the catalog is on the list.
    pub onboarded: bool,
    pub version: u32,
    pub categories: Vec<CategoryStatus>,
}

pub fn status(conn: &Connection) -> AppResult<PresetStatus> {
    let selection = read_selection(conn)?;
    Ok(PresetStatus {
        onboarded: selection.is_some(),
        version: PRESET_VERSION,
        categories: CATALOG
            .iter()
            .map(|cat| CategoryStatus {
                id: cat.id,
                label: cat.label,
                default_on: cat.default_on,
                enabled: is_enabled(selection.as_ref(), cat),
                apps: cat.apps.iter().map(|app| normalize_app_id(app)).collect(),
                sites: cat.sites.iter().map(|site| (*site).to_owned()).collect(),
            })
            .collect(),
    })
}

/// Inserts the category's rows that are missing, keeping existing rows as they are.
fn materialize(conn: &Connection, cat: &Category, enabled: bool, at_ms: i64) -> AppResult<()> {
    for app in cat.apps {
        store::insert_preset_entry(
            conn,
            EntryKind::App,
            &normalize_app_id(app),
            cat.id,
            enabled,
            at_ms,
        )?;
    }
    for site in cat.sites {
        let pattern = normalize_pattern(site).ok_or_else(|| {
            AppError::Internal(format!(
                "preset site {site:?} in {} is not a usable pattern",
                cat.id
            ))
        })?;
        store::insert_preset_entry(conn, EntryKind::Site, &pattern, cat.id, enabled, at_ms)?;
    }
    Ok(())
}

/// Saves the category choice and applies it to the block list. `choice` maps category id to
/// on/off; categories it leaves out keep their current state (the PLAN default before the first
/// choice). Locked during study: the attempt is recorded and nothing changes. The first call is
/// the first-run selection; later calls are the settings screen.
pub fn set_categories(
    conn: &mut Connection,
    choice: &BTreeMap<String, bool>,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    if let Some(unknown) = choice.keys().find(|id| category(id).is_none()) {
        return Err(AppError::Invalid(format!(
            "unknown preset category {unknown:?}"
        )));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = read_selection(&tx)?;
    let mut next = Selection {
        version: PRESET_VERSION,
        enabled: BTreeMap::new(),
    };
    for cat in CATALOG {
        let wanted = choice
            .get(cat.id)
            .copied()
            .unwrap_or_else(|| is_enabled(current.as_ref(), cat));
        next.enabled.insert(cat.id.to_owned(), wanted);
    }
    let changed: Vec<String> = CATALOG
        .iter()
        .filter(|cat| {
            current.is_none() || is_enabled(current.as_ref(), cat) != next.enabled[cat.id]
        })
        .map(|cat| {
            format!(
                "{}:{}",
                cat.id,
                if next.enabled[cat.id] { "on" } else { "off" }
            )
        })
        .collect();
    if crate::study::lock_held(&tx)? {
        for item in &changed {
            store::append_history(
                &tx,
                at_ms,
                HISTORY_KEY,
                None,
                item,
                Reason::User,
                Outcome::RejectedLocked,
            )?;
        }
        tx.commit()?;
        return Ok(UpdateOutcome::RejectedLocked {
            key: HISTORY_KEY.into(),
        });
    }
    for cat in CATALOG {
        let enabled = next.enabled[cat.id];
        materialize(&tx, cat, enabled, at_ms)?;
        store::set_preset_category_enabled(&tx, cat.id, enabled)?;
    }
    let json = serde_json::to_string(&next).map_err(|err| AppError::Internal(err.to_string()))?;
    store::put_setting(&tx, SELECTION_KEY, SETTING_CATEGORY, &json, at_ms)?;
    for item in &changed {
        store::append_history(
            &tx,
            at_ms,
            HISTORY_KEY,
            None,
            item,
            Reason::User,
            Outcome::Applied,
        )?;
    }
    tx.commit()?;
    Ok(UpdateOutcome::Applied {
        key: HISTORY_KEY.into(),
    })
}

/// Startup: after a catalog update, adds the entries the new version brings. Existing rows keep
/// their on/off state and user entries are untouched; a category the user has never seen takes its
/// PLAN default. Does nothing before the first-run choice or when already current.
pub fn refresh(conn: &mut Connection, at_ms: i64) -> AppResult<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(mut selection) = read_selection(&tx)? else {
        return Ok(());
    };
    if selection.version >= PRESET_VERSION {
        return Ok(());
    }
    for cat in CATALOG {
        let enabled = *selection
            .enabled
            .entry(cat.id.to_owned())
            .or_insert(cat.default_on);
        materialize(&tx, cat, enabled, at_ms)?;
    }
    let before = format!("version={}", selection.version);
    selection.version = PRESET_VERSION;
    let json =
        serde_json::to_string(&selection).map_err(|err| AppError::Internal(err.to_string()))?;
    store::put_setting(&tx, SELECTION_KEY, SETTING_CATEGORY, &json, at_ms)?;
    store::append_history(
        &tx,
        at_ms,
        HISTORY_KEY,
        Some(&before),
        &format!("version={PRESET_VERSION}"),
        Reason::Default,
        Outcome::Applied,
    )?;
    tx.commit()?;
    Ok(())
}
