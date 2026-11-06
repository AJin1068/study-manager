//! The lock lease: what the desktop app publishes at every segment change and the phone obeys.

use serde::{Deserialize, Serialize};

/// Slack added after the end of a segment so a slow phone does not unlock a few seconds early.
pub const GRACE_MS: i64 = 60_000;
/// No study segment is longer than this; a lease that would outlast it is refused by both sides.
pub const MAX_SEGMENT_MS: i64 = 3 * 60 * 60 * 1000;
/// The longest a lease may stay valid after it was issued.
pub const MAX_LEASE_MS: i64 = MAX_SEGMENT_MS + GRACE_MS;
const RUN_ID_MAX: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    Locked,
    Free,
}

/// Why the desktop app released the phone. Carried only by `Free` leases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseCause {
    RunEnded,
    EmergencyPass,
    SkipToday,
    EarlyRelease,
    PairRevoked,
}

/// One published lock state. `seq` rises with every publication of one user; all times are
/// milliseconds on the SERVER's clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockLease {
    pub run_id: String,
    pub seq: i64,
    pub state: LeaseState,
    pub release_cause: Option<ReleaseCause>,
    pub issued_at_ms: i64,
    pub valid_until_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseError {
    BadRunId,
    BadSeq,
    /// A locked lease must not carry a release cause.
    LockedWithCause,
    /// A free lease must say why it is free.
    FreeWithoutCause,
    /// A locked lease must end after it was issued.
    EmptySpan,
    /// A free lease is valid for exactly the instant it was issued.
    FreeWithSpan,
    TooLong,
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LeaseError::BadRunId => "bad_run_id",
            LeaseError::BadSeq => "bad_seq",
            LeaseError::LockedWithCause => "locked_with_cause",
            LeaseError::FreeWithoutCause => "free_without_cause",
            LeaseError::EmptySpan => "empty_span",
            LeaseError::FreeWithSpan => "free_with_span",
            LeaseError::TooLong => "lease_too_long",
        })
    }
}

impl std::error::Error for LeaseError {}

impl LockLease {
    /// A lock that holds until the end of the segment plus the grace.
    pub fn locked(run_id: &str, seq: i64, issued_at_ms: i64, segment_end_ms: i64) -> Result<Self, LeaseError> {
        let lease = Self {
            run_id: run_id.to_string(),
            seq,
            state: LeaseState::Locked,
            release_cause: None,
            issued_at_ms,
            valid_until_ms: segment_end_ms.saturating_add(GRACE_MS),
        };
        lease.validate()?;
        Ok(lease)
    }

    pub fn free(run_id: &str, seq: i64, issued_at_ms: i64, cause: ReleaseCause) -> Result<Self, LeaseError> {
        let lease = Self {
            run_id: run_id.to_string(),
            seq,
            state: LeaseState::Free,
            release_cause: Some(cause),
            issued_at_ms,
            valid_until_ms: issued_at_ms,
        };
        lease.validate()?;
        Ok(lease)
    }

    pub fn validate(&self) -> Result<(), LeaseError> {
        if self.run_id.is_empty() || self.run_id.len() > RUN_ID_MAX {
            return Err(LeaseError::BadRunId);
        }
        if self.seq < 1 {
            return Err(LeaseError::BadSeq);
        }
        match self.state {
            LeaseState::Locked => {
                if self.release_cause.is_some() {
                    return Err(LeaseError::LockedWithCause);
                }
                if self.valid_until_ms <= self.issued_at_ms {
                    return Err(LeaseError::EmptySpan);
                }
                // Saturating: the wire carries any i64, and a span that overflows is too long anyway.
                if self.valid_until_ms.saturating_sub(self.issued_at_ms) > MAX_LEASE_MS {
                    return Err(LeaseError::TooLong);
                }
            }
            LeaseState::Free => {
                if self.release_cause.is_none() {
                    return Err(LeaseError::FreeWithoutCause);
                }
                if self.valid_until_ms != self.issued_at_ms {
                    return Err(LeaseError::FreeWithSpan);
                }
            }
        }
        Ok(())
    }
}

/// A lease as the phone received it: the lease plus the server's clock reading at that moment.
/// Together with the phone's monotonic elapsed time this is the only input of `lock_decision`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseReceipt {
    lease: LockLease,
    server_now_ms: i64,
}

impl LeaseReceipt {
    /// Refuses an invalid lease, and a locked lease that would still be valid for longer than the
    /// cap when it arrives (a forged or replayed "lock forever").
    pub fn new(lease: LockLease, server_now_ms: i64) -> Result<Self, LeaseError> {
        lease.validate()?;
        if lease.state == LeaseState::Locked && lease.valid_until_ms.saturating_sub(server_now_ms) > MAX_LEASE_MS {
            return Err(LeaseError::TooLong);
        }
        Ok(Self { lease, server_now_ms })
    }

    pub fn lease(&self) -> &LockLease {
        &self.lease
    }

    pub fn server_now_ms(&self) -> i64 {
        self.server_now_ms
    }

    /// Milliseconds of lock left at the moment of receipt (never negative).
    pub fn remaining_at_receipt_ms(&self) -> i64 {
        self.lease.valid_until_ms.saturating_sub(self.server_now_ms).max(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseReject {
    Invalid(LeaseError),
    /// The same `seq` again: already applied.
    Duplicate,
    /// A lower `seq` than the one in force: an old publication replayed.
    Regressed,
}

impl std::fmt::Display for LeaseReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LeaseReject::Invalid(err) => write!(f, "{err}"),
            LeaseReject::Duplicate => f.write_str("duplicate_seq"),
            LeaseReject::Regressed => f.write_str("stale_seq"),
        }
    }
}

impl std::error::Error for LeaseReject {}

/// Whether `incoming` may replace the lease in force: it must be valid and carry a strictly
/// higher `seq`.
pub fn accept_lease(current: Option<&LockLease>, incoming: &LockLease) -> Result<(), LeaseReject> {
    incoming.validate().map_err(LeaseReject::Invalid)?;
    match current {
        Some(current) if incoming.seq == current.seq => Err(LeaseReject::Duplicate),
        Some(current) if incoming.seq < current.seq => Err(LeaseReject::Regressed),
        _ => Ok(()),
    }
}
