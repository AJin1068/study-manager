//! Power-source and autostart status commands.

use serde::Serialize;
use tauri::State;

use super::AppState;
use crate::error::AppResult;

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PowerSourceStatus {
    /// False when the OS power source is unavailable; the heartbeat fallback still runs.
    pub active: bool,
    pub reason: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AutostartStatus {
    pub enabled: bool,
}

#[tauri::command]
pub fn get_autostart_status(state: State<'_, AppState>) -> AppResult<AutostartStatus> {
    Ok(AutostartStatus { enabled: state.autostart.is_enabled()? })
}

#[tauri::command]
pub fn get_power_status(state: State<'_, AppState>) -> PowerSourceStatus {
    state.power.clone()
}
