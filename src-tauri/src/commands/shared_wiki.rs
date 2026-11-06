//! Shared-wiki commands. Calls that reach the server or the model block, so they run on the
//! blocking pool with the database lock released.

use std::sync::Arc;

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::shared_wiki::service::{ActionView, MergeView, QuizQuestionView, SharePreview, SharedRoomView, SharedWikiService, SyncView};

type Service<'a> = State<'a, Arc<SharedWikiService>>;

async fn blocking<T: Send + 'static>(
    service: &Service<'_>,
    work: impl FnOnce(&SharedWikiService) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let service = Arc::clone(service);
    tauri::async_runtime::spawn_blocking(move || work(&service))
        .await
        .map_err(|_| AppError::Internal("the shared wiki job did not finish".into()))?
}

#[tauri::command]
pub async fn shared_wiki_preview(service: Service<'_>, page_id: i64, room_id: String) -> AppResult<SharePreview> {
    blocking(&service, move |s| s.share_preview(page_id, &room_id)).await
}

/// `confirmed` is true only after the user saw the confirmation screen.
#[tauri::command]
pub async fn shared_wiki_share(service: Service<'_>, page_id: i64, room_id: String, allow_llm_merge: bool, confirmed: bool) -> AppResult<ActionView> {
    blocking(&service, move |s| s.share(page_id, &room_id, allow_llm_merge, confirmed)).await
}

#[tauri::command]
pub async fn shared_wiki_sync(service: Service<'_>, room_id: String) -> AppResult<SyncView> {
    blocking(&service, move |s| s.sync_marked(&room_id)).await
}

#[tauri::command]
pub async fn shared_wiki_withdraw(service: Service<'_>, page_id: i64, room_id: String) -> AppResult<ActionView> {
    blocking(&service, move |s| s.withdraw(page_id, &room_id)).await
}

#[tauri::command]
pub async fn shared_wiki_room(service: Service<'_>, room_id: String) -> AppResult<SharedRoomView> {
    blocking(&service, move |s| s.room_view(&room_id)).await
}

#[tauri::command]
pub async fn shared_wiki_acknowledge(service: Service<'_>, room_id: String, titles: Vec<String>) -> AppResult<()> {
    blocking(&service, move |s| s.acknowledge_concepts(&room_id, &titles)).await
}

#[tauri::command]
pub async fn shared_wiki_merge(service: Service<'_>, room_id: String, page_name: String) -> AppResult<MergeView> {
    blocking(&service, move |s| s.merge_page(&room_id, &page_name)).await
}

#[tauri::command]
pub async fn shared_wiki_quiz_start(service: Service<'_>, room_id: String) -> AppResult<Vec<QuizQuestionView>> {
    blocking(&service, move |s| s.quiz_start(&room_id)).await
}

#[tauri::command]
pub fn shared_wiki_quiz_answer(service: Service<'_>, item_id: i64, answer: String) -> Option<bool> {
    service.quiz_answer(item_id, &answer)
}
