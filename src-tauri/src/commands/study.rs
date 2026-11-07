//! Lock state and study-mode commands.

use std::sync::Arc;

use chrono::Local;
use tauri::{AppHandle, State};

use super::plan::announce_plans_changed;
use super::{AppState, lock_db};
use crate::clock::now_ms;
use crate::error::{AppError, AppResult};
use crate::plan_adjust;
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

/// The mode change and, around it, the LLM adjustment of the rest of today's plans (rule-based
/// fallback inside). Runs on the blocking pool because the model call may take seconds.
#[tauri::command]
pub async fn set_daily_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    level: StudyLevel,
) -> AppResult<ModeChange> {
    let (db, llm) = (Arc::clone(&state.db), Arc::clone(&state.llm));
    let at_ms = now_ms()?;
    let change = tauri::async_runtime::spawn_blocking(move || {
        plan_adjust::change_daily_mode(&db, &llm, &Local, &today_key(), level, at_ms)
    })
    .await
    .map_err(|_| AppError::Internal("the mode change did not finish".into()))??;
    announce_if_moved(&app, &change);
    Ok(change)
}

#[tauri::command]
pub async fn update_mode_config(
    app: AppHandle,
    state: State<'_, AppState>,
    level: StudyLevel,
    study_min: i64,
    break_min: i64,
    long_break_min: Option<i64>,
    sets_before_long_break: Option<i64>,
) -> AppResult<ModeChange> {
    let (db, llm) = (Arc::clone(&state.db), Arc::clone(&state.llm));
    let at_ms = now_ms()?;
    let config = ModeConfig {
        level,
        study_min,
        break_min,
        long_break_min,
        sets_before_long_break,
    };
    let change = tauri::async_runtime::spawn_blocking(move || {
        plan_adjust::change_mode_config(&db, &llm, &Local, &today_key(), config, at_ms)
    })
    .await
    .map_err(|_| AppError::Internal("the mode change did not finish".into()))??;
    announce_if_moved(&app, &change);
    Ok(change)
}

/// A mode edit that moved plans changes plan data like any plan edit.
fn announce_if_moved(app: &AppHandle, change: &ModeChange) {
    if change.moved_plans() {
        announce_plans_changed(app);
    }
}
