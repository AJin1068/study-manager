//! Today's topic panel and next-topic assignment. Everything here is local: no network, no model.

use chrono::Local;
use tauri::State;

use super::{lock_db, AppState};
use crate::error::AppResult;
use crate::wiki::topic::{self, AssignReport, AssignmentView, TodayTopic};

#[tauri::command]
pub fn get_today_topic(state: State<'_, AppState>) -> AppResult<TodayTopic> {
    let now = state.high_water.observe(crate::clock::now_ms()?);
    topic::today_topic(&*lock_db(&state.db)?, &Local, now)
}

/// Picks pages for the coming plans from the wiki gaps and the user's focus by hour.
#[tauri::command]
pub fn assign_next_topics(state: State<'_, AppState>) -> AppResult<AssignReport> {
    let now = state.high_water.observe(crate::clock::now_ms()?);
    topic::assign_next(&*lock_db(&state.db)?, &Local, now)
}

/// The user chooses the page; refused while the lock is held.
#[tauri::command]
pub fn set_topic_assignment(state: State<'_, AppState>, plan_id: i64, date: String, page_id: i64) -> AppResult<()> {
    topic::set_manual(&*lock_db(&state.db)?, plan_id, &date, page_id, crate::clock::now_ms()?)
}

#[tauri::command]
pub fn list_topic_assignments(state: State<'_, AppState>, from: String, to: String) -> AppResult<Vec<AssignmentView>> {
    topic::list_assignments(&*lock_db(&state.db)?, &from, &to)
}

/// Every assignment ever made for one occurrence, newest first.
#[tauri::command]
pub fn get_topic_history(state: State<'_, AppState>, plan_id: i64, date: String) -> AppResult<Vec<AssignmentView>> {
    topic::assignment_history(&*lock_db(&state.db)?, plan_id, &date)
}
