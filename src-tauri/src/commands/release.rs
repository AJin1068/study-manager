//! Release procedures: emergency pass, skip today, early release. The challenge and the exact
//! match live in `timer::release`; the effects and the append-only record in `timer::study_pass`.
//! There is deliberately no command that changes `study_state` directly.

use chrono::Local;
use tauri::{AppHandle, Emitter, State};

use super::{lock_db, AppState};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::timer::engine::TIMER_CHANGED_EVENT;
use crate::timer::release::{ReleaseChallenge, ReleaseOutcome};
use crate::timer::study_pass::{self, PassKind, PassRecord, PassStatus};
use crate::timer::{self};

pub const PASS_CHANGED_EVENT: &str = "pass_changed";
const RECENT_PASSES: i64 = 100;

/// Starts a release: returns the sentence the user must type. Refused when the release could not
/// be granted right now (no lock to release, daily limit spent, a pass already running).
#[tauri::command]
pub fn begin_release(state: State<'_, AppState>, kind: PassKind) -> AppResult<ReleaseChallenge> {
    let effective = state.high_water.observe(now_ms()?);
    let conn = lock_db(&state.db)?;
    state.release.begin(&conn, &Local, kind, effective)
}

/// Applies the release if `typed` matches the issued sentence exactly. Works without the engine
/// thread; on success the UI is told directly.
#[tauri::command]
pub fn submit_release(
    app: AppHandle,
    state: State<'_, AppState>,
    challenge_id: u64,
    typed: String,
) -> AppResult<ReleaseOutcome> {
    let effective = state.high_water.observe(now_ms()?);
    let (outcome, snapshot) = {
        let mut conn = lock_db(&state.db)?;
        let outcome = state.release.submit(&mut conn, &Local, challenge_id, &typed, effective)?;
        let snapshot = if matches!(outcome, ReleaseOutcome::Released { .. }) {
            Some((timer::current_state(&conn, &Local, effective)?, study_pass::status(&conn, &Local, effective)?))
        } else {
            None
        };
        (outcome, snapshot)
    };
    if let Some((timer_state, pass_status)) = snapshot {
        if let Err(err) = app.emit(TIMER_CHANGED_EVENT, timer_state) {
            tracing::error!(error = %err, "failed to emit timer_changed after release");
        }
        if let Err(err) = app.emit(PASS_CHANGED_EVENT, pass_status) {
            tracing::error!(error = %err, "failed to emit pass_changed");
        }
    }
    Ok(outcome)
}

/// Today's emergency-pass allowance and the running pass, if any.
#[tauri::command]
pub fn get_pass_status(state: State<'_, AppState>) -> AppResult<PassStatus> {
    let effective = state.high_water.observe(now_ms()?);
    let conn = lock_db(&state.db)?;
    study_pass::status(&conn, &Local, effective)
}

/// Every recorded release, newest first (history and statistics).
#[tauri::command]
pub fn list_passes(state: State<'_, AppState>) -> AppResult<Vec<PassRecord>> {
    let conn = lock_db(&state.db)?;
    study_pass::list_passes(&conn, RECENT_PASSES)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::timer::study_pass::PassKind;

    #[test]
    fn release_wire_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/release-contract.json");
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let actual = json!({
            "challenge": ReleaseChallenge { challenge_id: 3, kind: PassKind::EmergencyPass, sentence: "예시 문장".into() },
            "outcomes": [
                ReleaseOutcome::Released { kind: PassKind::EmergencyPass, expires_at_ms: Some(1_773_057_000_000) },
                ReleaseOutcome::Released { kind: PassKind::SkipToday, expires_at_ms: None },
                ReleaseOutcome::Mismatch { attempts: 2 },
                ReleaseOutcome::NoChallenge,
                ReleaseOutcome::NothingToRelease,
                ReleaseOutcome::LimitReached { limit: 1 },
                ReleaseOutcome::AlreadyActive { expires_at_ms: 1_773_057_000_000 },
            ],
            "passStatus": PassStatus {
                daily_limit: 1,
                used_today: 1,
                remaining_today: 0,
                active_until_ms: Some(1_773_057_000_000),
                duration_min: 10,
                penalty_points: 5,
            },
            "record": PassRecord {
                id: 4,
                at_ms: 1_773_056_400_000,
                date: "2026-03-09".into(),
                kind: "emergency_pass".into(),
                run_id: Some(2),
                expires_at_ms: Some(1_773_057_000_000),
                challenge_len: 95,
                typing_ms: 61_000,
                penalty_points: Some(5),
            },
        });
        assert_eq!(actual, expected);
    }
}
