//! Cheer/scold message settings. The messages themselves are produced by `messages::service` and
//! reach the window as the `message://cheer` event.

use tauri::State;

use super::{lock_db, AppState};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::messages::config::{update_setting, MessageConfig, MessageSettingKey};
use crate::settings::{SettingValue, UpdateOutcome};

#[tauri::command]
pub fn get_message_settings(state: State<'_, AppState>) -> AppResult<MessageConfig> {
    MessageConfig::load(&*lock_db(&state.db)?)
}

#[tauri::command]
pub fn update_message_setting(state: State<'_, AppState>, key: MessageSettingKey, value: SettingValue) -> AppResult<UpdateOutcome> {
    update_setting(&mut *lock_db(&state.db)?, key, value, now_ms()?)
}
