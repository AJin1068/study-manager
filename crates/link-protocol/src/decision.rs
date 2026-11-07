//! The pure decision: is the phone locked right now?

use serde::{Deserialize, Serialize};

use crate::lease::{LeaseReceipt, LeaseReject, LeaseState, LockLease, ReleaseCause, accept_lease};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreeReason {
    /// No lease was ever received (or it was discarded).
    NoLease,
    /// The lease ran out. The only way a locked phone becomes free without the desktop app: a
    /// dead PC or server never keeps the phone locked past the segment.
    LeaseExpired,
    /// The monotonic clock went backwards (a reboot): the phone asks the server again.
    ClockInvalid,
    RunEnded,
    EmergencyPass,
    SkipToday,
    EarlyRelease,
    PairRevoked,
}

impl From<ReleaseCause> for FreeReason {
    fn from(cause: ReleaseCause) -> Self {
        match cause {
            ReleaseCause::RunEnded => FreeReason::RunEnded,
            ReleaseCause::EmergencyPass => FreeReason::EmergencyPass,
            ReleaseCause::SkipToday => FreeReason::SkipToday,
            ReleaseCause::EarlyRelease => FreeReason::EarlyRelease,
            ReleaseCause::PairRevoked => FreeReason::PairRevoked,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PhoneLock {
    Locked,
    Free { reason: FreeReason },
}

/// `mono_elapsed_ms` is how long ago the lease arrived by the phone's MONOTONIC clock. Whether
/// the server is reachable is deliberately not an input: a valid lease keeps the phone locked
/// offline (fail-closed) and an expired one frees it online or offline (no eternal lock).
///
/// The lease ends at `valid_until`: with `remaining` ms left at receipt, `elapsed == remaining - 1`
/// is still locked and `elapsed == remaining` is free.
pub fn lock_decision(receipt: &LeaseReceipt, mono_elapsed_ms: i64) -> PhoneLock {
    if mono_elapsed_ms < 0 {
        return PhoneLock::Free {
            reason: FreeReason::ClockInvalid,
        };
    }
    let lease = receipt.lease();
    match (lease.state, lease.release_cause) {
        (LeaseState::Free, Some(cause)) => PhoneLock::Free {
            reason: cause.into(),
        },
        // A validated free lease always has a cause; reaching here means it was built by hand.
        (LeaseState::Free, None) => PhoneLock::Free {
            reason: FreeReason::NoLease,
        },
        (LeaseState::Locked, _) => {
            if mono_elapsed_ms >= receipt.remaining_at_receipt_ms() {
                PhoneLock::Free {
                    reason: FreeReason::LeaseExpired,
                }
            } else {
                PhoneLock::Locked
            }
        }
    }
}

/// The lease in force on one phone: applies the sequence rules on arrival and decides from the
/// phone's monotonic readings. Both the readings and the server time are passed in.
/// Rejections kept for the diagnostics screen. The phone polls the same lease every few seconds, so
/// "duplicate" arrives all day: only the newest are kept, the counters stay exact.
pub const MAX_REJECTED_KEPT: usize = 32;

#[derive(Debug, Default)]
pub struct LeaseHolder {
    held: Option<(LeaseReceipt, i64)>,
    /// The newest [`MAX_REJECTED_KEPT`] rejections, oldest first.
    rejected: Vec<LeaseReject>,
    rejected_total: u64,
    duplicates: u64,
    pair_revoked: bool,
}

impl LeaseHolder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes a lease fetched from the server. A stale, repeated or invalid lease changes nothing
    /// and is remembered for the diagnostics screen.
    pub fn offer(
        &mut self,
        lease: LockLease,
        server_now_ms: i64,
        mono_now_ms: i64,
    ) -> Result<(), LeaseReject> {
        let current = self.held.as_ref().map(|(receipt, _)| receipt.lease());
        let verdict = accept_lease(current, &lease)
            .and_then(|()| LeaseReceipt::new(lease, server_now_ms).map_err(LeaseReject::Invalid));
        match verdict {
            Ok(receipt) => {
                self.held = Some((receipt, mono_now_ms));
                self.pair_revoked = false;
                Ok(())
            }
            Err(reject) => {
                if self.rejected.len() >= MAX_REJECTED_KEPT {
                    self.rejected.remove(0);
                }
                self.rejected.push(reject);
                self.rejected_total = self.rejected_total.saturating_add(1);
                if reject == LeaseReject::Duplicate {
                    self.duplicates = self.duplicates.saturating_add(1);
                }
                Err(reject)
            }
        }
    }

    /// The server says this phone is no longer paired (`not_paired`). Nobody can publish a free
    /// lease to a revoked phone, so the phone frees itself; the next lease it receives after a new
    /// pairing locks it again.
    pub fn pair_revoked(&mut self) {
        self.pair_revoked = true;
    }

    pub fn decide(&self, mono_now_ms: i64) -> PhoneLock {
        if self.pair_revoked {
            return PhoneLock::Free {
                reason: FreeReason::PairRevoked,
            };
        }
        match &self.held {
            None => PhoneLock::Free {
                reason: FreeReason::NoLease,
            },
            Some((receipt, received_at)) => {
                lock_decision(receipt, mono_now_ms.saturating_sub(*received_at))
            }
        }
    }

    pub fn current_seq(&self) -> Option<i64> {
        self.held.as_ref().map(|(receipt, _)| receipt.lease().seq)
    }

    /// Milliseconds until a locked phone frees itself; `None` when not locked.
    pub fn remaining_ms(&self, mono_now_ms: i64) -> Option<i64> {
        let (receipt, received_at) = self.held.as_ref().filter(|_| !self.pair_revoked)?;
        let elapsed = mono_now_ms.saturating_sub(*received_at);
        (lock_decision(receipt, elapsed) == PhoneLock::Locked)
            .then(|| receipt.remaining_at_receipt_ms() - elapsed)
    }

    /// The newest rejections (at most [`MAX_REJECTED_KEPT`]), oldest first.
    pub fn rejected(&self) -> &[LeaseReject] {
        &self.rejected
    }

    /// Every rejection since the holder was made, including those no longer in [`Self::rejected`].
    pub fn rejected_total(&self) -> u64 {
        self.rejected_total
    }

    /// How many of those were the lease already in force offered again (harmless polling repeats).
    pub fn duplicates(&self) -> u64 {
        self.duplicates
    }
}
