//! Runs the shared JSON fixtures (`fixtures/phone/` at the repository root) against the crate.
//! The desktop app and the phone project read the same files.

use link_protocol::*;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = format!("{}/../../fixtures/phone/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn from<T: DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).expect("fixture value decodes")
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is text"))
}

fn num(value: &Value, key: &str) -> i64 {
    value[key]
        .as_i64()
        .unwrap_or_else(|| panic!("{key} is a number"))
}

#[test]
fn wire_samples_round_trip_byte_for_byte() {
    let f = load("link-protocol.json");
    fn round<T: DeserializeOwned + serde::Serialize>(f: &Value, key: &str) {
        let parsed: T = from(&f[key]);
        assert_eq!(
            serde_json::to_value(&parsed).expect("encodes"),
            f[key],
            "{key}"
        );
    }
    round::<PairingOffer>(&f, "pairingOffer");
    round::<LockLease>(&f, "lockedLease");
    round::<LockLease>(&f, "freeLease");
    round::<PhoneBeat>(&f, "beat");
    round::<PickupReport>(&f, "pickup");
    round::<PhoneLock>(&f, "phoneLockLocked");
    round::<PhoneLock>(&f, "phoneLockFree");
    let lease: LockLease = from(&f["lockedLease"]);
    assert_eq!(lease.validate(), Ok(()));
    let free: LockLease = from(&f["freeLease"]);
    assert_eq!(free.validate(), Ok(()));
}

#[test]
fn the_qr_text_is_exactly_the_fixture_and_parses_back() {
    let f = load("link-protocol.json");
    let offer = PairingOffer::new(
        "abcde-fghjk",
        "https://proj.supabase.co/",
        "anon-public-key",
    )
    .expect("offer");
    assert_eq!(
        offer.to_qr_text().expect("text"),
        text(&f, "pairingOfferQrText")
    );
    assert_eq!(
        PairingOffer::parse(text(&f, "pairingOfferQrText")),
        Ok(offer)
    );
}

#[test]
fn unknown_fields_are_refused_so_nothing_extra_can_ride_along() {
    let f = load("link-protocol.json");
    let mut beat = f["beat"].clone();
    beat["app_name"] = Value::String("x".into());
    assert!(serde_json::from_value::<PhoneBeat>(beat).is_err());
    let mut pickup = f["pickup"].clone();
    pickup["package"] = Value::String("x".into());
    assert!(serde_json::from_value::<PickupReport>(pickup).is_err());
    let mut lease = f["lockedLease"].clone();
    lease["note"] = Value::String("x".into());
    assert!(serde_json::from_value::<LockLease>(lease).is_err());
}

fn expected_lock(expect: &Value) -> PhoneLock {
    from(expect)
}

#[test]
fn the_decision_table_holds_row_by_row() {
    let t = load("lock-decision-table.json");
    for row in t["decisions"].as_array().expect("rows") {
        let name = text(row, "name");
        let lease: LockLease = from(&t["leases"][text(row, "lease")]);
        let receipt = LeaseReceipt::new(lease, num(row, "server_now_ms"))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            lock_decision(&receipt, num(row, "elapsed_ms")),
            expected_lock(&row["expect"]),
            "{name}"
        );
    }
}

#[test]
fn a_holder_ignores_the_wall_clock_entirely() {
    // The holder is driven by monotonic readings only; a wall clock that jumps by an hour is
    // simply not an input. The fixture rows with wall_shift_ms must equal the unshifted answer.
    let t = load("lock-decision-table.json");
    let lease: LockLease = from(&t["leases"]["locked"]);
    let mut holder = LeaseHolder::new();
    holder.offer(lease, 1_010_000, 50_000).expect("offer");
    let before = holder.decide(50_000 + 300_000);
    let shifted_rows = t["decisions"]
        .as_array()
        .expect("rows")
        .iter()
        .filter(|r| r.get("wall_shift_ms").is_some())
        .count();
    assert_eq!(shifted_rows, 2);
    assert_eq!(before, PhoneLock::Locked);
    assert_eq!(holder.decide(50_000 + 589_999), PhoneLock::Locked);
    assert_eq!(
        holder.decide(50_000 + 590_000),
        PhoneLock::Free {
            reason: FreeReason::LeaseExpired
        }
    );
}

#[test]
fn sequence_rules_hold_row_by_row() {
    let t = load("lock-decision-table.json");
    for row in t["sequence"].as_array().expect("rows") {
        let name = text(row, "name");
        let make = |seq: i64| LockLease {
            run_id: "r".into(),
            seq,
            state: LeaseState::Locked,
            release_cause: None,
            issued_at_ms: 0,
            valid_until_ms: 1000,
        };
        let current = row["current_seq"].as_i64().map(make);
        let got = match accept_lease(current.as_ref(), &make(num(row, "incoming_seq"))) {
            Ok(()) => "ok".to_string(),
            Err(LeaseReject::Duplicate) => "duplicate".into(),
            Err(LeaseReject::Regressed) => "regressed".into(),
            Err(LeaseReject::Invalid(e)) => format!("invalid:{e}"),
        };
        assert_eq!(got, text(row, "expect"), "{name}");
    }
}

#[test]
fn invalid_leases_and_the_length_cap() {
    let t = load("lock-decision-table.json");
    for row in t["invalid"].as_array().expect("rows") {
        let lease: LockLease = from(&row["lease"]);
        let got = lease
            .validate()
            .map_or_else(|e| e.to_string(), |()| "ok".into());
        assert_eq!(got, text(row, "expect"), "{}", text(row, "name"));
    }
    for row in t["receiptCap"].as_array().expect("rows") {
        let lease = LockLease {
            run_id: "r".into(),
            seq: 1,
            state: LeaseState::Locked,
            release_cause: None,
            issued_at_ms: num(row, "issued_at_ms"),
            valid_until_ms: num(row, "valid_until_ms"),
        };
        let got = LeaseReceipt::new(lease, num(row, "server_now_ms"))
            .map_or_else(|e| e.to_string(), |_| "ok".into());
        assert_eq!(got, text(row, "expect"), "{}", text(row, "name"));
    }
}

#[test]
fn a_holder_rejects_replays_and_keeps_the_lease_in_force() {
    let mut holder = LeaseHolder::new();
    assert_eq!(
        holder.decide(0),
        PhoneLock::Free {
            reason: FreeReason::NoLease
        }
    );
    let lock = |seq| LockLease::locked("run-1", seq, 1_000, 601_000 - GRACE_MS).expect("lease");
    holder.offer(lock(5), 1_000, 0).expect("first");
    assert_eq!(
        holder.offer(lock(5), 1_000, 10),
        Err(LeaseReject::Duplicate)
    );
    assert_eq!(
        holder.offer(lock(4), 1_000, 10),
        Err(LeaseReject::Regressed)
    );
    assert_eq!(holder.current_seq(), Some(5));
    assert_eq!(holder.decide(100), PhoneLock::Locked);
    assert_eq!(
        holder.remaining_ms(100),
        Some(600_000 - 100 + GRACE_MS - GRACE_MS)
    );
    // A newer free lease releases at once, with its reason.
    holder
        .offer(
            LockLease::free("run-1", 6, 2_000, ReleaseCause::SkipToday).expect("free"),
            2_000,
            200,
        )
        .expect("free lease");
    assert_eq!(
        holder.decide(201),
        PhoneLock::Free {
            reason: FreeReason::SkipToday
        }
    );
    assert_eq!(holder.remaining_ms(201), None);
    assert_eq!(
        holder.rejected(),
        &[LeaseReject::Duplicate, LeaseReject::Regressed]
    );
}

#[test]
fn a_lock_longer_than_the_cap_is_refused_on_arrival() {
    let mut holder = LeaseHolder::new();
    let long = LockLease {
        run_id: "r".into(),
        seq: 1,
        state: LeaseState::Locked,
        release_cause: None,
        issued_at_ms: 0,
        valid_until_ms: MAX_LEASE_MS,
    };
    // Valid on its own, but it arrives with more than the cap still to run.
    assert_eq!(
        holder.offer(long.clone(), -1, 0),
        Err(LeaseReject::Invalid(LeaseError::TooLong))
    );
    assert!(holder.offer(long, 0, 0).is_ok());
}

#[test]
fn the_constructor_adds_the_grace_and_the_cap_binds_it() {
    let lease = LockLease::locked("r", 1, 0, 1_000).expect("lease");
    assert_eq!(lease.valid_until_ms, 1_000 + GRACE_MS);
    assert_eq!(
        LockLease::locked("r", 1, 0, MAX_SEGMENT_MS).map(|l| l.valid_until_ms),
        Ok(MAX_LEASE_MS)
    );
    assert_eq!(
        LockLease::locked("r", 1, 0, MAX_SEGMENT_MS + 1),
        Err(LeaseError::TooLong)
    );
}

#[test]
fn offers_are_refused_whole_when_any_part_is_off() {
    for (code, endpoint, key) in [
        ("SHORT", "https://proj.supabase.co", "k"),
        ("ABCDEFGHJU", "https://proj.supabase.co", "k"),
        ("ABCDEFGHJK", "http://evil.example", "k"),
        ("ABCDEFGHJK", "https://user:pw@x.test", "k"),
        ("ABCDEFGHJK", "https://x.test/path", "k"),
        ("ABCDEFGHJK", "https://proj.supabase.co", ""),
        ("ABCDEFGHJK", "https://proj.supabase.co", "has space"),
    ] {
        assert!(
            PairingOffer::new(code, endpoint, key).is_err(),
            "{code} {endpoint} {key}"
        );
    }
    assert!(
        PairingOffer::new("ABCDEFGHJK", "http://127.0.0.1:54321", "k").is_ok(),
        "loopback http is for local development"
    );
    assert_eq!(PairingOffer::parse("not json"), Err(OfferError::NotJson));
    assert_eq!(
        PairingOffer::parse(
            r#"{"v":2,"code":"ABCDEFGHJK","endpoint":"https://a.test","anon_key":"k"}"#
        ),
        Err(OfferError::Version)
    );
    assert_eq!(
        PairingOffer::parse(
            r#"{"v":1,"code":"abcdefghjk","endpoint":"https://a.test","anon_key":"k"}"#
        ),
        Err(OfferError::BadCode)
    );
}

#[test]
fn codes_normalize_like_a_person_types_them_and_debug_hides_the_offer() {
    assert_eq!(normalize_code("abcde-fghjk").as_deref(), Some("ABCDEFGHJK"));
    assert_eq!(normalize_code("OIL0OIL0OI").as_deref(), Some("0110011001"));
    assert_eq!(normalize_code("ABCDEFGHJ"), None);
    let offer =
        PairingOffer::new("ABCDEFGHJK", "https://a.test", "secret-looking-key").expect("offer");
    let shown = format!("{offer:?}");
    assert!(!shown.contains("ABCDE") && !shown.contains("secret-looking-key"));
}

#[test]
fn reports_carry_counts_and_ids_only() {
    assert_eq!(
        PickupReport {
            segment_id: "s".into(),
            count: 0
        }
        .validate(),
        Ok(())
    );
    assert_eq!(
        PickupReport {
            segment_id: "s".into(),
            count: MAX_PICKUPS_PER_REPORT
        }
        .validate(),
        Ok(())
    );
    assert_eq!(
        PickupReport {
            segment_id: "s".into(),
            count: MAX_PICKUPS_PER_REPORT + 1
        }
        .validate(),
        Err(ReportError::BadCount)
    );
    assert_eq!(
        PickupReport {
            segment_id: "s".into(),
            count: -1
        }
        .validate(),
        Err(ReportError::BadCount)
    );
    assert_eq!(
        PickupReport {
            segment_id: String::new(),
            count: 1
        }
        .validate(),
        Err(ReportError::BadSegmentId)
    );
    let keys: Vec<String> = serde_json::to_value(PickupReport {
        segment_id: "s".into(),
        count: 1,
    })
    .expect("json")
    .as_object()
    .expect("object")
    .keys()
    .cloned()
    .collect();
    assert_eq!(keys, ["count", "segment_id"]);
}

#[test]
fn an_unpaired_phone_frees_itself_and_a_new_pairing_locks_it_again() {
    let mut holder = LeaseHolder::new();
    holder
        .offer(
            LockLease::locked("run-1", 1, 0, 600_000).expect("lease"),
            0,
            0,
        )
        .expect("offer");
    assert_eq!(holder.decide(10), PhoneLock::Locked);
    holder.pair_revoked();
    assert_eq!(
        holder.decide(10),
        PhoneLock::Free {
            reason: FreeReason::PairRevoked
        }
    );
    assert_eq!(holder.remaining_ms(10), None);
    // After pairing again the server's sequence keeps rising, so the next lease is accepted.
    holder
        .offer(
            LockLease::locked("run-2", 2, 1_000, 601_000).expect("lease"),
            1_000,
            20,
        )
        .expect("offer");
    assert_eq!(holder.decide(21), PhoneLock::Locked);
}
