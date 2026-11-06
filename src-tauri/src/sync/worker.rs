//! The mirror worker: notices local changes and pushes them. It watches a cheap change marker
//! every couple of seconds (so a skip, a delete, or an edit reaches the server within 3 seconds)
//! and does a full comparison every 5 minutes as a safety net. Failures back off exponentially
//! and never reach the timer engine; a panic inside one cycle is logged and the worker goes on.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chrono::TimeZone;
use rusqlite::Connection;

use super::ports::{DeviceIdentity, PlanMirror, PresencePort, SnapshotOutcome};
use super::push::{self, PushError};
use super::status::SyncStatus;
use super::{Jitter, Mono};
use crate::commands::lock_db;
use crate::remote::http::backoff_ms;
use crate::remote::payload::NotifyChannelRow;
use crate::remote::Clock;
use crate::store::remote::{change_marker, reset_cursor, HISTORY_CURSOR};

/// How often the change marker is read. Must stay under the 3-second promise for a skip.
pub const POLL_MS: u64 = 2_000;
/// A full snapshot comparison even when no marker moved (day rollover, clock-dependent status,
/// UTC offset change).
pub const FULL_RESYNC_MS: u64 = 300_000;

/// Everything a worker needs to talk to the server, resolved fresh for each cycle. Resolution
/// returns `None` when signed out, not configured, or the login expired: then nothing is sent.
pub struct SyncTarget {
    pub mirror: Arc<dyn PlanMirror>,
    pub presence: Arc<dyn PresencePort>,
    pub device: DeviceIdentity,
}

pub trait SyncSource: Send + Sync {
    fn target(&self) -> Option<SyncTarget>;
    /// Is a planned study session holding the lock right now? (the beat's only extra fact)
    fn studying(&self) -> bool;
    /// The notification channel to mirror, or `None` when it cannot be read.
    fn notify_channel(&self) -> Option<NotifyChannelRow>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum CycleReport {
    Idle,
    Synced { snapshot_sent: bool, history_rows: u64, channel_sent: bool },
    Failed(&'static str),
    Panicked,
}

struct State {
    active: bool,
    registered: bool,
    seen_marker: Option<(i64, i64, i64)>,
    pushed_hash: Option<String>,
    pushed_channel: Option<String>,
    last_full_ms: Option<u64>,
    failures: u32,
    retry_at_ms: u64,
}

impl State {
    fn fresh() -> Self {
        Self {
            active: true,
            registered: false,
            seen_marker: None,
            pushed_hash: None,
            pushed_channel: None,
            last_full_ms: None,
            failures: 0,
            retry_at_ms: 0,
        }
    }

    fn inactive() -> Self {
        Self { active: false, ..Self::fresh() }
    }
}

pub struct MirrorWorker<Tz: TimeZone> {
    db: Arc<Mutex<Connection>>,
    clock: Arc<dyn Clock>,
    tz: Tz,
    source: Arc<dyn SyncSource>,
    status: Arc<SyncStatus>,
    jitter: Arc<dyn Jitter>,
    state: State,
}

impl<Tz: TimeZone> MirrorWorker<Tz> {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        clock: Arc<dyn Clock>,
        tz: Tz,
        source: Arc<dyn SyncSource>,
        status: Arc<SyncStatus>,
        jitter: Arc<dyn Jitter>,
    ) -> Self {
        Self { db, clock, tz, source, status, jitter, state: State::inactive() }
    }

    /// One look at the world. Called every `POLL_MS`; does nothing unless something changed, the
    /// safety-net interval passed, or a failed push is due for its retry.
    pub fn cycle(&mut self, now_ms: u64) -> CycleReport {
        let Some(target) = self.source.target() else {
            if self.state.active {
                // Everything remembered about the server belongs to the login that just ended.
                self.state = State::inactive();
                self.status.set_active(false);
            }
            return CycleReport::Idle;
        };
        if !self.state.active {
            self.state = State::fresh();
            self.status.set_active(true);
        }
        if now_ms < self.state.retry_at_ms {
            return CycleReport::Idle;
        }
        let marker = match lock_db(&self.db).and_then(|conn| change_marker(&conn)) {
            Ok(marker) => marker,
            Err(err) => {
                tracing::error!(event = "sync_marker_failed", error = %err);
                return CycleReport::Failed("database");
            }
        };
        let full_due = self.state.last_full_ms.is_none_or(|at| now_ms.saturating_sub(at) >= FULL_RESYNC_MS);
        if self.state.registered && self.state.seen_marker == Some(marker) && !full_due {
            return CycleReport::Idle;
        }
        match self.sync(&target) {
            Ok(report) => {
                self.state.seen_marker = Some(marker);
                self.state.last_full_ms = Some(now_ms);
                self.state.failures = 0;
                self.state.retry_at_ms = 0;
                self.status.sync(self.wall_ms(), Ok(()));
                report
            }
            Err(err) => {
                self.status.sync(self.wall_ms(), Err(err.code()));
                if err.is_device_missing() {
                    // The server lost the device: register it again and resend everything now.
                    self.state.registered = false;
                    self.state.pushed_hash = None;
                    self.state.pushed_channel = None;
                    self.state.retry_at_ms = now_ms;
                    // Its history went with it, so the position starts over (rows are idempotent).
                    if let Err(reset) = lock_db(&self.db).and_then(|conn| reset_cursor(&conn, HISTORY_CURSOR)) {
                        tracing::error!(event = "sync_cursor_reset_failed", error = %reset);
                    }
                } else {
                    self.state.failures = self.state.failures.saturating_add(1);
                    self.state.retry_at_ms = now_ms + backoff_ms(self.state.failures, None, self.jitter.permille());
                }
                CycleReport::Failed(err.code())
            }
        }
    }

    fn wall_ms(&self) -> i64 {
        self.clock.now_ms().unwrap_or(0)
    }

    fn sync(&mut self, target: &SyncTarget) -> Result<CycleReport, PushError> {
        let now = self.clock.now_ms().map_err(|_| PushError::Local("clock"))?;
        if !self.state.registered {
            target.mirror.register_device(&target.device)?;
            self.state.registered = true;
        }
        let snapshot = push::snapshot_now(&self.db, &self.tz, now)?;
        let mut snapshot_sent = false;
        if self.state.pushed_hash.as_deref() != Some(snapshot.hash.as_str()) {
            let outcome = target.mirror.push_snapshot(&target.device.id, &snapshot)?;
            snapshot_sent = outcome == SnapshotOutcome::Replaced;
            self.state.pushed_hash = Some(snapshot.hash);
        }
        let history_rows = push::push_history(&self.db, target.mirror.as_ref(), &target.device.id, now)?;
        let mut channel_sent = false;
        if let Some(row) = self.source.notify_channel() {
            // Kept in memory only, to compare; never logged.
            let key = serde_json::to_string(&row).map_err(|_| PushError::Local("encode"))?;
            if self.state.pushed_channel.as_deref() != Some(key.as_str()) {
                target.mirror.push_notify_channel(&row)?;
                self.state.pushed_channel = Some(key);
                channel_sent = true;
            }
        }
        Ok(CycleReport::Synced { snapshot_sent, history_rows, channel_sent })
    }
}

/// One cycle that cannot take the thread down.
pub fn guarded_cycle<Tz: TimeZone>(worker: &mut MirrorWorker<Tz>, now_ms: u64) -> CycleReport {
    match catch_unwind(AssertUnwindSafe(|| worker.cycle(now_ms))) {
        Ok(report) => report,
        Err(_) => {
            tracing::error!(event = "sync_cycle_panicked");
            CycleReport::Panicked
        }
    }
}

/// Starts the mirror thread.
pub fn spawn<Tz>(mut worker: MirrorWorker<Tz>, mono: Arc<dyn Mono>)
where
    Tz: TimeZone + Send + 'static,
{
    let spawned = thread::Builder::new().name("plan-mirror".into()).spawn(move || loop {
        guarded_cycle(&mut worker, mono.now_ms());
        thread::sleep(Duration::from_millis(POLL_MS));
    });
    if let Err(err) = spawned {
        tracing::error!(error = %err, "mirror thread did not start; plans will not be mirrored");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use chrono_tz::Asia::Seoul;
    use serde_json::json;

    use super::*;
    use crate::db::open_in_memory;
    use crate::plan::{self, CreatePlanInput, DeletePlanInput, Repeat, Scope};
    use crate::remote::fake::Caller;
    use crate::remote::ManualClock;
    use crate::sync::presence::tests::{rig, Rig, WALL_START};
    use crate::sync::push::tests::insert_history_rows;
    use crate::sync::testing::FixedJitter;

    struct Harness {
        rig: Rig,
        db: Arc<Mutex<Connection>>,
        worker: MirrorWorker<chrono_tz::Tz>,
    }

    /// 2026-06-01 12:00 KST, the instant the mock project starts at.
    const NOW: i64 = WALL_START;

    fn harness() -> Harness {
        let rig = rig();
        let db = Arc::new(Mutex::new(open_in_memory().expect("db")));
        let clock: Arc<dyn Clock> = rig.wall.clone();
        let worker = MirrorWorker::new(db.clone(), clock, Seoul, rig.source.clone(), Arc::new(SyncStatus::default()), Arc::new(FixedJitter(0)));
        Harness { rig, db, worker }
    }

    fn create_weekly(db: &Mutex<Connection>) -> i64 {
        let mut conn = db.lock().expect("lock");
        let input = CreatePlanInput {
            name: "Math".into(),
            subject: "math".into(),
            start_date: "2026-06-01".into(),
            start_time: "20:00".into(),
            sets: 2,
            repeat: Some(Repeat { weekdays: 0b000_0101, until: None }),
        };
        plan::create_plan(&mut conn, &Seoul, input, NOW - 3_600_000).expect("create").plan_id.expect("id")
    }

    fn server_occurrences(h: &Harness) -> Vec<serde_json::Value> {
        h.rig.backend.table_rows("plan_occurrences")
    }

    #[test]
    fn first_cycle_registers_pushes_the_snapshot_and_history_then_goes_quiet() {
        let mut h = harness();
        h.rig.backend.delete(&Caller::Service, "devices", &[]).expect("clear");
        create_weekly(&h.db);
        let report = h.worker.cycle(0);
        assert_eq!(report, CycleReport::Synced { snapshot_sent: true, history_rows: 1, channel_sent: false });
        assert_eq!(server_occurrences(&h).len(), 4);
        assert_eq!(h.rig.backend.table_rows("plan_history_mirror").len(), 1);
        assert_eq!(h.rig.backend.device_count(), 1);
        // Same state again: nothing is sent, not even a comparison round trip.
        for t in [2_000, 4_000, 6_000] {
            assert_eq!(h.worker.cycle(t), CycleReport::Idle);
        }
    }

    #[test]
    fn the_safety_net_resync_sends_no_snapshot_when_the_hash_is_unchanged() {
        let mut h = harness();
        create_weekly(&h.db);
        h.worker.cycle(0);
        let report = h.worker.cycle(FULL_RESYNC_MS);
        assert_eq!(report, CycleReport::Synced { snapshot_sent: false, history_rows: 0, channel_sent: false }, "re-push 0 times");
    }

    #[test]
    fn a_skip_reaches_the_server_within_three_seconds() {
        let mut h = harness();
        create_weekly(&h.db);
        h.worker.cycle(0);
        assert!(server_occurrences(&h).iter().all(|o| o["status"] == json!("planned")));
        // The user skips today at t=1s; the worker looks again at t=2s.
        h.db.lock().expect("lock")
            .execute(
                "INSERT INTO study_passes (at_ms, date, kind, challenge_len, typing_ms, penalty_points) VALUES (?1, '2026-06-01', 'skip_today', 10, 1000, 1)",
                [NOW],
            )
            .expect("skip pass");
        const _: () = assert!(POLL_MS <= 3_000);
        let report = h.worker.cycle(POLL_MS);
        assert!(matches!(report, CycleReport::Synced { snapshot_sent: true, .. }), "{report:?}");
        let skipped: Vec<_> = server_occurrences(&h).into_iter().filter(|o| o["status"] == json!("skipped")).collect();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0]["occurrence_key"], json!("s1:2026-06-01"));
    }

    #[test]
    fn delete_and_edit_are_pushed_immediately_too() {
        let mut h = harness();
        let id = create_weekly(&h.db);
        h.worker.cycle(0);
        {
            let mut conn = h.db.lock().expect("lock");
            plan::delete_plan(
                &mut conn,
                DeletePlanInput { plan_id: id, scope: Scope::Series, date: "2026-06-01".into(), reason_code: plan::ReasonCode::Sick, reason_text: None },
                NOW,
            )
            .expect("delete");
        }
        let report = h.worker.cycle(POLL_MS);
        assert!(matches!(report, CycleReport::Synced { snapshot_sent: true, history_rows: 1, .. }), "{report:?}");
        assert!(server_occurrences(&h).is_empty());
    }

    #[test]
    fn an_outage_keeps_local_work_going_and_catches_up_after_recovery() {
        let mut h = harness();
        create_weekly(&h.db);
        h.rig.backend.set_outage(true);
        let first = h.worker.cycle(0);
        assert!(matches!(first, CycleReport::Failed(_)), "{first:?}");
        // The local database is untouched and usable during the outage.
        assert_eq!(h.db.lock().expect("lock").query_row("SELECT COUNT(*) FROM plans", [], |r| r.get::<_, i64>(0)).expect("count"), 1);
        // Backoff: the next attempt waits (1 s for the first failure), so a cycle at +500 ms is idle.
        assert_eq!(h.worker.cycle(500), CycleReport::Idle);
        h.rig.backend.set_outage(false);
        let recovered = h.worker.cycle(1_000);
        assert_eq!(recovered, CycleReport::Synced { snapshot_sent: true, history_rows: 1, channel_sent: false });
        assert_eq!(server_occurrences(&h).len(), 4);
    }

    #[test]
    fn repeated_failures_back_off_up_to_a_minute() {
        let mut h = harness();
        create_weekly(&h.db);
        h.rig.backend.set_outage(true);
        let mut now = 0;
        let mut gaps = Vec::new();
        for _ in 0..9 {
            assert!(matches!(h.worker.cycle(now), CycleReport::Failed(_)));
            gaps.push(h.worker.state.retry_at_ms - now);
            now = h.worker.state.retry_at_ms;
        }
        assert_eq!(gaps, [1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 60_000, 60_000, 60_000]);
    }

    #[test]
    fn signing_out_and_back_in_resends_everything_the_server_dropped() {
        let mut h = harness();
        create_weekly(&h.db);
        h.worker.cycle(0);
        h.rig.source.signed_in.store(false, Ordering::SeqCst);
        assert_eq!(h.worker.cycle(2_000), CycleReport::Idle);
        // Sign-out removed the device and everything under it on the server, and reset the cursor.
        h.rig.backend.delete(&Caller::Service, "devices", &[]).expect("device gone");
        crate::store::remote::reset_cursor(&h.db.lock().expect("lock"), crate::store::remote::HISTORY_CURSOR).expect("reset");
        h.rig.source.signed_in.store(true, Ordering::SeqCst);
        let report = h.worker.cycle(4_000);
        assert_eq!(report, CycleReport::Synced { snapshot_sent: true, history_rows: 1, channel_sent: false });
        assert_eq!(server_occurrences(&h).len(), 4);
        assert_eq!(h.rig.backend.table_rows("plan_history_mirror").len(), 1);
    }

    #[test]
    fn a_device_the_server_forgot_is_registered_again_and_resent_without_waiting() {
        let mut h = harness();
        create_weekly(&h.db);
        h.worker.cycle(0);
        h.rig.backend.delete(&Caller::Service, "devices", &[]).expect("server forgot");
        // A change makes the worker push; the server answers device_not_owned.
        insert_history_rows(&h.db.lock().expect("lock"), 1);
        let failed = h.worker.cycle(2_000);
        assert_eq!(failed, CycleReport::Failed("device_not_owned"));
        let healed = h.worker.cycle(2_000);
        assert_eq!(healed, CycleReport::Synced { snapshot_sent: true, history_rows: 2, channel_sent: false });
        assert_eq!(h.rig.backend.device_count(), 1);
        assert_eq!(server_occurrences(&h).len(), 4);
    }

    #[test]
    fn nothing_is_sent_when_the_gate_is_closed() {
        let mut h = harness();
        create_weekly(&h.db);
        h.rig.source.signed_in.store(false, Ordering::SeqCst);
        for t in (0..20).map(|i| i * 2_000) {
            assert_eq!(h.worker.cycle(t), CycleReport::Idle);
        }
        assert!(server_occurrences(&h).is_empty());
        assert!(h.rig.backend.table_rows("plan_history_mirror").is_empty());
    }

    #[test]
    fn the_notify_channel_is_mirrored_once_and_again_only_when_it_changes() {
        let rig = rig();
        let db = Arc::new(Mutex::new(open_in_memory().expect("db")));
        let source = Arc::new(ChannelSource { inner: rig.source.clone(), topic: Mutex::new("a".repeat(24)) });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(NOW));
        let mut worker = MirrorWorker::new(db.clone(), clock, Seoul, source.clone(), Arc::new(SyncStatus::default()), Arc::new(FixedJitter(0)));
        assert_eq!(worker.cycle(0), CycleReport::Synced { snapshot_sent: true, history_rows: 0, channel_sent: true });
        let rows = rig.backend.table_rows("notify_channels");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["channel"], json!("ntfy"));
        assert_eq!(rows[0]["utc_offset_min"], json!(540));
        // Unchanged: a forced full resync sends nothing more.
        assert_eq!(worker.cycle(FULL_RESYNC_MS), CycleReport::Synced { snapshot_sent: false, history_rows: 0, channel_sent: false });
        // A new topic (settings history moved) is sent as an update of the same row.
        *source.topic.lock().expect("topic") = "b".repeat(24);
        crate::store::append_history(&db.lock().expect("lock"), 5, "ntfy_topic", Some("true"), "true", crate::settings::Reason::SecretSet, crate::settings::Outcome::Applied)
            .expect("history");
        assert_eq!(worker.cycle(FULL_RESYNC_MS + 2_000), CycleReport::Synced { snapshot_sent: false, history_rows: 0, channel_sent: true });
        let rows = rig.backend.table_rows("notify_channels");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["ntfy_topic"], json!("b".repeat(24)));
    }

    struct ChannelSource {
        inner: Arc<crate::sync::presence::tests::TestSource>,
        topic: Mutex<String>,
    }

    impl SyncSource for ChannelSource {
        fn target(&self) -> Option<SyncTarget> {
            self.inner.target()
        }
        fn studying(&self) -> bool {
            false
        }
        fn notify_channel(&self) -> Option<NotifyChannelRow> {
            Some(NotifyChannelRow {
                channel: "ntfy".into(),
                ntfy_topic: Some(self.topic.lock().expect("topic").clone()),
                ntfy_server: Some("https://ntfy.sh".into()),
                telegram_chat_id: None,
                missed_grace_min: 3,
                utc_offset_min: 540,
            })
        }
    }

    #[test]
    fn a_panic_in_a_cycle_does_not_escape_and_the_next_cycle_runs() {
        let mut h = harness();
        create_weekly(&h.db);
        h.rig.source.panic_once.store(true, Ordering::SeqCst);
        assert_eq!(guarded_cycle(&mut h.worker, 0), CycleReport::Panicked);
        assert!(matches!(guarded_cycle(&mut h.worker, 2_000), CycleReport::Synced { .. }));
    }
}
