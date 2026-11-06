//! Pairing and lease scenarios on the in-memory server. They run the same adapters the app uses
//! and pin the behavior the SQL in `supabase/migrations/*_s5_phone.sql` must also have (the SQL
//! itself is only checked by pgTAP, which has not been run here).

use std::sync::{Arc, Mutex};

use link_protocol::{FreeReason, LeaseHolder, LockLease, PhoneBeat, PhoneLock, PickupReport, ReleaseCause, MAX_LEASE_MS};
use rusqlite::Connection;
use serde_json::{json, Value};

use super::model::{PhoneCode, PhoneError};
use super::pairing::{PairingBackend, PairingService};
use super::port::{PairingApi, PhoneLink};
use super::rest::{RestPairing, RestPhoneLink};
use super::session::{PhoneSession, SupabaseAnonymousAuth};
use crate::db::open_in_memory;
use crate::remote::auth::{AccessTokenSource, AuthError};
use crate::remote::fake::{Caller, DataError, FAKE_ANON_KEY, FAKE_ENDPOINT};
use crate::remote::payload::{CANARY_TITLE, CANARY_URL};
use crate::remote::supabase::{RemoteError, SupabaseRest};
use crate::rooms::testkit::{World, T0};

const MIN: i64 = 60_000;

// ---- scaffolding ----

struct Owner {
    api: Arc<RestPairing>,
    user_id: String,
}

fn owner(world: &World, email: &str) -> Owner {
    let person = world.person(email);
    Owner { api: Arc::new(RestPairing::new(Arc::clone(&person.rest))), user_id: person.user_id }
}

struct Phone {
    link: RestPhoneLink,
    rest: Arc<SupabaseRest>,
    session: Arc<PhoneSession>,
    user_id: String,
}

/// A phone that signed in anonymously (no email, its own user id).
fn phone(world: &World) -> Phone {
    let session = Arc::new(PhoneSession::new(world.backend.anonymous_auth(), world.clock.clone()));
    let user_id = session.sign_in().expect("anonymous sign-in");
    let rest = Arc::new(SupabaseRest::new(FAKE_ENDPOINT, FAKE_ANON_KEY, world.recorder.clone(), session.clone()));
    Phone { link: RestPhoneLink::new(Arc::clone(&rest)), rest, session, user_id }
}

fn pair(owner: &Owner, phone: &Phone) {
    let code = owner.api.create_pair_code().expect("code").pair_code;
    phone.link.claim(&code).expect("claim");
}

fn locked(seq: i64, world: &World, minutes: i64) -> LockLease {
    let now = world.clock_now();
    LockLease::locked("run-1", seq, now, now + minutes * MIN).expect("lease")
}

trait ClockNow {
    fn clock_now(&self) -> i64;
}

impl ClockNow for World {
    fn clock_now(&self) -> i64 {
        use crate::remote::Clock;
        self.clock.now_ms().expect("clock")
    }
}

fn rejected(code: PhoneCode) -> PhoneError {
    PhoneError::Rejected(code)
}

// ---- pairing codes ----

#[test]
fn a_code_works_once_and_binds_the_phone_to_its_owner() {
    let world = World::new();
    let a = owner(&world, "a@example.test");
    let first = phone(&world);
    let second = phone(&world);
    let code = a.api.create_pair_code().expect("code");
    assert_eq!(code.expires_at_ms, T0 + 5 * MIN);
    assert_eq!(code.pair_code.len(), 10);

    let claimed = first.link.claim(&code.pair_code).expect("claim");
    assert_eq!(claimed.owner_user_id, a.user_id);
    assert_eq!(second.link.claim(&code.pair_code), Err(rejected(PhoneCode::Used)), "a used code is dead for everybody");
    assert_eq!(first.link.claim(&code.pair_code), Err(rejected(PhoneCode::Used)), "also for the phone that used it");
    assert_eq!(world.backend.phone_pair_rows(), 1);
    assert_eq!(world.backend.stored_pair_code_hashes(), 1, "only a hash is stored, never the plain code");
}

#[test]
fn a_code_dies_after_exactly_five_minutes() {
    let world = World::new();
    let (a, b) = (owner(&world, "a@example.test"), owner(&world, "b@example.test"));
    let (p1, p2) = (phone(&world), phone(&world));
    let code_a = a.api.create_pair_code().expect("code").pair_code;
    let code_b = b.api.create_pair_code().expect("code").pair_code;
    world.clock.advance(5 * MIN - 1);
    assert!(p1.link.claim(&code_a).is_ok(), "one ms before expiry");
    world.clock.advance(1);
    assert_eq!(p2.link.claim(&code_b), Err(rejected(PhoneCode::Expired)), "at expiry");
}

#[test]
fn a_new_code_retires_the_unused_one() {
    let world = World::new();
    let a = owner(&world, "a@example.test");
    let p = phone(&world);
    let old = a.api.create_pair_code().expect("code").pair_code;
    let new = a.api.create_pair_code().expect("code").pair_code;
    assert_ne!(old, new);
    assert_eq!(p.link.claim(&old), Err(rejected(PhoneCode::Expired)));
    assert!(p.link.claim(&new).is_ok());
}

#[test]
fn wrong_guesses_are_counted_even_though_they_fail() {
    let world = World::new();
    let a = owner(&world, "a@example.test");
    let p = phone(&world);
    let code = a.api.create_pair_code().expect("code").pair_code;
    for guess in ["0000000000", "1111111111", "2222222222", "3333333333", "not a code"] {
        assert_eq!(p.link.claim(guess), Err(rejected(PhoneCode::InvalidCode)), "{guess}");
    }
    assert_eq!(world.backend.pair_attempts_of(&p.user_id), 5, "the counter was not rolled back");
    assert_eq!(p.link.claim(&code), Err(rejected(PhoneCode::RateLimited)), "the sixth try in a minute is refused even with the right code");
    assert_eq!(world.backend.pair_attempts_of(&p.user_id), 5, "a refused try is not counted again");
    world.clock.advance(MIN);
    assert!(p.link.claim(&code).is_ok(), "a minute later the right code works");
}

#[test]
fn nobody_pairs_with_themselves_twice_or_as_a_phone_account() {
    let world = World::new();
    let (a, b) = (owner(&world, "a@example.test"), owner(&world, "b@example.test"));
    let (p1, p2) = (phone(&world), phone(&world));
    // An owner who is not anonymous can claim in principle, but not their own code.
    let own = world.person("c@example.test");
    let c = RestPairing::new(Arc::clone(&own.rest));
    let code_c = c.create_pair_code().expect("code").pair_code;
    assert_eq!(RestPhoneLink::new(Arc::clone(&own.rest)).claim(&code_c), Err(rejected(PhoneCode::CannotSelf)));

    // A phone (anonymous account) cannot make codes.
    assert_eq!(RestPairing::new(Arc::clone(&p1.rest)).create_pair_code().map(|_| ()), Err(rejected(PhoneCode::AnonymousNotAllowed)));

    pair(&a, &p1);
    assert_eq!(a.api.create_pair_code().map(|_| ()), Err(rejected(PhoneCode::AlreadyPaired)), "one phone per owner");
    let code_b = b.api.create_pair_code().expect("code").pair_code;
    assert_eq!(p1.link.claim(&code_b), Err(rejected(PhoneCode::PhoneAlreadyPaired)), "one owner per phone");
    assert!(p2.link.claim(&code_b).is_ok());
}

// ---- reading, writing and the matrix ----

#[test]
fn the_phone_reads_the_lease_with_the_server_clock_and_the_owner_reads_the_beat() {
    let world = World::new();
    let (a, p) = (owner(&world, "a@example.test"), phone(&world));
    assert_eq!(p.link.fetch_lease().map(|_| ()), Err(rejected(PhoneCode::NotPaired)), "an unpaired phone has nothing");
    pair(&a, &p);
    assert_eq!(p.link.fetch_lease().expect("fetch").lease, None, "nothing published yet");

    let lease = locked(1, &world, 50);
    let ack = a.api.publish_lock(&lease).expect("publish");
    assert_eq!((ack.seq, ack.replayed, ack.server_now_ms), (1, false, T0));
    let fetched = p.link.fetch_lease().expect("fetch");
    assert_eq!(fetched.server_now_ms, T0);
    assert_eq!(fetched.lease, Some(lease));

    assert_eq!(a.api.status().expect("status").last_beat_ms, None);
    world.clock.advance(15_000);
    p.link.beat(&PhoneBeat { lease_seq: 1, locked: true, service_enabled: true }).expect("beat");
    let status = a.api.status().expect("status");
    assert_eq!((status.paired, status.last_beat_ms, status.service_enabled, status.lease_seq), (true, Some(T0 + 15_000), Some(true), 1));
}

#[test]
fn pickup_reports_are_idempotent_and_bounded() {
    let world = World::new();
    let (a, p) = (owner(&world, "a@example.test"), phone(&world));
    pair(&a, &p);
    let report = PickupReport { segment_id: "run-1:1".into(), count: 3 };
    p.link.report_pickups(&report).expect("first");
    p.link.report_pickups(&report).expect("again");
    p.link.report_pickups(&PickupReport { segment_id: "run-1:1".into(), count: 4 }).expect("grown");
    assert_eq!(world.backend.table_rows("phone_pickups").len(), 0, "the table store is not where the phone rows live");
    let owner_view = world.backend.select(&Caller::User(a.user_id.clone()), "phone_pickups", &[]).expect("rows");
    assert_eq!(owner_view.len(), 2, "the repeated report added nothing; the larger count is a second row");
    assert_eq!(p.link.report_pickups(&PickupReport { segment_id: "s".into(), count: -1 }), Err(rejected(PhoneCode::InvalidInput)));
    assert_eq!(p.link.report_pickups(&PickupReport { segment_id: String::new(), count: 1 }), Err(rejected(PhoneCode::InvalidInput)));
}

#[test]
fn access_follows_the_unrevoked_pairing_and_nothing_else() {
    let world = World::new();
    let (a, b) = (owner(&world, "a@example.test"), owner(&world, "b@example.test"));
    let (paired, stranger) = (phone(&world), phone(&world));
    pair(&a, &paired);
    a.api.publish_lock(&locked(1, &world, 30)).expect("a lease");
    b.api.publish_lock(&locked(1, &world, 30)).expect("b lease");

    let read = |who: &Caller| world.backend.select(who, "phone_lock_state", &[]).map(|rows| rows.len());
    assert_eq!(read(&Caller::User(paired.user_id.clone())), Ok(1), "the paired phone sees only its owner's lease");
    let rows = world.backend.select(&Caller::User(paired.user_id.clone()), "phone_lock_state", &[]).expect("rows");
    assert_eq!(rows[0]["owner_user_id"], json!(a.user_id));
    assert_eq!(read(&Caller::User(stranger.user_id.clone())), Ok(0), "an unpaired phone sees nothing");
    assert_eq!(read(&Caller::User(a.user_id.clone())), Ok(1));
    assert_eq!(read(&Caller::User(b.user_id.clone())), Ok(1), "each owner sees their own row only");
    assert_eq!(read(&Caller::Anon), Err(DataError::Forbidden));
    let b_filter = [("owner_user_id".to_string(), b.user_id.clone())];
    assert_eq!(world.backend.select(&Caller::User(paired.user_id.clone()), "phone_lock_state", &b_filter).map(|r| r.len()), Ok(0), "asking for another owner by name gets nothing");

    // Revoking cuts access at once, though the phone still holds a valid token.
    assert!(paired.session.access_token().is_ok());
    a.api.revoke_pair().expect("revoke");
    assert_eq!(read(&Caller::User(paired.user_id.clone())), Ok(0));
    assert_eq!(paired.link.fetch_lease().map(|_| ()), Err(rejected(PhoneCode::NotPaired)));
    assert_eq!(paired.link.beat(&PhoneBeat { lease_seq: 1, locked: true, service_enabled: true }), Err(rejected(PhoneCode::NotPaired)));
    assert_eq!(paired.link.report_pickups(&PickupReport { segment_id: "s".into(), count: 1 }), Err(rejected(PhoneCode::NotPaired)));
    assert_eq!(a.api.revoke_pair(), Err(rejected(PhoneCode::NotFound)));
    assert_eq!(a.api.status().map(|s| s.paired), Ok(false));
    assert!(a.api.create_pair_code().is_ok(), "the owner can pair again");
}

#[test]
fn the_phone_can_leave_and_no_client_can_write_a_phone_table() {
    let world = World::new();
    let (a, p) = (owner(&world, "a@example.test"), phone(&world));
    pair(&a, &p);
    for who in [&a.user_id, &p.user_id] {
        let caller = Caller::User(who.clone());
        for table in ["phone_pairs", "phone_lock_state", "phone_beats", "phone_pickups", "pair_codes", "pair_attempts"] {
            let row = json!({"owner_user_id": who, "seq": 99});
            assert_eq!(world.backend.insert(&caller, table, &row, false), Err(DataError::Forbidden), "insert {table}");
            assert_eq!(world.backend.update(&caller, table, &[], &json!({"revoked_at": null})), Err(DataError::Forbidden), "update {table}");
            assert_eq!(world.backend.delete(&caller, table, &[]), Err(DataError::Forbidden), "delete {table}");
        }
        for hidden in ["pair_codes", "pair_attempts"] {
            assert_eq!(world.backend.select(&caller, hidden, &[]), Err(DataError::Forbidden), "select {hidden}");
        }
    }
    p.link.leave().expect("leave");
    assert_eq!(a.api.status().map(|s| s.paired), Ok(false), "the owner sees the pairing end");
}

#[test]
fn the_matrix_fixture_and_the_fake_agree() {
    let matrix: Value = serde_json::from_str(include_str!("../../../fixtures/phone/rls-matrix.json")).expect("matrix");
    let world = World::new();
    let a = owner(&world, "a@example.test");
    let sql = include_str!("../../../supabase/migrations/20260601000500_s5_phone.sql");
    for (table, ops) in matrix["tables"].as_object().expect("tables") {
        assert!(sql.contains(&format!("create table public.{table} (")), "{table} is in the migration");
        let read = world.backend.select(&Caller::User(a.user_id.clone()), table, &[]);
        match ops["select"].as_str() {
            Some("none") => assert_eq!(read, Err(DataError::Forbidden), "{table}"),
            Some(_) => assert!(read.is_ok(), "{table}"),
            None => panic!("{table} has no select rule"),
        }
        for op in ["insert", "update", "delete"] {
            assert_eq!(ops[op], json!("none"), "{table}.{op}");
        }
        assert_eq!(world.backend.select(&Caller::Anon, table, &[]), Err(DataError::Forbidden), "{table} for anon");
    }
}

#[test]
fn the_contract_fixture_matches_the_fake_and_the_sql() {
    let contract: Value = serde_json::from_str(include_str!("../../../fixtures/phone/rpc-contract.json")).expect("contract");
    let sql = include_str!("../../../supabase/migrations/20260601000500_s5_phone.sql");
    let world = World::new();
    let a = owner(&world, "a@example.test");
    for rpc in contract["rpcs"].as_array().expect("rpcs") {
        let name = rpc["name"].as_str().expect("name");
        assert!(sql.contains(&format!("function public.{name}(")), "{name} missing from the migration");
        let outcome = world.backend.rpc(&Caller::User(a.user_id.clone()), name, &json!({}));
        assert_ne!(outcome, Err(DataError::NotFound), "{name} is not implemented by the fake");
        assert_eq!(world.backend.rpc(&Caller::Anon, name, &json!({})), Err(DataError::Forbidden), "{name} must be closed to anon");
    }
}

// ---- the lease ----

#[test]
fn sequence_numbers_only_rise_and_a_retry_is_harmless() {
    let world = World::new();
    let a = owner(&world, "a@example.test");
    let lease = locked(5, &world, 50);
    assert!(a.api.publish_lock(&lease).is_ok());
    assert_eq!(a.api.publish_lock(&lease).map(|ack| ack.replayed), Ok(true), "the same publication again (a lost answer)");
    let other = LockLease::free("run-1", 5, world.clock_now(), ReleaseCause::EmergencyPass).expect("lease");
    assert_eq!(a.api.publish_lock(&other), Err(PhoneError::Stale { current_seq: 5 }), "same seq, other content");
    let older = locked(4, &world, 50);
    assert_eq!(a.api.publish_lock(&older), Err(PhoneError::Stale { current_seq: 5 }), "an old publication replayed");
    assert!(a.api.publish_lock(&locked(6, &world, 50)).is_ok());
    assert_eq!(world.backend.rpc(&Caller::User(a.user_id.clone()), "publish_phone_lock", &json!({"p_seq": 0, "p_run_id": "r", "p_state": "free", "p_release_cause": "run_ended", "p_valid_until_ms": 0})), Ok(json!({"code": "invalid_input"})));
}

#[test]
fn a_lease_longer_than_the_cap_is_refused_by_the_server_too() {
    let world = World::new();
    let a = owner(&world, "a@example.test");
    let now = world.clock_now();
    // Valid on its own (span == cap) but issued "in the future": the server measures from ITS clock.
    let sneaky = LockLease { run_id: "run-1".into(), seq: 1, state: link_protocol::LeaseState::Locked, release_cause: None, issued_at_ms: now + 3_600_000, valid_until_ms: now + 3_600_000 + MAX_LEASE_MS };
    assert_eq!(sneaky.validate(), Ok(()));
    assert_eq!(a.api.publish_lock(&sneaky), Err(rejected(PhoneCode::LeaseTooLong)));
    let exactly = LockLease { run_id: "run-1".into(), seq: 1, state: link_protocol::LeaseState::Locked, release_cause: None, issued_at_ms: now, valid_until_ms: now + MAX_LEASE_MS };
    assert!(a.api.publish_lock(&exactly).is_ok(), "exactly the cap is allowed");
    let past = LockLease { run_id: "run-1".into(), seq: 2, state: link_protocol::LeaseState::Locked, release_cause: None, issued_at_ms: now - 10, valid_until_ms: now };
    assert_eq!(a.api.publish_lock(&past), Err(rejected(PhoneCode::InvalidInput)), "a lease that already ended is not published");
    let invalid = LockLease { run_id: String::new(), ..locked(3, &world, 5) };
    assert_eq!(a.api.publish_lock(&invalid), Err(rejected(PhoneCode::InvalidInput)), "an invalid lease never leaves the PC");
}

/// One poll of the phone: a duplicate or stale lease is simply not applied, a revoked pairing
/// frees the phone.
fn tick(holder: &mut LeaseHolder, phone: &Phone, mono_ms: i64) -> Result<(), PhoneError> {
    match phone.link.fetch_lease() {
        Ok(fetched) => {
            if let Some(lease) = fetched.lease {
                let _ = holder.offer(lease, fetched.server_now_ms, mono_ms);
            }
            Ok(())
        }
        Err(PhoneError::Rejected(PhoneCode::NotPaired)) => {
            holder.pair_revoked();
            Ok(())
        }
        Err(other) => Err(other),
    }
}

#[test]
fn the_phone_locks_with_the_lease_frees_on_a_pass_and_survives_a_dead_server() {
    let world = World::new();
    let (a, p) = (owner(&world, "a@example.test"), phone(&world));
    pair(&a, &p);
    let mut holder = LeaseHolder::new();
    assert_eq!(holder.decide(0), PhoneLock::Free { reason: FreeReason::NoLease });

    // PC study start: a 10-minute segment.
    a.api.publish_lock(&locked(1, &world, 10)).expect("lock");
    tick(&mut holder, &p, 1_000).expect("tick");
    assert_eq!(holder.decide(1_000), PhoneLock::Locked);

    // The server (and the PC) go away: the lease keeps holding offline, then ends by itself.
    world.backend.set_outage(true);
    assert!(matches!(tick(&mut holder, &p, 5 * MIN), Err(PhoneError::Remote(RemoteError::Transport(_)))));
    assert_eq!(holder.decide(5 * MIN), PhoneLock::Locked, "offline with a valid lease stays locked");
    let end = 1_000 + 10 * MIN + 60_000;
    assert_eq!(holder.decide(end - 1), PhoneLock::Locked);
    assert_eq!(holder.decide(end), PhoneLock::Free { reason: FreeReason::LeaseExpired }, "an expired lease frees the phone");
    world.backend.set_outage(false);

    // A second segment, then an emergency pass at the PC.
    world.clock.advance(20 * MIN);
    a.api.publish_lock(&locked(2, &world, 30)).expect("lock again");
    tick(&mut holder, &p, end + 100).expect("tick");
    assert_eq!(holder.decide(end + 100), PhoneLock::Locked);
    a.api.publish_lock(&LockLease::free("run-1", 3, world.clock_now(), ReleaseCause::EmergencyPass).expect("free")).expect("pass");
    tick(&mut holder, &p, end + 200).expect("tick");
    assert_eq!(holder.decide(end + 200), PhoneLock::Free { reason: FreeReason::EmergencyPass });
    // The same free lease fetched again changes nothing (duplicate seq).
    tick(&mut holder, &p, end + 300).expect("tick");
    assert_eq!(holder.rejected().len(), 1);

    // The owner ends the pairing while the phone is locked: the phone cannot read a free lease any
    // more, so its next poll frees it.
    a.api.publish_lock(&locked(4, &world, 30)).expect("lock");
    tick(&mut holder, &p, end + 400).expect("tick");
    assert_eq!(holder.decide(end + 400), PhoneLock::Locked);
    a.api.revoke_pair().expect("revoke");
    tick(&mut holder, &p, end + 500).expect("tick");
    assert_eq!(holder.decide(end + 500), PhoneLock::Free { reason: FreeReason::PairRevoked });
}

// ---- the phone session ----

#[test]
fn an_anonymous_session_renews_and_never_invents_a_new_identity_on_failure() {
    let world = World::new();
    let session = PhoneSession::new(world.backend.anonymous_auth(), world.clock.clone());
    assert_eq!(session.access_token(), Err(AuthError::NotSignedIn));
    let user = session.sign_in().expect("sign in");
    let first = session.access_token().expect("token");
    assert_eq!(session.access_token().expect("cached"), first);

    world.clock.advance(3_600_000);
    let second = session.access_token().expect("renewed before use");
    assert_ne!(first, second);
    assert_eq!(world.backend.refresh_calls(), 1);

    let kept = session.refresh_token().expect("refresh token");
    let resumed = PhoneSession::new(world.backend.anonymous_auth(), world.clock.clone());
    assert_eq!(resumed.restore(&kept).expect("restore"), user, "the same identity after a restart");
    let again = PhoneSession::new(world.backend.anonymous_auth(), world.clock.clone());
    assert_eq!(again.restore(&kept), Err(AuthError::Expired), "a spent refresh token does not work twice");
    assert_eq!(again.user_id(), None, "and no new anonymous identity appears by itself");
}

#[test]
fn the_real_anonymous_auth_adapter_speaks_the_gotrue_routes() {
    let world = World::new();
    let auth = SupabaseAnonymousAuth::new(FAKE_ENDPOINT, FAKE_ANON_KEY, world.recorder.clone());
    let session = PhoneSession::new(Arc::new(auth), world.clock.clone());
    let user = session.sign_in().expect("sign in");
    let caller = world.backend.caller_for(Some(&session.access_token().expect("token"))).expect("caller");
    assert_eq!(caller, Caller::User(user));
    session.restore(&session.refresh_token().expect("refresh")).expect("restore over http");

    let wrong_key = SupabaseAnonymousAuth::new(FAKE_ENDPOINT, "other-key", world.recorder.clone());
    assert!(PhoneSession::new(Arc::new(wrong_key), world.clock.clone()).sign_in().is_err());
}

// ---- the desktop service ----

struct TestBackend {
    api: Option<Arc<RestPairing>>,
}

impl PairingBackend for TestBackend {
    fn api(&self) -> Result<Arc<dyn PairingApi>, PhoneError> {
        match &self.api {
            Some(api) => Ok(Arc::clone(api) as Arc<dyn PairingApi>),
            None => Err(PhoneError::Remote(RemoteError::NotSignedIn)),
        }
    }

    fn server(&self) -> Option<(String, String)> {
        Some((FAKE_ENDPOINT.to_string(), FAKE_ANON_KEY.to_string()))
    }
}

fn service(world: &World, a: &Owner) -> (PairingService, Arc<Mutex<Connection>>) {
    let db = Arc::new(Mutex::new(open_in_memory().expect("db")));
    let backend = Arc::new(TestBackend { api: Some(Arc::clone(&a.api)) });
    (PairingService::with_parts(Arc::clone(&db), world.clock.clone(), backend), db)
}

#[test]
fn the_service_shows_a_code_follows_the_pairing_and_ends_it_outside_a_run() {
    let world = World::new();
    let (a, p) = (owner(&world, "a@example.test"), phone(&world));
    let (service, db) = service(&world, &a);

    let start = service.start().expect("start");
    assert_eq!(start.code, PhoneCode::Ok);
    assert!(start.qr_svg.as_deref().is_some_and(|svg| svg.starts_with("<?xml") || svg.contains("<svg")));
    assert_eq!(start.expires_at_ms, Some(T0 + 5 * MIN));
    let shown = start.display_code.expect("display code");
    assert_eq!((shown.len(), shown.as_bytes()[5]), (11, b'-'), "ABCDE-FGHJK");
    assert_eq!(service.status().map(|s| (s.paired, s.online)).expect("status"), (false, true));

    // The phone types the displayed code (dash, lower case) instead of scanning.
    p.link.claim(&shown.to_lowercase()).expect("claim");
    let status = service.status().expect("status");
    assert_eq!((status.paired, status.online, status.paired_at_ms), (true, true, Some(T0)));

    // Offline, the last known state is shown.
    world.backend.set_outage(true);
    let offline = service.status().expect("status");
    assert_eq!((offline.paired, offline.online), (true, false));
    world.backend.set_outage(false);

    // Ending the pairing is refused while a study run holds the lock.
    crate::store::put_study_state(&db.lock().expect("lock"), true, T0).expect("lock held");
    assert_eq!(service.revoke().expect("revoke").code, PhoneCode::RunLocked);
    assert!(service.status().expect("status").paired, "nothing changed");
    crate::store::put_study_state(&db.lock().expect("lock"), false, T0 + 1).expect("lock released");
    assert_eq!(service.revoke().expect("revoke").code, PhoneCode::Ok);
    assert!(!service.status().expect("status").paired);
    assert_eq!(service.revoke().expect("again").code, PhoneCode::NotFound, "nothing left to end");
}

#[test]
fn the_service_reports_refusals_as_codes_and_a_missing_login_as_an_error() {
    let world = World::new();
    let a = owner(&world, "a@example.test");
    let (service, _db) = service(&world, &a);
    pair(&a, &phone(&world));
    let refused = service.start().expect("start");
    assert_eq!((refused.code, refused.qr_svg), (PhoneCode::AlreadyPaired, None));

    let signed_out = PairingService::with_parts(
        Arc::new(Mutex::new(open_in_memory().expect("db"))),
        world.clock.clone(),
        Arc::new(TestBackend { api: None }),
    );
    assert!(signed_out.start().is_err());
    let status = signed_out.status().expect("status");
    assert_eq!((status.paired, status.online), (false, false), "signed out: the local state, offline");
}

// ---- what leaves ----

#[test]
fn every_request_body_carries_only_contract_parameters_and_no_canary() {
    let world = World::new();
    let (a, p) = (owner(&world, "a@example.test"), phone(&world));
    let code = a.api.create_pair_code().expect("code").pair_code;
    p.link.claim(&code).expect("claim");
    a.api.publish_lock(&locked(1, &world, 5)).expect("publish");
    p.link.fetch_lease().expect("fetch");
    p.link.beat(&PhoneBeat { lease_seq: 1, locked: true, service_enabled: false }).expect("beat");
    p.link.report_pickups(&PickupReport { segment_id: "run-1:1".into(), count: 2 }).expect("pickups");
    a.api.status().expect("status");
    a.api.revoke_pair().expect("revoke");

    let contract: Value = serde_json::from_str(include_str!("../../../fixtures/phone/rpc-contract.json")).expect("contract");
    let mut seen = 0;
    for (name, body) in world.recorder.bodies() {
        assert!(!body.contains(CANARY_TITLE) && !body.contains(CANARY_URL), "{name}");
        let Some(rpc) = contract["rpcs"].as_array().and_then(|r| r.iter().find(|x| x["name"] == json!(name))) else { continue };
        let sent: Value = serde_json::from_str(&body).expect("json body");
        let mut keys: Vec<String> = sent.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
        keys.sort();
        let mut expected: Vec<String> = rpc["params"].as_array().expect("params").iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        expected.sort();
        assert_eq!(keys, expected, "{name}");
        seen += 1;
    }
    assert_eq!(seen, 8, "all eight RPCs were exercised");
}

// ---- shared fixtures ----

#[test]
fn the_desktop_reads_the_same_wire_fixtures_as_the_crate_and_the_phone() {
    let wire: Value = serde_json::from_str(include_str!("../../../fixtures/phone/link-protocol.json")).expect("wire");
    fn round<T: serde::de::DeserializeOwned + serde::Serialize>(wire: &Value, key: &str) {
        let parsed: T = serde_json::from_value(wire[key].clone()).expect(key);
        assert_eq!(serde_json::to_value(parsed).expect("json"), wire[key], "{key}");
    }
    round::<link_protocol::PairingOffer>(&wire, "pairingOffer");
    round::<LockLease>(&wire, "lockedLease");
    round::<LockLease>(&wire, "freeLease");
    round::<PhoneBeat>(&wire, "beat");
    round::<PickupReport>(&wire, "pickup");
    round::<PhoneLock>(&wire, "phoneLockLocked");
    round::<PhoneLock>(&wire, "phoneLockFree");

    // The published lease has the shape the RPC sends: the same fields, the same names.
    let lease: LockLease = serde_json::from_value(wire["lockedLease"].clone()).expect("lease");
    let args = serde_json::to_value(super::payload::PublishLockArgs::from_lease(&lease)).expect("args");
    assert_eq!(args["p_state"], wire["lockedLease"]["state"]);
    assert_eq!(args["p_valid_until_ms"], wire["lockedLease"]["valid_until_ms"]);
    assert_eq!(args["p_seq"], wire["lockedLease"]["seq"]);
}
