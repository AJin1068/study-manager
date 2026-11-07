//! LLM settings and usage. The API key goes through `save_secret` like every other secret; model
//! calls themselves are made by the features that need them, never from here (except the test).

use tauri::{AppHandle, Emitter, State};

use super::{AppState, lock_db};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::llm::config::{LlmConfig, LlmSettingKey, update_setting};
use crate::llm::service::LlmStatus;
use crate::settings::{SettingValue, UpdateOutcome};

use super::remote::REMOTE_STATE_EVENT;

#[tauri::command]
pub fn get_llm_settings(state: State<'_, AppState>) -> AppResult<LlmConfig> {
    let conn = lock_db(&state.db)?;
    LlmConfig::load(&conn)
}

#[tauri::command]
pub fn update_llm_setting(
    app: AppHandle,
    state: State<'_, AppState>,
    key: LlmSettingKey,
    value: SettingValue,
) -> AppResult<UpdateOutcome> {
    let outcome = {
        let mut conn = lock_db(&state.db)?;
        update_setting(&mut conn, key, value, now_ms()?)?
    };
    // The integration card reads the same states; refresh it.
    let view = lock_db(&state.db).and_then(|conn| state.remote.view(&conn));
    match view {
        Ok(view) => {
            if let Err(err) = app.emit(REMOTE_STATE_EVENT, view) {
                tracing::error!(error = %err, "failed to emit remote state");
            }
        }
        Err(err) => tracing::error!(error = %err, "could not build remote state for the UI"),
    }
    Ok(outcome)
}

#[tauri::command]
pub fn get_llm_status(state: State<'_, AppState>) -> AppResult<LlmStatus> {
    state.llm.status()
}
