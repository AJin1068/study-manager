//! Retrospective merge and wiki lint commands. Anything that may call the model is async and runs
//! on the blocking pool; the service never holds the database lock while it waits for the model.

use std::sync::Arc;

use tauri::State;

use super::{lock_db, AppState};
use crate::clock::now_ms;
use crate::error::{AppError, AppResult};
use crate::settings::UpdateOutcome;
use crate::wiki::retro_service::{self, LintReport, RetroOutcome, RetroService, RetroView};

type Service<'a> = State<'a, Arc<RetroService>>;

async fn blocking<T, F>(service: &Service<'_>, job: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce(Arc<RetroService>) -> AppResult<T> + Send + 'static,
{
    let service = Arc::clone(service);
    tauri::async_runtime::spawn_blocking(move || job(service))
        .await
        .map_err(|_| AppError::Internal("the retrospective job did not finish".into()))?
}

/// Saves the note of a finished run and prepares its merge into the wiki.
#[tauri::command]
pub async fn submit_retro(service: Service<'_>, run_id: i64, subject_id: i64, page_name: Option<String>, note: String) -> AppResult<RetroOutcome> {
    blocking(&service, move |s| s.submit(run_id, subject_id, page_name.as_deref(), &note)).await
}

#[tauri::command]
pub async fn retry_retro(service: Service<'_>, run_id: i64) -> AppResult<RetroOutcome> {
    blocking(&service, move |s| s.retry(run_id)).await
}

#[tauri::command]
pub fn apply_retro(service: Service<'_>, preview_id: String) -> AppResult<RetroOutcome> {
    service.apply(&preview_id)
}

#[tauri::command]
pub fn cancel_retro(service: Service<'_>, preview_id: String) {
    service.cancel(&preview_id);
}

#[tauri::command]
pub fn undo_retro(service: Service<'_>, run_id: i64) -> AppResult<RetroView> {
    service.undo(run_id)
}

#[tauri::command]
pub fn skip_retro(service: Service<'_>, run_id: i64) -> AppResult<()> {
    service.skip(run_id)
}

#[tauri::command]
pub fn get_retro(service: Service<'_>, run_id: i64) -> AppResult<Option<RetroView>> {
    service.get(run_id)
}

#[tauri::command]
pub fn list_open_retros(service: Service<'_>) -> AppResult<Vec<RetroView>> {
    service.list_open()
}

#[tauri::command]
pub async fn lint_wiki(service: Service<'_>, subject_id: i64) -> AppResult<LintReport> {
    blocking(&service, move |s| s.lint(subject_id)).await
}

#[tauri::command]
pub fn get_retro_auto_apply(state: State<'_, AppState>) -> AppResult<bool> {
    retro_service::auto_apply(&*lock_db(&state.db)?)
}

#[tauri::command]
pub fn set_retro_auto_apply(state: State<'_, AppState>, value: bool) -> AppResult<UpdateOutcome> {
    retro_service::set_auto_apply(&mut *lock_db(&state.db)?, value, now_ms()?)
}
