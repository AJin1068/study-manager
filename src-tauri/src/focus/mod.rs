//! Focus measurement (stage 1-5). While a study block holds the lock, a sampler reads the front
//! app and the input-idle time every few seconds and feeds the pure tracker in `calc`; the
//! resulting events and minute buckets are stored. Nothing is recorded outside a locked study
//! block, and no window title, URL, or input content is ever read or stored: app ids and counts.
//!
//! Focus is a separate measure from block attempts. `away` events never feed `attempt_count`
//! (that comes from `block_events` alone), and the focus score ignores attempts (D-25), so one
//! trip into a blocked app is one attempt and, at most, one deviation, never a double penalty.

pub mod calc;
pub mod model;
pub mod signals;
#[cfg(test)]
mod tests;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chrono::Local;
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;

use crate::clock::now_ms;
use crate::commands::lock_db;
use crate::enforce::model::EntryKind;
use crate::enforce::{self, StudyWindow};
use crate::error::{AppError, AppResult};
use crate::platform::{AppIdentity, FocusProbe, TitleAccess};
use crate::settings::{Outcome, Reason, UpdateOutcome};
use crate::store;
use crate::timer::model::SegmentRow;
use crate::timer::{self, HighWater};

use calc::{Foreground, Observation, SAMPLE_INTERVAL_MS, Tracker};
use model::NewFocusEvent;

pub(crate) const STUDY_APP_LIST_KEY: &str = "studyAppList";

// ---- study-app list ----

/// Runs `apply` inside an Immediate transaction unless the lock is held: during study the change
/// is refused and recorded, because adding a distraction as a "study app" would be a way around
/// the measurement.
fn apply_unlocked(
    conn: &mut Connection,
    at_ms: i64,
    after: &str,
    apply: impl FnOnce(&Connection) -> AppResult<()>,
) -> AppResult<UpdateOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if crate::study::lock_held(&tx)? {
        store::append_history(
            &tx,
            at_ms,
            STUDY_APP_LIST_KEY,
            None,
            after,
            Reason::User,
            Outcome::RejectedLocked,
        )?;
        tx.commit()?;
        return Ok(UpdateOutcome::RejectedLocked {
            key: STUDY_APP_LIST_KEY.into(),
        });
    }
    apply(&tx)?;
    store::append_history(
        &tx,
        at_ms,
        STUDY_APP_LIST_KEY,
        None,
        after,
        Reason::User,
        Outcome::Applied,
    )?;
    tx.commit()?;
    Ok(UpdateOutcome::Applied {
        key: STUDY_APP_LIST_KEY.into(),
    })
}

pub fn add_study_app(
    conn: &mut Connection,
    raw_app_id: &str,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    let app_id = enforce::checked_app_id(raw_app_id)?;
    apply_unlocked(conn, at_ms, &format!("add:{app_id}"), |tx| {
        if store::insert_study_app(tx, EntryKind::App, &app_id, at_ms)? {
            Ok(())
        } else {
            Err(AppError::Invalid(
                "this app is already on the study app list".into(),
            ))
        }
    })
}

pub fn remove_study_app(conn: &mut Connection, id: i64, at_ms: i64) -> AppResult<UpdateOutcome> {
    apply_unlocked(conn, at_ms, &format!("remove:{id}"), |tx| {
        if store::delete_study_app(tx, id)? {
            Ok(())
        } else {
            Err(AppError::Invalid("unknown study app".into()))
        }
    })
}

pub fn set_study_app_enabled(
    conn: &mut Connection,
    id: i64,
    enabled: bool,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    let after = format!("{}:{id}", if enabled { "enable" } else { "disable" });
    apply_unlocked(conn, at_ms, &after, |tx| {
        if store::set_study_app_enabled(tx, id, enabled)? {
            Ok(())
        } else {
            Err(AppError::Invalid("unknown study app".into()))
        }
    })
}

/// With no enabled study app nothing can be recognized, so every minute would score zero (D-31).
pub fn needs_study_apps(enabled_count: usize) -> bool {
    enabled_count == 0
}

/// What the first-run screen and the dashboard warning need.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FocusSetup {
    pub study_app_count: i64,
    /// Show "공부 앱을 선택하세요": the study-app list has no enabled entry.
    pub needs_study_apps: bool,
    pub title_access: TitleAccess,
    /// The foreground and idle sensors answer on this machine.
    pub sensors_available: bool,
}

pub fn setup_status(conn: &Connection, probe: &dyn FocusProbe) -> AppResult<FocusSetup> {
    let enabled = store::list_enabled_study_app_ids(conn, EntryKind::App)?.len();
    Ok(FocusSetup {
        study_app_count: i64::try_from(enabled)
            .map_err(|_| AppError::Internal("study app count overflows".into()))?,
        needs_study_apps: needs_study_apps(enabled),
        title_access: probe.window_title_access(),
        sensors_available: probe.frontmost_app().is_ok() && probe.idle_seconds().is_ok(),
    })
}

// ---- classification ----

/// This app in front counts as studying: looking at the timer must not read as a deviation.
pub fn classify(front: Option<&AppIdentity>, study_ids: &[String], self_pid: u32) -> Foreground {
    match front {
        None => Foreground::Unknown,
        Some(app) if app.pid == self_pid || study_ids.contains(&app.id) => Foreground::Study,
        Some(app) => Foreground::Other(app.id.clone()),
    }
}

// ---- recording ----

#[derive(Clone, Copy, Debug)]
struct Scope {
    run_id: i64,
    segment_id: i64,
    started_at_ms: i64,
}

/// Memory of the sampler thread: the tracker of the current study segment.
#[derive(Default)]
pub struct Recorder {
    scope: Option<Scope>,
    tracker: Tracker,
    /// The sensors failed on the previous sample, so the failure is logged once, not every tick.
    probe_failed: bool,
    /// When the opt-in study context last looked at the front study app.
    context_sampled_ms: i64,
}

impl Recorder {
    fn leave(&mut self) {
        self.scope = None;
        self.tracker = Tracker::default();
    }
}

/// One sampling pass. `window` is `None` outside a locked study block, which forgets the segment
/// and records nothing (breaks and idle time are never measured).
pub fn sample(
    db: &Mutex<Connection>,
    window: Option<StudyWindow>,
    probe: &dyn FocusProbe,
    rec: &mut Recorder,
    self_pid: u32,
    now_ms: i64,
) -> AppResult<()> {
    let Some(StudyWindow {
        run_id: Some(run_id),
        segment_id: Some(segment_id),
    }) = window
    else {
        rec.leave();
        return Ok(());
    };
    if rec.scope.map(|s| (s.run_id, s.segment_id)) != Some((run_id, segment_id)) {
        let segment = store::get_segment(&*lock_db(db)?, segment_id)?
            .ok_or_else(|| AppError::Internal("study segment is missing".into()))?;
        rec.tracker = Tracker::default();
        rec.scope = Some(Scope {
            run_id,
            segment_id,
            started_at_ms: segment.started_at_ms,
        });
    }
    let Some(scope) = rec.scope else {
        return Ok(());
    };

    let study_ids = store::list_enabled_study_app_ids(&*lock_db(db)?, EntryKind::App)?;
    let (front, idle_secs) = match (probe.frontmost_app(), probe.idle_seconds()) {
        (Ok(front), Ok(idle)) => (front, idle),
        (Err(err), _) | (_, Err(err)) => {
            if !rec.probe_failed {
                tracing::warn!(event = "focus_probe_failed", error = %err);
                rec.probe_failed = true;
            }
            return Ok(());
        }
    };
    rec.probe_failed = false;
    // The opt-in "what was I studying" label; `capture` returns at once unless the user turned it on.
    if now_ms.saturating_sub(rec.context_sampled_ms) >= crate::study_context::SAMPLE_EVERY_MS {
        rec.context_sampled_ms = now_ms;
        if let Some(app) = front
            .as_ref()
            .filter(|a| a.pid != self_pid && study_ids.contains(&a.id))
        {
            if let Err(err) =
                crate::study_context::capture(db, probe, app, scope.segment_id, now_ms)
            {
                tracing::warn!(event = "study_context_failed", error = %err);
            }
        }
    }
    let idle_ms = i64::try_from(idle_secs.saturating_mul(1000)).unwrap_or(i64::MAX);
    let obs = Observation {
        at_ms: now_ms,
        foreground: classify(front.as_ref(), &study_ids, self_pid),
        idle_ms,
    };
    let step = rec.tracker.step(&obs, scope.started_at_ms);
    if step.events.is_empty() && step.minutes.is_empty() {
        return Ok(());
    }

    let mut conn = lock_db(db)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // A release between the timer read and now must not add time to a block that is no longer held.
    if !crate::study::lock_held(&tx)? {
        return Ok(());
    }
    for event in &step.events {
        let row = NewFocusEvent {
            run_id: scope.run_id,
            segment_id: scope.segment_id,
            at_ms: event.at_ms,
            kind: event.kind,
            app_id: event.app_id.as_deref(),
        };
        store::insert_focus_event(&tx, &row)?;
    }
    for minute in &step.minutes {
        store::add_focus_minute(&tx, scope.segment_id, minute)?;
    }
    tx.commit()?;
    for event in &step.events {
        // App id only; the display name and any title stay out of the log.
        tracing::info!(
            event = event.kind.as_str(),
            app_id = event.app_id.as_deref().unwrap_or("none"),
            "focus"
        );
    }
    Ok(())
}

fn pass(
    db: &Mutex<Connection>,
    high_water: &HighWater,
    probe: &dyn FocusProbe,
    rec: &mut Recorder,
) -> AppResult<()> {
    let effective = high_water.observe(now_ms()?);
    let state = timer::current_state(&*lock_db(db)?, &Local, effective)?;
    sample(
        db,
        enforce::study_window(&state),
        probe,
        rec,
        std::process::id(),
        effective,
    )
}

/// Starts the sampling thread. A failing or panicking pass is logged and retried at the next
/// interval; it never stops the thread.
pub fn spawn(db: Arc<Mutex<Connection>>, high_water: Arc<HighWater>, probe: Arc<dyn FocusProbe>) {
    thread::spawn(move || {
        let mut rec = Recorder::default();
        loop {
            thread::sleep(Duration::from_millis(SAMPLE_INTERVAL_MS.unsigned_abs()));
            let result = catch_unwind(AssertUnwindSafe(|| {
                pass(&db, &high_water, probe.as_ref(), &mut rec)
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::error!(event = "focus_pass_failed", error = %err),
                Err(_) => tracing::error!(
                    event = "focus_pass_panicked",
                    "focus pass panicked; retrying"
                ),
            }
        }
    });
}

// ---- summaries ----

/// What a finished set adds to its summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentFocus {
    pub credited_ms: i64,
    pub score: Option<i64>,
    pub first_away_after_ms: Option<i64>,
    pub return_after_ms: Option<i64>,
}

/// Focus of one study segment. `studied_ms` is the study time with downtime removed, the same
/// number the set summary reports, so score and summary agree.
pub fn segment_focus(
    conn: &Connection,
    segment: &SegmentRow,
    studied_ms: i64,
) -> AppResult<SegmentFocus> {
    let totals = store::focus_totals(conn, segment.run_id, Some(segment.id))?;
    let events = store::list_focus_events(conn, segment.id)?;
    let metrics = calc::event_metrics(&events, segment.started_at_ms);
    Ok(SegmentFocus {
        credited_ms: totals.credited_ms,
        score: calc::focus_score(totals.credited_ms, studied_ms),
        first_away_after_ms: metrics.first_away_after_ms,
        return_after_ms: metrics.return_after_ms,
    })
}

/// Recognized time and score of a whole run.
pub fn run_focus(conn: &Connection, run_id: i64, studied_ms: i64) -> AppResult<(i64, Option<i64>)> {
    let totals = store::focus_totals(conn, run_id, None)?;
    Ok((
        totals.credited_ms,
        calc::focus_score(totals.credited_ms, studied_ms),
    ))
}
