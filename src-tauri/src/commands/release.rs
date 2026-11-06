//! Release procedures: emergency pass, skip today, early release. The challenge and the exact
//! match live in `timer::release`; the effects and the append-only record in `timer::study_pass`.
//! There is deliberately no command that changes `study_state` directly.

use chrono::Local;
use tauri::{AppHandle, Emitter, State};

use super::{lock_db, AppState};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::quiz::release::{self as quiz_release, ReleaseQuizStatus};
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
        let snapshot = released_snapshot(&conn, &outcome, effective)?;
        (outcome, snapshot)
    };
    announce(&app, snapshot);
    wake_phone(&state, &outcome);
    Ok(outcome)
}

/// Grades the answers of an early-release quiz (one per question, in order) in Rust and applies
/// the release only if all are right. The reply says pass or fail, never how many were right.
#[tauri::command]
pub fn submit_release_quiz(
    app: AppHandle,
    state: State<'_, AppState>,
    challenge_id: u64,
    answers: Vec<String>,
) -> AppResult<ReleaseOutcome> {
    let effective = state.high_water.observe(now_ms()?);
    let (outcome, snapshot) = {
        let mut conn = lock_db(&state.db)?;
        let outcome = state.release.submit_quiz(&mut conn, &Local, challenge_id, &answers, effective)?;
        let snapshot = released_snapshot(&conn, &outcome, effective)?;
        (outcome, snapshot)
    };
    announce(&app, snapshot);
    wake_phone(&state, &outcome);
    Ok(outcome)
}

/// What the early-release card shows before it starts: quiz or typing, attempts, cooldown.
#[tauri::command]
pub fn get_release_quiz_status(state: State<'_, AppState>) -> AppResult<ReleaseQuizStatus> {
    let effective = state.high_water.observe(now_ms()?);
    let conn = lock_db(&state.db)?;
    quiz_release::status(&conn, &Local, effective)
}

/// A release is in the database: the phone is told now (a `Free` lease), not at the next tick.
fn wake_phone(state: &AppState, outcome: &ReleaseOutcome) {
    if matches!(outcome, ReleaseOutcome::Released { .. }) {
        state.phone.wake();
    }
}

type ReleasedSnapshot = Option<(crate::timer::model::TimerState, PassStatus)>;

fn released_snapshot(conn: &rusqlite::Connection, outcome: &ReleaseOutcome, effective: i64) -> AppResult<ReleasedSnapshot> {
    if matches!(outcome, ReleaseOutcome::Released { .. }) {
        Ok(Some((timer::current_state(conn, &Local, effective)?, study_pass::status(conn, &Local, effective)?)))
    } else {
        Ok(None)
    }
}

fn announce(app: &AppHandle, snapshot: ReleasedSnapshot) {
    if let Some((timer_state, pass_status)) = snapshot {
        if let Err(err) = app.emit(TIMER_CHANGED_EVENT, timer_state) {
            tracing::error!(error = %err, "failed to emit timer_changed after release");
        }
        if let Err(err) = app.emit(PASS_CHANGED_EVENT, pass_status) {
            tracing::error!(error = %err, "failed to emit pass_changed");
        }
    }
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
    use crate::quiz::{ItemKind, QuizQuestion};
    use crate::timer::release::ChallengeProof;
    use crate::timer::study_pass::PassKind;

    #[test]
    fn release_wire_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/release-contract.json");
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let actual = json!({
            "challenge": ReleaseChallenge {
                challenge_id: 3,
                kind: PassKind::EmergencyPass,
                proof: ChallengeProof::Typing,
                sentence: "예시 문장".into(),
                questions: Vec::new(),
                attempts_left: None,
            },
            "quizChallenge": ReleaseChallenge {
                challenge_id: 4,
                kind: PassKind::EarlyRelease,
                proof: ChallengeProof::Quiz,
                sentence: String::new(),
                questions: vec![
                    QuizQuestion { item_id: 7, kind: ItemKind::Mcq, prompt: "도함수는?".into(), choices: vec!["순간 변화율".into(), "넓이".into(), "평균".into(), "누적".into()] },
                    QuizQuestion { item_id: 9, kind: ItemKind::Cloze, prompt: "미분은 ____으로 계산한다".into(), choices: Vec::new() },
                ],
                attempts_left: Some(3),
            },
            "quizStatus": ReleaseQuizStatus {
                mode: "cooldown",
                required: 3,
                drawable: 12,
                attempts_used: 1,
                max_attempts: 4,
                cooldown_remaining_ms: 120_000,
                cooldown_min: 3,
            },
            "outcomes": [
                ReleaseOutcome::Released { kind: PassKind::EmergencyPass, expires_at_ms: Some(1_773_057_000_000) },
                ReleaseOutcome::Released { kind: PassKind::SkipToday, expires_at_ms: None },
                ReleaseOutcome::Mismatch { attempts: 2 },
                ReleaseOutcome::NoChallenge,
                ReleaseOutcome::NothingToRelease,
                ReleaseOutcome::LimitReached { limit: 1 },
                ReleaseOutcome::AlreadyActive { expires_at_ms: 1_773_057_000_000 },
                ReleaseOutcome::QuizFailed { retry_after_ms: 180_000, attempts_left: 3 },
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
                proof_kind: "typing".into(),
            },
        });
        assert_eq!(actual, expected);
    }
}
