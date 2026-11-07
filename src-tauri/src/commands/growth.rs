//! Growth view commands: local aggregates, and the weekly one-line insight (model or local rule).

use std::sync::Arc;

use tauri::State;

use super::{AppState, lock_db};
use crate::clock::now_ms;
use crate::error::{AppError, AppResult};
use crate::growth::{self, GrowthService, GrowthStatus, WeeklyInsight};
use crate::settings::UpdateOutcome;

type Service<'a> = State<'a, Arc<GrowthService>>;

#[tauri::command]
pub fn get_growth_status(service: Service<'_>) -> AppResult<GrowthStatus> {
    service.status()
}

/// May call the model, so it runs on the blocking pool.
#[tauri::command]
pub async fn get_weekly_insight(
    service: Service<'_>,
    refresh: Option<bool>,
) -> AppResult<WeeklyInsight> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || service.weekly_insight(refresh.unwrap_or(false)))
        .await
        .map_err(|_| AppError::Internal("the insight job did not finish".into()))?
}

#[tauri::command]
pub fn get_insight_subject(state: State<'_, AppState>) -> AppResult<bool> {
    growth::insight_subject_enabled(&*lock_db(&state.db)?)
}

#[tauri::command]
pub fn set_insight_subject(state: State<'_, AppState>, value: bool) -> AppResult<UpdateOutcome> {
    growth::set_insight_subject(&mut *lock_db(&state.db)?, value, now_ms()?)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::growth::{GrowthStatus, InsightSource, NextReview, WeeklyInsight};
    use crate::wiki::recommend::Reason;

    #[test]
    fn growth_wire_shapes_match_the_shared_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/api/fixtures/growth-contract.json"
        );
        let fixture: Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let status = GrowthStatus {
            total_study_ms: 3_600_000,
            level: 2,
            stage_label: "씨앗".into(),
            earned_points: 35,
            next_level_at: Some(50),
            level_progress_pct: 50,
            wiki_pages: 4,
            quiz_answered: 10,
            quiz_correct: 7,
            quiz_accuracy_pct: Some(70),
            next_review: Some(NextReview {
                subject: "수학".into(),
                page_name: "미분".into(),
                reason: Reason::LowAccuracy,
            }),
        };
        assert_eq!(
            serde_json::to_value(&status).expect("status"),
            fixture["status"]
        );
        let insight = WeeklyInsight {
            week_start: "2027-01-11".into(),
            text: "저녁 복습을 넣어 보세요.".into(),
            source: InsightSource::Rule,
            fallback_reason: Some("llm_disabled".into()),
        };
        assert_eq!(
            serde_json::to_value(&insight).expect("insight"),
            fixture["insight"]
        );
    }
}
