//! App blocking. While a study block holds the lock, apps on the block list are either only
//! logged (dry-run, the default) or asked to quit, then force-killed after a grace time when the
//! user allowed it (AGENTS.md section 5). Judgement is pure (`decide`); ending a process is the
//! platform layer's job (`ProcessControl`); this module connects them and records every outcome
//! in `block_events` and the structured log. Logs and rows carry app ids only, never titles.

pub mod decide;
pub mod model;
#[cfg(test)]
mod tests;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chrono::{Local, TimeZone};
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::clock::now_ms;
use crate::commands::lock_db;
use crate::error::{AppError, AppResult};
use crate::platform::{self, AppIdentity, Notifier, PlatformError, ProcessControl};
use crate::settings::{
    self, ENFORCEMENT_MODE, Outcome, Reason, SettingKey, SettingValue, UpdateOutcome,
};
use crate::store;
use crate::timer::model::{Phase, TimerState};
use crate::timer::{self, HighWater};

use decide::{Action, Config, Requested, Tracker};
use model::{
    BlockAction, BlockEvent, EnforceMode, EntryKind, EntrySource, NewBlockEvent, TargetKind,
};

pub const BLOCK_ATTEMPT_EVENT: &str = "block_attempt";
/// Seconds between polls while the app is running. Listing apps is cheap but not free.
const POLL: Duration = Duration::from_secs(3);
/// The sentence that must be typed to switch to enforce mode.
pub const ENFORCE_CONFIRMATION: &str = "실행 모드를 켭니다";
pub(crate) const BLOCK_LIST_KEY: &str = "blockList";
const MAX_PATTERN_CHARS: usize = 512;
const RECENT_EVENTS: i64 = 20;

// ---- configuration ----

pub fn read_config(conn: &Connection) -> AppResult<Config> {
    let mode = match settings::read_value(conn, &ENFORCEMENT_MODE)? {
        SettingValue::Text(raw) => EnforceMode::parse(&raw)
            .ok_or_else(|| AppError::Internal(format!("unknown enforcement mode {raw:?}")))?,
        other => {
            return Err(AppError::Internal(format!(
                "enforcementMode is not text: {other:?}"
            )));
        }
    };
    let grace_secs = match settings::read_value(conn, &SettingKey::GraceWaitSec.spec())? {
        SettingValue::Int(secs) => secs,
        other => {
            return Err(AppError::Internal(format!(
                "graceWaitSec is not an integer: {other:?}"
            )));
        }
    };
    Ok(Config {
        mode,
        grace_ms: grace_secs * 1000,
        force_quit_allowed: settings::general_settings(conn)?.force_quit_allowed,
    })
}

/// Switches the enforcement mode. Enforce needs the typed confirmation sentence (not stored);
/// the change is rejected and recorded while the lock is held, like every other setting.
pub fn set_mode(
    conn: &mut Connection,
    mode: EnforceMode,
    confirmation: &str,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    if mode == EnforceMode::Enforce && confirmation != ENFORCE_CONFIRMATION {
        return Err(AppError::Invalid(
            "the confirmation sentence does not match".into(),
        ));
    }
    settings::apply_setting(
        conn,
        &ENFORCEMENT_MODE,
        &SettingValue::Text(mode.as_str().into()),
        at_ms,
        || Ok(()),
    )
}

// ---- block list ----

/// Normalizes an app id typed or picked by the user and rejects shapes that cannot be an id.
/// Shared with the study-app list so both lists accept exactly the same ids.
pub(crate) fn checked_app_id(raw: &str) -> AppResult<String> {
    let id = platform::normalize_app_id(raw);
    if id.is_empty() || id.chars().count() > MAX_PATTERN_CHARS || id.chars().any(char::is_control) {
        return Err(AppError::Invalid(
            "app id is empty, too long, or has control characters".into(),
        ));
    }
    Ok(id)
}

/// Adds an app to the block list. Protected apps are refused up front. Locked during study: the
/// attempt is recorded in the setting history and nothing changes.
pub fn add_entry(
    conn: &mut Connection,
    raw_pattern: &str,
    category: Option<&str>,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    let pattern = checked_app_id(raw_pattern)?;
    if platform::is_protected_id(&pattern) {
        return Err(AppError::Invalid(
            "system, security, and this app cannot be blocked".into(),
        ));
    }
    let after = format!("add:{pattern}");
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if crate::study::lock_held(&tx)? {
        store::append_history(
            &tx,
            at_ms,
            BLOCK_LIST_KEY,
            None,
            &after,
            Reason::User,
            Outcome::RejectedLocked,
        )?;
        tx.commit()?;
        return Ok(UpdateOutcome::RejectedLocked {
            key: BLOCK_LIST_KEY.into(),
        });
    }
    if !store::insert_block_entry(
        &tx,
        EntryKind::App,
        &pattern,
        EntrySource::User,
        category,
        at_ms,
    )? {
        return Err(AppError::Invalid(
            "this app is already on the block list".into(),
        ));
    }
    store::append_history(
        &tx,
        at_ms,
        BLOCK_LIST_KEY,
        None,
        &after,
        Reason::User,
        Outcome::Applied,
    )?;
    tx.commit()?;
    Ok(UpdateOutcome::Applied {
        key: BLOCK_LIST_KEY.into(),
    })
}

/// Removes a user entry. Locked during study, like adding. Preset entries follow their category.
pub fn remove_entry(conn: &mut Connection, id: i64, at_ms: i64) -> AppResult<UpdateOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let entry = store::get_block_entry(&tx, id)?
        .ok_or_else(|| AppError::Invalid("unknown block list entry".into()))?;
    if entry.source == EntrySource::Preset.as_str() {
        return Err(AppError::Invalid(
            "a preset entry is switched off with its category, not deleted".into(),
        ));
    }
    let before = format!("add:{}", entry.pattern);
    if crate::study::lock_held(&tx)? {
        store::append_history(
            &tx,
            at_ms,
            BLOCK_LIST_KEY,
            Some(&before),
            "remove",
            Reason::User,
            Outcome::RejectedLocked,
        )?;
        tx.commit()?;
        return Ok(UpdateOutcome::RejectedLocked {
            key: BLOCK_LIST_KEY.into(),
        });
    }
    store::delete_block_entry(&tx, id)?;
    store::append_history(
        &tx,
        at_ms,
        BLOCK_LIST_KEY,
        Some(&before),
        "remove",
        Reason::User,
        Outcome::Applied,
    )?;
    tx.commit()?;
    Ok(UpdateOutcome::Applied {
        key: BLOCK_LIST_KEY.into(),
    })
}

// ---- status ----

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnforcementStatus {
    pub mode: String,
    pub grace_wait_sec: i64,
    pub force_quit_allowed: bool,
    /// Blocking is in force right now: lock held during a study block.
    pub active: bool,
    pub enforce_confirmation: String,
    pub recent_events: Vec<BlockEvent>,
}

pub fn status(conn: &Connection, state: &TimerState) -> AppResult<EnforcementStatus> {
    let cfg = read_config(conn)?;
    Ok(EnforcementStatus {
        mode: cfg.mode.as_str().into(),
        grace_wait_sec: cfg.grace_ms / 1000,
        force_quit_allowed: cfg.force_quit_allowed,
        active: decide::applies(state.phase, state.lock_held),
        enforce_confirmation: ENFORCE_CONFIRMATION.into(),
        recent_events: store::list_block_events(conn, RECENT_EVENTS)?,
    })
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunningApp {
    pub id: String,
    pub display_name: String,
    pub is_protected: bool,
}

/// Running apps for the "pick from running apps" list: one row per app id.
pub fn running_apps(control: &dyn ProcessControl) -> AppResult<Vec<RunningApp>> {
    let mut apps: Vec<RunningApp> = Vec::new();
    for app in control.list_running()? {
        if apps.iter().all(|seen| seen.id != app.id) {
            apps.push(RunningApp {
                id: app.id,
                display_name: app.display_name,
                is_protected: app.is_protected,
            });
        }
    }
    apps.sort_by_key(|a| a.display_name.to_lowercase());
    Ok(apps)
}

// ---- the poll ----

/// The study block enforcement applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StudyWindow {
    pub run_id: Option<i64>,
    pub segment_id: Option<i64>,
}

/// `Some` only for lock held during a study block; breaks, idle, and a free lock give `None`.
pub fn study_window(state: &TimerState) -> Option<StudyWindow> {
    decide::applies(state.phase, state.lock_held).then_some(StudyWindow {
        run_id: state.run_id,
        segment_id: state.segment_id,
    })
}

/// What `block_attempt` carries to the UI.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub target_id: String,
    pub action: String,
}

fn detail_for(err: &PlatformError) -> &'static str {
    match err {
        PlatformError::Unsupported(_) => "unsupported",
        _ => "os_refused",
    }
}

/// Writes event rows and the matching structured log lines for one poll, and queues them for the UI.
struct Recorder<'a> {
    db: &'a Mutex<Connection>,
    window: StudyWindow,
    mode: EnforceMode,
    at_ms: i64,
    out: Vec<Attempt>,
}

impl Recorder<'_> {
    fn record(&mut self, id: &str, action: BlockAction, detail: Option<&str>) -> AppResult<()> {
        let event = NewBlockEvent {
            at_ms: self.at_ms,
            run_id: self.window.run_id,
            segment_id: self.window.segment_id,
            target_kind: TargetKind::App,
            target_id: id,
            action,
            mode: self.mode,
            detail_code: detail,
        };
        store::insert_block_event(&*lock_db(self.db)?, &event)?;
        // One format for every action; the id is an app identifier, never a title.
        let (mode, name, detail_code) = (
            self.mode.as_str(),
            action.as_str(),
            detail.unwrap_or("none"),
        );
        if action == BlockAction::Failed {
            tracing::warn!(
                event = name,
                target_kind = "app",
                target_id = id,
                mode,
                detail_code
            );
        } else {
            tracing::info!(
                event = name,
                target_kind = "app",
                target_id = id,
                mode,
                detail_code
            );
        }
        self.out.push(Attempt {
            target_id: id.to_owned(),
            action: name.to_owned(),
        });
        Ok(())
    }
}

fn notify_ended(notifier: &dyn Notifier, app: &AppIdentity) {
    let body = format!("{} 앱을 종료했습니다", app.display_name);
    if let Err(err) = notifier.notify("공부 중에는 열 수 없는 앱", &body) {
        tracing::warn!(error = %err, "notification not delivered");
    }
}

/// The lock is re-read just before ending anything: an emergency release between the poll's start
/// and now must stop the kill.
fn still_locked(db: &Mutex<Connection>) -> AppResult<bool> {
    Ok(store::read_study_state(&*lock_db(db)?)?.0)
}

/// One enforcement pass. `window` is `None` outside a locked study block, which clears the
/// tracker and does nothing else (the lock stays; only blocking pauses during breaks).
pub fn poll(
    db: &Mutex<Connection>,
    window: Option<StudyWindow>,
    control: &dyn ProcessControl,
    notifier: &dyn Notifier,
    tracker: &mut Tracker,
    now_ms: i64,
) -> AppResult<Vec<Attempt>> {
    let Some(window) = window else {
        tracker.leave();
        return Ok(Vec::new());
    };
    // An emergency pass pauses enforcement (the lock stays). Clearing the tracker restarts every
    // grace period when the pass ends.
    if timer::study_pass::active_until(&*lock_db(db)?, now_ms)?.is_some() {
        tracker.leave();
        return Ok(Vec::new());
    }
    tracker.enter((window.run_id, window.segment_id));
    let (cfg, blocklist) = {
        let conn = lock_db(db)?;
        (
            read_config(&conn)?,
            store::list_enabled_patterns(&conn, EntryKind::App)?,
        )
    };
    if blocklist.is_empty() {
        return Ok(Vec::new());
    }
    let running = match control.list_running() {
        Ok(running) => {
            tracker.list_failed = false;
            running
        }
        Err(err) => {
            if !tracker.list_failed {
                tracing::warn!(event = "enforce_list_failed", error = %err);
                tracker.list_failed = true;
            }
            return Ok(Vec::new());
        }
    };
    let mut rec = Recorder {
        db,
        window,
        mode: cfg.mode,
        at_ms: now_ms,
        out: Vec::new(),
    };

    // Apps we asked to quit that are gone now (or whose pid was reused by another app).
    let gone: Vec<u32> = tracker
        .requested
        .iter()
        .filter(|(pid, request)| {
            !running
                .iter()
                .any(|a| a.pid == **pid && a.id == request.app.id)
        })
        .map(|(pid, _)| *pid)
        .collect();
    for pid in gone {
        if let Some(request) = tracker.requested.remove(&pid) {
            if !request.forced {
                rec.record(&request.app.id, BlockAction::Terminated, None)?;
            }
            notify_ended(notifier, &request.app);
        }
    }

    for action in decide::decide(
        &running,
        &blocklist,
        Phase::Study,
        true,
        &cfg,
        tracker,
        now_ms,
    ) {
        match action {
            Action::WouldTerminate(app) => {
                tracker.first_report(app.pid, decide::KIND_WOULD_TERMINATE);
                rec.record(&app.id, BlockAction::WouldTerminate, None)?;
            }
            Action::SkipProtected(app) => {
                tracker.first_report(app.pid, decide::KIND_SKIPPED_PROTECTED);
                rec.record(&app.id, BlockAction::SkippedProtected, None)?;
            }
            Action::ForceNotAllowed(app) => {
                tracker.first_report(app.pid, decide::KIND_FORCE_NOT_ALLOWED);
                rec.record(
                    &app.id,
                    BlockAction::Failed,
                    Some("grace_expired_force_not_allowed"),
                )?;
            }
            Action::RequestQuit(app) => {
                if !still_locked(db)? {
                    tracker.leave();
                    break;
                }
                let result = control.request_quit(&app);
                // Tracked even when the request failed: the grace clock still decides about a force kill.
                tracker.requested.insert(
                    app.pid,
                    Requested {
                        app: app.clone(),
                        at_ms: now_ms,
                        forced: false,
                    },
                );
                match result {
                    Ok(()) => rec.record(&app.id, BlockAction::TerminateRequested, None)?,
                    Err(err) => rec.record(&app.id, BlockAction::Failed, Some(detail_for(&err)))?,
                }
            }
            Action::ForceKill(app) => {
                if !still_locked(db)? {
                    tracker.leave();
                    break;
                }
                let result = control.force_kill(&app);
                if let Some(request) = tracker.requested.get_mut(&app.pid) {
                    request.forced = true;
                }
                match result {
                    Ok(()) => rec.record(&app.id, BlockAction::ForceKilled, None)?,
                    Err(err) => rec.record(&app.id, BlockAction::Failed, Some(detail_for(&err)))?,
                }
            }
        }
    }
    Ok(rec.out)
}

// ---- thread ----

fn emit_attempts(app: &AppHandle, attempts: Vec<Attempt>) {
    for attempt in attempts {
        if let Err(err) = app.emit(BLOCK_ATTEMPT_EVENT, attempt) {
            tracing::error!(error = %err, "failed to emit block_attempt");
        }
    }
}

fn pass<Tz: TimeZone>(
    db: &Mutex<Connection>,
    tz: &Tz,
    high_water: &HighWater,
    control: &dyn ProcessControl,
    notifier: &dyn Notifier,
    tracker: &mut Tracker,
) -> AppResult<Vec<Attempt>> {
    let effective = high_water.observe(now_ms()?);
    let state = timer::current_state(&*lock_db(db)?, tz, effective)?;
    poll(
        db,
        study_window(&state),
        control,
        notifier,
        tracker,
        effective,
    )
}

/// Starts the enforcement thread. It reads the timer state every few seconds but lists processes
/// only while a locked study block is running. A failing or panicking pass is logged and retried
/// at the next interval; it never stops the thread.
pub fn spawn(
    app: AppHandle,
    db: Arc<Mutex<Connection>>,
    high_water: Arc<HighWater>,
    control: Arc<dyn ProcessControl>,
    notifier: Arc<dyn Notifier>,
) {
    thread::spawn(move || {
        let mut tracker = Tracker::default();
        loop {
            thread::sleep(POLL);
            let result = catch_unwind(AssertUnwindSafe(|| {
                pass(
                    &db,
                    &Local,
                    &high_water,
                    control.as_ref(),
                    notifier.as_ref(),
                    &mut tracker,
                )
            }));
            match result {
                Ok(Ok(attempts)) => emit_attempts(&app, attempts),
                Ok(Err(err)) => tracing::error!(event = "enforce_pass_failed", error = %err),
                Err(_) => tracing::error!(
                    event = "enforce_pass_panicked",
                    "enforcement pass panicked; retrying"
                ),
            }
        }
    });
}
