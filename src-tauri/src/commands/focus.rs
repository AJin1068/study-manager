//! Focus-measurement commands: the study-app list and the first-run setup status.

use tauri::State;

use super::{lock_db, AppState};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::focus::signals::{self, CheckinPrompt, SelfReportOutcome, SelfReportStatus};
use crate::focus::{self, model::StudyApp, FocusSetup};
use crate::settings::UpdateOutcome;
use crate::store;

#[tauri::command]
pub fn list_study_apps(state: State<'_, AppState>) -> AppResult<Vec<StudyApp>> {
    store::list_study_apps(&*lock_db(&state.db)?)
}

/// `appId` is a bundle id or executable path, e.g. picked from `list_running_apps`.
#[tauri::command]
pub fn add_study_app(state: State<'_, AppState>, app_id: String) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    focus::add_study_app(&mut conn, &app_id, now_ms()?)
}

#[tauri::command]
pub fn remove_study_app(state: State<'_, AppState>, id: i64) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    focus::remove_study_app(&mut conn, id, now_ms()?)
}

#[tauri::command]
pub fn set_study_app_enabled(state: State<'_, AppState>, id: i64, enabled: bool) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    focus::set_study_app_enabled(&mut conn, id, enabled, now_ms()?)
}

#[tauri::command]
pub fn get_focus_setup(state: State<'_, AppState>) -> AppResult<FocusSetup> {
    focus::setup_status(&*lock_db(&state.db)?, state.focus_probe.as_ref())
}

/// The in-app "just zoned out" button; the global shortcut records the same thing.
#[tauri::command]
pub fn report_distraction(state: State<'_, AppState>) -> AppResult<SelfReportOutcome> {
    signals::report_now(&state.db, &state.high_water)
}

#[tauri::command]
pub fn get_self_report_status(state: State<'_, AppState>) -> AppResult<SelfReportStatus> {
    state.signals.status()
}

#[tauri::command]
pub fn get_pending_checkin(state: State<'_, AppState>) -> AppResult<Option<CheckinPrompt>> {
    state.signals.pending(state.high_water.observe(now_ms()?))
}

/// `focused: false` records a self report; `true` only acknowledges.
#[tauri::command]
pub fn answer_checkin(state: State<'_, AppState>, focused: bool) -> AppResult<SelfReportOutcome> {
    signals::answer_checkin_now(&state.db, &state.high_water, &state.signals, focused)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::focus::signals::{CheckinPrompt, IgnoreReason, SelfReportOutcome, SelfReportStatus, SELF_REPORT_SHORTCUT};
    use crate::focus::{model::StudyApp, FocusSetup};
    use crate::platform::TitleAccess;

    #[test]
    fn focus_wire_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/focus-contract.json");
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let actual = json!({
            "studyApp": StudyApp {
                id: 2,
                kind: "app".into(),
                app_id: "com.microsoft.vscode".into(),
                enabled: true,
                created_at_ms: 1_773_000_000_000,
            },
            "setup": FocusSetup {
                study_app_count: 0,
                needs_study_apps: true,
                title_access: TitleAccess::Denied,
                sensors_available: true,
            },
            "selfReportStatus": SelfReportStatus {
                shortcut: SELF_REPORT_SHORTCUT.into(),
                shortcut_registered: false,
                shortcut_error: Some("already registered".into()),
            },
            "checkinPrompt": CheckinPrompt { run_id: 3, segment_id: 5, asked_at_ms: 1_773_000_000_000 },
            "outcomes": {
                "recorded": SelfReportOutcome::Recorded,
                "acknowledged": SelfReportOutcome::Acknowledged,
                "ignored": SelfReportOutcome::Ignored { reason: IgnoreReason::NotStudying },
            },
        });
        assert_eq!(actual, expected);
    }
}
