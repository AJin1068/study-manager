//! The "this PC is on" signal. While the app runs and the system is awake it sends a beat every
//! 60 seconds; the server stamps its own time, so the PC clock is never trusted.
//!
//! Timing uses a monotonic clock. Sleep stops the beats; wake sends one at once. A failed beat is
//! retried on a short schedule (5, 10, 20, then 30 seconds, never longer) instead of the usual
//! exponential backoff: the server decides a plan was missed after a 3-minute grace, so at least
//! five attempts must fit in it. Nothing is sent when signed out or not configured.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::ports::SyncError;
use super::worker::SyncSource;
use super::{Jitter, Mono};
use crate::platform::PowerEvent;
use crate::remote::payload::PresenceBeat;

pub const BEAT_INTERVAL_MS: u64 = 60_000;
pub const RETRY_BASE_MS: [u64; 4] = [5_000, 10_000, 20_000, 30_000];
/// Failed beats in a row before the signal is reported as unreachable.
pub const DEGRADED_AFTER_FAILURES: u32 = 3;
/// How often a waiting engine looks again at whether it is signed in.
const GATE_RECHECK_MS: u64 = 1_000;
const ASLEEP_WAIT_MS: u64 = 60_000;

/// Delay before retry number `failures` (1-based), at most 30 seconds. `jitter_permille` shortens
/// it by up to 20% so retries do not align; it never lengthens it.
pub fn retry_delay_ms(failures: u32, jitter_permille: u32) -> u64 {
    let step = usize::try_from(failures.saturating_sub(1)).unwrap_or(usize::MAX).min(RETRY_BASE_MS.len() - 1);
    let base = RETRY_BASE_MS.get(step).copied().unwrap_or(30_000);
    base - base * u64::from(jitter_permille.min(1000)) / 1000 / 5
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenceMsg {
    Sleep,
    Wake,
    #[cfg_attr(not(test), allow(dead_code))]
    Stop,
}

/// How `power.rs` reaches the presence thread.
#[derive(Clone)]
pub struct PresenceHandle(Sender<PresenceMsg>);

impl PresenceHandle {
    /// Forwards a system power event. A closed channel (thread gone) is not an error for the
    /// power worker.
    pub fn power_event(&self, event: PowerEvent) {
        let msg = match event {
            PowerEvent::WillSleep => PresenceMsg::Sleep,
            PowerEvent::DidWake => PresenceMsg::Wake,
        };
        let _ = self.0.send(msg);
    }

    #[cfg(test)]
    pub fn stop(&self) {
        let _ = self.0.send(PresenceMsg::Stop);
    }
}

/// Where outcomes go (status screen, integration health).
pub trait PresenceReport: Send + Sync {
    fn beat(&self, result: Result<(), &'static str>);
    fn degraded(&self, degraded: bool);
    fn inactive(&self);
}

#[derive(Debug, PartialEq, Eq)]
pub enum PollOutcome {
    /// Nothing to do now.
    Idle,
    /// Signed out or not configured: nothing was sent.
    Inactive,
    Sent(Result<(), &'static str>),
    /// The poll panicked; the engine stays as it was and keeps going.
    Panicked,
}

pub struct PresenceEngine {
    asleep: bool,
    active: bool,
    next_due_ms: u64,
    failures: u32,
    degraded: bool,
}

impl Default for PresenceEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PresenceEngine {
    pub fn new() -> Self {
        Self { asleep: false, active: false, next_due_ms: 0, failures: 0, degraded: false }
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    pub fn on_sleep(&mut self) {
        self.asleep = true;
    }

    /// Wake: the next beat is due now, and an earlier failure streak starts over.
    pub fn on_wake(&mut self, now_ms: u64) {
        self.asleep = false;
        self.next_due_ms = now_ms;
        self.failures = 0;
    }

    pub fn apply(&mut self, msg: PresenceMsg, now_ms: u64) {
        match msg {
            PresenceMsg::Sleep => self.on_sleep(),
            PresenceMsg::Wake => self.on_wake(now_ms),
            PresenceMsg::Stop => {}
        }
    }

    /// How long the thread may wait before it has to look again.
    pub fn wait_ms(&self, now_ms: u64) -> u64 {
        if self.asleep {
            ASLEEP_WAIT_MS
        } else if !self.active {
            GATE_RECHECK_MS
        } else {
            self.next_due_ms.saturating_sub(now_ms).clamp(1, ASLEEP_WAIT_MS)
        }
    }

    /// Sends a beat if one is due. `source` decides whether anything may be sent at all.
    pub fn poll(&mut self, now_ms: u64, source: &dyn SyncSource, jitter: &dyn Jitter) -> PollOutcome {
        if self.asleep || (self.active && now_ms < self.next_due_ms) {
            return PollOutcome::Idle;
        }
        let Some(target) = source.target() else {
            self.active = false;
            self.failures = 0;
            self.degraded = false;
            return PollOutcome::Inactive;
        };
        if !self.active {
            // Start of the app, sign-in, or settings change: send at once.
            self.active = true;
            self.next_due_ms = now_ms;
        }
        if now_ms < self.next_due_ms {
            return PollOutcome::Idle;
        }
        let beat = PresenceBeat {
            device_id: target.device.id.clone(),
            app_version: target.device.app_version.clone(),
            studying: source.studying(),
        };
        let mut result = target.presence.beat(&beat);
        if matches!(result, Err(err) if err.is_device_missing()) && target.mirror.register_device(&target.device).is_ok() {
            result = target.presence.beat(&beat);
        }
        match result {
            Ok(()) => {
                self.failures = 0;
                self.degraded = false;
                self.next_due_ms = now_ms + BEAT_INTERVAL_MS;
                PollOutcome::Sent(Ok(()))
            }
            Err(err) => {
                self.failures = self.failures.saturating_add(1);
                self.next_due_ms = now_ms + retry_delay_ms(self.failures, jitter.permille());
                if self.failures >= DEGRADED_AFTER_FAILURES {
                    self.degraded = true;
                }
                PollOutcome::Sent(Err(SyncError::code(err)))
            }
        }
    }
}

/// One guarded poll: a panic in the adapter or the source is logged and swallowed, never carried
/// into the app.
pub fn guarded_poll(engine: &mut PresenceEngine, now_ms: u64, source: &dyn SyncSource, jitter: &dyn Jitter) -> PollOutcome {
    match catch_unwind(AssertUnwindSafe(|| engine.poll(now_ms, source, jitter))) {
        Ok(outcome) => outcome,
        Err(_) => {
            tracing::error!(event = "presence_poll_panicked");
            PollOutcome::Panicked
        }
    }
}

fn publish(sink: &dyn PresenceReport, engine: &PresenceEngine, outcome: &PollOutcome, was_degraded: bool) {
    match outcome {
        PollOutcome::Sent(result) => sink.beat(*result),
        PollOutcome::Inactive => sink.inactive(),
        PollOutcome::Idle | PollOutcome::Panicked => {}
    }
    if engine.is_degraded() != was_degraded {
        sink.degraded(engine.is_degraded());
    }
}

/// Runs the presence loop until `Stop` (or the sender is dropped). Blocks; the caller owns the
/// thread.
pub fn run(rx: &Receiver<PresenceMsg>, source: &dyn SyncSource, mono: &dyn Mono, jitter: &dyn Jitter, sink: &dyn PresenceReport) {
    let mut engine = PresenceEngine::new();
    loop {
        let now = mono.now_ms();
        let was_degraded = engine.is_degraded();
        let outcome = guarded_poll(&mut engine, now, source, jitter);
        publish(sink, &engine, &outcome, was_degraded);
        match rx.recv_timeout(Duration::from_millis(engine.wait_ms(mono.now_ms()))) {
            Ok(PresenceMsg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(msg) => engine.apply(msg, mono.now_ms()),
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Starts the presence thread and returns the handle `power.rs` forwards sleep/wake events to.
pub fn spawn(
    source: Arc<dyn SyncSource>,
    mono: Arc<dyn Mono>,
    jitter: Arc<dyn Jitter>,
    sink: Arc<dyn PresenceReport>,
) -> PresenceHandle {
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new().name("presence".into()).spawn(move || {
        run(&rx, source.as_ref(), mono.as_ref(), jitter.as_ref(), sink.as_ref());
    });
    if let Err(err) = spawned {
        tracing::error!(error = %err, "presence thread did not start; no signal will be sent");
    }
    PresenceHandle(tx)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::remote::fake::FakeBackend;
    use crate::remote::supabase::{RemoteError, SupabaseRest};
    use crate::remote::ManualClock;
    use crate::sync::ports::{DeviceIdentity, PlanMirror, RestSync};
    use crate::sync::testing::{FixedJitter, ManualMono};
    use crate::sync::worker::SyncTarget;

    /// A source whose gate and lock state the test flips, backed by a real port.
    pub(crate) struct TestSource {
        pub port: Arc<RestSync>,
        pub signed_in: AtomicBool,
        pub studying: AtomicBool,
        pub target_calls: AtomicU32,
        pub panic_once: AtomicBool,
        pub device: DeviceIdentity,
    }

    impl SyncSource for TestSource {
        fn target(&self) -> Option<SyncTarget> {
            self.target_calls.fetch_add(1, Ordering::SeqCst);
            if self.panic_once.swap(false, Ordering::SeqCst) {
                panic!("injected");
            }
            if !self.signed_in.load(Ordering::SeqCst) {
                return None;
            }
            Some(SyncTarget { mirror: self.port.clone(), presence: self.port.clone(), device: self.device.clone() })
        }
        fn studying(&self) -> bool {
            self.studying.load(Ordering::SeqCst)
        }
        fn notify_channel(&self) -> Option<crate::remote::payload::NotifyChannelRow> {
            None
        }
    }

    /// 2026-06-01 12:00 KST as epoch milliseconds: where the mock project's clock starts.
    pub(crate) const WALL_START: i64 = 1_780_282_800_000;
    pub(crate) const DEVICE: &str = "10000000-0000-4000-8000-00000000000a";

    pub(crate) struct Rig {
        pub backend: Arc<FakeBackend>,
        pub wall: Arc<ManualClock>,
        pub source: Arc<TestSource>,
    }

    /// A signed-in mock project with this device registered.
    pub(crate) fn rig() -> Rig {
        use crate::remote::auth::AuthService;
        use crate::remote::fake::{FAKE_ANON_KEY, FAKE_ENDPOINT};
        use crate::remote::secrets::MemorySecretStore;
        let wall = Arc::new(ManualClock::new(WALL_START));
        let backend = Arc::new(FakeBackend::new(wall.clone()));
        let service = Arc::new(AuthService::new(backend.auth(), Arc::new(MemorySecretStore::in_memory()), backend.clone(), wall.clone()));
        service.request_otp("a@example.test").expect("otp");
        service.verify_otp("a@example.test", "000000").expect("verify");
        let rest = Arc::new(SupabaseRest::new(FAKE_ENDPOINT, FAKE_ANON_KEY, backend.clone(), service));
        let port = Arc::new(RestSync::new(rest));
        let device = DeviceIdentity { id: DEVICE.into(), app_version: "0.1.0".into(), platform: "macos" };
        port.register_device(&device).expect("register");
        let source = Arc::new(TestSource {
            port,
            signed_in: AtomicBool::new(true),
            studying: AtomicBool::new(false),
            target_calls: AtomicU32::new(0),
            panic_once: AtomicBool::new(false),
            device,
        });
        Rig { backend, wall, source }
    }

    fn beats(rig: &Rig) -> usize {
        rig.backend.table_rows("presence").len()
    }

    #[test]
    fn retry_schedule_is_5_10_20_30_and_never_longer() {
        let delays: Vec<u64> = (1..=8).map(|n| retry_delay_ms(n, 0)).collect();
        assert_eq!(delays, [5_000, 10_000, 20_000, 30_000, 30_000, 30_000, 30_000, 30_000]);
        for n in 1..=10 {
            for jitter in [0, 300, 1000, 5000] {
                let d = retry_delay_ms(n, jitter);
                assert!(d <= 30_000 && d * 5 >= RETRY_BASE_MS[(n as usize - 1).min(3)] * 4, "n={n} jitter={jitter} d={d}");
            }
        }
    }

    #[test]
    fn at_least_five_attempts_fit_in_the_three_minute_grace_while_offline() {
        let rig = rig();
        rig.backend.set_outage(true);
        let mono = ManualMono::new(0);
        let mut engine = PresenceEngine::new();
        let jitter = FixedJitter(0);
        let mut attempts = 0;
        for _ in 0..(180_000 / 100) {
            if let PollOutcome::Sent(_) = engine.poll(mono.now_ms(), rig.source.as_ref(), &jitter) {
                attempts += 1;
            }
            mono.advance(100);
        }
        assert!(attempts >= 5, "attempts in 180s: {attempts}");
        assert_eq!(attempts, 8, "t = 0, 5, 15, 35, 65, 95, 125, 155");
    }

    #[test]
    fn healthy_beats_are_sixty_seconds_apart() {
        let rig = rig();
        let mono = ManualMono::new(0);
        let mut engine = PresenceEngine::new();
        let jitter = FixedJitter(0);
        let mut sent_at = Vec::new();
        for _ in 0..(5 * 60 * 1000 / 500) {
            if let PollOutcome::Sent(Ok(())) = engine.poll(mono.now_ms(), rig.source.as_ref(), &jitter) {
                sent_at.push(mono.now_ms());
            }
            mono.advance(500);
        }
        assert_eq!(sent_at, [0, 60_000, 120_000, 180_000, 240_000]);
        assert!(sent_at.windows(2).all(|w| (55_000..=65_000).contains(&(w[1] - w[0]))));
        assert_eq!(beats(&rig), 1, "one presence row per device, refreshed by each beat");
    }

    #[test]
    fn sleep_stops_the_beats_and_wake_sends_at_once() {
        let rig = rig();
        let mono = ManualMono::new(0);
        let mut engine = PresenceEngine::new();
        let jitter = FixedJitter(0);
        assert_eq!(engine.poll(0, rig.source.as_ref(), &jitter), PollOutcome::Sent(Ok(())));
        engine.on_sleep();
        let calls_before = rig.source.target_calls.load(Ordering::SeqCst);
        for _ in 0..20 {
            mono.advance(60_000);
            assert_eq!(engine.poll(mono.now_ms(), rig.source.as_ref(), &jitter), PollOutcome::Idle);
        }
        assert_eq!(rig.source.target_calls.load(Ordering::SeqCst), calls_before, "asleep: not even the gate is read");
        // Wake: the very next poll (the thread polls as soon as the message arrives) sends.
        let woke_at = mono.now_ms();
        engine.on_wake(woke_at);
        assert_eq!(engine.poll(woke_at, rig.source.as_ref(), &jitter), PollOutcome::Sent(Ok(())));
        assert_eq!(engine.wait_ms(woke_at), 60_000);
    }

    #[test]
    fn a_failure_streak_degrades_after_three_and_recovers_on_success() {
        let rig = rig();
        let mut engine = PresenceEngine::new();
        let jitter = FixedJitter(0);
        rig.backend.set_outage(true);
        let mut now = 0;
        for expected_degraded in [false, false, true, true] {
            assert!(matches!(engine.poll(now, rig.source.as_ref(), &jitter), PollOutcome::Sent(Err(_))));
            assert_eq!(engine.is_degraded(), expected_degraded);
            now += 30_000;
        }
        rig.backend.set_outage(false);
        assert_eq!(engine.poll(now, rig.source.as_ref(), &jitter), PollOutcome::Sent(Ok(())));
        assert!(!engine.is_degraded());
    }

    #[test]
    fn after_the_network_returns_the_next_beat_is_at_most_thirty_seconds_away() {
        let rig = rig();
        let mut engine = PresenceEngine::new();
        let jitter = FixedJitter(0);
        rig.backend.set_outage(true);
        let mut now = 0;
        let mut last_attempt = 0;
        for _ in 0..6 {
            assert!(matches!(engine.poll(now, rig.source.as_ref(), &jitter), PollOutcome::Sent(Err(_))));
            last_attempt = now;
            now += engine.wait_ms(now);
        }
        rig.backend.set_outage(false);
        // The network is back right after the last attempt; the next attempt is within 30 s.
        assert!(now - last_attempt <= 30_000, "gap {}", now - last_attempt);
        assert_eq!(engine.poll(now, rig.source.as_ref(), &jitter), PollOutcome::Sent(Ok(())));
    }

    #[test]
    fn signed_out_or_unconfigured_sends_nothing_and_login_sends_at_once() {
        let rig = rig();
        let mut engine = PresenceEngine::new();
        let jitter = FixedJitter(0);
        rig.source.signed_in.store(false, Ordering::SeqCst);
        for now in [0, 1_000, 61_000, 500_000] {
            assert_eq!(engine.poll(now, rig.source.as_ref(), &jitter), PollOutcome::Inactive);
        }
        assert_eq!(beats(&rig), 0);
        rig.source.signed_in.store(true, Ordering::SeqCst);
        assert_eq!(engine.poll(501_000, rig.source.as_ref(), &jitter), PollOutcome::Sent(Ok(())));
        assert_eq!(beats(&rig), 1);
        // Sign out in the middle of the wait: the due beat is dropped at the gate.
        rig.source.signed_in.store(false, Ordering::SeqCst);
        assert_eq!(engine.poll(561_000, rig.source.as_ref(), &jitter), PollOutcome::Inactive);
    }

    #[test]
    fn the_server_clock_stamps_the_beat_and_a_backwards_wall_clock_changes_nothing() {
        let rig = rig();
        let mono = ManualMono::new(0);
        let mut engine = PresenceEngine::new();
        let jitter = FixedJitter(0);
        let mut sent = 0;
        for step in 0..600 {
            // The PC wall clock jumps back an hour in the middle of the run.
            if step == 300 {
                rig.wall.set(WALL_START - 3_600_000);
            }
            if let PollOutcome::Sent(_) = engine.poll(mono.now_ms(), rig.source.as_ref(), &jitter) {
                sent += 1;
            }
            mono.advance(1_000);
        }
        assert_eq!(sent, 10, "10 minutes at 60 s: no storm, no stall");
    }

    #[test]
    fn the_beat_carries_the_lock_state_and_nothing_else() {
        let rig = rig();
        let mut engine = PresenceEngine::new();
        let jitter = FixedJitter(0);
        rig.source.studying.store(true, Ordering::SeqCst);
        engine.poll(0, rig.source.as_ref(), &jitter);
        let rows = rig.backend.table_rows("presence");
        assert_eq!(rows[0]["studying"], serde_json::json!(true));
        let mut keys: Vec<&str> = rows[0].as_object().expect("row").keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["app_version", "device_id", "last_seen_at_ms", "studying", "user_id"]);
    }

    #[test]
    fn an_unregistered_device_is_registered_and_the_beat_retried() {
        let rig = rig();
        // The server forgot the device (sign-out removed it, a new session began).
        rig.backend.delete(&crate::remote::fake::Caller::Service, "devices", &[]).expect("delete devices");
        let mut engine = PresenceEngine::new();
        assert_eq!(engine.poll(0, rig.source.as_ref(), &FixedJitter(0)), PollOutcome::Sent(Ok(())));
        assert_eq!(rig.backend.device_count(), 1);
    }

    #[test]
    fn a_panicking_source_does_not_escape_and_the_next_poll_works() {
        let rig = rig();
        let mut engine = PresenceEngine::new();
        let jitter = FixedJitter(0);
        rig.source.panic_once.store(true, Ordering::SeqCst);
        assert_eq!(guarded_poll(&mut engine, 0, rig.source.as_ref(), &jitter), PollOutcome::Panicked);
        assert_eq!(guarded_poll(&mut engine, 1_000, rig.source.as_ref(), &jitter), PollOutcome::Sent(Ok(())));
    }

    #[derive(Default)]
    struct Log(Mutex<Vec<String>>);

    impl PresenceReport for Log {
        fn beat(&self, result: Result<(), &'static str>) {
            self.0.lock().expect("log").push(format!("beat:{result:?}"));
        }
        fn degraded(&self, degraded: bool) {
            self.0.lock().expect("log").push(format!("degraded:{degraded}"));
        }
        fn inactive(&self) {
            self.0.lock().expect("log").push("inactive".into());
        }
    }

    #[test]
    fn power_events_reach_the_loop_through_the_handle_and_stop_ends_it() {
        let rig = rig();
        let (tx, rx) = mpsc::channel();
        let handle = PresenceHandle(tx);
        handle.power_event(PowerEvent::WillSleep);
        handle.power_event(PowerEvent::DidWake);
        handle.stop();
        let log = Log::default();
        let mono = ManualMono::new(0);
        // Messages are queued before the loop starts: it sends the first beat, then reads
        // Sleep, Wake (which makes a beat due again), sends that, and reads Stop.
        run(&rx, rig.source.as_ref(), &mono, &FixedJitter(0), &log);
        assert_eq!(log.0.lock().expect("log").clone(), ["beat:Ok(())", "beat:Ok(())"], "the wake made a second beat due at once");
    }

    #[test]
    fn the_status_shows_stable_codes() {
        assert_eq!(SyncError::Remote(RemoteError::Http { status: 503 }).code(), "http_error");
        assert_eq!(SyncError::Rejected("device_not_owned").code(), "device_not_owned");
    }
}
