//! The lease poll driver: one `tick` every [`POLL_INTERVAL_MS`] fetches the lease, lets
//! `link_protocol::LeaseHolder` decide from the phone's MONOTONIC clock, brings the blocker in step
//! and sends the heartbeat.
//!
//! Fail-closed, fail-open on purpose:
//! * A fetch that fails (offline, server down) changes nothing: a valid lease keeps the phone
//!   locked until it expires on the monotonic clock, so a dead PC or server never locks the phone
//!   forever and a cut network never unlocks it early.
//! * The wall clock is not an input; the monotonic reading comes from [`Clock::mono_ms`].
//! * A fresh process that has fetched nothing yet never releases a lock the plugin restored after
//!   a reboot or a kill: it releases only on an explicit reason (a free lease, `not_paired`, the
//!   lease running out).
//! * A switched-off accessibility service is reported as `service_enabled: false` in the
//!   heartbeat (at once when the plugin says so); the server turns that into a `phone_gap`.
//!   Nothing here can stop the user from switching it off.

use std::sync::Arc;

use link_protocol::{FreeReason, PhoneBeat, PhoneLock};
use serde::Serialize;

use crate::Clock;
use crate::allow_list::AllowList;
use crate::android_blocker::BridgeEvent;
use crate::blocker::BlockerStatus;
use crate::blocker_port::{BlockerDriver, BlockerError, PhoneBlocker};
use crate::model::{PhoneCode, PhoneError};
use crate::rest::PhoneLink;
use crate::schedule::{LeaseDiagnostics, diagnose};

pub const POLL_INTERVAL_MS: u64 = 15_000;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PollReport {
    pub diagnostics: LeaseDiagnostics,
    pub blocker: BlockerStatus,
    pub service_enabled: bool,
    /// Whether the last server call got an answer.
    pub online: bool,
    pub last_error: Option<&'static str>,
}

pub struct PhonePoller {
    link: Arc<dyn PhoneLink>,
    blocker: Arc<dyn PhoneBlocker>,
    clock: Arc<dyn Clock>,
    driver: BlockerDriver<Arc<dyn PhoneBlocker>>,
    last_contact_mono: Option<i64>,
    last_status: BlockerStatus,
    service_enabled: bool,
    online: bool,
    last_error: Option<&'static str>,
}

/// Free for a reason someone gave, as opposed to "nothing received yet" or an unreadable clock.
fn explicit(reason: FreeReason) -> bool {
    !matches!(reason, FreeReason::NoLease | FreeReason::ClockInvalid)
}

impl PhonePoller {
    pub fn new(link: Arc<dyn PhoneLink>, blocker: Arc<dyn PhoneBlocker>, clock: Arc<dyn Clock>, allow: AllowList) -> Self {
        Self {
            link,
            blocker: blocker.clone(),
            clock,
            driver: BlockerDriver::new(blocker, allow),
            last_contact_mono: None,
            last_status: BlockerStatus::Unreachable,
            service_enabled: true,
            online: false,
            last_error: None,
        }
    }

    pub fn set_allow_list(&mut self, allow: AllowList) {
        self.driver.set_allow_list(allow);
    }

    fn fetch(&mut self, now: i64) {
        match self.link.fetch_lease() {
            Ok(fetch) => {
                self.online = true;
                self.last_error = None;
                self.last_contact_mono = Some(now);
                if let Some(lease) = fetch.lease {
                    // The same lease on every poll is not news (and not a "rejected" lease).
                    if self.driver.holder().current_seq() != Some(lease.seq) {
                        let _ = self.driver.offer(lease, fetch.server_now_ms, now);
                    }
                }
            }
            Err(PhoneError::Rejected(PhoneCode::NotPaired)) => {
                self.online = true;
                self.last_contact_mono = Some(now);
                self.driver.pair_revoked();
            }
            Err(err) => {
                self.online = false;
                self.last_error = Some(err.code());
            }
        }
    }

    fn read_status(&mut self) -> BlockerStatus {
        let status = self.blocker.status();
        self.last_status = status;
        match status {
            BlockerStatus::Active | BlockerStatus::Inactive => self.service_enabled = true,
            BlockerStatus::ServiceDisabled | BlockerStatus::Unsupported { .. } => self.service_enabled = false,
            BlockerStatus::Unreachable => {}
        }
        status
    }

    /// One round: fetch, decide, enforce, report.
    pub fn tick(&mut self) -> PollReport {
        let now = self.clock.mono_ms();
        self.fetch(now);
        let decision = self.driver.holder().decide(now);
        let before = self.read_status();
        match decision {
            PhoneLock::Free { reason } if explicit(reason) && before == BlockerStatus::Active => {
                // Also covers a lock the plugin restored for a process that has not engaged it.
                match self.blocker.release() {
                    Ok(()) => self.driver.forget_engagement(),
                    Err(err) => self.last_error = Some(err.code()),
                }
            }
            PhoneLock::Locked if matches!(before, BlockerStatus::Inactive | BlockerStatus::ServiceDisabled) => {
                // The plugin lost the lock behind our back: engage again.
                self.driver.forget_engagement();
            }
            _ => {}
        }
        match self.driver.tick(now) {
            Ok(_) => {}
            Err(BlockerError::ServiceDisabled) => self.service_enabled = false,
            Err(err) => self.last_error = Some(err.code()),
        }
        self.read_status();
        self.send_beat(&self.beat(now));
        self.report(now)
    }

    /// The beat that describes the phone right now. `locked` means the phone IS being blocked:
    /// a lease nobody can enforce (service off, iPhone) is not reported as a lock.
    fn beat(&self, now: i64) -> PhoneBeat {
        let locked = self.driver.holder().decide(now) == PhoneLock::Locked
            && !matches!(self.last_status, BlockerStatus::ServiceDisabled | BlockerStatus::Unsupported { .. } | BlockerStatus::Inactive);
        PhoneBeat { lease_seq: self.driver.holder().current_seq().unwrap_or(0), locked, service_enabled: self.service_enabled }
    }

    fn send_beat(&mut self, beat: &PhoneBeat) {
        match self.link.beat(beat) {
            Ok(()) => self.online = true,
            Err(PhoneError::Rejected(PhoneCode::NotPaired)) => {
                self.online = true;
                self.driver.pair_revoked();
            }
            Err(err) => {
                self.online = false;
                self.last_error = Some(err.code());
            }
        }
    }

    /// The plugin said something on its own. The matching heartbeat goes out at once, so a service
    /// switched off during a lock reaches the desktop without waiting for the next poll.
    pub fn on_event(&mut self, event: BridgeEvent) {
        let now = self.clock.mono_ms();
        let beat = event.apply(&self.beat(now));
        self.service_enabled = beat.service_enabled;
        if event == BridgeEvent::LockEnded {
            self.driver.forget_engagement();
        }
        self.send_beat(&beat);
    }

    pub fn report(&self, now: i64) -> PollReport {
        PollReport {
            diagnostics: diagnose(self.driver.holder(), now, self.last_contact_mono),
            blocker: self.last_status,
            service_enabled: self.service_enabled,
            online: self.online,
            last_error: self.last_error,
        }
    }

    /// The report without any network call (the diagnostics screen between polls).
    pub fn snapshot(&self) -> PollReport {
        self.report(self.clock.mono_ms())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use link_protocol::{LockLease, PickupReport, ReleaseCause};

    use super::*;
    use crate::blocker::ScreenTimeBlocker;
    use crate::blocker_port::{BlockerCall, MockBlocker};
    use crate::http::TransportError;
    use crate::model::{ClaimedPair, LeaseFetch};
    use crate::testing::TestClock;

    const MIN: i64 = 60_000;
    const SERVER_NOW: i64 = 1_000_000;

    type Fetch = Result<LeaseFetch, PhoneError>;

    #[derive(Default)]
    struct FakeLink {
        fetches: Mutex<VecDeque<Fetch>>,
        beats: Mutex<Vec<PhoneBeat>>,
        beat_error: Mutex<Option<PhoneError>>,
        down: std::sync::atomic::AtomicBool,
    }

    impl FakeLink {
        fn push(&self, fetch: Fetch) {
            self.fetches.lock().expect("fetches").push_back(fetch);
        }

        fn beats(&self) -> Vec<PhoneBeat> {
            self.beats.lock().expect("beats").clone()
        }
    }

    impl PhoneLink for FakeLink {
        fn claim(&self, _code: &str) -> Result<ClaimedPair, PhoneError> {
            Err(PhoneError::Decode)
        }

        fn fetch_lease(&self) -> Fetch {
            if self.down.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(PhoneError::Transport(TransportError::Timeout));
            }
            self.fetches.lock().expect("fetches").pop_front().unwrap_or(Err(PhoneError::Transport(TransportError::Network)))
        }

        fn beat(&self, beat: &PhoneBeat) -> Result<(), PhoneError> {
            self.beats.lock().expect("beats").push(beat.clone());
            if self.down.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(PhoneError::Transport(TransportError::Timeout));
            }
            match *self.beat_error.lock().expect("beat error") {
                Some(err) => Err(err),
                None => Ok(()),
            }
        }

        fn report_pickups(&self, _report: &PickupReport) -> Result<(), PhoneError> {
            Ok(())
        }

        fn leave(&self) -> Result<(), PhoneError> {
            Ok(())
        }
    }

    fn locked(seq: i64, minutes: i64) -> LockLease {
        LockLease::locked("run-1", seq, SERVER_NOW, SERVER_NOW + minutes * MIN).expect("lease")
    }

    fn has(lease: LockLease) -> Fetch {
        Ok(LeaseFetch { lease: Some(lease), server_now_ms: SERVER_NOW })
    }

    fn offline() -> Fetch {
        Err(PhoneError::Transport(TransportError::Timeout))
    }

    struct Rig {
        poller: PhonePoller,
        link: Arc<FakeLink>,
        blocker: Arc<MockBlocker>,
        clock: Arc<TestClock>,
    }

    fn rig() -> Rig {
        let link = Arc::new(FakeLink::default());
        let blocker = Arc::new(MockBlocker::default());
        let clock = Arc::new(TestClock::new(100, 5_000_000));
        let poller = PhonePoller::new(link.clone(), blocker.clone(), clock.clone(), AllowList::default());
        Rig { poller, link, blocker, clock }
    }

    impl Rig {
        /// The device and the clock move together, as they do on a phone.
        fn pass(&self, ms: i64) {
            self.clock.advance_mono(ms);
            self.blocker.advance(ms);
        }
    }

    #[test]
    fn a_locked_lease_engages_with_the_time_left_and_the_default_allow_list_then_beats_locked() {
        let mut rig = rig();
        rig.link.push(has(locked(1, 30)));
        let report = rig.poller.tick();
        let engaged = rig.blocker.engaged().expect("engaged");
        assert_eq!((engaged.run_id.as_str(), engaged.seq, engaged.remaining_ms), ("run-1", 1, 30 * MIN + MIN));
        match &rig.blocker.calls()[0] {
            BlockerCall::Engage { allow, .. } => assert_eq!(allow["roles"], serde_json::json!(["emergency_call", "dialer", "messages"])),
            other => panic!("{other:?}"),
        }
        assert_eq!(rig.link.beats(), [PhoneBeat { lease_seq: 1, locked: true, service_enabled: true }]);
        assert_eq!((report.diagnostics.lock, report.blocker, report.online), (PhoneLock::Locked, BlockerStatus::Active, true));
    }

    #[test]
    fn polling_the_same_lease_again_changes_nothing_and_counts_no_rejection() {
        let mut rig = rig();
        for _ in 0..4 {
            rig.link.push(has(locked(1, 30)));
            rig.pass(15_000);
            rig.poller.tick();
        }
        assert_eq!(rig.blocker.calls().len(), 1, "engaged once");
        assert_eq!(rig.poller.snapshot().diagnostics.rejected_leases, 0);
        assert_eq!(rig.link.beats().len(), 4);
    }

    #[test]
    fn offline_the_lock_holds_until_the_lease_ends_on_the_monotonic_clock_and_the_wall_clock_is_no_input() {
        let mut rig = rig();
        rig.link.push(has(locked(1, 10)));
        rig.poller.tick();
        // 10 min + 60 s of lease. The network goes away and the user moves the wall clock a day
        // forward: nothing changes.
        rig.link.down.store(true, std::sync::atomic::Ordering::SeqCst);
        rig.clock.shift_wall(86_400_000);
        rig.pass(5 * MIN);
        let mid = rig.poller.tick();
        assert!(!mid.online);
        assert_eq!(mid.last_error, Some("timeout"));
        assert_eq!(mid.diagnostics.lock, PhoneLock::Locked);
        assert!(rig.blocker.engaged().is_some());
        // One millisecond before the end (11 min after receipt): still locked. At the end: free.
        rig.pass(6 * MIN - 1);
        rig.poller.tick();
        assert_eq!(rig.poller.snapshot().diagnostics.lock, PhoneLock::Locked);
        rig.pass(1);
        let end = rig.poller.tick();
        assert_eq!(end.diagnostics.lock, PhoneLock::Free { reason: FreeReason::LeaseExpired });
        assert_eq!(rig.blocker.calls().last(), Some(&BlockerCall::Release));
        assert_eq!(rig.link.beats().last().map(|b| b.locked), Some(false), "the beat is attempted even offline; the server sees it when the network returns");
    }

    #[test]
    fn a_free_lease_or_a_revoked_pairing_releases_with_its_reason() {
        let mut rig = rig();
        rig.link.push(has(locked(1, 30)));
        rig.poller.tick();
        rig.link.push(has(LockLease::free("run-1", 2, SERVER_NOW, ReleaseCause::EmergencyPass).expect("lease")));
        let report = rig.poller.tick();
        assert_eq!(report.diagnostics.lock, PhoneLock::Free { reason: FreeReason::EmergencyPass });
        assert!(rig.blocker.engaged().is_none());

        let mut rig = rig_locked();
        rig.link.push(Err(PhoneError::Rejected(PhoneCode::NotPaired)));
        let report = rig.poller.tick();
        assert_eq!(report.diagnostics.lock, PhoneLock::Free { reason: FreeReason::PairRevoked });
        assert!(rig.blocker.engaged().is_none());
        assert!(report.online, "a not_paired answer is an answer");
    }

    fn rig_locked() -> Rig {
        let mut rig = rig();
        rig.link.push(has(locked(1, 30)));
        rig.poller.tick();
        rig
    }

    #[test]
    fn a_service_that_is_off_at_lock_time_is_a_gap_report_not_a_lock() {
        let mut rig = rig();
        rig.blocker.set_service_disabled(true);
        rig.link.push(has(locked(1, 30)));
        let report = rig.poller.tick();
        assert!(!report.service_enabled);
        assert_eq!(report.blocker, BlockerStatus::ServiceDisabled);
        assert_eq!(rig.link.beats(), [PhoneBeat { lease_seq: 1, locked: false, service_enabled: false }]);
        // The user switches it on: the next poll engages without a new lease.
        rig.blocker.set_service_disabled(false);
        rig.pass(15_000);
        rig.link.push(has(locked(1, 30)));
        let report = rig.poller.tick();
        assert!(report.service_enabled);
        assert_eq!(rig.blocker.engaged().map(|e| e.seq), Some(1));
        assert_eq!(rig.link.beats().last(), Some(&PhoneBeat { lease_seq: 1, locked: true, service_enabled: true }));
    }

    #[test]
    fn a_service_switched_off_during_the_lock_is_beaten_at_once_and_a_return_engages_again() {
        let mut rig = rig_locked();
        rig.blocker.set_service_disabled(true);
        rig.poller.on_event(BridgeEvent::ServiceDisabled);
        assert_eq!(rig.link.beats().last(), Some(&PhoneBeat { lease_seq: 1, locked: true, service_enabled: false }), "fixture: service_disabled keeps locked, drops service_enabled");
        assert!(!rig.poller.snapshot().service_enabled);

        rig.blocker.set_service_disabled(false);
        rig.poller.on_event(BridgeEvent::ServiceEnabled);
        rig.pass(15_000);
        rig.link.push(has(locked(1, 30)));
        let report = rig.poller.tick();
        assert_eq!(report.blocker, BlockerStatus::Active, "the lock the plugin lost is engaged again");
        assert_eq!(rig.blocker.calls().iter().filter(|c| matches!(c, BlockerCall::Engage { .. })).count(), 2);
        assert!(report.service_enabled);
    }

    #[test]
    fn the_device_ending_the_lock_is_beaten_as_not_locked() {
        let mut rig = rig_locked();
        rig.poller.on_event(BridgeEvent::LockEnded);
        assert_eq!(rig.link.beats().last(), Some(&PhoneBeat { lease_seq: 1, locked: false, service_enabled: true }));
    }

    #[test]
    fn a_new_process_does_not_release_a_lock_the_plugin_restored_until_the_server_says_so() {
        let mut rig = rig();
        // The plugin restored a lock after a reboot; this process has fetched nothing.
        let engagement = crate::blocker_port::Engagement { run_id: "run-1".into(), seq: 1, remaining_ms: 20 * MIN };
        rig.blocker.engage(&engagement, &AllowList::default()).expect("restore");

        rig.link.push(offline());
        rig.poller.tick();
        rig.link.push(Ok(LeaseFetch { lease: None, server_now_ms: SERVER_NOW }));
        rig.poller.tick();
        assert!(rig.blocker.engaged().is_some(), "no lease yet is not a reason to release");
        assert!(!rig.blocker.calls().contains(&BlockerCall::Release));

        rig.link.push(has(LockLease::free("run-1", 2, SERVER_NOW, ReleaseCause::EarlyRelease).expect("lease")));
        rig.poller.tick();
        assert!(rig.blocker.engaged().is_none(), "an explicit free lease releases a restored lock");
    }

    #[test]
    fn a_stale_or_forged_lease_is_refused_and_counted_while_the_lock_stays() {
        let mut rig = rig();
        rig.link.push(has(locked(5, 30)));
        rig.poller.tick();
        rig.link.push(has(locked(4, 30)));
        rig.poller.tick();
        let too_long = LockLease { valid_until_ms: SERVER_NOW + 5 * 60 * MIN, ..locked(6, 30) };
        rig.link.push(has(too_long));
        let report = rig.poller.tick();
        assert_eq!(report.diagnostics.seq, Some(5));
        assert_eq!(report.diagnostics.rejected_leases, 2);
        assert_eq!(report.diagnostics.lock, PhoneLock::Locked);
    }

    #[test]
    fn the_heartbeat_failing_with_not_paired_frees_the_phone_at_the_next_round() {
        let mut rig = rig_locked();
        *rig.link.beat_error.lock().expect("beat error") = Some(PhoneError::Rejected(PhoneCode::NotPaired));
        rig.link.push(has(locked(1, 30)));
        rig.poller.tick();
        rig.link.push(has(locked(1, 30)));
        let report = rig.poller.tick();
        assert_eq!(report.diagnostics.lock, PhoneLock::Free { reason: FreeReason::PairRevoked });
    }

    #[test]
    fn an_iphone_build_never_claims_a_lock() {
        let link = Arc::new(FakeLink::default());
        let clock = Arc::new(TestClock::new(0, 0));
        let mut poller = PhonePoller::new(link.clone(), Arc::new(ScreenTimeBlocker), clock, AllowList::default());
        link.push(has(locked(1, 30)));
        let report = poller.tick();
        assert!(matches!(report.blocker, BlockerStatus::Unsupported { .. }));
        assert!(!report.service_enabled);
        assert_eq!(report.last_error, Some("unsupported"));
        assert_eq!(link.beats(), [PhoneBeat { lease_seq: 1, locked: false, service_enabled: false }], "a lease nobody enforces is not reported as a lock");
    }
}
