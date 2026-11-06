//! Wiki commands. Anything that may call the model is async and runs on the blocking pool with
//! the database lock released; nothing here writes a page except `apply_wiki_ingest`.

use std::sync::Arc;

use tauri::State;

use super::AppState;
use crate::error::{AppError, AppResult};
use crate::store::wiki::{PageRow, SourceRow};
use crate::wiki::index::ReindexReport;
use crate::wiki::ingest::SourceKind;
use crate::wiki::service::{ApplyReport, AskOutcome, IngestView, PageContent, SubjectView, WikiService};

async fn blocking<T, F>(state: &State<'_, AppState>, job: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce(Arc<WikiService>) -> AppResult<T> + Send + 'static,
{
    let wiki = Arc::clone(&state.wiki);
    tauri::async_runtime::spawn_blocking(move || job(wiki))
        .await
        .map_err(|_| AppError::Internal("the wiki job did not finish".into()))?
}

#[tauri::command]
pub fn list_wiki_subjects(state: State<'_, AppState>, include_archived: Option<bool>) -> AppResult<Vec<SubjectView>> {
    state.wiki.list_subjects(include_archived.unwrap_or(false))
}

#[tauri::command]
pub fn create_wiki_subject(state: State<'_, AppState>, name: String) -> AppResult<SubjectView> {
    state.wiki.create_subject(&name)
}

#[tauri::command]
pub fn ensure_wiki_subject(state: State<'_, AppState>, name: String) -> AppResult<SubjectView> {
    state.wiki.ensure_subject(&name)
}

#[tauri::command]
pub fn set_wiki_subject_archived(state: State<'_, AppState>, id: i64, archived: bool) -> AppResult<SubjectView> {
    state.wiki.set_archived(id, archived)
}

#[tauri::command]
pub fn wiki_subject_for_plan(state: State<'_, AppState>, plan_subject: String) -> AppResult<Option<SubjectView>> {
    state.wiki.subject_for_plan(&plan_subject)
}

#[tauri::command]
pub fn list_wiki_pages(state: State<'_, AppState>, subject_id: i64) -> AppResult<Vec<PageRow>> {
    state.wiki.list_pages(subject_id)
}

#[tauri::command]
pub fn list_wiki_sources(state: State<'_, AppState>, subject_id: i64) -> AppResult<Vec<SourceRow>> {
    state.wiki.list_sources(subject_id)
}

#[tauri::command]
pub fn read_wiki_page(state: State<'_, AppState>, subject_id: i64, page_name: String) -> AppResult<PageContent> {
    state.wiki.read_page(subject_id, &page_name)
}

#[tauri::command]
pub fn reindex_wiki(state: State<'_, AppState>, subject_id: i64) -> AppResult<ReindexReport> {
    state.wiki.reindex(subject_id)
}

#[tauri::command]
pub async fn ingest_wiki_text(state: State<'_, AppState>, subject_id: i64, kind: SourceKind, label: String, text: String) -> AppResult<IngestView> {
    blocking(&state, move |wiki| wiki.ingest_text(subject_id, kind, &label, &text)).await
}

#[tauri::command]
pub async fn ingest_wiki_file(state: State<'_, AppState>, subject_id: i64, path: String) -> AppResult<IngestView> {
    blocking(&state, move |wiki| wiki.ingest_file(subject_id, &path)).await
}

#[tauri::command]
pub async fn ingest_wiki_folder(state: State<'_, AppState>, subject_id: i64, path: String) -> AppResult<IngestView> {
    blocking(&state, move |wiki| wiki.ingest_folder(subject_id, &path)).await
}

#[tauri::command]
pub async fn draft_wiki_outline(state: State<'_, AppState>, subject_id: i64) -> AppResult<IngestView> {
    blocking(&state, move |wiki| wiki.outline(subject_id)).await
}

#[tauri::command]
pub async fn resume_wiki_ingest(state: State<'_, AppState>, preview_id: String) -> AppResult<IngestView> {
    blocking(&state, move |wiki| wiki.resume(&preview_id)).await
}

#[tauri::command]
pub fn cancel_wiki_ingest(state: State<'_, AppState>, preview_id: String) {
    state.wiki.cancel(&preview_id);
}

#[tauri::command]
pub fn apply_wiki_ingest(state: State<'_, AppState>, preview_id: String, selected: Option<Vec<String>>) -> AppResult<ApplyReport> {
    state.wiki.apply_ingest(&preview_id, selected)
}

#[tauri::command]
pub async fn ask_wiki(state: State<'_, AppState>, subject_id: i64, question: String) -> AppResult<AskOutcome> {
    blocking(&state, move |wiki| wiki.ask(subject_id, &question)).await
}

#[tauri::command]
pub fn preview_wiki_answer_page(state: State<'_, AppState>, ask_id: String) -> AppResult<IngestView> {
    state.wiki.save_answer_preview(&ask_id)
}
