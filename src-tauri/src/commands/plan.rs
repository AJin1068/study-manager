//! Calendar plan commands. Changes go through `plan::*`, which checks the lock and writes the
//! history in one transaction; this layer only supplies the local time zone and the clock.

use chrono::Local;
use tauri::{AppHandle, Emitter, Manager, State};

use super::{lock_db, AppState, DEFAULT_LIMIT};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::timer::engine::WakeReason;
use crate::plan_import::{self, IcsApplyResult, IcsExport, IcsPreview};
use crate::plan_shift;
use crate::plan::{
    self, CreatePlanInput, DeletePlanInput, Occurrence, PlanChange, PlanHistoryEntry, PlanStats, UpdatePlanInput,
};

pub const PLANS_CHANGED_EVENT: &str = "plans_changed";

/// Tells the UI to refetch and the timer engine to reconcile after plan data changed.
pub fn announce_plans_changed(app: &AppHandle) {
    if let Err(err) = app.emit(PLANS_CHANGED_EVENT, ()) {
        tracing::error!(error = %err, "failed to emit plans_changed event");
    }
    app.state::<AppState>().engine.wake(WakeReason::PlanChanged);
}

/// Rejected attempts change no plan data, so they announce nothing.
fn notify(app: &AppHandle, change: &PlanChange) {
    if change.is_applied() {
        announce_plans_changed(app);
    }
}

#[tauri::command]
pub fn create_plan(app: AppHandle, state: State<'_, AppState>, input: CreatePlanInput) -> AppResult<PlanChange> {
    let change = {
        let mut conn = lock_db(&state.db)?;
        plan::create_plan(&mut conn, &Local, input, now_ms()?)?
    };
    notify(&app, &change);
    Ok(change)
}

#[tauri::command]
pub fn update_plan(app: AppHandle, state: State<'_, AppState>, input: UpdatePlanInput) -> AppResult<PlanChange> {
    let change = {
        let mut conn = lock_db(&state.db)?;
        plan::update_plan(&mut conn, &Local, input, now_ms()?)?
    };
    notify(&app, &change);
    Ok(change)
}

#[tauri::command]
pub fn delete_plan(app: AppHandle, state: State<'_, AppState>, input: DeletePlanInput) -> AppResult<PlanChange> {
    let change = {
        let mut conn = lock_db(&state.db)?;
        plan::delete_plan(&mut conn, input, now_ms()?)?
    };
    notify(&app, &change);
    Ok(change)
}

/// Reverts one mode-shift batch (the toast's undo button): plans and the mode that caused them.
#[tauri::command]
pub fn undo_plan_batch(app: AppHandle, state: State<'_, AppState>, batch_id: String) -> AppResult<PlanChange> {
    let change = {
        let mut conn = lock_db(&state.db)?;
        plan_shift::undo_plan_batch(&mut conn, &Local, &batch_id, now_ms()?)?
    };
    notify(&app, &change);
    Ok(change)
}

/// Occurrences with `from <= date <= to` (both `YYYY-MM-DD`, at most 400 days apart).
#[tauri::command]
pub fn list_occurrences(state: State<'_, AppState>, from: String, to: String) -> AppResult<Vec<Occurrence>> {
    let conn = lock_db(&state.db)?;
    plan::list_occurrences(&conn, &Local, &from, &to)
}

/// With a plan id: the timeline of that plan's whole series. Without: everything, newest first.
#[tauri::command]
pub fn get_plan_history(
    state: State<'_, AppState>,
    plan_id: Option<i64>,
    limit: Option<i64>,
) -> AppResult<Vec<PlanHistoryEntry>> {
    let conn = lock_db(&state.db)?;
    plan::plan_history(&conn, plan_id, limit.unwrap_or(DEFAULT_LIMIT))
}

#[tauri::command]
pub fn get_plan_stats(state: State<'_, AppState>, month: String) -> AppResult<PlanStats> {
    let conn = lock_db(&state.db)?;
    plan::plan_stats(&conn, &Local, &month)
}

// ---- calendar file import / export (off until the setting is on) ----

#[tauri::command]
pub fn get_calendar_io_enabled(state: State<'_, AppState>) -> AppResult<bool> {
    let conn = lock_db(&state.db)?;
    plan_import::calendar_io_enabled(&conn)
}

/// The `.ics` text for the UI to save through the user's own save dialog.
#[tauri::command]
pub fn export_plans_ics(state: State<'_, AppState>) -> AppResult<IcsExport> {
    let conn = lock_db(&state.db)?;
    plan_import::export_ics(&conn, now_ms()?)
}

/// What `apply_ics_import` would create from this file text. Changes nothing.
#[tauri::command]
pub fn preview_ics_import(state: State<'_, AppState>, content: String) -> AppResult<IcsPreview> {
    let conn = lock_db(&state.db)?;
    plan_import::preview_ics_import(&conn, &Local, &content, now_ms()?)
}

#[tauri::command]
pub fn apply_ics_import(app: AppHandle, state: State<'_, AppState>, content: String) -> AppResult<IcsApplyResult> {
    let result = {
        let mut conn = lock_db(&state.db)?;
        plan_import::apply_ics_import(&mut conn, &Local, &content, now_ms()?)?
    };
    if result.imported > 0 {
        announce_plans_changed(&app);
    }
    Ok(result)
}

#[tauri::command]
pub fn undo_ics_import(app: AppHandle, state: State<'_, AppState>, batch_id: String) -> AppResult<PlanChange> {
    let change = {
        let mut conn = lock_db(&state.db)?;
        plan_import::undo_ics_import(&mut conn, &batch_id, now_ms()?)?
    };
    notify(&app, &change);
    Ok(change)
}
