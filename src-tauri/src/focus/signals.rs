//! Self-reported focus signals (stage 1-5, D-32): the "just zoned out" global shortcut and the
//! random check-in ("were you focused just now?"). Both are recorded as `focus_events` rows of
//! kind `self_report` and only while a study block holds the lock; anywhere else the signal is
//! ignored and nothing is written. Rows carry no app id, no text, no window title.
//!
//! The check-in is a notification (the existing `Notifier`) plus a pending prompt the UI
//! answers in-app: desktop notifications cannot carry a reliable answer. "Not focused" records a
//! `self_report`; "focused" is acknowledged and stores nothing, since a focused answer needs no
//! correction of the measured data.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chrono::Local;
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use super::calc::SAMPLE_INTERVAL_MS;
use super::model::{FocusEventKind, NewFocusEvent};
use crate::clock::now_ms;
use crate::commands::lock_db;
use crate::enforce::{self, StudyWindow};
use crate::error::{AppError, AppResult};
use crate::platform::Notifier;
use crate::store;
use crate::timer::{self, HighWater};

/// The shortcut for "I just zoned out". The Alt+Shift pair keeps it clear of common app bindings.
pub const SELF_REPORT_SHORTCUT: &str = "CommandOrControl+Alt+Shift+D";
/// Presses closer together than this are one report (key repeat, a double tap).
pub const SELF_REPORT_DEDUP_MS: i64 = 5_000;
/// A check-in comes at a random point this many minutes after the block starts or after the
/// previous check-in.
pub const CHECKIN_MIN_MINUTES: u64 = 15;
pub const CHECKIN_MAX_MINUTES: u64 = 40;
/// An unanswered check-in lapses after this long.
pub const CHECKIN_ANSWER_WINDOW_MS: i64 = 2 * 60_000;

const CHECKIN_EVENT: &str = "focus_checkin";
const SELF_REPORT_EVENT: &str = "self_report_recorded";

// ---- recording ----

/// Why a signal wrote nothing.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IgnoreReason {
    /// No locked study block: a break, idle time, or a free lock.
    NotStudying,
    /// Another report landed a moment ago.
    TooSoon,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum SelfReportOutcome {
    Recorded,
    /// A "focused" check-in answer: acknowledged, nothing stored.
    Acknowledged,
    Ignored {
        reason: IgnoreReason,
    },
}

/// Writes one `self_report` row for the window's segment. The lock is checked again inside the
/// transaction so a block that ended a moment ago cannot receive a row.
fn record_in(
    conn: &mut Connection,
    window: Option<StudyWindow>,
    at_ms: i64,
    dedupe: bool,
) -> AppResult<SelfReportOutcome> {
    let Some(StudyWindow {
        run_id: Some(run_id),
        segment_id: Some(segment_id),
    }) = window
    else {
        return Ok(SelfReportOutcome::Ignored {
            reason: IgnoreReason::NotStudying,
        });
    };
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !crate::study::lock_held(&tx)? {
        return Ok(SelfReportOutcome::Ignored {
            reason: IgnoreReason::NotStudying,
        });
    }
    if dedupe {
        let last = store::last_focus_event_at(&tx, segment_id, FocusEventKind::SelfReport)?;
        if last.is_some_and(|last| at_ms.saturating_sub(last) < SELF_REPORT_DEDUP_MS) {
            return Ok(SelfReportOutcome::Ignored {
                reason: IgnoreReason::TooSoon,
            });
        }
    }
    store::insert_focus_event(
        &tx,
        &NewFocusEvent {
            run_id,
            segment_id,
            at_ms,
            kind: FocusEventKind::SelfReport,
            app_id: None,
        },
    )?;
    tx.commit()?;
    tracing::info!(event = "self_report", run_id, segment_id, "focus");
    Ok(SelfReportOutcome::Recorded)
}

fn current_window(
    db: &Mutex<Connection>,
    high_water: &HighWater,
) -> AppResult<(Option<StudyWindow>, i64)> {
    let effective = high_water.observe(now_ms()?);
    let state = timer::current_state(&*lock_db(db)?, &Local, effective)?;
    Ok((enforce::study_window(&state), effective))
}

/// "Just zoned out" at the current time.
pub fn report_now(db: &Mutex<Connection>, high_water: &HighWater) -> AppResult<SelfReportOutcome> {
    let (window, at_ms) = current_window(db, high_water)?;
    record_in(&mut *lock_db(db)?, window, at_ms, true)
}

// ---- check-in scheduling ----

/// Source of randomness, injected so tests are deterministic.
pub trait RandomSource: Send {
    /// A value in `0..bound`; `bound` is at least 1.
    fn below(&mut self, bound: u64) -> u64;
}

/// Randomness from the standard library's per-process random hasher keys. A check-in time needs
/// no cryptographic strength.
#[derive(Default)]
pub struct SystemRandom {
    counter: u64,
}

impl RandomSource for SystemRandom {
    fn below(&mut self, bound: u64) -> u64 {
        self.counter = self.counter.wrapping_add(1);
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(self.counter);
        hasher.finish() % bound.max(1)
    }
}

/// A check-in waiting for an answer.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckinPrompt {
    pub run_id: i64,
    pub segment_id: i64,
    pub asked_at_ms: i64,
}

struct Schedule {
    segment: Option<(i64, i64)>,
    next_at_ms: i64,
    pending: Option<CheckinPrompt>,
    rng: Box<dyn RandomSource>,
}

impl Schedule {
    fn interval_ms(&mut self) -> i64 {
        let span = CHECKIN_MAX_MINUTES - CHECKIN_MIN_MINUTES + 1;
        let minutes = CHECKIN_MIN_MINUTES + self.rng.below(span);
        i64::try_from(minutes).unwrap_or(0).saturating_mul(60_000)
    }

    fn schedule_from(&mut self, now_ms: i64) {
        self.next_at_ms = now_ms.saturating_add(self.interval_ms());
    }
}

/// Whether the shortcut could be registered. A failure is shown to the user, who still has the
/// in-app button; it is never silent.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SelfReportStatus {
    pub shortcut: String,
    pub shortcut_registered: bool,
    pub shortcut_error: Option<String>,
}

/// Check-in schedule and pending prompt, shared by the check-in thread and the commands.
pub struct SignalHub {
    schedule: Mutex<Schedule>,
    status: Mutex<SelfReportStatus>,
}

fn poisoned<T>(_: T) -> AppError {
    AppError::Internal("self-report state lock was poisoned".into())
}

impl SignalHub {
    pub fn new(rng: Box<dyn RandomSource>) -> Self {
        Self {
            schedule: Mutex::new(Schedule {
                segment: None,
                next_at_ms: 0,
                pending: None,
                rng,
            }),
            status: Mutex::new(SelfReportStatus {
                shortcut: SELF_REPORT_SHORTCUT.into(),
                shortcut_registered: false,
                shortcut_error: Some("not registered yet".into()),
            }),
        }
    }

    pub fn set_status(&self, status: SelfReportStatus) -> AppResult<()> {
        *self.status.lock().map_err(poisoned)? = status;
        Ok(())
    }

    pub fn status(&self) -> AppResult<SelfReportStatus> {
        Ok(self.status.lock().map_err(poisoned)?.clone())
    }

    /// Advances the schedule. Returns the prompt when a check-in falls due now. Outside a study
    /// block the schedule and any pending prompt are dropped, so a break never asks anything.
    pub fn tick(
        &self,
        window: Option<StudyWindow>,
        now_ms: i64,
    ) -> AppResult<Option<CheckinPrompt>> {
        let mut s = self.schedule.lock().map_err(poisoned)?;
        let Some(StudyWindow {
            run_id: Some(run_id),
            segment_id: Some(segment_id),
        }) = window
        else {
            s.segment = None;
            s.pending = None;
            return Ok(None);
        };
        let key = (run_id, segment_id);
        let max_ms = i64::try_from(CHECKIN_MAX_MINUTES)
            .unwrap_or(0)
            .saturating_mul(60_000);
        if s.segment != Some(key) {
            s.segment = Some(key);
            s.pending = None;
            s.schedule_from(now_ms);
            return Ok(None);
        }
        if s.next_at_ms.saturating_sub(now_ms) > max_ms {
            // The clock moved back: a schedule from the old time would be far too late.
            s.schedule_from(now_ms);
        }
        if let Some(pending) = s.pending {
            if now_ms.saturating_sub(pending.asked_at_ms) >= CHECKIN_ANSWER_WINDOW_MS {
                s.pending = None;
            }
            return Ok(None);
        }
        if now_ms < s.next_at_ms {
            return Ok(None);
        }
        let prompt = CheckinPrompt {
            run_id,
            segment_id,
            asked_at_ms: now_ms,
        };
        s.pending = Some(prompt);
        s.schedule_from(now_ms);
        Ok(Some(prompt))
    }

    /// The prompt still waiting for an answer at `now_ms`.
    pub fn pending(&self, now_ms: i64) -> AppResult<Option<CheckinPrompt>> {
        let s = self.schedule.lock().map_err(poisoned)?;
        Ok(s.pending
            .filter(|p| now_ms.saturating_sub(p.asked_at_ms) < CHECKIN_ANSWER_WINDOW_MS))
    }

    fn take_pending(&self, now_ms: i64, window: Option<StudyWindow>) -> AppResult<CheckinPrompt> {
        let mut s = self.schedule.lock().map_err(poisoned)?;
        let pending = s.pending.take();
        let current = window.and_then(|w| w.run_id.zip(w.segment_id));
        match pending {
            Some(p)
                if now_ms.saturating_sub(p.asked_at_ms) < CHECKIN_ANSWER_WINDOW_MS
                    && current == Some((p.run_id, p.segment_id)) =>
            {
                Ok(p)
            }
            _ => Err(AppError::Invalid(
                "no check-in is waiting for an answer".into(),
            )),
        }
    }
}

/// Answers the pending check-in. "Not focused" records a `self_report`; "focused" stores nothing.
pub fn answer_checkin(
    db: &Mutex<Connection>,
    hub: &SignalHub,
    window: Option<StudyWindow>,
    at_ms: i64,
    focused: bool,
) -> AppResult<SelfReportOutcome> {
    hub.take_pending(at_ms, window)?;
    if focused {
        return Ok(SelfReportOutcome::Acknowledged);
    }
    record_in(&mut *lock_db(db)?, window, at_ms, false)
}

/// Command entry: reads the clock and the timer state, then answers.
pub fn answer_checkin_now(
    db: &Mutex<Connection>,
    high_water: &HighWater,
    hub: &SignalHub,
    focused: bool,
) -> AppResult<SelfReportOutcome> {
    let (window, at_ms) = current_window(db, high_water)?;
    answer_checkin(db, hub, window, at_ms, focused)
}

/// One check-in pass: advance the schedule and, when a check-in is due, notify. A failed
/// notification is logged; the prompt stays pending for the in-app banner.
pub fn checkin_pass(
    hub: &SignalHub,
    notifier: &dyn Notifier,
    window: Option<StudyWindow>,
    now_ms: i64,
) -> AppResult<Option<CheckinPrompt>> {
    let prompt = hub.tick(window, now_ms)?;
    if prompt.is_some() {
        // No subject or plan name: a notification body can show on a lock screen.
        if let Err(err) =
            notifier.notify("집중 체크인", "방금 집중하고 있었나요? 앱에서 답해 주세요.")
        {
            tracing::warn!(event = "checkin_notify_failed", error = %err);
        }
    }
    Ok(prompt)
}

/// Starts the check-in thread. A failing or panicking pass is logged and retried next interval.
pub fn spawn(
    app: AppHandle,
    db: Arc<Mutex<Connection>>,
    high_water: Arc<HighWater>,
    hub: Arc<SignalHub>,
    notifier: Arc<dyn Notifier>,
) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(SAMPLE_INTERVAL_MS.unsigned_abs()));
            let result = catch_unwind(AssertUnwindSafe(|| -> AppResult<Option<CheckinPrompt>> {
                let (window, now) = current_window(&db, &high_water)?;
                checkin_pass(&hub, notifier.as_ref(), window, now)
            }));
            match result {
                Ok(Ok(Some(prompt))) => {
                    if let Err(err) = app.emit(CHECKIN_EVENT, prompt) {
                        tracing::error!(error = %err, "failed to emit focus_checkin");
                    }
                }
                Ok(Ok(None)) => {}
                Ok(Err(err)) => tracing::error!(event = "checkin_pass_failed", error = %err),
                Err(_) => tracing::error!(
                    event = "checkin_pass_panicked",
                    "check-in pass panicked; retrying"
                ),
            }
        }
    });
}

// ---- global shortcut ----

/// Registers the plugin and the shortcut. Any failure (plugin, a binding another app owns, a
/// missing OS permission) comes back in the status instead of failing startup.
pub fn install_shortcut(
    app: &AppHandle,
    db: Arc<Mutex<Connection>>,
    high_water: Arc<HighWater>,
) -> SelfReportStatus {
    let registered = register_shortcut(app, db, high_water);
    if let Err(err) = &registered {
        tracing::warn!(event = "self_report_shortcut_unavailable", error = %err);
    }
    SelfReportStatus {
        shortcut: SELF_REPORT_SHORTCUT.into(),
        shortcut_registered: registered.is_ok(),
        shortcut_error: registered.err(),
    }
}

fn register_shortcut(
    app: &AppHandle,
    db: Arc<Mutex<Connection>>,
    high_water: Arc<HighWater>,
) -> Result<(), String> {
    app.plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .map_err(|e| e.to_string())?;
    let emitter = app.clone();
    app.global_shortcut()
        .on_shortcut(SELF_REPORT_SHORTCUT, move |_app, _shortcut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            match report_now(&db, &high_water) {
                Ok(outcome) => {
                    if let Err(err) = emitter.emit(SELF_REPORT_EVENT, outcome) {
                        tracing::error!(error = %err, "failed to emit self_report_recorded");
                    }
                }
                Err(err) => tracing::error!(event = "self_report_failed", error = %err),
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::db::open_in_memory;
    use crate::plan::{CreatePlanInput, create_plan};
    use crate::study::{ModeConfig, StudyLevel};
    use crate::testutil::FakeNotifier;
    use crate::timer::model::TimerState;

    /// Returns scripted values (each reduced into the bound), then zeros.
    struct Script(VecDeque<u64>);

    impl RandomSource for Script {
        fn below(&mut self, bound: u64) -> u64 {
            self.0.pop_front().unwrap_or(0) % bound
        }
    }

    fn hub(values: &[u64]) -> SignalHub {
        SignalHub::new(Box::new(Script(values.iter().copied().collect())))
    }

    fn t(h: i64, m: i64, s: i64) -> i64 {
        Utc.with_ymd_and_hms(
            2026,
            11,
            5,
            u32::try_from(h).expect("h"),
            u32::try_from(m).expect("m"),
            u32::try_from(s).expect("s"),
        )
        .single()
        .expect("time")
        .timestamp_millis()
    }

    const MIN: i64 = 60_000;

    /// One plan at 20:00 with 60-minute study blocks and 10-minute breaks.
    fn rig() -> Mutex<Connection> {
        let mut conn = open_in_memory().expect("db");
        let config = ModeConfig {
            level: StudyLevel::Medium,
            study_min: 60,
            break_min: 10,
            long_break_min: None,
            sets_before_long_break: None,
        };
        store::put_mode_config(&conn, &config).expect("mode");
        let input = CreatePlanInput {
            name: "수학".into(),
            subject: "수학".into(),
            start_date: "2026-11-05".into(),
            start_time: "20:00".into(),
            sets: 3,
            repeat: None,
        };
        create_plan(&mut conn, &Utc, input, t(0, 0, 0) - 86_400_000).expect("plan");
        Mutex::new(conn)
    }

    fn state_at(db: &Mutex<Connection>, now: i64) -> TimerState {
        let mut conn = db.lock().expect("db");
        timer::reconcile(&mut conn, &Utc, now)
            .expect("reconcile")
            .state
    }

    fn self_reports(db: &Mutex<Connection>) -> Vec<(i64, Option<String>)> {
        let conn = db.lock().expect("db");
        let mut stmt = conn
            .prepare(
                "SELECT at_ms, app_id FROM focus_events WHERE kind = 'self_report' ORDER BY id",
            )
            .expect("prepare");
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("query")
            .map(|r| r.expect("row"))
            .collect()
    }

    fn report(db: &Mutex<Connection>, state: &TimerState, at_ms: i64) -> SelfReportOutcome {
        record_in(
            &mut db.lock().expect("db"),
            enforce::study_window(state),
            at_ms,
            true,
        )
        .expect("record")
    }

    #[test]
    fn a_shortcut_press_during_a_locked_study_block_writes_one_self_report_row_without_app_id() {
        let db = rig();
        let state = state_at(&db, t(20, 5, 0));
        assert!(state.lock_held);
        assert_eq!(
            report(&db, &state, t(20, 5, 0)),
            SelfReportOutcome::Recorded
        );
        assert_eq!(self_reports(&db), vec![(t(20, 5, 0), None)]);
    }

    #[test]
    fn presses_outside_a_study_block_are_ignored_and_write_nothing() {
        let db = rig();
        // Before the plan: the lock is free.
        let before = state_at(&db, t(19, 0, 0));
        assert_eq!(
            report(&db, &before, t(19, 0, 0)),
            SelfReportOutcome::Ignored {
                reason: IgnoreReason::NotStudying
            }
        );
        // Break between the first and second set (21:00..21:10).
        let during_break = state_at(&db, t(21, 5, 0));
        assert!(
            during_break.lock_held,
            "the lock is held during a break, but it is not a study phase"
        );
        assert_eq!(
            report(&db, &during_break, t(21, 5, 0)),
            SelfReportOutcome::Ignored {
                reason: IgnoreReason::NotStudying
            }
        );
        assert!(self_reports(&db).is_empty());
    }

    #[test]
    fn a_block_whose_lock_just_ended_receives_no_row() {
        let db = rig();
        let state = state_at(&db, t(20, 5, 0));
        crate::testutil::set_lock_for_test(&db.lock().expect("db"), false);
        assert_eq!(
            report(&db, &state, t(20, 5, 1)),
            SelfReportOutcome::Ignored {
                reason: IgnoreReason::NotStudying
            }
        );
        assert!(self_reports(&db).is_empty());
    }

    #[test]
    fn presses_within_five_seconds_are_one_report_and_five_seconds_apart_are_two() {
        let db = rig();
        let state = state_at(&db, t(20, 5, 0));
        assert_eq!(
            report(&db, &state, t(20, 5, 0)),
            SelfReportOutcome::Recorded
        );
        assert_eq!(
            report(&db, &state, t(20, 5, 4)),
            SelfReportOutcome::Ignored {
                reason: IgnoreReason::TooSoon
            }
        );
        assert_eq!(
            report(&db, &state, t(20, 5, 5)),
            SelfReportOutcome::Recorded
        );
        assert_eq!(self_reports(&db).len(), 2);
    }

    fn window(run: i64, segment: i64) -> Option<StudyWindow> {
        Some(StudyWindow {
            run_id: Some(run),
            segment_id: Some(segment),
        })
    }

    #[test]
    fn the_first_check_in_is_scheduled_at_a_random_time_within_the_configured_range() {
        // below(26) values: 0 -> 15 minutes, 25 -> 40 minutes.
        for (draw, minutes) in [(0, 15), (25, 40)] {
            let hub = hub(&[draw]);
            let start = t(20, 0, 0);
            assert_eq!(hub.tick(window(1, 1), start).expect("tick"), None);
            assert_eq!(
                hub.tick(window(1, 1), start + minutes * MIN - 1)
                    .expect("tick"),
                None
            );
            let prompt = hub
                .tick(window(1, 1), start + minutes * MIN)
                .expect("tick")
                .expect("due");
            assert_eq!(
                (prompt.run_id, prompt.segment_id, prompt.asked_at_ms),
                (1, 1, start + minutes * MIN)
            );
        }
        assert_eq!(CHECKIN_MIN_MINUTES + 25, CHECKIN_MAX_MINUTES);
    }

    #[test]
    fn leaving_the_study_block_drops_the_schedule_and_the_pending_prompt() {
        let hub = hub(&[0, 0, 0]);
        let start = t(20, 0, 0);
        hub.tick(window(1, 1), start).expect("tick");
        let prompt = hub
            .tick(window(1, 1), start + 15 * MIN)
            .expect("tick")
            .expect("due");
        assert_eq!(
            hub.pending(prompt.asked_at_ms).expect("pending"),
            Some(prompt)
        );
        // A break: nothing is asked and the prompt is gone.
        assert_eq!(hub.tick(None, start + 16 * MIN).expect("tick"), None);
        assert_eq!(hub.pending(start + 16 * MIN).expect("pending"), None);
        // The next block starts a new schedule from its own start, not an old one.
        let next_start = start + 40 * MIN;
        assert_eq!(hub.tick(window(1, 2), next_start).expect("tick"), None);
        assert_eq!(
            hub.tick(window(1, 2), next_start + 14 * MIN).expect("tick"),
            None
        );
        assert!(
            hub.tick(window(1, 2), next_start + 15 * MIN)
                .expect("tick")
                .is_some()
        );
    }

    #[test]
    fn an_unanswered_check_in_lapses_after_two_minutes_and_the_next_one_is_rescheduled() {
        let hub = hub(&[0, 0]);
        let start = t(20, 0, 0);
        hub.tick(window(1, 1), start).expect("tick");
        let asked = start + 15 * MIN;
        assert!(hub.tick(window(1, 1), asked).expect("tick").is_some());
        assert_eq!(hub.tick(window(1, 1), asked + 119_000).expect("tick"), None);
        assert!(hub.pending(asked + 119_000).expect("pending").is_some());
        assert_eq!(hub.pending(asked + 120_000).expect("pending"), None);
        hub.tick(window(1, 1), asked + 120_000).expect("tick");
        // Next one is 15 minutes after the first prompt, so it is due again soon after the lapse.
        assert!(
            hub.tick(window(1, 1), asked + 15 * MIN)
                .expect("tick")
                .is_some()
        );
    }

    #[test]
    fn a_clock_set_back_does_not_push_the_check_in_far_into_the_future() {
        let hub = hub(&[25, 0]);
        let start = t(20, 0, 0);
        hub.tick(window(1, 1), start).expect("tick"); // due at +40 minutes
        let back = start - 3 * 60 * MIN;
        assert_eq!(hub.tick(window(1, 1), back).expect("tick"), None);
        assert!(
            hub.tick(window(1, 1), back + 15 * MIN)
                .expect("tick")
                .is_some()
        );
    }

    #[test]
    fn the_due_check_in_notifies_once_with_a_body_that_carries_no_subject() {
        let hub = hub(&[0, 0]);
        let notifier = FakeNotifier::default();
        let start = t(20, 0, 0);
        checkin_pass(&hub, &notifier, window(1, 1), start).expect("pass");
        assert!(notifier.sent().is_empty());
        let due = start + 15 * MIN;
        assert!(
            checkin_pass(&hub, &notifier, window(1, 1), due)
                .expect("pass")
                .is_some()
        );
        assert!(
            checkin_pass(&hub, &notifier, window(1, 1), due + 5_000)
                .expect("pass")
                .is_none()
        );
        let sent = notifier.sent();
        assert_eq!(sent.len(), 1);
        assert!(
            !sent[0].1.contains("수학"),
            "a lock screen must not show the subject"
        );
    }

    #[test]
    fn answering_not_focused_records_a_self_report_and_focused_stores_nothing() {
        let db = rig();
        let state = state_at(&db, t(20, 0, 1));
        let win = enforce::study_window(&state).expect("window");
        let hub = hub(&[0, 0]);
        hub.tick(Some(win), t(20, 0, 1)).expect("tick");
        let asked = t(20, 0, 1) + 15 * MIN;
        let later = state_at(&db, asked);
        let win = enforce::study_window(&later);
        hub.tick(win, asked).expect("tick").expect("due");

        let at = asked + 20_000;
        assert_eq!(
            answer_checkin(&db, &hub, win, at, false).expect("answer"),
            SelfReportOutcome::Recorded
        );
        assert_eq!(self_reports(&db), vec![(at, None)]);
        // The prompt is consumed: a second answer is refused.
        assert!(answer_checkin(&db, &hub, win, at + 1_000, false).is_err());

        // A focused answer to the next prompt stores nothing.
        let second = asked + 15 * MIN;
        let win = enforce::study_window(&state_at(&db, second));
        hub.tick(win, second - 1).expect("tick");
        hub.tick(win, second).expect("tick").expect("due");
        assert_eq!(
            answer_checkin(&db, &hub, win, second + 1_000, true).expect("answer"),
            SelfReportOutcome::Acknowledged
        );
        assert_eq!(self_reports(&db).len(), 1);
    }

    #[test]
    fn a_late_or_wrong_segment_answer_is_refused_and_writes_nothing() {
        let db = rig();
        let state = state_at(&db, t(20, 0, 1));
        let win = enforce::study_window(&state);
        let hub = hub(&[0]);
        hub.tick(win, t(20, 0, 1)).expect("tick");
        let asked = t(20, 0, 1) + 15 * MIN;
        hub.tick(win, asked).expect("tick").expect("due");
        // Two minutes later the prompt has lapsed.
        assert!(answer_checkin(&db, &hub, win, asked + CHECKIN_ANSWER_WINDOW_MS, false).is_err());

        let hub = hub_with_prompt(win, asked);
        let other = Some(StudyWindow {
            run_id: Some(99),
            segment_id: Some(99),
        });
        assert!(answer_checkin(&db, &hub, other, asked + 1_000, false).is_err());
        assert!(
            answer_checkin(
                &db,
                &hub_with_prompt(win, asked),
                None,
                asked + 1_000,
                false
            )
            .is_err()
        );
        assert!(self_reports(&db).is_empty());
    }

    fn hub_with_prompt(win: Option<StudyWindow>, asked: i64) -> SignalHub {
        let hub = hub(&[0]);
        hub.tick(win, asked - 15 * MIN).expect("tick");
        hub.tick(win, asked).expect("tick").expect("due");
        hub
    }

    #[test]
    fn answers_outside_a_study_block_are_refused() {
        let db = rig();
        let state = state_at(&db, t(20, 0, 1));
        let win = enforce::study_window(&state);
        let asked = t(20, 0, 1) + 15 * MIN;
        let hub = hub_with_prompt(win, asked);
        assert!(answer_checkin(&db, &hub, None, asked + 1_000, false).is_err());
        assert!(self_reports(&db).is_empty());
    }

    #[test]
    fn system_random_stays_within_the_bound() {
        let mut rng = SystemRandom::default();
        assert!((0..200).all(|_| rng.below(26) < 26));
        assert_eq!(rng.below(1), 0);
        assert_eq!(rng.below(0), 0);
    }
}
