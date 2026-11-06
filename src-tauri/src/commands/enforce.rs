//! App-blocking commands: the block list, the enforcement mode, and what was done lately.

use chrono::Local;
use tauri::State;

use super::{lock_db, AppState};
use crate::clock::now_ms;
use crate::enforce::{self, model::BlockEntry, model::EnforceMode, model::EntryKind, EnforcementStatus, RunningApp};
use crate::error::{AppError, AppResult};
use crate::settings::UpdateOutcome;
use crate::store;
use crate::timer;

#[tauri::command]
pub fn list_block_entries(state: State<'_, AppState>) -> AppResult<Vec<BlockEntry>> {
    let conn = lock_db(&state.db)?;
    Ok(store::list_block_entries(&conn)?.into_iter().filter(|e| e.kind == EntryKind::App.as_str()).collect())
}

#[tauri::command]
pub fn add_block_entry(state: State<'_, AppState>, pattern: String, category: Option<String>) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    enforce::add_entry(&mut conn, &pattern, category.as_deref(), now_ms()?)
}

#[tauri::command]
pub fn remove_block_entry(state: State<'_, AppState>, id: i64) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    enforce::remove_entry(&mut conn, id, now_ms()?)
}

#[tauri::command]
pub fn get_enforcement_status(state: State<'_, AppState>) -> AppResult<EnforcementStatus> {
    let conn = lock_db(&state.db)?;
    let timer_state = timer::current_state(&conn, &Local, state.high_water.observe(now_ms()?))?;
    enforce::status(&conn, &timer_state)
}

/// Apps currently running, for picking block-list entries. Reports `Unsupported` on targets
/// without process control.
#[tauri::command]
pub fn list_running_apps(state: State<'_, AppState>) -> AppResult<Vec<RunningApp>> {
    enforce::running_apps(state.process.as_ref())
}

/// `mode` is `dryRun` or `enforce`; `confirmation` must equal the sentence from
/// `get_enforcement_status` for `enforce` and is ignored for `dryRun`. It is never stored.
#[tauri::command]
pub fn set_enforcement_mode(state: State<'_, AppState>, mode: String, confirmation: String) -> AppResult<UpdateOutcome> {
    let mode = EnforceMode::parse(&mode).ok_or_else(|| AppError::Invalid("unknown enforcement mode".into()))?;
    let mut conn = lock_db(&state.db)?;
    enforce::set_mode(&mut conn, mode, &confirmation, now_ms()?)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::enforce::model::BlockEvent;

    #[test]
    fn enforce_wire_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/enforce-contract.json");
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let actual = json!({
            "entry": BlockEntry {
                id: 3,
                kind: "app".into(),
                pattern: "com.example.game".into(),
                source: "user".into(),
                category: None,
                enabled: true,
                created_at_ms: 1_773_000_000_000,
            },
            "status": EnforcementStatus {
                mode: "dryRun".into(),
                grace_wait_sec: 10,
                force_quit_allowed: false,
                active: true,
                enforce_confirmation: enforce::ENFORCE_CONFIRMATION.into(),
                recent_events: vec![
                    BlockEvent {
                        id: 7,
                        at_ms: 1_773_000_001_000,
                        run_id: Some(4),
                        segment_id: Some(19),
                        target_kind: "app".into(),
                        target_id: "com.example.game".into(),
                        action: "would_terminate".into(),
                        mode: "dryRun".into(),
                        detail_code: None,
                    },
                    BlockEvent {
                        id: 8,
                        at_ms: 1_773_000_002_000,
                        run_id: None,
                        segment_id: None,
                        target_kind: "app".into(),
                        target_id: "com.example.game".into(),
                        action: "failed".into(),
                        mode: "enforce".into(),
                        detail_code: Some("os_refused".into()),
                    },
                ],
            },
            "runningApp": RunningApp { id: "com.apple.finder".into(), display_name: "Finder".into(), is_protected: true },
            "attempt": enforce::Attempt { target_id: "com.example.game".into(), action: "terminate_requested".into() },
        });
        assert_eq!(actual, expected);
    }
}
