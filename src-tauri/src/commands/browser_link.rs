//! Browser link: connection status, site rules, site block entries, host registration, coverage.

use std::collections::BTreeMap;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use super::{AppState, lock_db};
use crate::browser_link::coverage::{self, Coverage};
use crate::browser_link::{LinkStatus, register};
use crate::clock::now_ms;
use crate::enforce::model::{BlockEntry, EntryKind};
use crate::error::{AppError, AppResult};
use crate::platform;
use crate::presets::{self, PresetStatus};
use crate::settings::UpdateOutcome;
use crate::site_rules::{self, SiteRulesConfig, edit};
use crate::store;

#[tauri::command]
pub fn get_browser_link_status(state: State<'_, AppState>) -> LinkStatus {
    state.browser_link.status()
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SiteRulesView {
    pub config: SiteRulesConfig,
    /// What strict mode allows while `config.allow` is `null`.
    pub strict_default_allow: Vec<String>,
}

#[tauri::command]
pub fn get_site_rules(state: State<'_, AppState>) -> AppResult<SiteRulesView> {
    let conn = lock_db(&state.db)?;
    Ok(SiteRulesView {
        config: edit::read_config(&conn)?,
        strict_default_allow: site_rules::strict_default_allow()
            .iter()
            .map(|d| (*d).to_owned())
            .collect(),
    })
}

#[tauri::command]
pub fn update_site_rules(
    state: State<'_, AppState>,
    config: SiteRulesConfig,
) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    edit::update_config(&mut conn, config, now_ms()?)
}

#[tauri::command]
pub fn list_site_entries(state: State<'_, AppState>) -> AppResult<Vec<BlockEntry>> {
    let conn = lock_db(&state.db)?;
    Ok(store::list_block_entries(&conn)?
        .into_iter()
        .filter(|e| e.kind == EntryKind::Site.as_str())
        .collect())
}

#[tauri::command]
pub fn add_site_entry(state: State<'_, AppState>, pattern: String) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    edit::add_site(&mut conn, &pattern, now_ms()?)
}

#[tauri::command]
pub fn get_preset_status(state: State<'_, AppState>) -> AppResult<PresetStatus> {
    let conn = lock_db(&state.db)?;
    presets::status(&conn)
}

/// `choice` maps category id to on/off; the first call is the first-run selection.
#[tauri::command]
pub fn set_preset_categories(
    state: State<'_, AppState>,
    choice: BTreeMap<String, bool>,
) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    presets::set_categories(&mut conn, &choice, now_ms()?)
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostRegistration {
    pub supported: bool,
    pub registered: bool,
}

fn host_target(app: &AppHandle) -> AppResult<platform::HostTarget> {
    let home = app
        .path()
        .home_dir()
        .map_err(|err| AppError::Internal(err.to_string()))?;
    let data = app
        .path()
        .app_data_dir()
        .map_err(|err| AppError::Internal(err.to_string()))?;
    Ok(platform::chrome_host_target(&home, &data)?)
}

#[tauri::command]
pub fn get_browser_host_registration(app: AppHandle) -> HostRegistration {
    match host_target(&app) {
        Ok(target) => HostRegistration {
            supported: true,
            registered: platform::is_host_registered(&target),
        },
        Err(_) => HostRegistration {
            supported: false,
            registered: false,
        },
    }
}

/// Installs or removes the Chrome native-messaging manifest. Removal is refused during study.
#[tauri::command]
pub fn set_browser_host_registered(
    app: AppHandle,
    state: State<'_, AppState>,
    registered: bool,
) -> AppResult<UpdateOutcome> {
    let target = host_target(&app)?;
    let host = std::env::current_exe().map_err(|err| AppError::Internal(err.to_string()))?;
    let mut conn = lock_db(&state.db)?;
    register::set_registered(&mut conn, &target, &host, registered, now_ms()?)
}

/// Study time of a run and the part of it without a connected extension.
#[tauri::command]
pub fn get_run_browser_coverage(state: State<'_, AppState>, run_id: i64) -> AppResult<Coverage> {
    let conn = lock_db(&state.db)?;
    coverage::run_coverage(&conn, run_id, state.high_water.observe(now_ms()?))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::presets::CategoryStatus;

    #[test]
    fn browser_wire_shapes_match_the_shared_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/api/fixtures/browser-contract.json"
        );
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let actual = json!({
            "siteRules": SiteRulesView {
                config: SiteRulesConfig {
                    strict: true,
                    allow: Some(vec!["github.com".into(), "docs.rs".into()]),
                    youtube_playlists: vec!["PLstudy".into()],
                    youtube_channels: vec!["@StudyChannel".into()],
                },
                strict_default_allow: vec!["github.com".into(), "docs.python.org".into()],
            },
            "siteRulesDefaultAllow": SiteRulesView { config: SiteRulesConfig::default(), strict_default_allow: vec![] },
            "presetStatus": PresetStatus {
                onboarded: true,
                version: 1,
                categories: vec![CategoryStatus {
                    id: "messenger",
                    label: "메신저",
                    default_on: true,
                    enabled: false,
                    apps: vec!["com.hnc.discord".into()],
                    sites: vec!["discord.com".into()],
                }],
            },
            "linkStatus": LinkStatus {
                transport_active: true,
                transport_error: None,
                connected: false,
                connections: 0,
                last_connected_at_ms: Some(1_773_000_000_000),
                last_disconnected_at_ms: Some(1_773_000_005_000),
                last_disconnect_reason: Some("closed".into()),
            },
            "hostRegistration": HostRegistration { supported: true, registered: false },
            "coverage": Coverage { study_ms: 3_000_000, unmonitored_ms: 600_000 },
        });
        assert_eq!(actual, expected);
    }
}
