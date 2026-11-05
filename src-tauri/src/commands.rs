//! Tauri commands: the only surface the UI uses. Argument and return types here are mirrored
//! in `src/api/commands.ts`; the shared fixture test keeps the two in sync.

use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;
use tauri::State;

use crate::clock::now_ms;
use crate::downtime::DowntimeRecord;
use crate::error::{AppError, AppResult};
use crate::platform::AutostartBackend;
use crate::settings::{self, GeneralSettings, HistoryEntry, Outcome, Reason, SettingKey, UpdateOutcome};
use crate::store;
use crate::study::{self, ModeSnapshot, StudyLevel, StudyState};

const DEFAULT_LIMIT: i64 = 200;

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

pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub session_id: i64,
    pub autostart: Arc<dyn AutostartBackend>,
    pub power: PowerSourceStatus,
}

pub fn lock_db(db: &Mutex<Connection>) -> AppResult<MutexGuard<'_, Connection>> {
    db.lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))
}

/// Local calendar date used as the daily-mode key. Local time is used only for the day
/// boundary; stored timestamps stay in UTC epoch milliseconds.
pub fn today_key() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppResult<GeneralSettings> {
    let conn = lock_db(&state.db)?;
    settings::general_settings(&conn)
}

#[tauri::command]
pub fn update_setting(state: State<'_, AppState>, key: SettingKey, value: bool) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    settings::update_setting(&mut conn, state.autostart.as_ref(), key, value, now_ms()?)
}

#[tauri::command]
pub fn get_setting_history(state: State<'_, AppState>, limit: Option<i64>) -> AppResult<Vec<HistoryEntry>> {
    let conn = lock_db(&state.db)?;
    settings::read_history(&conn, limit.unwrap_or(DEFAULT_LIMIT))
}

#[tauri::command]
pub fn get_study_state(state: State<'_, AppState>) -> AppResult<StudyState> {
    let conn = lock_db(&state.db)?;
    study::read_state(&conn)
}

#[tauri::command]
pub fn set_study_state_test(state: State<'_, AppState>, studying: bool) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    study::set_study_state_test(&mut conn, studying, now_ms()?)
}

#[tauri::command]
pub fn get_mode_config(state: State<'_, AppState>) -> AppResult<ModeSnapshot> {
    let conn = lock_db(&state.db)?;
    study::mode_snapshot(&conn, &today_key())
}

#[tauri::command]
pub fn set_daily_mode(state: State<'_, AppState>, level: StudyLevel) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    study::set_daily_mode(&mut conn, &today_key(), level, now_ms()?)
}

#[tauri::command]
pub fn get_downtime_records(state: State<'_, AppState>, limit: Option<i64>) -> AppResult<Vec<DowntimeRecord>> {
    let conn = lock_db(&state.db)?;
    store::list_downtime(&conn, limit.unwrap_or(DEFAULT_LIMIT))
}

#[tauri::command]
pub fn get_autostart_status(state: State<'_, AppState>) -> AppResult<AutostartStatus> {
    Ok(AutostartStatus { enabled: state.autostart.is_enabled()? })
}

#[tauri::command]
pub fn get_power_status(state: State<'_, AppState>) -> PowerSourceStatus {
    state.power.clone()
}

/// Exit gate shared by every quit path (tray "종료", Cmd+Q, app.exit).
/// Returns `Ok(true)` when the exit may proceed. While studying it records an `appExit`
/// `rejected_locked` attempt and returns `Ok(false)`.
pub fn allow_exit(state: &AppState) -> AppResult<bool> {
    let mut conn = lock_db(&state.db)?;
    exit_decision(&mut conn, state.session_id, now_ms()?)
}

/// Decision and its side effects in one Immediate transaction, so the lock check and the
/// clean-exit mark cannot interleave with a concurrent study-state change.
fn exit_decision(conn: &mut Connection, session_id: i64, at_ms: i64) -> AppResult<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if study::is_studying(&tx)? {
        store::append_history(&tx, at_ms, "appExit", None, "exit", Reason::User, Outcome::RejectedLocked)?;
        tx.commit()?;
        return Ok(false);
    }
    store::set_clean_exit(&tx, session_id, true, at_ms)?;
    tx.commit()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn exit_is_allowed_outside_study_and_marks_clean_exit() {
        let mut conn = open_in_memory().expect("db");
        let session = store::insert_session(&conn, 1).expect("session");
        assert!(exit_decision(&mut conn, session, 2).expect("exit"));
        let clean: Option<i64> = conn
            .query_row("SELECT clean_exit FROM sessions WHERE id = ?1", [session], |r| r.get(0))
            .expect("row");
        assert_eq!(clean, Some(1));
    }

    #[test]
    fn exit_is_refused_when_lock_is_unknown_and_session_stays_open() {
        let mut conn = open_in_memory().expect("db");
        let session = store::insert_session(&conn, 1).expect("session");
        conn.execute("DELETE FROM study_state", []).expect("delete");
        assert!(exit_decision(&mut conn, session, 2).is_err());
        let clean: Option<i64> = conn
            .query_row("SELECT clean_exit FROM sessions WHERE id = ?1", [session], |r| r.get(0))
            .expect("row");
        assert_eq!(clean, None, "an unknown lock must not mark the session clean");
    }

    #[test]
    fn exit_is_refused_while_studying_and_records_attempt() {
        let mut conn = open_in_memory().expect("db");
        let session = store::insert_session(&conn, 1).expect("session");
        study::set_study_state_test(&mut conn, true, 2).expect("on");
        assert!(!exit_decision(&mut conn, session, 3).expect("exit"));
        let clean: Option<i64> = conn
            .query_row("SELECT clean_exit FROM sessions WHERE id = ?1", [session], |r| r.get(0))
            .expect("row");
        assert_eq!(clean, None, "a refused exit must not mark the session clean");
        let latest = &settings::read_history(&conn, 1).expect("history")[0];
        assert_eq!(latest.key, "appExit");
        assert_eq!(latest.outcome, "rejected_locked");
    }
}
