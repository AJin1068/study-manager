//! Quiz commands: the question bank, review cards, and the quiz settings. The early-release proof
//! has its own commands in `release`. Answers are graded in Rust; the UI never receives the
//! answer of a question it has not answered.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use super::{AppState, lock_db};
use crate::clock::now_ms;
use crate::error::{AppError, AppResult};
use crate::quiz::config::{QuizConfig, QuizSettingKey, update_setting};
use crate::quiz::gate::{CARD_EVENT, CardResult, QuizCard};
use crate::quiz::service::{GenerateReport, QuizService};
use crate::settings::{SettingValue, UpdateOutcome};
use crate::store::quiz::BankStats;

/// Shows a card in the window. Failing to emit only loses the pop-up.
pub fn emit_card(app: &AppHandle, card: QuizCard) {
    if let Err(err) = app.emit(CARD_EVENT, card) {
        tracing::error!(error = %err, "failed to emit quiz card");
    }
}

#[tauri::command]
pub fn get_quiz_settings(state: State<'_, AppState>) -> AppResult<QuizConfig> {
    let conn = lock_db(&state.db)?;
    QuizConfig::load(&conn)
}

#[tauri::command]
pub fn update_quiz_setting(
    state: State<'_, AppState>,
    key: QuizSettingKey,
    value: SettingValue,
) -> AppResult<UpdateOutcome> {
    let mut conn = lock_db(&state.db)?;
    update_setting(&mut conn, key, value, now_ms()?)
}

#[tauri::command]
pub fn get_quiz_stats(state: State<'_, AppState>) -> AppResult<BankStats> {
    state.quiz.stats()
}

/// Writes questions for the subject's pages that have none. Async: it may call the model.
#[tauri::command]
pub async fn generate_quiz_bank(
    state: State<'_, AppState>,
    subject_id: i64,
) -> AppResult<GenerateReport> {
    let quiz: Arc<QuizService> = Arc::clone(&state.quiz);
    tauri::async_runtime::spawn_blocking(move || quiz.generate_for_subject(subject_id))
        .await
        .map_err(|_| AppError::Internal("the quiz job did not finish".into()))?
}

#[tauri::command]
pub fn flag_quiz_item(state: State<'_, AppState>, item_id: i64, reason: String) -> AppResult<()> {
    state.quiz.flag(item_id, &reason)
}

/// Practice questions on request.
#[tauri::command]
pub fn start_review_quiz(
    state: State<'_, AppState>,
    subject_id: Option<i64>,
    count: Option<u32>,
) -> AppResult<Option<QuizCard>> {
    state
        .quiz
        .review_card(subject_id, usize::try_from(count.unwrap_or(1)).unwrap_or(1))
}

/// The quiz of the run that just finished, if any (once per run).
#[tauri::command]
pub fn get_session_end_quiz(state: State<'_, AppState>) -> AppResult<Option<QuizCard>> {
    state.quiz.session_end_card()
}

/// The question that goes with the check-in being shown. `None` unless a check-in is pending,
/// the option is on, and a locked study block is running.
#[tauri::command]
pub fn get_checkin_quiz(state: State<'_, AppState>) -> AppResult<Option<QuizCard>> {
    let effective = state.high_water.observe(now_ms()?);
    if state.signals.pending(effective)?.is_none() {
        return Ok(None);
    }
    state.quiz.checkin_card()
}

/// Grades a card. `skip` is for the session-end quiz (recorded) or dismisses any other card.
#[tauri::command]
pub fn answer_quiz_card(
    state: State<'_, AppState>,
    card_id: u64,
    answers: Vec<String>,
    skip: bool,
) -> AppResult<Option<CardResult>> {
    state.quiz.answer_card(card_id, &answers, skip)
}
