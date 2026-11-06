//! "Install the phone app" card: the APK link and its QR code, and the iPhone support status.

use tauri::State;

use super::{lock_db, AppState};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::phone::apk::{self, PhoneInstallView};
use crate::settings::UpdateOutcome;

#[tauri::command]
pub fn get_phone_install(state: State<'_, AppState>) -> AppResult<PhoneInstallView> {
    let conn = lock_db(&state.db)?;
    apk::view(&conn)
}

/// Empty restores the default link. Locked like any setting during a planned run.
#[tauri::command]
pub fn update_phone_apk_link(state: State<'_, AppState>, link: String) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    apk::update_link(&mut conn, &link, now_ms()?)
}
