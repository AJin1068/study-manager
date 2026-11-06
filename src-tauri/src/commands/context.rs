//! Study-context commands (opt-in "what was I studying" labels). See `study_context`.

use tauri::State;

use super::{lock_db, AppState};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::settings::{SettingValue, UpdateOutcome};
use crate::study_context::{self, ContextSettingKey, ContextStatus};

#[tauri::command]
pub fn get_context_status(state: State<'_, AppState>) -> AppResult<ContextStatus> {
    let conn = lock_db(&state.db)?;
    study_context::status(&conn, state.focus_probe.as_ref(), now_ms()?)
}

#[tauri::command]
pub fn update_context_setting(state: State<'_, AppState>, key: ContextSettingKey, value: SettingValue) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    study_context::update_setting(&mut conn, key, value, now_ms()?)
}

/// Deletes every stored label; returns how many rows went.
#[tauri::command]
pub fn delete_study_context(state: State<'_, AppState>) -> AppResult<usize> {
    study_context::delete_all(&*lock_db(&state.db)?)
}
