//! Lock state and study-mode commands.

use chrono::Local;
use tauri::{AppHandle, State};

use super::plan::announce_plans_changed;
use super::{lock_db, AppState};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::plan_shift::ModeChange;
use crate::study::{self, LockState, ModeConfig, ModeSnapshot, StudyLevel};

/// Local calendar date used as the daily-mode key. Local time is used only for the day
/// boundary; stored timestamps stay in UTC epoch milliseconds.
pub fn today_key() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[tauri::command]
pub fn get_lock_state(state: State<'_, AppState>) -> AppResult<LockState> {
    let conn = lock_db(&state.db)?;
    study::read_lock_state(&conn)
}

#[tauri::command]
pub fn get_mode_config(state: State<'_, AppState>) -> AppResult<ModeSnapshot> {
    let conn = lock_db(&state.db)?;
    study::mode_snapshot(&conn, &today_key())
}

#[tauri::command]
pub fn set_daily_mode(app: AppHandle, state: State<'_, AppState>, level: StudyLevel) -> AppResult<ModeChange> {
    let change = {
        let mut conn = lock_db(&state.db)?;
        study::set_daily_mode(&mut conn, &Local, &today_key(), level, now_ms()?)?
    };
    announce_if_moved(&app, &change);
    Ok(change)
}

#[tauri::command]
pub fn update_mode_config(
    app: AppHandle,
    state: State<'_, AppState>,
    level: StudyLevel,
    study_min: i64,
    break_min: i64,
    long_break_min: Option<i64>,
    sets_before_long_break: Option<i64>,
) -> AppResult<ModeChange> {
    let change = {
        let mut conn = lock_db(&state.db)?;
        let config = ModeConfig { level, study_min, break_min, long_break_min, sets_before_long_break };
        study::update_mode_config(&mut conn, &Local, &today_key(), config, now_ms()?)?
    };
    announce_if_moved(&app, &change);
    Ok(change)
}

/// A mode edit that moved plans changes plan data like any plan edit.
fn announce_if_moved(app: &AppHandle, change: &ModeChange) {
    if change.moved_plans() {
        announce_plans_changed(app);
    }
}
