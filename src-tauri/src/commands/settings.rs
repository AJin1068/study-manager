//! Settings commands.

use tauri::State;

use super::{AppState, DEFAULT_LIMIT, lock_db};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::settings::{
    self, GeneralSettings, HistoryEntry, SettingKey, SettingValue, UpdateOutcome,
};

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppResult<GeneralSettings> {
    let conn = lock_db(&state.db)?;
    settings::general_settings(&conn)
}

#[tauri::command]
pub fn update_setting(
    state: State<'_, AppState>,
    key: SettingKey,
    value: SettingValue,
) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    settings::update_setting(&mut conn, state.autostart.as_ref(), key, value, now_ms()?)
}

#[tauri::command]
pub fn get_setting_history(
    state: State<'_, AppState>,
    limit: Option<i64>,
) -> AppResult<Vec<HistoryEntry>> {
    let conn = lock_db(&state.db)?;
    settings::read_history(&conn, limit.unwrap_or(DEFAULT_LIMIT))
}
