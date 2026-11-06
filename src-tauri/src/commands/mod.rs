//! Tauri commands: the only surface the UI uses. Argument and return types here are mirrored
//! in `src/api/`; the shared fixture test keeps the two in sync. Each module groups the
//! handlers of one area; `lib.rs` lists them in a single `generate_handler!` call.

pub mod browser_link;
pub mod character;
pub mod chat;
pub mod context;
pub mod dashboard;
pub mod downtime;
pub mod enforce;
pub mod focus;
pub mod growth;
pub mod llm;
pub mod messages;
pub mod plan;
pub mod points;
pub mod power;
pub mod profile;
pub mod quiz;
pub mod release;
pub mod remote;
pub mod retro;
pub mod rooms;
pub mod shared_wiki;
pub mod settings;
pub mod shop;
pub mod study;
pub mod timer;
pub mod topic;
pub mod wiki;

use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, TransactionBehavior};

use crate::clock::now_ms;
use crate::error::AppResult;
use crate::platform::{AutostartBackend, FocusProbe, ProcessControl, QuitReason};
use crate::settings::{Outcome, Reason};
use crate::store;
use crate::timer::engine::{EngineHandle, EngineHealth};
use crate::timer::release::ReleaseDesk;
use crate::timer::HighWater;

pub use power::PowerSourceStatus;

pub(crate) const DEFAULT_LIMIT: i64 = 200;

pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub session_id: i64,
    pub autostart: Arc<dyn AutostartBackend>,
    pub power: PowerSourceStatus,
    pub engine: EngineHandle,
    pub high_water: Arc<HighWater>,
    pub health: Arc<EngineHealth>,
    pub release: ReleaseDesk,
    pub process: Arc<dyn ProcessControl>,
    pub focus_probe: Arc<dyn FocusProbe>,
    pub browser_link: Arc<crate::browser_link::LinkHub>,
    pub signals: Arc<crate::focus::signals::SignalHub>,
    pub remote: Arc<crate::remote::runtime::RemoteRuntime>,
    pub sync: Arc<crate::sync::app::SyncHub>,
    pub llm: Arc<crate::llm::service::LlmService>,
    pub wiki: Arc<crate::wiki::service::WikiService>,
    pub quiz: Arc<crate::quiz::service::QuizService>,
}

/// Locks the shared connection. A panic in another holder (the timer engine is supervised and
/// may panic) poisons the mutex, but the connection itself is intact: SQLite rolls back the
/// unfinished transaction when its guard unwinds. Refusing to lock would also disable the
/// emergency release, so the poison is cleared and the event logged.
pub fn lock_db(db: &Mutex<Connection>) -> AppResult<MutexGuard<'_, Connection>> {
    match db.lock() {
        Ok(guard) => Ok(guard),
        Err(poisoned) => {
            tracing::error!(event = "db_lock_recovered", "database lock was poisoned by a panic");
            db.clear_poison();
            Ok(poisoned.into_inner())
        }
    }
}

/// Exit gate shared by every quit path (tray "종료", Cmd+Q, app.exit).
/// Returns `Ok(true)` when the exit may proceed. While the lock is held it records an `appExit`
/// `rejected_locked` attempt and returns `Ok(false)`.
pub fn allow_exit(state: &AppState) -> AppResult<bool> {
    let mut conn = lock_db(&state.db)?;
    exit_decision(&mut conn, state.session_id, now_ms()?)
}

/// Decision and its side effects in one Immediate transaction, so the lock check and the
/// clean-exit mark cannot interleave with a concurrent lock change.
pub(crate) fn exit_decision(conn: &mut Connection, session_id: i64, at_ms: i64) -> AppResult<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if crate::study::lock_held(&tx)? {
        store::append_history(&tx, at_ms, "appExit", None, "exit", Reason::User, Outcome::RejectedLocked)?;
        tx.commit()?;
        return Ok(false);
    }
    store::set_clean_exit(&tx, session_id, true, at_ms)?;
    tx.commit()?;
    Ok(true)
}

/// Gate for the native quit path (`applicationShouldTerminate:`). Ordinary requests use the
/// regular exit gate and fail closed. System reasons (logout/restart/shutdown) are always
/// allowed: a held lock or an unreadable lock only changes what is recorded.
pub fn allow_quit(state: &AppState, reason: QuitReason) -> bool {
    let decided = lock_db(&state.db).and_then(|mut conn| {
        let at_ms = now_ms()?;
        quit_decision(&mut conn, state.session_id, at_ms, reason)
    });
    match decided {
        Ok(allowed) => allowed,
        Err(err) => {
            tracing::error!(error = %err, "quit gate failed");
            reason.is_system()
        }
    }
}

/// `Requested` is exactly `exit_decision`. A system quit under a held lock is let through but
/// is not a clean exit: the session stays open, so the next launch records it as `forced_quit`,
/// and an `appExit` row notes the reason code (no titles, no URLs).
pub(crate) fn quit_decision(
    conn: &mut Connection,
    session_id: i64,
    at_ms: i64,
    reason: QuitReason,
) -> AppResult<bool> {
    if !reason.is_system() {
        return exit_decision(conn, session_id, at_ms);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if crate::study::lock_held(&tx)? {
        let after = format!("system_{}", reason.as_str());
        store::append_history(&tx, at_ms, "appExit", None, &after, Reason::User, Outcome::Applied)?;
    } else {
        store::set_clean_exit(&tx, session_id, true, at_ms)?;
    }
    tx.commit()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn exit_is_allowed_when_lock_is_free_and_marks_clean_exit() {
        let mut conn = open_in_memory().expect("db");
        let session = store::insert_session(&conn, 1).expect("session");
        assert!(exit_decision(&mut conn, session, 2).expect("exit"));
        let clean: Option<i64> = conn
            .query_row("SELECT clean_exit FROM sessions WHERE id = ?1", [session], |r| r.get(0))
            .expect("row");
        assert_eq!(clean, Some(1));
    }

    #[test]
    fn exit_is_refused_when_lock_is_unknown_and_session_stays_open() {
        let mut conn = open_in_memory().expect("db");
        let session = store::insert_session(&conn, 1).expect("session");
        conn.execute("DELETE FROM study_state", []).expect("delete");
        assert!(exit_decision(&mut conn, session, 2).is_err());
        let clean: Option<i64> = conn
            .query_row("SELECT clean_exit FROM sessions WHERE id = ?1", [session], |r| r.get(0))
            .expect("row");
        assert_eq!(clean, None, "an unknown lock must not mark the session clean");
    }

    #[test]
    fn exit_is_refused_while_lock_is_held_and_records_attempt() {
        let mut conn = open_in_memory().expect("db");
        let session = store::insert_session(&conn, 1).expect("session");
        crate::testutil::set_lock_for_test(&conn, true);
        assert!(!exit_decision(&mut conn, session, 3).expect("exit"));
        let clean: Option<i64> = conn
            .query_row("SELECT clean_exit FROM sessions WHERE id = ?1", [session], |r| r.get(0))
            .expect("row");
        assert_eq!(clean, None, "a refused exit must not mark the session clean");
        let latest = &crate::settings::read_history(&conn, 1).expect("history")[0];
        assert_eq!(latest.key, "appExit");
        assert_eq!(latest.outcome, "rejected_locked");
    }

    #[test]
    fn requested_quit_under_lock_is_refused_and_recorded_like_exit() {
        let mut conn = open_in_memory().expect("db");
        let session = store::insert_session(&conn, 1).expect("session");
        crate::testutil::set_lock_for_test(&conn, true);
        assert!(!quit_decision(&mut conn, session, 3, QuitReason::Requested).expect("quit"));
        let latest = &crate::settings::read_history(&conn, 1).expect("history")[0];
        assert_eq!((latest.key.as_str(), latest.outcome.as_str()), ("appExit", "rejected_locked"));
    }

    #[test]
    fn requested_quit_without_lock_is_clean() {
        let mut conn = open_in_memory().expect("db");
        let session = store::insert_session(&conn, 1).expect("session");
        assert!(quit_decision(&mut conn, session, 3, QuitReason::Requested).expect("quit"));
        assert_eq!(clean_exit_of(&conn, session), Some(1));
    }

    #[test]
    fn system_quit_under_lock_is_allowed_recorded_and_left_unclean_for_forced_quit() {
        for reason in [QuitReason::LogOut, QuitReason::Restart, QuitReason::ShutDown] {
            let mut conn = open_in_memory().expect("db");
            let session = store::insert_session(&conn, 1).expect("session");
            crate::testutil::set_lock_for_test(&conn, true);
            assert!(quit_decision(&mut conn, session, 3, reason).expect("quit"));
            assert_eq!(clean_exit_of(&conn, session), None, "must be recovered as forced_quit");
            let latest = &crate::settings::read_history(&conn, 1).expect("history")[0];
            assert_eq!(latest.key, "appExit");
            assert_eq!(latest.outcome, "applied");
            assert_eq!(latest.after, format!("system_{}", reason.as_str()));
            // Next launch turns the open session into a forced_quit downtime row.
            assert_eq!(store::open_sessions(&conn).expect("open").len(), 1);
        }
    }

    #[test]
    fn system_quit_without_lock_is_clean() {
        let mut conn = open_in_memory().expect("db");
        let session = store::insert_session(&conn, 1).expect("session");
        assert!(quit_decision(&mut conn, session, 3, QuitReason::ShutDown).expect("quit"));
        assert_eq!(clean_exit_of(&conn, session), Some(1));
    }

    fn clean_exit_of(conn: &Connection, session: i64) -> Option<i64> {
        conn.query_row("SELECT clean_exit FROM sessions WHERE id = ?1", [session], |r| r.get(0)).expect("row")
    }
}
