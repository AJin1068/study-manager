//! Dashboard commands: read-only aggregates computed in the core (the UI never reads the database).

use chrono::Local;
use tauri::State;

use super::{AppState, lock_db};
use crate::clock::now_ms;
use crate::dashboard::{self, TodayDashboard, Weakness, WeekDashboard};
use crate::error::AppResult;

#[tauri::command]
pub fn get_dashboard_today(state: State<'_, AppState>) -> AppResult<TodayDashboard> {
    let mut conn = lock_db(&state.db)?;
    dashboard::today(&mut conn, &Local, now_ms()?)
}

#[tauri::command]
pub fn get_dashboard_week(state: State<'_, AppState>) -> AppResult<WeekDashboard> {
    let conn = lock_db(&state.db)?;
    dashboard::week(&conn, &Local, now_ms()?)
}

#[tauri::command]
pub fn get_weakness(state: State<'_, AppState>) -> AppResult<Weakness> {
    let conn = lock_db(&state.db)?;
    dashboard::weakness(&conn, &Local, now_ms()?)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::dashboard::{
        AppCount, AppOffStat, DayStat, NextPlan, PhoneStat, PostponedPlan, ReleaseUsage,
        SubjectTime, TodayDashboard, Weakness, WeekDashboard,
    };
    use crate::points;

    #[test]
    fn dashboard_wire_shapes_match_the_shared_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/api/fixtures/dashboard-contract.json"
        );
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let day = DayStat {
            date: "2026-11-05".into(),
            weekday: 3,
            planned_ms: 12_000_000,
            actual_ms: 9_600_000,
            achievement_pct: Some(80),
            achieved: true,
            focus_score: Some(92),
        };
        let mut heatmap = vec![vec![0_i64; 24]; 7];
        heatmap[3][20] = 2;
        let actual = json!({
            "today": TodayDashboard {
                date: "2026-11-05".into(),
                next_plan: Some(NextPlan {
                    plan_id: 4,
                    name: "수학".into(),
                    subject: "수학".into(),
                    planned_start_ms: 1_773_000_000_000,
                    starts_in_ms: 3_600_000,
                }),
                planned_ms: 12_000_000,
                actual_ms: 9_600_000,
                achievement_pct: Some(80),
                streak_days: 3,
                streak_threshold_pct: 80,
                points: points::status_for(60, 90),
            },
            "week": WeekDashboard {
                week_start: "2026-11-02".into(),
                week_end: "2026-11-08".into(),
                days: vec![day],
                planned_ms: 12_000_000,
                actual_ms: 9_600_000,
                achievement_pct: Some(80),
                achieved_days: 1,
                planned_days: 1,
                by_subject: vec![SubjectTime { subject: "수학".into(), actual_ms: 9_600_000 }],
            },
            "weakness": Weakness {
                from_ms: 1_770_000_000_000,
                to_ms: 1_772_419_200_000,
                deviation_apps: vec![AppCount { app_id: "com.example.game".into(), count: 4 }],
                avg_first_away_ms: Some(600_000),
                sets_with_away: 2,
                sets_total: 5,
                heatmap,
                postponed_plans: vec![PostponedPlan { plan_id: 4, name: "수학".into(), subject: "수학".into(), count: 2 }],
                app_off: AppOffStat { sleep_ms: 100, forced_quit_ms: 200, forced_quit_count: 1, planned_overlap_ms: 50 },
                releases: ReleaseUsage { emergency_pass: 1, skip_today: 0, early_release: 2, aborted_runs: 1 },
                phone: PhoneStat {
                    unreachable_ms: 180_000,
                    gaps: 2,
                    service_disabled_gaps: 1,
                    pickup_sets_reported: 3,
                    pickup_sets_with_pickups: 2,
                    pickups_total: 7,
                },
            },
        });
        assert_eq!(actual, expected);
    }
}
