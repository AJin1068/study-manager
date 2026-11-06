//! Edge cases of the lease rules: hostile values on the wire, replays, and the clock rules.

use link_protocol::*;

fn locked(seq: i64, issued: i64, end: i64) -> LockLease {
    LockLease::locked("run", seq, issued, end).expect("lease")
}

fn raw(seq: i64, state: LeaseState, issued_at_ms: i64, valid_until_ms: i64) -> LockLease {
    LockLease { run_id: "r".into(), seq, state, release_cause: None, issued_at_ms, valid_until_ms }
}

/// The wall clock never enters the decision: only (lease, server time at receipt, monotonic elapsed).
#[test]
fn only_the_monotonic_reading_moves_the_decision() {
    let mut holder = LeaseHolder::new();
    let server_now = 1_000_000_000_000;
    holder.offer(locked(1, server_now, server_now + 600_000), server_now, 5_000).expect("offer");
    assert_eq!(holder.decide(5_000 + 659_999), PhoneLock::Locked);
    assert_eq!(holder.decide(5_000 + 660_000), PhoneLock::Free { reason: FreeReason::LeaseExpired });
    assert_eq!(holder.decide(5_000 + 660_001), PhoneLock::Free { reason: FreeReason::LeaseExpired });
    // A monotonic reading before receipt (reboot) is invalid: free, ask the server.
    assert_eq!(holder.decide(4_999), PhoneLock::Free { reason: FreeReason::ClockInvalid });
    // A later server time does not renew a lease: the same seq again is a duplicate.
    let again = locked(1, server_now + 3_600_000, server_now + 4_200_000);
    assert_eq!(holder.offer(again, server_now + 3_600_000, 9_000), Err(LeaseReject::Duplicate));
    assert_eq!(holder.decide(5_000 + 659_999), PhoneLock::Locked);
}

#[test]
fn sequence_rules_hold_against_replays_and_hostile_numbers() {
    let mut holder = LeaseHolder::new();
    holder.offer(locked(10, 0, 1_000), 0, 0).expect("first");
    assert_eq!(holder.offer(locked(10, 0, 1_000), 0, 1), Err(LeaseReject::Duplicate));
    assert_eq!(holder.offer(locked(9, 0, 99_000), 0, 1), Err(LeaseReject::Regressed));
    // An older FREE lease replayed cannot unlock a newer locked lease.
    let old_free = LockLease::free("run", 3, 0, ReleaseCause::EmergencyPass).expect("free");
    assert_eq!(holder.offer(old_free, 0, 2), Err(LeaseReject::Regressed));
    assert_eq!(holder.decide(2), PhoneLock::Locked);
    for seq in [0, -1, i64::MIN] {
        assert_eq!(LeaseHolder::new().offer(raw(seq, LeaseState::Locked, 0, 10), 0, 0), Err(LeaseReject::Invalid(LeaseError::BadSeq)));
    }
    assert_eq!(LockLease::locked(&"x".repeat(65), 1, 0, 10), Err(LeaseError::BadRunId));
    assert_eq!(LockLease::locked("", 1, 0, 10), Err(LeaseError::BadRunId));
}

#[test]
fn the_length_cap_is_exact_and_extreme_values_do_not_overflow() {
    let at = |d: i64| raw(1, LeaseState::Locked, 0, MAX_LEASE_MS + d);
    assert!(at(0).validate().is_ok());
    assert_eq!(at(1).validate(), Err(LeaseError::TooLong));
    assert!(at(-1).validate().is_ok());
    assert!(LeaseReceipt::new(at(0), 0).is_ok());
    assert_eq!(LeaseReceipt::new(at(0), -1), Err(LeaseError::TooLong));
    // i64::MAX - i64::MIN overflows a plain subtraction (a panic in debug builds).
    let widest = raw(1, LeaseState::Locked, i64::MIN, i64::MAX);
    assert_eq!(widest.validate(), Err(LeaseError::TooLong));
    assert_eq!(LeaseHolder::new().offer(widest, 0, 0), Err(LeaseReject::Invalid(LeaseError::TooLong)));
    let far = raw(1, LeaseState::Locked, 0, i64::MAX);
    assert_eq!(far.validate(), Err(LeaseError::TooLong));
    assert_eq!(LeaseReceipt::new(far, i64::MIN).map(|_| ()), Err(LeaseError::TooLong));
    assert!(LockLease::locked("r", 1, 0, i64::MAX).is_err(), "the end saturates and is then too long");
    // A receipt with an extreme server time keeps a non-negative remainder.
    let receipt = LeaseReceipt::new(locked(1, 0, 1_000), i64::MAX).expect("receipt");
    assert_eq!(receipt.remaining_at_receipt_ms(), 0);
    assert_eq!(LeaseReceipt::new(locked(1, 0, 1_000), i64::MIN).map(|_| ()), Err(LeaseError::TooLong));
    // A lease whose end is already behind the server's clock at receipt is free at once.
    let mut holder = LeaseHolder::new();
    holder.offer(locked(1, 0, 1_000), 10_000_000, 0).expect("offer");
    assert_eq!(holder.decide(0), PhoneLock::Free { reason: FreeReason::LeaseExpired });
}

/// Server outage: the lock holds to the instant, then frees; a later lease locks again.
#[test]
fn an_outage_keeps_the_lock_until_expiry_then_frees_and_recovers() {
    let mut holder = LeaseHolder::new();
    holder.offer(locked(1, 0, 60_000), 0, 100).expect("offer");
    for t in [100, 60_000, 119_999 + 100] {
        assert_eq!(holder.decide(t), PhoneLock::Locked);
    }
    assert_eq!(holder.decide(120_100), PhoneLock::Free { reason: FreeReason::LeaseExpired });
    assert_eq!(holder.remaining_ms(120_100), None);
    holder.offer(locked(2, 500_000, 600_000), 500_000, 130_000).expect("relock");
    assert_eq!(holder.decide(130_001), PhoneLock::Locked);
    holder.pair_revoked();
    assert_eq!(holder.decide(130_002), PhoneLock::Free { reason: FreeReason::PairRevoked });
    assert_eq!(holder.offer(locked(2, 0, 60_000), 0, 140_000), Err(LeaseReject::Duplicate));
    holder.offer(locked(3, 700_000, 800_000), 700_000, 150_000).expect("re-pair lock");
    assert_eq!(holder.decide(150_001), PhoneLock::Locked);
}

#[test]
fn the_wire_rejects_unknown_fields_and_tampered_states() {
    let extra = r#"{"run_id":"r","seq":1,"state":"locked","release_cause":null,"issued_at_ms":0,"valid_until_ms":10,"extra":1}"#;
    assert!(serde_json::from_str::<LockLease>(extra).is_err(), "deny_unknown_fields");
    let tampered = r#"{"run_id":"r","seq":1,"state":"locked","release_cause":"run_ended","issued_at_ms":0,"valid_until_ms":10}"#;
    let lease: LockLease = serde_json::from_str(tampered).expect("parses");
    assert_eq!(lease.validate(), Err(LeaseError::LockedWithCause));
    assert_eq!(LeaseHolder::new().offer(lease, 0, 0), Err(LeaseReject::Invalid(LeaseError::LockedWithCause)));
}

/// The phone polls the lease in force every few seconds, so duplicates must not grow memory.
#[test]
fn rejections_are_capped_but_counted_exactly() {
    let mut holder = LeaseHolder::new();
    holder.offer(locked(5, 0, 1_000), 0, 0).expect("first");
    for i in 0..(MAX_REJECTED_KEPT as i64 * 3) {
        assert_eq!(holder.offer(locked(5, 0, 1_000), 0, i), Err(LeaseReject::Duplicate));
    }
    assert_eq!(holder.offer(locked(4, 0, 1_000), 0, 0), Err(LeaseReject::Regressed));
    assert_eq!(holder.rejected().len(), MAX_REJECTED_KEPT);
    assert_eq!(holder.rejected().last(), Some(&LeaseReject::Regressed), "the newest is kept");
    assert_eq!(holder.rejected_total(), MAX_REJECTED_KEPT as u64 * 3 + 1);
    assert_eq!(holder.duplicates(), MAX_REJECTED_KEPT as u64 * 3);
    assert_eq!(holder.current_seq(), Some(5), "rejections never touch the lease in force");
}
