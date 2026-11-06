//! Planning chatbot commands. A turn may call the model, so it runs on the blocking pool with the
//! database lock released. Applying a draft goes through the plan functions and so through the
//! study lock; a refusal comes back as a value, and the attempt is in the plan history.

use std::sync::Arc;

use tauri::{AppHandle, State};

use super::plan::announce_plans_changed;
use crate::chat::service::{ApplyResult, ChatMessage, ChatReply, ChatService, DraftView};
use crate::error::{AppError, AppResult};

type Service<'a> = State<'a, Arc<ChatService>>;

/// One user message with the conversation so far (the UI keeps it; the app stores none of it).
#[tauri::command]
pub async fn send_chat_message(service: Service<'_>, history: Vec<ChatMessage>, message: String) -> AppResult<ChatReply> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || service.send(&history, &message))
        .await
        .map_err(|_| AppError::Internal("the chat job did not finish".into()))?
}

#[tauri::command]
pub fn apply_chat_draft(app: AppHandle, service: Service<'_>, draft_id: i64) -> AppResult<ApplyResult> {
    let result = service.apply_draft(draft_id)?;
    if result.applied {
        announce_plans_changed(&app);
    }
    Ok(result)
}

#[tauri::command]
pub fn discard_chat_draft(service: Service<'_>, draft_id: i64) -> AppResult<bool> {
    service.discard_draft(draft_id)
}

#[tauri::command]
pub fn get_chat_draft(service: Service<'_>, draft_id: i64) -> AppResult<Option<DraftView>> {
    service.draft_view(draft_id)
}

/// Opens the small chat window (the tray's "챗봇 입력"), also reachable from the calendar chat.
#[tauri::command]
pub fn open_chat_input(app: AppHandle) -> AppResult<()> {
    crate::tray::open_chat_window(&app).map_err(|err| AppError::Internal(format!("chat window failed: {err}")))
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::chat::service::{ApplyResult, ChatReply, DraftItem, DraftView, ReplySource};

    #[test]
    fn chat_wire_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/chat-contract.json");
        let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let reply = ChatReply {
            text: "초안이에요.".into(),
            source: ReplySource::Llm,
            fallback_reason: None,
            draft: Some(DraftView { id: 7, items: vec![DraftItem { kind: "create", label: "복습 (수학)".into() }] }),
            locked: false,
            over_asked: false,
        };
        assert_eq!(serde_json::to_value(&reply).expect("reply"), fixture["reply"]);
        let applied = ApplyResult { applied: true, locked: false, changed: 1, batch_id: Some("chat-7".into()) };
        assert_eq!(serde_json::to_value(&applied).expect("apply"), fixture["applied"]);
        let request: Vec<crate::chat::service::ChatMessage> = serde_json::from_value(fixture["history"].clone()).expect("history");
        assert_eq!(request.len(), 2);
        assert_eq!(json!(fixture["history"][1]["role"]), "assistant");
    }
}
