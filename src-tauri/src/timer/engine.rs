//! The scheduler thread and its supervision. Each wake-up (1 s tick, resume from sleep, plan
//! edit) runs the same `reconcile`. A tick body that errors or panics is retried on the next
//! wake-up; after `DEGRADED_AFTER` failures in a row the UI is told the automatic transitions
//! stopped (the emergency release in `release` keeps working without this thread).

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::clock::now_ms;
use crate::commands::lock_db;
use crate::error::AppResult;
use crate::platform::{self, Notifier};
use crate::tray;

use super::model::TimerState;
use super::notice::{self, BreakEndingSoon, BREAK_ENDING_SOON_EVENT};
use super::summary::{self, RunSummary, SetSummary, RUN_SUMMARY_EVENT, SET_SUMMARY_EVENT};
use super::{reconcile, HighWater};

pub const TIMER_CHANGED_EVENT: &str = "timer_changed";
pub const PLAN_RECHECKED_EVENT: &str = "plan_rechecked";
pub const ENGINE_DEGRADED_EVENT: &str = "engine_degraded";

const TICK: Duration = Duration::from_secs(1);
/// Consecutive failed wake-ups before the UI is told the engine is degraded.
pub const DEGRADED_AFTER: u32 = 5;
/// A tick later than this is logged (App Nap, a busy machine).
const LATE_LOG_MS: u128 = 500;
/// A wall-clock step this large between ticks is logged as a discontinuity (sleep or a clock set).
const DISCONTINUITY_MS: i64 = 60_000;
const RESTART_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeReason {
    Tick,
    /// The system woke from sleep (`power.rs` `resumed`).
    Resumed,
    /// A plan was created, moved, or deleted.
    PlanChanged,
}

/// Sends a reconcile request to the engine thread.
#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<WakeReason>,
}

impl EngineHandle {
    pub fn wake(&self, reason: WakeReason) {
        if self.tx.send(reason).is_err() {
            tracing::error!("timer engine is not running; wake-up dropped");
        }
    }
}

pub fn channel() -> (EngineHandle, Receiver<WakeReason>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (EngineHandle { tx }, rx)
}

/// Failure bookkeeping shared by the engine thread and `get_engine_health`.
#[derive(Default)]
pub struct EngineHealth {
    consecutive_failures: AtomicU32,
    restarts: AtomicU32,
    degraded: AtomicBool,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HealthSnapshot {
    pub degraded: bool,
    pub consecutive_failures: u32,
    pub restarts: u32,
}

/// What a supervised run changed about the engine's health.
#[derive(Debug, PartialEq, Eq)]
pub enum Supervised<T> {
    Done { value: T, recovered: bool },
    Failed { just_degraded: bool },
}

impl EngineHealth {
    pub fn snapshot(&self) -> HealthSnapshot {
        HealthSnapshot {
            degraded: self.degraded.load(Ordering::SeqCst),
            consecutive_failures: self.consecutive_failures.load(Ordering::SeqCst),
            restarts: self.restarts.load(Ordering::SeqCst),
        }
    }

    fn fail(&self) -> bool {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        failures >= DEGRADED_AFTER && !self.degraded.swap(true, Ordering::SeqCst)
    }

    /// Runs one tick body. A panic is caught, counted as a restart, and reported as a failure
    /// rather than unwinding the thread, so the next wake-up simply tries again.
    pub fn run<T>(&self, body: impl FnOnce() -> AppResult<T>) -> Supervised<T> {
        match catch_unwind(AssertUnwindSafe(body)) {
            Ok(Ok(value)) => {
                self.consecutive_failures.store(0, Ordering::SeqCst);
                let recovered = self.degraded.swap(false, Ordering::SeqCst);
                Supervised::Done { value, recovered }
            }
            Ok(Err(err)) => {
                tracing::error!(event = "engine_tick_failed", error = %err);
                Supervised::Failed { just_degraded: self.fail() }
            }
            Err(_) => {
                self.restarts.fetch_add(1, Ordering::SeqCst);
                tracing::error!(event = "engine_restart", "timer tick panicked; retrying on the next wake-up");
                Supervised::Failed { just_degraded: self.fail() }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Emission {
    TimerChanged(TimerState),
    PlanRechecked(TimerState),
    BreakEndingSoon(BreakEndingSoon),
    SetSummary(SetSummary),
    RunSummary(RunSummary),
}

/// Whether two states differ in anything the UI shows (the clock reading alone is not a change).
fn same_view(a: &TimerState, b: &TimerState) -> bool {
    TimerState { now_ms: 0, ..a.clone() } == TimerState { now_ms: 0, ..b.clone() }
}

/// One wake-up: reconcile at the effective time and decide which events to send. Pure in its
/// inputs (clock reading, tz, and notifier are passed in) so tests drive it with a fake clock.
pub fn tick_once<Tz: TimeZone>(
    db: &Mutex<Connection>,
    tz: &Tz,
    high_water: &HighWater,
    now: i64,
    reason: WakeReason,
    last: &mut Option<TimerState>,
    notifier: &dyn Notifier,
) -> AppResult<Vec<Emission>> {
    let previous_high = high_water.get();
    let effective = high_water.observe(now);
    if now < previous_high {
        tracing::warn!(event = "clock_regression_held", behind_ms = previous_high - now);
    } else if reason == WakeReason::Tick && previous_high > 0 && now - previous_high > DISCONTINUITY_MS {
        tracing::info!(event = "clock_discontinuity", ahead_ms = now - previous_high);
    }
    let reconciled = {
        let mut conn = lock_db(db)?;
        reconcile(&mut conn, tz, effective)?
    };
    let state = reconciled.state;
    // Without a previous state (first tick after a start) nothing is summarized, so a restart
    // cannot replay what ended before it.
    let (settled, notice) = {
        let conn = lock_db(db)?;
        let settled = match last.as_ref() {
            Some(prev) => summary::settle(&conn, prev, &state)?,
            None => summary::Settled::default(),
        };
        (settled, notice::claim_due(&conn, &state, effective)?)
    };
    let mut out = Vec::new();
    let changed = reconciled.changed || last.as_ref().is_none_or(|prev| !same_view(prev, &state));
    if changed || reason != WakeReason::Tick {
        out.push(Emission::TimerChanged(state.clone()));
    }
    if reason == WakeReason::Resumed {
        out.push(Emission::PlanRechecked(state.clone()));
    }
    for set in settled.sets {
        let (title, body) = summary::set_notification(&set);
        send(notifier, &title, &body);
        out.push(Emission::SetSummary(set));
    }
    out.extend(settled.run.map(Emission::RunSummary));
    if let Some(soon) = notice {
        let (title, body) = notice::notification();
        send(notifier, title, body);
        out.push(Emission::BreakEndingSoon(soon));
    }
    *last = Some(state);
    Ok(out)
}

/// A failed notification is logged by kind only; the claim already made it at-most-once.
fn send(notifier: &dyn Notifier, title: &str, body: &str) {
    if let Err(err) = notifier.notify(title, body) {
        tracing::warn!(error = %err, "notification not delivered");
    }
}

fn emit_one<S: Serialize + Clone>(app: &AppHandle, name: &str, payload: S) {
    if let Err(err) = app.emit(name, payload) {
        tracing::error!(error = %err, event_name = name, "failed to emit timer event");
    }
}

fn emit_all(app: &AppHandle, emissions: Vec<Emission>) {
    for emission in emissions {
        match emission {
            Emission::TimerChanged(state) => emit_one(app, TIMER_CHANGED_EVENT, state),
            Emission::PlanRechecked(state) => emit_one(app, PLAN_RECHECKED_EVENT, state),
            Emission::BreakEndingSoon(soon) => emit_one(app, BREAK_ENDING_SOON_EVENT, soon),
            Emission::SetSummary(set) => emit_one(app, SET_SUMMARY_EVENT, set),
            Emission::RunSummary(run) => emit_one(app, RUN_SUMMARY_EVENT, run),
        }
    }
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct DegradedPayload {
    degraded: bool,
    consecutive_failures: u32,
}

fn emit_health(app: &AppHandle, health: &EngineHealth, degraded: bool) {
    let payload = DegradedPayload { degraded, consecutive_failures: health.snapshot().consecutive_failures };
    if let Err(err) = app.emit(ENGINE_DEGRADED_EVENT, payload) {
        tracing::error!(error = %err, "failed to emit engine_degraded");
    }
}

/// Starts the engine. The first iteration runs immediately, which is the startup reconcile.
pub fn spawn(
    app: AppHandle,
    db: Arc<Mutex<Connection>>,
    high_water: Arc<HighWater>,
    health: Arc<EngineHealth>,
    wakes: Receiver<WakeReason>,
    notifier: Arc<dyn Notifier>,
) {
    thread::spawn(move || {
        // Held for the thread's life so macOS does not throttle the timer while the window is hidden.
        let _activity = match platform::begin_activity("study timer") {
            Ok(guard) => Some(guard),
            Err(err) => {
                tracing::warn!(error = %err, "background activity guard unavailable; ticks may be delayed");
                None
            }
        };
        let mut last: Option<TimerState> = None;
        let mut tray_label: Option<tray::TrayLabel> = None;
        let mut next_tick = Instant::now();
        loop {
            let reason = match wakes.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
                Ok(reason) => reason,
                Err(RecvTimeoutError::Timeout) => {
                    let late = Instant::now().saturating_duration_since(next_tick).as_millis();
                    if late > LATE_LOG_MS {
                        tracing::warn!(event = "tick_late", tick_late_ms = late);
                    }
                    WakeReason::Tick
                }
                Err(RecvTimeoutError::Disconnected) => {
                    tracing::error!("timer wake channel closed; engine stopping");
                    return;
                }
            };
            if reason == WakeReason::Tick {
                next_tick = Instant::now() + TICK;
            }
            let outcome = health.run(|| {
                tick_once(&db, &Local, &high_water, now_ms()?, reason, &mut last, notifier.as_ref())
            });
            match outcome {
                Supervised::Done { value, recovered } => {
                    emit_all(&app, value);
                    if let Some(state) = last.as_ref() {
                        tray::apply_label(&app, &mut tray_label, tray::tray_label(state));
                    }
                    if recovered {
                        emit_health(&app, &health, false);
                    }
                }
                Supervised::Failed { just_degraded } => {
                    if just_degraded {
                        emit_health(&app, &health, true);
                    }
                    thread::sleep(RESTART_DELAY);
                }
            }
        }
    });
}
