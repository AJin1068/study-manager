//! The desktop side of the phone lock: it looks at the timer, decides what the phone should be
//! told and publishes it as a lease (`publish_phone_lock`) from a worker thread. It also watches
//! the phone's heartbeat while a lock is wanted (gaps) and pulls the pick-up counts.
//!
//! Why a worker that reads the timer itself, not one that is handed states: the lock decision must
//! stay correct when the engine thread is late or dead (the release procedures work without it),
//! so the worker re-derives the state from the database with the same read-only projection the UI
//! uses. The engine and the release commands only `wake` it; a wake is a channel send and never
//! waits, so a slow or dead server cannot touch a tick. Network calls run with the database lock
//! released.
//!
//! What is published (one publication per change, never twice for the same thing):
//! - a study segment that holds the lock: `Locked` until the segment's end plus the grace;
//! - a break, the end of the run, an emergency pass, "skip today", an early release: `Free`
//!   with the matching cause, at once (the pass and release rows are in the database before the
//!   command returns, and the command wakes the worker);
//! - after an emergency pass ends inside a study segment the lock is published again.
//!
//! `Free` is published only when the server holds a `Locked` lease: a phone that was never locked
//! needs nothing. The lease's `run_id` field carries the segment key `"<run>:<segment>"`; the phone
//! reports its pick-up counts under the same key.
//!
//! Sequence numbers: one above the highest ever used here and above anything the server says it
//! holds (`stale_seq` carries it), kept in `phone_publish`, so a restart never reuses a number. A
//! failed publication is retried with the SAME number (the server treats a repeat as already
//! applied) and growing waits; nothing is published for a state that is no longer wanted.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chrono::TimeZone;
use link_protocol::{LockLease, ReleaseCause, GRACE_MS, MAX_PICKUPS_PER_REPORT, MAX_SEGMENT_MS};
use rusqlite::{params, Connection};
use serde::Serialize;

use super::gap::{GapTracker, Heard, Observation, Watch};
use super::model::{cause_text, state_text, PhoneError};
use super::pairing::PairingBackend;
use crate::commands::lock_db;
use crate::error::AppResult;
use crate::plan::BlockKind;
use crate::remote::Clock;
use crate::store::{self, phone as repo};
use crate::timer::model::{Phase, TimerState};
use crate::timer::{self, study_pass, HighWater};

const BUSY_WAIT: Duration = Duration::from_secs(1);
const IDLE_WAIT: Duration = Duration::from_secs(5);
/// How often the phone's heartbeat is read while a lock is wanted (the phone beats every 15 s).
pub const GAP_POLL_MS: i64 = 15_000;
pub const PICKUP_POLL_MS: i64 = 30_000;
const RETRY_FIRST_MS: i64 = 2_000;
const RETRY_MAX_MS: i64 = 30_000;
/// Sequence catch-ups per look: one is the normal case (a restart with a wiped local database).
const STALE_RETRIES: usize = 3;

// ---- segment keys ----

/// The text that names a study segment to the phone: the lease's `run_id` and the pick-up
/// report's `segment_id`.
pub fn segment_key(run_id: i64, segment_id: i64) -> String {
    format!("{run_id}:{segment_id}")
}

pub fn parse_segment_key(key: &str) -> Option<(i64, i64)> {
    let (run, segment) = key.split_once(':')?;
    let (run, segment) = (run.parse::<i64>().ok()?, segment.parse::<i64>().ok()?);
    (run > 0 && segment > 0 && segment_key(run, segment) == key).then_some((run, segment))
}

// ---- the decision (pure) ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Desired {
    /// Nothing to tell the phone (never locked, or the engine has not written the segment yet).
    Nothing,
    Lock { key: String, run_id: i64, segment_started_ms: i64, segment_end_ms: i64 },
    Free { key: String, cause: ReleaseCause },
}

pub struct Facts<'a> {
    pub state: &'a TimerState,
    pub now_ms: i64,
    pub segment_started_ms: Option<i64>,
    pub emergency_pass_active: bool,
    /// The segment key of the last lease the server holds.
    pub last_key: Option<&'a str>,
    /// How the run named by `last_key` was released, when it was ("skip today", "early release").
    pub last_run_release: Option<ReleaseCause>,
}

/// What the phone should be told for these facts. A study segment holds the phone; a break, the
/// end of the run or a pass frees it.
pub fn decide(facts: &Facts<'_>) -> Desired {
    let state = facts.state;
    match state.phase {
        Phase::Study => {
            let (Some(run_id), Some(segment_id), Some(end), Some(started)) =
                (state.run_id, state.segment_id, state.planned_end_ms, facts.segment_started_ms)
            else {
                return Desired::Nothing;
            };
            let key = segment_key(run_id, segment_id);
            if facts.emergency_pass_active {
                Desired::Free { key, cause: ReleaseCause::EmergencyPass }
            } else if end.saturating_add(GRACE_MS) <= facts.now_ms {
                // The segment ran out and the engine has not moved on yet.
                Desired::Free { key, cause: ReleaseCause::RunEnded }
            } else {
                Desired::Lock { key, run_id, segment_started_ms: started, segment_end_ms: end }
            }
        }
        Phase::Break | Phase::LongBreak => match (state.run_id, state.segment_id) {
            (Some(run), Some(segment)) => Desired::Free { key: segment_key(run, segment), cause: ReleaseCause::RunEnded },
            _ => Desired::Nothing,
        },
        Phase::Idle => match facts.last_key {
            Some(key) => Desired::Free { key: key.to_string(), cause: facts.last_run_release.unwrap_or(ReleaseCause::RunEnded) },
            None => Desired::Nothing,
        },
    }
}

/// What a lease is about: the part of it that makes two publications "the same".
#[derive(Clone, Debug, PartialEq, Eq)]
struct Signature {
    key: String,
    state: &'static str,
    cause: Option<&'static str>,
}

fn signature_of(desired: &Desired) -> Option<Signature> {
    match desired {
        Desired::Nothing => None,
        Desired::Lock { key, .. } => Some(Signature { key: key.clone(), state: "locked", cause: None }),
        Desired::Free { key, cause } => Some(Signature { key: key.clone(), state: "free", cause: Some(cause_text(*cause)) }),
    }
}

fn published_matches(row: &repo::PublishRow, sig: &Signature) -> bool {
    row.last_key.as_deref() == Some(sig.key.as_str()) && row.last_state.as_deref() == Some(sig.state) && row.last_cause.as_deref() == sig.cause
}

/// The lease for `desired` at sequence number `seq`. A lock that would outlast the segment cap
/// is cut at the cap (the server and the phone refuse longer ones).
fn lease_for(desired: &Desired, seq: i64, now_ms: i64) -> Option<LockLease> {
    match desired {
        Desired::Nothing => None,
        Desired::Lock { key, segment_end_ms, .. } => {
            LockLease::locked(key, seq, now_ms, (*segment_end_ms).min(now_ms.saturating_add(MAX_SEGMENT_MS))).ok()
        }
        Desired::Free { key, cause } => LockLease::free(key, seq, now_ms, *cause).ok(),
    }
}

fn retry_delay_ms(failures: u32) -> i64 {
    RETRY_FIRST_MS.saturating_mul(1_i64 << failures.saturating_sub(1).min(5)).min(RETRY_MAX_MS)
}

// ---- diagnostics for the UI ----

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedView {
    pub seq: i64,
    pub state: &'static str,
    pub reason: Option<&'static str>,
    pub at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenGapView {
    pub kind: &'static str,
    pub since_ms: i64,
}

/// What the PC shows about the link while a lock is wanted. Ids, counts and times only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PhoneLinkView {
    pub paired: bool,
    /// A study segment currently holds the lock, so the phone should be locked.
    pub want_locked: bool,
    pub published: Option<PublishedView>,
    /// Publications have failed in a row (the server could not be reached with the lock).
    pub publish_failing: bool,
    pub last_error: Option<String>,
    /// Gaps happening now (a row is written when each ends).
    pub open_gaps: Vec<OpenGapView>,
    pub last_beat_ms: Option<i64>,
    pub service_enabled: Option<bool>,
}

/// Shared between the worker (writer) and the command (reader).
#[derive(Default)]
pub struct Diagnostics {
    inner: Mutex<PhoneLinkView>,
}

impl Diagnostics {
    pub fn view(&self) -> PhoneLinkView {
        self.inner.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn update(&self, change: impl FnOnce(&mut PhoneLinkView)) {
        if let Ok(mut view) = self.inner.lock() {
            change(&mut view);
        }
    }
}

// ---- the worker ----

/// Wakes the worker. Cloneable; sending never waits.
#[derive(Clone)]
pub struct PublisherHandle {
    tx: Sender<()>,
}

impl PublisherHandle {
    pub fn wake(&self) {
        // The worker is gone only at shutdown; nothing to do then.
        let _ = self.tx.send(());
    }
}

struct Pending {
    signature: Signature,
    desired: Desired,
    lease: LockLease,
    failures: u32,
    next_try_ms: i64,
}

pub struct Publisher<Tz: TimeZone> {
    db: Arc<Mutex<Connection>>,
    clock: Arc<dyn Clock>,
    backend: Arc<dyn PairingBackend>,
    tz: Tz,
    high_water: Arc<HighWater>,
    diag: Arc<Diagnostics>,
    pending: Option<Pending>,
    tracker: GapTracker,
    heard: Option<Heard>,
    next_gap_poll_ms: i64,
    next_pickup_poll_ms: i64,
    was_locked: bool,
}

struct Read {
    paired: bool,
    state: TimerState,
    segment_started_ms: Option<i64>,
    emergency_pass_active: bool,
    publish: repo::PublishRow,
    last_run_release: Option<ReleaseCause>,
    effective_now_ms: i64,
}

impl<Tz: TimeZone> Publisher<Tz> {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        clock: Arc<dyn Clock>,
        backend: Arc<dyn PairingBackend>,
        tz: Tz,
        high_water: Arc<HighWater>,
        diag: Arc<Diagnostics>,
    ) -> Self {
        Self {
            db,
            clock,
            backend,
            tz,
            high_water,
            diag,
            pending: None,
            tracker: GapTracker::new(),
            heard: None,
            next_gap_poll_ms: 0,
            next_pickup_poll_ms: 0,
            was_locked: false,
        }
    }

    fn read(&self, now_ms: i64) -> AppResult<Read> {
        let effective = self.high_water.observe(now_ms);
        let conn = lock_db(&self.db)?;
        let paired = repo::pair(&conn)?.paired;
        let publish = repo::publish_row(&conn)?;
        let state = timer::current_state(&conn, &self.tz, effective)?;
        let segment_started_ms = match state.segment_id {
            Some(id) => store::get_segment(&conn, id)?.map(|s| s.started_at_ms),
            None => None,
        };
        let emergency_pass_active = study_pass::active_until(&conn, effective)?.is_some();
        let last_run_release = match publish.last_key.as_deref().and_then(parse_segment_key) {
            Some((run_id, _)) => released_by(&conn, run_id)?,
            None => None,
        };
        Ok(Read { paired, state, segment_started_ms, emergency_pass_active, publish, last_run_release, effective_now_ms: effective })
    }

    /// One look: publish what changed, watch the heartbeat, pull the pick-up counts. Returns how
    /// long to wait before the next look (a wake cuts the wait short).
    pub fn step(&mut self) -> Duration {
        let now = match self.clock.now_ms() {
            Ok(now) => now,
            Err(err) => {
                tracing::warn!(event = "phone_publisher_clock", error = %err);
                return IDLE_WAIT;
            }
        };
        let read = match self.read(now) {
            Ok(read) => read,
            Err(err) => {
                tracing::warn!(event = "phone_publisher_read", error = %err);
                self.diag.update(|v| v.last_error = Some("read_failed".into()));
                return IDLE_WAIT;
            }
        };
        if !read.paired {
            self.stand_down(now);
            return IDLE_WAIT;
        }
        let desired = decide(&Facts {
            state: &read.state,
            now_ms: read.effective_now_ms,
            segment_started_ms: read.segment_started_ms,
            emergency_pass_active: read.emergency_pass_active,
            last_key: read.publish.last_key.as_deref(),
            last_run_release: read.last_run_release,
        });
        self.publish(&read, &desired, now);
        self.watch_gaps(&read, &desired, now);
        let locked = matches!(desired, Desired::Lock { .. });
        if self.was_locked && !locked {
            // The phone's last count of the segment is due now.
            self.next_pickup_poll_ms = now;
        }
        self.was_locked = locked;
        self.pull_pickups(now);
        self.diag.update(|v| {
            v.paired = true;
            v.want_locked = locked;
        });
        match &self.pending {
            Some(p) => Duration::from_millis(u64::try_from((p.next_try_ms - now).max(100)).unwrap_or(1_000)).min(BUSY_WAIT),
            None => BUSY_WAIT,
        }
    }

    /// Not paired: nothing is published or watched, and any open gap is closed.
    fn stand_down(&mut self, now: i64) {
        self.pending = None;
        self.was_locked = false;
        let rows = self.tracker.observe(&Observation { now_ms: now, watch: None, heard: None, covered_until_ms: None });
        self.store_gaps(&rows);
        self.diag.update(|v| *v = PhoneLinkView::default());
    }

    fn publish(&mut self, read: &Read, desired: &Desired, now: i64) {
        let Some(signature) = signature_of(desired) else {
            self.pending = None;
            return;
        };
        let nothing_to_free = signature.state == "free" && read.publish.last_state.as_deref() != Some("locked");
        if nothing_to_free || published_matches(&read.publish, &signature) {
            self.pending = None;
            return;
        }
        if self.pending.as_ref().is_some_and(|p| p.signature != signature) {
            self.pending = None;
        }
        if self.pending.is_none() {
            let seq = match lock_db(&self.db).and_then(|conn| repo::reserve_seq(&conn, 0, now)) {
                Ok(seq) => seq,
                Err(err) => {
                    tracing::warn!(event = "phone_publisher_seq", error = %err);
                    return;
                }
            };
            let Some(lease) = lease_for(desired, seq, now) else {
                self.diag.update(|v| v.last_error = Some("lease_invalid".into()));
                return;
            };
            self.pending = Some(Pending { signature, desired: desired.clone(), lease, failures: 0, next_try_ms: now });
        }
        let Some(pending) = self.pending.as_mut() else { return };
        if now < pending.next_try_ms {
            return;
        }
        // The network call: the database lock is not held here.
        let mut outcome = self.backend.api().and_then(|api| api.publish_lock(&pending.lease));
        for _ in 0..STALE_RETRIES {
            let Err(PhoneError::Stale { current_seq }) = outcome else { break };
            let renumbered = lock_db(&self.db).and_then(|conn| repo::reserve_seq(&conn, current_seq, now)).ok().and_then(|seq| lease_for(&pending.desired, seq, now));
            let Some(lease) = renumbered else { break };
            pending.lease = lease;
            outcome = self.backend.api().and_then(|api| api.publish_lock(&pending.lease));
        }
        match outcome {
            Ok(_) => {
                let lease = pending.lease.clone();
                let signature = pending.signature.clone();
                self.pending = None;
                let saved = lock_db(&self.db).and_then(|conn| {
                    repo::save_published(&conn, &signature.key, signature.state, signature.cause, lease.valid_until_ms, now)
                });
                if let Err(err) = saved {
                    // The server has it; the next look would publish the same state again under a
                    // new number, which is harmless.
                    tracing::warn!(event = "phone_publisher_save", error = %err);
                }
                tracing::info!(event = "phone_lease", seq = lease.seq, state = state_text(lease.state), reason = lease.release_cause.map_or("none", cause_text));
                self.diag.update(|v| {
                    v.published = Some(PublishedView {
                        seq: lease.seq,
                        state: state_text(lease.state),
                        reason: lease.release_cause.map(cause_text),
                        at_ms: now,
                    });
                    v.publish_failing = false;
                    v.last_error = None;
                });
            }
            Err(err) => {
                pending.failures = pending.failures.saturating_add(1);
                pending.next_try_ms = now.saturating_add(retry_delay_ms(pending.failures));
                tracing::warn!(event = "phone_publish_failed", code = err.code(), failures = pending.failures);
                let code = err.code().to_string();
                self.diag.update(|v| {
                    v.publish_failing = true;
                    v.last_error = Some(code);
                });
            }
        }
    }

    fn watch_gaps(&mut self, read: &Read, desired: &Desired, now: i64) {
        let watch = match desired {
            Desired::Lock { run_id, segment_started_ms, segment_end_ms, .. } => {
                Some(Watch { run_id: *run_id, segment_started_ms: *segment_started_ms, segment_end_ms: *segment_end_ms })
            }
            _ => None,
        };
        if watch.is_some() && now >= self.next_gap_poll_ms {
            self.next_gap_poll_ms = now.saturating_add(GAP_POLL_MS);
            self.heard = self.backend.api().and_then(|api| api.status()).ok().filter(|wire| wire.paired).map(|wire| {
                // The beat time is the server's; the offset to this PC's clock is taken from the
                // same answer.
                let offset = wire.server_now_ms.map_or(0, |server| now - server);
                Heard { last_beat_ms: wire.last_beat_ms.map(|beat| beat.saturating_add(offset)), service_enabled: wire.service_enabled }
            });
        }
        let covered_until_ms = match desired {
            Desired::Lock { key, .. } if read.publish.last_key.as_deref() == Some(key.as_str()) && read.publish.last_state.as_deref() == Some("locked") => {
                read.publish.last_valid_until_ms
            }
            _ => None,
        };
        let rows = self.tracker.observe(&Observation { now_ms: now, watch, heard: self.heard, covered_until_ms });
        self.store_gaps(&rows);
        let open = self.tracker.open_gaps();
        let (beat, service) = (self.heard.and_then(|h| h.last_beat_ms), self.heard.and_then(|h| h.service_enabled));
        self.diag.update(|v| {
            v.open_gaps = open.iter().map(|g| OpenGapView { kind: g.kind.as_str(), since_ms: g.since_ms }).collect();
            v.last_beat_ms = beat;
            v.service_enabled = service;
        });
    }

    fn store_gaps(&self, rows: &[repo::GapRow]) {
        if rows.is_empty() {
            return;
        }
        let Ok(conn) = lock_db(&self.db) else { return };
        for row in rows {
            match repo::insert_gap(&conn, row) {
                Ok(_) => tracing::warn!(event = "phone_gap", kind = row.kind.as_str(), run_id = row.run_id, ms = row.end_ms - row.start_ms),
                Err(err) => tracing::error!(event = "phone_gap_write_failed", error = %err),
            }
        }
    }

    fn pull_pickups(&mut self, now: i64) {
        if now < self.next_pickup_poll_ms {
            return;
        }
        self.next_pickup_poll_ms = now.saturating_add(PICKUP_POLL_MS);
        let Ok(rows) = self.backend.api().and_then(|api| api.fetch_pickups()) else { return };
        let Ok(conn) = lock_db(&self.db) else { return };
        for row in rows {
            if let Err(err) = record_report(&conn, &row.segment_id, row.count, now) {
                tracing::warn!(event = "phone_pickup_write_failed", error = %err);
            }
        }
    }
}

/// How the run was released by a pass, when it was ("skip today" or "early release").
fn released_by(conn: &Connection, run_id: i64) -> AppResult<Option<ReleaseCause>> {
    let kind: Option<String> = conn
        .query_row(
            "SELECT kind FROM study_passes WHERE run_id = ?1 AND kind IN ('skip_today', 'early_release') ORDER BY id DESC LIMIT 1",
            params![run_id],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    Ok(kind.as_deref().and_then(|k| match k {
        "skip_today" => Some(ReleaseCause::SkipToday),
        "early_release" => Some(ReleaseCause::EarlyRelease),
        _ => None,
    }))
}

/// Takes one pick-up report from the server into the local table. Only a study segment that
/// exists here, under the run the key names, counts; anything else is ignored. Idempotent: the
/// table takes a count only when it is higher than what the segment has. Returns whether a row
/// was added.
pub fn record_report(conn: &Connection, key: &str, count: i64, at_ms: i64) -> AppResult<bool> {
    let Some((run_id, segment_id)) = parse_segment_key(key) else { return Ok(false) };
    if !(0..=MAX_PICKUPS_PER_REPORT).contains(&count) {
        return Ok(false);
    }
    let known = store::get_segment(conn, segment_id)?.is_some_and(|s| s.run_id == run_id && s.kind == BlockKind::Study);
    if !known {
        return Ok(false);
    }
    repo::record_pickups(conn, segment_id, count, at_ms)
}

/// Starts the worker thread. It looks at every wake and at least once per second (every five
/// seconds while nothing is paired).
pub fn spawn<Tz>(mut publisher: Publisher<Tz>) -> PublisherHandle
where
    Tz: TimeZone + Send + 'static,
{
    let (tx, rx): (Sender<()>, Receiver<()>) = mpsc::channel();
    thread::spawn(move || {
        let mut wait = Duration::ZERO;
        loop {
            match rx.recv_timeout(wait) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            // Wakes that piled up while a look was running count as one.
            while rx.try_recv().is_ok() {}
            wait = catch_unwind(AssertUnwindSafe(|| publisher.step())).unwrap_or_else(|_| {
                tracing::error!(event = "phone_publisher_panic", "a publisher look panicked; retrying");
                IDLE_WAIT
            });
        }
    });
    PublisherHandle { tx }
}
