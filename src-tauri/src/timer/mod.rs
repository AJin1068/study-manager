//! Timer engine core: the calendar decides the lock. `derive` (pure) turns plans, run history,
//! and the effective clock into the rows that must exist; `reconcile` applies that in one
//! Immediate transaction. Whatever wakes the engine (tick, wake from sleep, restart, plan edit),
//! the same `reconcile` runs, so every interruption heals by recomputation.
//!
//! `study_state` is written only here: by `reconcile` and by `end_active_runs`, the release
//! exit (`study_pass`) that does not depend on the engine thread.
//!
//! Clock policy: run bounds are UTC and frozen at creation (a time zone or DST change cannot move
//! a run). The engine never trusts a clock that went backwards: `HighWater` keeps the highest time
//! seen, and `effective_now = max(now, high_water)`, so setting the clock back neither releases
//! the lock nor rewinds a segment. A forward jump is allowed (it cannot be told from sleep);
//! `clock_jump_intervals` derives the unexplained stretches at read time.

use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{Duration, NaiveDate, TimeZone};
use rusqlite::{Connection, TransactionBehavior};

use crate::downtime::{self, Interval};
use crate::error::{AppError, AppResult};
use crate::plan::{self, ModeLookup, Occurrence};
use crate::store;

pub mod derive;
pub mod engine;
pub mod model;
pub mod notice;
pub mod release;
pub mod study_pass;
pub mod summary;
#[cfg(test)]
mod tests;

use derive::{derive, DeriveInput, Derived, RunDelta, RunTarget, RunWithSegments};
use model::{EndReason, RunStatus, TimerState};

/// Days before today that are scanned for runs to catch up (missed marking, late closing).
const LOOKBACK_DAYS: i64 = 7;
/// Days ahead scanned for the "next plan" display and overlap shifting.
const LOOKAHEAD_DAYS: i64 = 14;
const DATE_FMT: &str = "%Y-%m-%d";

/// Highest wall-clock time observed so far. Shared by the engine, commands, and abort so every
/// reader of "now" agrees and none of them can be walked backwards.
pub struct HighWater(AtomicI64);

impl HighWater {
    /// `floor` is the persisted lower bound (highest session heartbeat) at startup.
    pub fn new(floor: i64) -> HighWater {
        HighWater(AtomicI64::new(floor))
    }

    /// Records `now` and returns the effective time: never earlier than anything seen before.
    pub fn observe(&self, now: i64) -> i64 {
        self.0.fetch_max(now, Ordering::SeqCst).max(now)
    }

    pub fn get(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn local_date<Tz: TimeZone>(tz: &Tz, at_ms: i64) -> AppResult<NaiveDate> {
    tz.timestamp_millis_opt(at_ms)
        .single()
        .map(|dt| dt.date_naive())
        .ok_or_else(|| AppError::Internal("timestamp has no local date".into()))
}

struct Loaded {
    occurrences: Vec<Occurrence>,
    modes: ModeLookup,
    runs: Vec<RunWithSegments>,
}

fn load<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<Loaded> {
    let today = local_date(tz, now_ms)?;
    let from = today - Duration::days(LOOKBACK_DAYS);
    let to = today + Duration::days(LOOKAHEAD_DAYS);
    let (occurrences, modes) = plan::occurrences_in(conn, tz, from, to)?;
    let mut runs = Vec::new();
    for run in store::list_runs_since(conn, &from.format(DATE_FMT).to_string())? {
        let segments = if run.status == RunStatus::Active { store::list_segments(conn, run.id)? } else { Vec::new() };
        runs.push(RunWithSegments { run, segments });
    }
    Ok(Loaded { occurrences, modes, runs })
}

/// Read-only derivation at `now_ms` (already an effective time). Writes nothing.
fn project<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<Derived> {
    let loaded = load(conn, tz, now_ms)?;
    derive(&DeriveInput { now_ms, occurrences: &loaded.occurrences, modes: &loaded.modes, runs: &loaded.runs })
}

/// Snapshot for `get_timer_state`. Read-only, so it is correct even while a reconcile is racing.
/// The lock row must exist: an unknown lock is an error, never a quiet "idle".
pub fn current_state<Tz: TimeZone>(conn: &Connection, tz: &Tz, effective_now_ms: i64) -> AppResult<TimerState> {
    store::read_study_state(conn)?;
    Ok(project(conn, tz, effective_now_ms)?.state)
}

#[derive(Debug)]
pub struct Reconciled {
    /// True when rows or the lock changed.
    pub changed: bool,
    pub state: TimerState,
}

fn apply_delta(conn: &Connection, delta: &RunDelta) -> AppResult<()> {
    let run_id = match &delta.target {
        RunTarget::Existing(id) => *id,
        RunTarget::New(run) => {
            let id = store::insert_run(conn, run)?;
            tracing::info!(event = "run_created", run_id = id, status = run.status.as_str());
            id
        }
    };
    for close in &delta.closes {
        store::close_segment(conn, close.segment_id, close.ended_at_ms, close.reason)?;
        tracing::info!(
            event = "timer_transition",
            run_id,
            segment_id = close.segment_id,
            transition = "close",
            reason = close.reason.as_str(),
            at_ms = close.ended_at_ms
        );
    }
    for open in &delta.opens {
        let segment_id = store::insert_segment(conn, run_id, open)?;
        tracing::info!(
            event = "timer_transition",
            run_id,
            segment_id,
            transition = "open",
            kind = model::kind_str(open.kind),
            late_join = open.late_join,
            at_ms = open.started_at_ms
        );
    }
    if let Some(finish) = &delta.finish {
        store::finish_run(conn, run_id, finish.status, finish.ended_at_ms, finish.detail_code)?;
        tracing::info!(event = "run_finished", run_id, status = finish.status.as_str(), at_ms = finish.ended_at_ms);
    }
    Ok(())
}

/// Brings the database in line with the calendar at `effective_now_ms` and returns the resulting
/// state. A read-only pre-check keeps the idle ticks from taking the write lock; a change is then
/// re-derived inside the transaction so it cannot act on a view another writer already changed.
/// A missing lock row is an error before anything is written.
pub fn reconcile<Tz: TimeZone>(conn: &mut Connection, tz: &Tz, effective_now_ms: i64) -> AppResult<Reconciled> {
    let (lock_before, _) = store::read_study_state(conn)?;
    let pre = project(conn, tz, effective_now_ms)?;
    if pre.deltas.is_empty() && pre.lock_target == lock_before {
        return Ok(Reconciled { changed: false, state: pre.state });
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (lock_now, _) = store::read_study_state(&tx)?;
    let plan = project(&tx, tz, effective_now_ms)?;
    for delta in &plan.deltas {
        apply_delta(&tx, delta)?;
    }
    if plan.lock_target != lock_now {
        store::put_study_state(&tx, plan.lock_target, effective_now_ms)?;
        tracing::info!(event = "lock_changed", lock_held = plan.lock_target, at_ms = effective_now_ms);
    }
    let state = project(&tx, tz, effective_now_ms)?.state;
    tx.commit()?;
    Ok(Reconciled { changed: !plan.deltas.is_empty() || plan.lock_target != lock_now, state })
}

/// Ends every active run with `status`/`reason` and frees the lock, inside the caller's
/// transaction. Returns whether anything changed: an active run (or a stuck lock) was ended.
/// A held lock with no active run (corrupt state) is released too. Terminal: an ended run is
/// never resumed. This and `reconcile` are the only writers of `study_state`; the release
/// procedures (`study_pass`) go through here and touch only the database, never the engine
/// thread, so they work when the engine is dead or degraded.
fn end_active_runs(
    tx: &Connection,
    at_ms: i64,
    status: RunStatus,
    reason: EndReason,
    detail: &'static str,
) -> AppResult<bool> {
    let (lock_held, _) = store::read_study_state(tx)?;
    let active = store::list_active_runs(tx)?;
    if active.is_empty() && !lock_held {
        return Ok(false);
    }
    for run in &active {
        for seg in store::list_segments(tx, run.id)?.into_iter().filter(|s| s.ended_at_ms.is_none()) {
            store::close_segment(tx, seg.id, at_ms.max(seg.started_at_ms), reason)?;
        }
        store::finish_run(tx, run.id, status, at_ms, Some(detail))?;
        tracing::info!(event = "run_released", run_id = run.id, status = status.as_str(), reason = detail, at_ms);
    }
    if lock_held {
        store::put_study_state(tx, false, at_ms)?;
        tracing::info!(event = "lock_changed", lock_held = false, at_ms);
    }
    Ok(true)
}

/// Stretches of the plan that elapsed unobserved and are covered by no downtime record: a
/// forward clock jump (or an untracked outage). Derived on read because the append-only history
/// cannot be corrected after the fact.
///
/// A segment the engine was watching when it started was alive until the outage began, so
/// everything before the first downtime record inside it is dropped. Without any record the
/// whole remainder of the segment is reported, which can overcount its watched lead-in: the
/// result is an upper bound there and exact for segments that were never watched.
pub fn clock_jump_intervals(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<Vec<Interval>> {
    let covers = store::list_downtime_intervals(conn, from_ms, to_ms)?;
    let mut out = Vec::new();
    for (span, watched_at_start) in store::list_unwatched_spans(conn, from_ms, to_ms)? {
        let mut clipped = Interval { start: span.start.max(from_ms), end: span.end.min(to_ms) };
        if watched_at_start {
            if let Some(first) = covers.iter().map(|c| c.start).filter(|s| *s > clipped.start && *s < clipped.end).min() {
                clipped.start = first;
            }
        }
        out.extend(downtime::subtract(clipped, &covers));
    }
    Ok(out)
}

pub fn clock_jump_ms(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<i64> {
    Ok(clock_jump_intervals(conn, from_ms, to_ms)?.iter().map(|i| i.end - i.start).sum())
}
