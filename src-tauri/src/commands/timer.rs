//! Timer state commands. The engine thread owns the automatic transitions; these commands only
//! read a derived snapshot. Releases (pass, skip, early release) are in `commands::release`.

use chrono::Local;
use tauri::State;

use super::{lock_db, AppState};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::timer::engine::HealthSnapshot;
use crate::timer::model::TimerState;
use crate::timer::{self};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Read-only derived snapshot: current phase, remaining time, and the next plan.
#[tauri::command]
pub fn get_timer_state(state: State<'_, AppState>) -> AppResult<TimerState> {
    let conn = lock_db(&state.db)?;
    timer::current_state(&conn, &Local, state.high_water.observe(now_ms()?))
}

#[derive(serde::Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EngineHealthView {
    #[serde(flatten)]
    pub health: HealthSnapshot,
    /// Plan time of the last 24 hours that passed unobserved and unexplained by any downtime.
    pub clock_jump_ms: i64,
}

#[tauri::command]
pub fn get_engine_health(state: State<'_, AppState>) -> AppResult<EngineHealthView> {
    let now = state.high_water.get();
    let conn = lock_db(&state.db)?;
    Ok(EngineHealthView { health: state.health.snapshot(), clock_jump_ms: timer::clock_jump_ms(&conn, now - DAY_MS, now)? })
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::timer::model::{NextPlan, Phase};
    use crate::timer::notice::BreakEndingSoon;
    use crate::timer::summary::{RunSummary, SetSummary};

    #[test]
    fn timer_wire_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/timer-contract.json");
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let idle = TimerState {
            phase: Phase::Idle,
            run_id: None,
            segment_id: None,
            set_index: 0,
            sets_total: 0,
            planned_end_ms: None,
            lock_held: false,
            now_ms: 1_773_000_000_000,
            next: Some(NextPlan {
                plan_id: 7,
                name: "수학 문제풀이".into(),
                subject: "수학".into(),
                date: "2026-03-09".into(),
                sets: 3,
                planned_start_ms: 1_773_054_000_000,
                planned_end_ms: 1_773_064_800_000,
            }),
        };
        let long_break = TimerState {
            phase: Phase::LongBreak,
            run_id: Some(4),
            segment_id: Some(19),
            set_index: 4,
            sets_total: 5,
            planned_end_ms: Some(1_773_057_000_000),
            lock_held: true,
            now_ms: 1_773_056_000_000,
            next: None,
        };
        let actual = json!({
            "idleState": idle,
            "breakState": long_break,
            "health": EngineHealthView {
                health: HealthSnapshot { degraded: true, consecutive_failures: 5, restarts: 2 },
                clock_jump_ms: 60_000,
            },
            "breakEndingSoon": BreakEndingSoon { run_id: 4, segment_id: 19, planned_end_ms: 1_773_057_000_000, remaining_ms: 60_000 },
            "setSummary": SetSummary { run_id: 4, set_index: 2, set_studied_ms: 3_000_000, today_studied_ms: 6_000_000, attempt_count: 3, focus_credited_ms: 2_700_000, focus_score: Some(90), first_away_after_ms: Some(600_000), return_after_ms: None },
            "runSummary": RunSummary {
                run_id: 4,
                sets_total: 5,
                run_studied_ms: 12_000_000,
                today_studied_ms: 12_000_000,
                attempt_count: 5,
                focus_credited_ms: 10_800_000,
                focus_score: Some(90),
                status: "completed".into(),
            },
        });
        assert_eq!(actual, expected);
    }
}
