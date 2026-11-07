//! Notices blocked-app and blocked-site attempts by reading the `block_events` log, and offers the
//! review question for them. It only reads: the enforcement thread and the browser link decide and
//! record exactly as before, never wait for this, and cannot be affected by it (a slow database or
//! an error here changes nothing about blocking). A pass that finds a new attempt asks the gate,
//! which applies the cooldown, the daily limit, and the locked-study-block rule.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tauri::AppHandle;

use super::gate::QuizCard;
use super::service::QuizService;
use crate::commands::lock_db;
use crate::commands::quiz::emit_card;
use crate::error::AppResult;
use crate::store::quiz as repo;

const POLL: Duration = Duration::from_secs(3);

/// Looks for attempts newer than `last_seen`; returns the new high mark and, when there was one
/// and the gate allows it, a card.
pub fn poll(
    db: &std::sync::Mutex<rusqlite::Connection>,
    quiz: &QuizService,
    last_seen: i64,
) -> AppResult<(i64, Option<QuizCard>)> {
    let latest = repo::latest_attempt_event_id(&*lock_db(db)?)?;
    if latest <= last_seen {
        return Ok((last_seen, None));
    }
    Ok((latest, quiz.on_block_attempt()?))
}

pub fn spawn(
    app: AppHandle,
    db: Arc<std::sync::Mutex<rusqlite::Connection>>,
    quiz: Arc<QuizService>,
) {
    thread::spawn(move || {
        let mut last_seen = lock_db(&db)
            .and_then(|conn| repo::latest_attempt_event_id(&conn))
            .unwrap_or(0);
        loop {
            thread::sleep(POLL);
            match catch_unwind(AssertUnwindSafe(|| poll(&db, &quiz, last_seen))) {
                Ok(Ok((seen, card))) => {
                    last_seen = seen;
                    if let Some(card) = card {
                        emit_card(&app, card);
                    }
                }
                Ok(Err(err)) => tracing::warn!(event = "quiz_watch_failed", error = %err),
                Err(_) => tracing::error!(
                    event = "quiz_watch_panicked",
                    "quiz watch pass panicked; retrying"
                ),
            }
        }
    });
}
