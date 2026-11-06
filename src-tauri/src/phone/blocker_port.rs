//! The phone-side blocker port and the driver that turns the lease in force into port calls.
//!
//! Adapters: `AndroidAccessibilityBlocker` (plugin bridge, `android_blocker`), `ScreenTimeBlocker`
//! (iPhone: always `Unsupported`, in `blocker`) and `MockBlocker` (host tests, records the calls).
//!
//! The port takes an `Engagement` (run id, sequence number, milliseconds of lock left) instead of
//! the raw `LockLease`: the device enforces the end of the lock by its own monotonic clock, so the
//! plugin needs a duration, not the server's wall-clock `valid_until_ms`. A lock therefore ends on
//! the device even if this process dies or the network never comes back.

use std::sync::{Mutex, MutexGuard};

use link_protocol::{FreeReason, LeaseHolder, LeaseReject, LeaseState, LockLease, PhoneLock, MAX_LEASE_MS};
use serde_json::Value;

use super::allow_list::AllowList;
use super::blocker::{BlockerStatus, UnsupportedReason};

/// What the blocker is asked to enforce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Engagement {
    pub run_id: String,
    pub seq: i64,
    /// Lock time left right now, 1..=`MAX_LEASE_MS`.
    pub remaining_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BlockerError {
    /// Nothing can be blocked on this platform. Never reported as a success.
    #[error("unsupported")]
    Unsupported(UnsupportedReason),
    /// The accessibility service is switched off (or its permission was never granted).
    #[error("service_disabled")]
    ServiceDisabled,
    /// A free lease, or an invalid one, cannot be engaged.
    #[error("not_locked")]
    NotLocked,
    #[error("bad_remaining")]
    BadRemaining,
    /// The plugin did not answer, or answered something the contract does not describe.
    #[error("bridge_unreachable")]
    BridgeUnreachable,
    #[error("bridge_malformed")]
    BridgeMalformed,
    /// The plugin refused with a code of its own.
    #[error("refused")]
    Refused,
}

impl BlockerError {
    pub fn code(self) -> &'static str {
        match self {
            BlockerError::Unsupported(_) => "unsupported",
            BlockerError::ServiceDisabled => "service_disabled",
            BlockerError::NotLocked => "not_locked",
            BlockerError::BadRemaining => "bad_remaining",
            BlockerError::BridgeUnreachable => "bridge_unreachable",
            BlockerError::BridgeMalformed => "bridge_malformed",
            BlockerError::Refused => "refused",
        }
    }
}

impl Engagement {
    /// The lease -> engage mapping. Only a valid locked lease with time left can be engaged; a
    /// free lease must go through `release`.
    pub fn new(lease: &LockLease, remaining_ms: i64) -> Result<Self, BlockerError> {
        lease.validate().map_err(|_| BlockerError::NotLocked)?;
        if lease.state != LeaseState::Locked {
            return Err(BlockerError::NotLocked);
        }
        if !(1..=MAX_LEASE_MS).contains(&remaining_ms) {
            return Err(BlockerError::BadRemaining);
        }
        Ok(Self { run_id: lease.run_id.clone(), seq: lease.seq, remaining_ms })
    }
}

pub trait PhoneBlocker: Send + Sync {
    fn status(&self) -> BlockerStatus;
    /// Starts (or replaces, for a higher `seq`) the lock. Engaging twice with the same `seq` is
    /// harmless.
    fn engage(&self, engagement: &Engagement, allow: &AllowList) -> Result<(), BlockerError>;
    fn release(&self) -> Result<(), BlockerError>;
}

impl PhoneBlocker for super::blocker::ScreenTimeBlocker {
    fn status(&self) -> BlockerStatus {
        super::blocker::ScreenTimeBlocker::status(self)
    }

    fn engage(&self, _engagement: &Engagement, _allow: &AllowList) -> Result<(), BlockerError> {
        Err(BlockerError::Unsupported(UnsupportedReason::AppleScreenTimeAuthorizationRequired))
    }

    fn release(&self) -> Result<(), BlockerError> {
        Err(BlockerError::Unsupported(UnsupportedReason::AppleScreenTimeAuthorizationRequired))
    }
}

// ---- mock ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockerCall {
    Engage { engagement: Engagement, allow: Value },
    Release,
}

#[derive(Debug, Default)]
struct MockState {
    calls: Vec<BlockerCall>,
    engaged: Option<Engagement>,
    service_disabled: bool,
    unreachable: bool,
}

/// Host-test blocker. It records every call and models the device side: an engaged lock counts
/// down on `advance` and ends by itself at zero, like the Kotlin service does.
#[derive(Debug, Default)]
pub struct MockBlocker {
    state: Mutex<MockState>,
}

impl MockBlocker {
    fn lock(&self) -> MutexGuard<'_, MockState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn calls(&self) -> Vec<BlockerCall> {
        self.lock().calls.clone()
    }

    pub fn engaged(&self) -> Option<Engagement> {
        self.lock().engaged.clone()
    }

    /// The user switched the accessibility service off (or on again).
    pub fn set_service_disabled(&self, disabled: bool) {
        let mut state = self.lock();
        state.service_disabled = disabled;
        if disabled {
            state.engaged = None;
        }
    }

    pub fn set_unreachable(&self, unreachable: bool) {
        self.lock().unreachable = unreachable;
    }

    /// Device time passes.
    pub fn advance(&self, ms: i64) {
        let mut state = self.lock();
        if let Some(engaged) = state.engaged.as_mut() {
            engaged.remaining_ms -= ms;
            if engaged.remaining_ms <= 0 {
                state.engaged = None;
            }
        }
    }
}

impl PhoneBlocker for MockBlocker {
    fn status(&self) -> BlockerStatus {
        let state = self.lock();
        if state.unreachable {
            BlockerStatus::Unreachable
        } else if state.service_disabled {
            BlockerStatus::ServiceDisabled
        } else if state.engaged.is_some() {
            BlockerStatus::Active
        } else {
            BlockerStatus::Inactive
        }
    }

    fn engage(&self, engagement: &Engagement, allow: &AllowList) -> Result<(), BlockerError> {
        let mut state = self.lock();
        if state.unreachable {
            return Err(BlockerError::BridgeUnreachable);
        }
        if state.service_disabled {
            return Err(BlockerError::ServiceDisabled);
        }
        state.calls.push(BlockerCall::Engage { engagement: engagement.clone(), allow: allow.to_wire() });
        state.engaged = Some(engagement.clone());
        Ok(())
    }

    fn release(&self) -> Result<(), BlockerError> {
        let mut state = self.lock();
        if state.unreachable {
            return Err(BlockerError::BridgeUnreachable);
        }
        state.calls.push(BlockerCall::Release);
        state.engaged = None;
        Ok(())
    }
}

// ---- driver ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickOutcome {
    Unchanged,
    Engaged { seq: i64 },
    Released { reason: FreeReason },
}

/// Follows the lease in force and keeps the blocker in step with it. The caller passes the
/// phone's monotonic clock reading and the server time of each fetch; the wall clock is no input.
/// A failed port call leaves the driver unchanged, so the next `tick` retries it.
pub struct BlockerDriver<B: PhoneBlocker> {
    blocker: B,
    allow: AllowList,
    holder: LeaseHolder,
    run_id: Option<String>,
    engaged_seq: Option<i64>,
}

impl<B: PhoneBlocker> BlockerDriver<B> {
    pub fn new(blocker: B, allow: AllowList) -> Self {
        Self { blocker, allow, holder: LeaseHolder::new(), run_id: None, engaged_seq: None }
    }

    pub fn blocker(&self) -> &B {
        &self.blocker
    }

    pub fn set_allow_list(&mut self, allow: AllowList) {
        self.allow = allow;
    }

    /// A fetched lease. Stale or repeated ones are refused as `LeaseHolder` does.
    pub fn offer(&mut self, lease: LockLease, server_now_ms: i64, mono_now_ms: i64) -> Result<(), LeaseReject> {
        let run_id = lease.run_id.clone();
        self.holder.offer(lease, server_now_ms, mono_now_ms)?;
        self.run_id = Some(run_id);
        Ok(())
    }

    pub fn pair_revoked(&mut self) {
        self.holder.pair_revoked();
    }

    pub fn tick(&mut self, mono_now_ms: i64) -> Result<TickOutcome, BlockerError> {
        match self.holder.decide(mono_now_ms) {
            PhoneLock::Locked => {
                let (Some(seq), Some(run_id), Some(remaining_ms)) =
                    (self.holder.current_seq(), self.run_id.as_ref(), self.holder.remaining_ms(mono_now_ms))
                else {
                    return Err(BlockerError::NotLocked);
                };
                if self.engaged_seq == Some(seq) {
                    return Ok(TickOutcome::Unchanged);
                }
                let engagement = Engagement { run_id: run_id.clone(), seq, remaining_ms };
                if !(1..=MAX_LEASE_MS).contains(&engagement.remaining_ms) {
                    return Err(BlockerError::BadRemaining);
                }
                self.blocker.engage(&engagement, &self.allow)?;
                self.engaged_seq = Some(seq);
                Ok(TickOutcome::Engaged { seq })
            }
            PhoneLock::Free { reason } => {
                if self.engaged_seq.is_none() {
                    return Ok(TickOutcome::Unchanged);
                }
                self.blocker.release()?;
                self.engaged_seq = None;
                Ok(TickOutcome::Released { reason })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use link_protocol::ReleaseCause;
    use serde_json::json;

    use super::*;
    use crate::phone::blocker::ScreenTimeBlocker;

    const MIN: i64 = 60_000;
    const SERVER_NOW: i64 = 1_000_000;

    fn locked(seq: i64, minutes: i64) -> LockLease {
        LockLease::locked("run-1", seq, SERVER_NOW, SERVER_NOW + minutes * MIN).expect("lease")
    }

    fn driver() -> BlockerDriver<MockBlocker> {
        BlockerDriver::new(MockBlocker::default(), AllowList::default())
    }

    fn mapping_fixture() -> Value {
        serde_json::from_str::<Value>(include_str!("../../../fixtures/phone/blocker-bridge.json")).expect("fixture")["leaseMapping"].clone()
    }

    #[test]
    fn fixture_lease_to_engage_mapping() {
        for case in mapping_fixture().as_array().expect("cases") {
            let name = case["name"].as_str().expect("name");
            let lease: LockLease = serde_json::from_value(case["lease"].clone()).expect("lease");
            let outcome = Engagement::new(&lease, case["remaining_ms"].as_i64().expect("remaining"));
            match case["expect"].as_object().expect("expect") {
                ok if ok.contains_key("run_id") => {
                    let engagement = outcome.unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert_eq!(engagement.run_id, ok["run_id"].as_str().expect("run_id"), "{name}");
                    assert_eq!(engagement.seq, ok["seq"].as_i64().expect("seq"), "{name}");
                    assert_eq!(engagement.remaining_ms, ok["remaining_ms"].as_i64().expect("remaining"), "{name}");
                }
                err => assert_eq!(outcome.expect_err(name).code(), err["error"].as_str().expect("error"), "{name}"),
            }
        }
    }

    #[test]
    fn lock_start_engages_with_the_remaining_time_and_the_allow_list() {
        let mut driver = driver();
        assert_eq!(driver.tick(0), Ok(TickOutcome::Unchanged), "no lease yet");
        driver.offer(locked(1, 30), SERVER_NOW + 5_000, 100).expect("offer");
        // 30 minutes + 60 s grace, 5 s already gone on the server clock.
        assert_eq!(driver.tick(100), Ok(TickOutcome::Engaged { seq: 1 }));
        let engaged = driver.blocker().engaged().expect("engaged");
        assert_eq!((engaged.run_id.as_str(), engaged.seq, engaged.remaining_ms), ("run-1", 1, 30 * MIN + MIN - 5_000));
        assert_eq!(driver.blocker().status(), BlockerStatus::Active);
        let calls = driver.blocker().calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            BlockerCall::Engage { allow, .. } => assert_eq!(allow["roles"][0], json!("emergency_call")),
            other => panic!("{other:?}"),
        }
        assert_eq!(driver.tick(200), Ok(TickOutcome::Unchanged), "the same lease is not engaged twice");
        assert_eq!(driver.blocker().calls().len(), 1);
    }

    #[test]
    fn release_by_the_desktop_ends_the_lock() {
        let mut driver = driver();
        driver.offer(locked(1, 30), SERVER_NOW, 0).expect("offer");
        driver.tick(0).expect("engage");
        let free = LockLease::free("run-1", 2, SERVER_NOW + MIN, ReleaseCause::EmergencyPass).expect("free");
        driver.offer(free, SERVER_NOW + MIN, MIN).expect("offer free");
        assert_eq!(driver.tick(MIN), Ok(TickOutcome::Released { reason: FreeReason::EmergencyPass }));
        assert_eq!(driver.blocker().status(), BlockerStatus::Inactive);
        assert_eq!(driver.blocker().calls().last(), Some(&BlockerCall::Release));
        assert_eq!(driver.tick(MIN + 1), Ok(TickOutcome::Unchanged), "released once");
    }

    #[test]
    fn a_lease_that_runs_out_releases_without_any_message_from_the_desktop() {
        let mut driver = driver();
        driver.offer(locked(1, 1), SERVER_NOW, 0).expect("offer");
        driver.tick(0).expect("engage");
        // 1 minute + 60 s grace: the last locked millisecond, then free.
        assert_eq!(driver.tick(2 * MIN - 1), Ok(TickOutcome::Unchanged));
        assert_eq!(driver.tick(2 * MIN), Ok(TickOutcome::Released { reason: FreeReason::LeaseExpired }));
        assert_eq!(driver.blocker().engaged(), None);
    }

    #[test]
    fn the_device_side_deadline_frees_the_phone_even_if_the_driver_never_ticks_again() {
        let mut driver = driver();
        driver.offer(locked(1, 1), SERVER_NOW, 0).expect("offer");
        driver.tick(0).expect("engage");
        driver.blocker().advance(2 * MIN);
        assert_eq!(driver.blocker().status(), BlockerStatus::Inactive);
    }

    #[test]
    fn a_higher_seq_replaces_the_engagement_and_a_stale_one_changes_nothing() {
        let mut driver = driver();
        driver.offer(locked(2, 10), SERVER_NOW, 0).expect("offer");
        driver.tick(0).expect("engage");
        assert_eq!(driver.offer(locked(1, 90), SERVER_NOW, 5), Err(LeaseReject::Regressed));
        assert_eq!(driver.offer(locked(2, 90), SERVER_NOW, 5), Err(LeaseReject::Duplicate));
        assert_eq!(driver.tick(5), Ok(TickOutcome::Unchanged));
        driver.offer(locked(3, 20), SERVER_NOW, 10).expect("extension");
        assert_eq!(driver.tick(10), Ok(TickOutcome::Engaged { seq: 3 }));
        assert_eq!(driver.blocker().engaged().expect("engaged").seq, 3);
    }

    #[test]
    fn a_revoked_pairing_releases_the_phone() {
        let mut driver = driver();
        driver.offer(locked(1, 30), SERVER_NOW, 0).expect("offer");
        driver.tick(0).expect("engage");
        driver.pair_revoked();
        assert_eq!(driver.tick(1), Ok(TickOutcome::Released { reason: FreeReason::PairRevoked }));
    }

    #[test]
    fn a_disabled_service_is_an_error_and_the_next_tick_retries() {
        let mut driver = driver();
        driver.blocker().set_service_disabled(true);
        driver.offer(locked(1, 30), SERVER_NOW, 0).expect("offer");
        assert_eq!(driver.tick(0), Err(BlockerError::ServiceDisabled));
        assert_eq!(driver.blocker().status(), BlockerStatus::ServiceDisabled);
        driver.blocker().set_service_disabled(false);
        assert_eq!(driver.tick(1_000), Ok(TickOutcome::Engaged { seq: 1 }));
        assert_eq!(driver.blocker().engaged().expect("engaged").remaining_ms, 31 * MIN - 1_000);
    }

    #[test]
    fn a_failed_release_is_retried() {
        let mut driver = driver();
        driver.offer(locked(1, 1), SERVER_NOW, 0).expect("offer");
        driver.tick(0).expect("engage");
        driver.blocker().set_unreachable(true);
        assert_eq!(driver.tick(2 * MIN), Err(BlockerError::BridgeUnreachable));
        driver.blocker().set_unreachable(false);
        assert_eq!(driver.tick(2 * MIN + 1), Ok(TickOutcome::Released { reason: FreeReason::LeaseExpired }));
    }

    #[test]
    fn the_iphone_blocker_never_engages_silently() {
        let blocker = ScreenTimeBlocker;
        let engagement = Engagement::new(&locked(1, 30), 30 * MIN).expect("engagement");
        let unsupported = BlockerError::Unsupported(UnsupportedReason::AppleScreenTimeAuthorizationRequired);
        assert_eq!(PhoneBlocker::engage(&blocker, &engagement, &AllowList::default()), Err(unsupported));
        assert_eq!(PhoneBlocker::release(&blocker), Err(unsupported));
        assert!(matches!(PhoneBlocker::status(&blocker), BlockerStatus::Unsupported { .. }));

        let mut driver = BlockerDriver::new(ScreenTimeBlocker, AllowList::default());
        driver.offer(locked(1, 30), SERVER_NOW, 0).expect("offer");
        assert_eq!(driver.tick(0), Err(unsupported), "a lock request on an iPhone is an explicit error");
    }

    #[test]
    fn free_and_out_of_range_leases_cannot_be_engaged() {
        let free = LockLease::free("run-1", 2, SERVER_NOW, ReleaseCause::RunEnded).expect("free");
        assert_eq!(Engagement::new(&free, MIN), Err(BlockerError::NotLocked));
        assert_eq!(Engagement::new(&locked(1, 1), 0), Err(BlockerError::BadRemaining));
        assert_eq!(Engagement::new(&locked(1, 1), MAX_LEASE_MS + 1), Err(BlockerError::BadRemaining));
        assert!(Engagement::new(&locked(1, 1), MAX_LEASE_MS).is_ok());
    }
}
