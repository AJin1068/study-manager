//! Downtime record commands.

use tauri::State;

use super::{lock_db, AppState, DEFAULT_LIMIT};
use crate::downtime::{self, DowntimeRecord, DowntimeSummary};
use crate::error::AppResult;
use crate::store;

#[tauri::command]
pub fn get_downtime_records(state: State<'_, AppState>, limit: Option<i64>) -> AppResult<Vec<DowntimeRecord>> {
    let conn = lock_db(&state.db)?;
    store::list_downtime(&conn, limit.unwrap_or(DEFAULT_LIMIT))
}

/// Sleep and forced-quit time inside `[from_ms, to_ms)` for the statistics screen.
#[tauri::command]
pub fn get_downtime_summary(state: State<'_, AppState>, from_ms: i64, to_ms: i64) -> AppResult<DowntimeSummary> {
    let conn = lock_db(&state.db)?;
    let records = store::list_downtime(&conn, i64::MAX)?;
    Ok(downtime::summarize(&records, from_ms, to_ms))
}
