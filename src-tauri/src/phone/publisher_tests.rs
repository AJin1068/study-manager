//! The publisher, the gap rows and the pick-up counts end to end: a real timer in an in-memory
//! database, the in-memory server, a paired owner and a paired phone, and a fake clock. The server
//! SQL is covered by pgTAP only (not run here); these tests pin the behavior it must also have.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use link_protocol::{LeaseState, LockLease, PhoneBeat, PickupReport, ReleaseCause, GRACE_MS};
use rusqlite::Connection;
use serde_json::{json, Value};

use super::gap::UNREACHABLE_AFTER_MS;
use super::model::{LeaseFetch, PhoneError};
use super::pairing::PairingBackend;
use super::port::{PairingApi, PhoneLink};
use super::publisher::{decide, parse_segment_key, segment_key, Desired, Diagnostics, Facts, Publisher};
use super::rest::{RestPairing, RestPhoneLink};
use super::session::PhoneSession;
use crate::db::open_in_memory;
use crate::plan::{create_plan, CreatePlanInput};
use crate::remote::fake::{Caller, FAKE_ANON_KEY, FAKE_ENDPOINT};
use crate::remote::payload::{CANARY_TITLE, CANARY_URL};
use crate::remote::supabase::SupabaseRest;
use crate::rooms::testkit::{World, T0};
use crate::store::phone as repo;
use crate::timer::model::{Phase, TimerState};
use crate::timer::study_pass::{self, PassKind, ReleaseProof, Typing};
use crate::timer::{self, HighWater};

mod e2e;

const SEC: i64 = 1_000;
const MIN: i64 = 60_000;
const DAY: i64 = 86_400_000;
const PACKAGE_CANARY: &str = "com.canary.secretapp";

// ---- the pure decision ----

fn state(phase: Phase, ids: Option<(i64, i64)>, end: Option<i64>) -> TimerState {
    TimerState {
        phase,
        run_id: ids.map(|i| i.0),
        segment_id: ids.map(|i| i.1),
        set_index: 1,
        sets_total: 2,
        planned_end_ms: end,
        lock_held: phase != Phase::Idle,
        now_ms: 0,
        next: None,
    }
}

fn facts<'a>(s: &'a TimerState, now: i64) -> Facts<'a> {
    Facts { state: s, now_ms: now, segment_started_ms: Some(0), emergency_pass_active: false, last_key: None, last_run_release: None }
}

#[test]
fn segment_keys_round_trip_and_nothing_else_parses() {
    assert_eq!(segment_key(12, 345), "12:345");
    assert_eq!(parse_segment_key("12:345"), Some((12, 345)));
    for bad in ["", "12", "12:", ":3", "a:b", "0:1", "1:0", "-1:2", "01:2", "1:2:3", "1: 2", "+1:2", "12:345 "] {
        assert_eq!(parse_segment_key(bad), None, "{bad:?}");
    }
}

#[test]
fn the_decision_table() {
    let study = state(Phase::Study, Some((3, 9)), Some(50 * MIN));
    // A study segment holds the phone until its end plus the grace.
    assert_eq!(
        decide(&facts(&study, 10 * MIN)),
        Desired::Lock { key: "3:9".into(), run_id: 3, segment_started_ms: 0, segment_end_ms: 50 * MIN }
    );
    // The engine has not moved on although the segment is long over: free.
    assert_eq!(decide(&facts(&study, 50 * MIN + GRACE_MS - 1)), Desired::Lock { key: "3:9".into(), run_id: 3, segment_started_ms: 0, segment_end_ms: 50 * MIN });
    assert_eq!(decide(&facts(&study, 50 * MIN + GRACE_MS)), Desired::Free { key: "3:9".into(), cause: ReleaseCause::RunEnded });
    // An emergency pass frees it while it lasts.
    let pass = Facts { emergency_pass_active: true, ..facts(&study, MIN) };
    assert_eq!(decide(&pass), Desired::Free { key: "3:9".into(), cause: ReleaseCause::EmergencyPass });
    // Not written by the engine yet: wait.
    assert_eq!(decide(&facts(&state(Phase::Study, None, Some(50 * MIN)), MIN)), Desired::Nothing);
    assert_eq!(decide(&Facts { segment_started_ms: None, ..facts(&study, MIN) }), Desired::Nothing);
    // Breaks free the phone, each under its own key.
    for phase in [Phase::Break, Phase::LongBreak] {
        assert_eq!(decide(&facts(&state(phase, Some((3, 10)), Some(60 * MIN)), MIN)), Desired::Free { key: "3:10".into(), cause: ReleaseCause::RunEnded });
        assert_eq!(decide(&facts(&state(phase, None, Some(60 * MIN)), MIN)), Desired::Nothing);
    }
    // Idle: nothing before anything was ever published, afterwards the way the run ended.
    let idle = state(Phase::Idle, None, None);
    assert_eq!(decide(&facts(&idle, MIN)), Desired::Nothing);
    let ended = Facts { last_key: Some("3:9"), ..facts(&idle, MIN) };
    assert_eq!(decide(&ended), Desired::Free { key: "3:9".into(), cause: ReleaseCause::RunEnded });
    for (cause, expected) in [(ReleaseCause::SkipToday, ReleaseCause::SkipToday), (ReleaseCause::EarlyRelease, ReleaseCause::EarlyRelease)] {
        let released = Facts { last_run_release: Some(cause), ..ended };
        assert_eq!(decide(&released), Desired::Free { key: "3:9".into(), cause: expected });
    }
}

// ---- the rig ----

struct RigBackend {
    api: Arc<RestPairing>,
}

impl PairingBackend for RigBackend {
    fn api(&self) -> Result<Arc<dyn PairingApi>, PhoneError> {
        Ok(Arc::clone(&self.api) as Arc<dyn PairingApi>)
    }

    fn server(&self) -> Option<(String, String)> {
        Some((FAKE_ENDPOINT.to_string(), FAKE_ANON_KEY.to_string()))
    }
}

struct Rig {
    world: World,
    owner: Arc<RestPairing>,
    phone: RestPhoneLink,
    db: Arc<Mutex<Connection>>,
    high_water: Arc<HighWater>,
    diag: Arc<Diagnostics>,
    publisher: Publisher<Utc>,
}

impl Rig {
    /// A plan of `sets` sets at 15:00 today (the clock starts at 14:30), an owner and a phone paired.
    fn new(sets: i64) -> Rig {
        let world = World::new();
        let person = world.person("owner@example.test");
        let owner = Arc::new(RestPairing::new(Arc::clone(&person.rest)));
        let session = Arc::new(PhoneSession::new(world.backend.anonymous_auth(), world.clock.clone()));
        session.sign_in().expect("anonymous sign-in");
        let rest = Arc::new(SupabaseRest::new(FAKE_ENDPOINT, FAKE_ANON_KEY, world.recorder.clone(), session));
        let phone = RestPhoneLink::new(rest);
        let code = owner.create_pair_code().expect("code").pair_code;
        phone.claim(&code).expect("claim");

        let mut conn = open_in_memory().expect("db");
        let input = CreatePlanInput {
            name: "수학".into(),
            subject: "수학".into(),
            start_date: "2026-11-06".into(),
            start_time: "15:00".into(),
            sets,
            repeat: None,
        };
        create_plan(&mut conn, &Utc, input, T0 - DAY).expect("plan");
        repo::save_pair(&conn, true, Some(T0), T0).expect("pair row");
        let db = Arc::new(Mutex::new(conn));
        let high_water = Arc::new(HighWater::new(0));
        let diag = Arc::new(Diagnostics::default());
        let publisher = Self::publisher_over(&world, &owner, &db, &high_water, &diag);
        Rig { world, owner, phone, db, high_water, diag, publisher }
    }

    fn publisher_over(world: &World, owner: &Arc<RestPairing>, db: &Arc<Mutex<Connection>>, high_water: &Arc<HighWater>, diag: &Arc<Diagnostics>) -> Publisher<Utc> {
        Publisher::new(
            Arc::clone(db),
            world.clock.clone(),
            Arc::new(RigBackend { api: Arc::clone(owner) }),
            Utc,
            Arc::clone(high_water),
            Arc::clone(diag),
        )
    }

    fn restart_publisher(&mut self) {
        self.publisher = Self::publisher_over(&self.world, &self.owner, &self.db, &self.high_water, &self.diag);
    }

    fn set(&self, ms: i64) {
        self.world.clock.set(ms);
    }

    fn now(&self) -> i64 {
        use crate::remote::Clock;
        self.world.clock.now_ms().expect("clock")
    }

    /// The engine's reconcile at the clock's time.
    fn engine(&self) -> TimerState {
        let mut conn = self.db.lock().expect("db");
        timer::reconcile(&mut conn, &Utc, self.now()).expect("reconcile").state
    }

    fn step(&mut self) -> Duration {
        self.publisher.step()
    }

    /// Steps once per `every_ms` of fake time until `until_ms`.
    fn run_until(&mut self, until_ms: i64, every_ms: i64) {
        while self.now() < until_ms {
            self.set((self.now() + every_ms).min(until_ms));
            self.step();
        }
    }

    fn lease(&self) -> Option<LockLease> {
        let LeaseFetch { lease, .. } = self.phone.fetch_lease().expect("fetch");
        lease
    }

    fn publications(&self) -> usize {
        self.world.recorder.bodies_of("publish_phone_lock").len()
    }

    fn count(&self, table: &str) -> i64 {
        self.db.lock().expect("db").query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).expect("count")
    }

    fn gaps(&self) -> Vec<repo::GapRow> {
        repo::gaps_between(&self.db.lock().expect("db"), i64::MIN, i64::MAX).expect("gaps")
    }

    fn beat(&self, lease_seq: i64, service_enabled: bool) {
        self.phone.beat(&PhoneBeat { lease_seq, locked: true, service_enabled }).expect("beat");
    }

    fn grant(&self, kind: PassKind) {
        let proof = ReleaseProof::Typing(Typing { challenge_len: 90, typing_ms: 1 });
        let mut conn = self.db.lock().expect("db");
        study_pass::grant(&mut conn, &Utc, kind, self.now(), proof).expect("grant");
    }

    fn lock_held(&self) -> bool {
        crate::store::read_study_state(&self.db.lock().expect("db")).expect("lock").0
    }
}

fn start_of_study(rig: &mut Rig) -> TimerState {
    rig.set(T0 + 30 * MIN + SEC);
    let state = rig.engine();
    assert_eq!(state.phase, Phase::Study, "{state:?}");
    state
}

// ---- publishing ----

#[test]
fn every_segment_change_is_published_once_with_a_rising_sequence() {
    let mut rig = Rig::new(2);
    assert!(rig.lease().is_none(), "nothing before the plan starts");
    rig.step();
    assert_eq!(rig.publications(), 0, "an idle desktop app tells the phone nothing");

    let first = start_of_study(&mut rig);
    rig.step();
    let lease = rig.lease().expect("lease");
    let (run, segment) = parse_segment_key(&lease.run_id).expect("the lease names its segment");
    assert_eq!((Some(run), Some(segment)), (first.run_id, first.segment_id));
    assert_eq!((lease.seq, lease.state), (1, LeaseState::Locked));
    assert_eq!(lease.valid_until_ms, first.planned_end_ms.expect("end") + GRACE_MS, "until the segment's end plus the grace");

    // Looking again, and again, changes nothing.
    rig.step();
    rig.step();
    assert_eq!(rig.publications(), 1);

    let end = first.planned_end_ms.expect("end");
    rig.set(end + 2 * SEC);
    let brk = rig.engine();
    assert_eq!(brk.phase, Phase::Break);
    rig.step();
    let free = rig.lease().expect("lease");
    assert_eq!((free.seq, free.state, free.release_cause), (2, LeaseState::Free, Some(ReleaseCause::RunEnded)));
    rig.step();
    assert_eq!(rig.publications(), 2);

    rig.set(brk.planned_end_ms.expect("break end") + 2 * SEC);
    let second = rig.engine();
    assert_eq!(second.phase, Phase::Study);
    rig.step();
    assert_eq!(rig.lease().map(|l| (l.seq, l.state)), Some((3, LeaseState::Locked)));

    rig.set(second.planned_end_ms.expect("end") + 2 * SEC);
    assert_eq!(rig.engine().phase, Phase::Idle);
    rig.step();
    rig.step();
    let done = rig.lease().expect("lease");
    assert_eq!((done.seq, done.state, done.release_cause), (4, LeaseState::Free, Some(ReleaseCause::RunEnded)));
    assert_eq!(rig.publications(), 4, "one publication per change, no more");
}

#[test]
fn an_emergency_pass_frees_the_phone_at_once_and_the_lock_returns_when_it_ends() {
    let mut rig = Rig::new(2);
    let study = start_of_study(&mut rig);
    rig.step();
    let key = rig.lease().expect("lease").run_id;

    rig.grant(PassKind::EmergencyPass); // no engine tick in between
    rig.step();
    let free = rig.lease().expect("lease");
    assert_eq!((free.seq, free.state, free.release_cause), (2, LeaseState::Free, Some(ReleaseCause::EmergencyPass)));
    assert!(rig.lock_held(), "the run goes on; only the phone is freed");

    rig.run_until(rig.now() + 10 * MIN - SEC, 5 * SEC);
    assert_eq!(rig.lease().map(|l| l.state), Some(LeaseState::Free), "still within the pass");
    rig.run_until(rig.now() + 2 * SEC, SEC);
    let back = rig.lease().expect("lease");
    assert_eq!((back.seq, back.state, back.run_id.as_str()), (3, LeaseState::Locked, key.as_str()));
    assert_eq!(back.valid_until_ms, study.planned_end_ms.expect("end") + GRACE_MS);
}

#[test]
fn skip_today_and_early_release_free_the_phone_at_once_with_their_cause() {
    for (kind, cause) in [(PassKind::SkipToday, ReleaseCause::SkipToday), (PassKind::EarlyRelease, ReleaseCause::EarlyRelease)] {
        let mut rig = Rig::new(2);
        start_of_study(&mut rig);
        rig.step();
        rig.grant(kind); // no engine tick in between: the release itself woke the publisher
        assert!(!rig.lock_held());
        rig.step();
        let free = rig.lease().expect("lease");
        assert_eq!((free.seq, free.state, free.release_cause), (2, LeaseState::Free, Some(cause)), "{kind:?}");
        // The engine's next tick finds nothing new to tell.
        rig.set(rig.now() + SEC);
        rig.engine();
        rig.step();
        assert_eq!(rig.publications(), 2, "{kind:?}");
    }
}

#[test]
fn a_dead_server_never_touches_the_engine_and_the_retry_keeps_its_sequence_number() {
    let mut rig = Rig::new(2);
    start_of_study(&mut rig);
    let begin = rig.now();
    rig.world.backend.set_outage(true);

    rig.step();
    assert_eq!(rig.publications(), 1);
    assert!(rig.diag.view().publish_failing && rig.diag.view().last_error.is_some());
    // Backing off: a look one second later does not call the server again, and the worker keeps
    // looking meanwhile.
    rig.set(begin + SEC);
    let waited = rig.step();
    assert!(waited <= Duration::from_secs(1));
    assert_eq!(rig.publications(), 1);
    // The engine runs on and the lock is as it must be.
    rig.set(begin + 20 * SEC);
    assert!(rig.engine().lock_held && rig.lock_held());

    rig.world.backend.set_outage(false);
    rig.set(begin + 40 * SEC);
    rig.step();
    let lease = rig.lease().expect("the lease got through");
    assert_eq!((lease.seq, lease.state), (1, LeaseState::Locked), "same number: nothing burned by the failures");
    assert!(!rig.diag.view().publish_failing);
    assert_eq!(rig.diag.view().published.map(|p| (p.seq, p.state)), Some((1, "locked")));
}

#[test]
fn a_state_that_is_no_longer_wanted_is_not_published_when_the_server_returns() {
    let mut rig = Rig::new(2);
    start_of_study(&mut rig);
    rig.world.backend.set_outage(true);
    rig.step();
    // The run is released while the server is away; the lock was never published.
    rig.grant(PassKind::EarlyRelease);
    rig.world.backend.set_outage(false);
    rig.set(rig.now() + 10 * SEC);
    rig.step();
    assert!(rig.lease().is_none(), "a phone that was never locked is never told to lock, nor to free");
    assert_eq!(rig.count("phone_gap"), 0);
}

#[test]
fn a_server_that_holds_a_higher_sequence_number_is_caught_up_to_not_fought() {
    let mut rig = Rig::new(2);
    let now = rig.now();
    rig.owner.publish_lock(&LockLease::locked("9:9", 7, now, now + MIN).expect("lease")).expect("an older publication");
    start_of_study(&mut rig);
    rig.step();
    let lease = rig.lease().expect("lease");
    assert_eq!((lease.seq, lease.state), (8, LeaseState::Locked));
    assert_eq!(repo::publish_row(&rig.db.lock().expect("db")).expect("row").seq, 8, "remembered for the next number");
}

#[test]
fn a_restart_neither_reuses_a_number_nor_publishes_the_same_state_again() {
    let mut rig = Rig::new(2);
    let study = start_of_study(&mut rig);
    rig.step();
    assert_eq!(rig.publications(), 1);
    rig.restart_publisher();
    rig.step();
    rig.step();
    assert_eq!(rig.publications(), 1, "the state on the server is remembered across a restart");

    rig.set(study.planned_end_ms.expect("end") + 2 * SEC);
    rig.engine();
    rig.step();
    assert_eq!(rig.lease().map(|l| l.seq), Some(2), "the next number continues");
}

#[test]
fn nothing_is_published_or_watched_without_a_pairing() {
    let mut rig = Rig::new(2);
    repo::save_pair(&rig.db.lock().expect("db"), false, None, T0).expect("unpair");
    start_of_study(&mut rig);
    rig.step();
    assert_eq!(rig.publications(), 0);
    assert_eq!(rig.world.recorder.bodies_of("phone_status").len(), 0);
    assert!(!rig.diag.view().paired);
}

// ---- gaps ----

#[test]
fn a_ninety_second_silence_of_the_phone_becomes_exactly_one_gap_row() {
    let mut rig = Rig::new(2);
    let study = start_of_study(&mut rig);
    let begin = rig.now();
    rig.step();
    let seq = rig.lease().expect("lease").seq;
    // The phone beats every 15 s for two minutes.
    for _ in 0..8 {
        rig.beat(seq, true);
        rig.set(rig.now() + 15 * SEC);
        rig.step();
    }
    let last_beat = begin + 7 * 15 * SEC;
    // Then it goes silent. Nothing is written while the silence goes on.
    rig.run_until(last_beat + UNREACHABLE_AFTER_MS - 15 * SEC, 15 * SEC);
    assert!(rig.diag.view().open_gaps.is_empty(), "under 90 s is not a gap");
    rig.run_until(last_beat + UNREACHABLE_AFTER_MS + 30 * SEC, 15 * SEC);
    let open = rig.diag.view().open_gaps;
    assert_eq!(open.iter().map(|g| (g.kind, g.since_ms)).collect::<Vec<_>>(), vec![("unreachable", last_beat)]);
    assert_eq!(rig.count("phone_gap"), 0);

    // The phone is back.
    let back = rig.now();
    rig.beat(seq, true);
    rig.set(back + 15 * SEC);
    rig.step();
    assert_eq!(rig.gaps(), vec![repo::GapRow { run_id: study.run_id.expect("run"), start_ms: last_beat, end_ms: back, kind: "unreachable".into() }]);
    assert!(rig.diag.view().open_gaps.is_empty());
    rig.beat(seq, true);
    rig.set(rig.now() + 15 * SEC);
    rig.step();
    assert_eq!(rig.count("phone_gap"), 1, "one row for the whole stretch");
}

#[test]
fn a_gap_in_progress_when_the_segment_ends_is_closed_at_the_segment_end() {
    let mut rig = Rig::new(2);
    let study = start_of_study(&mut rig);
    rig.step();
    let seq = rig.lease().expect("lease").seq;
    rig.beat(seq, true);
    let beat_at = rig.now();
    let end = study.planned_end_ms.expect("end");
    rig.run_until(end - 15 * SEC, 15 * SEC);
    assert!(!rig.diag.view().open_gaps.is_empty());
    rig.set(end + 2 * SEC);
    rig.engine();
    rig.step();
    let gaps = rig.gaps();
    assert_eq!(gaps.len(), 1);
    assert_eq!((gaps[0].start_ms, gaps[0].end_ms, gaps[0].kind.as_str()), (beat_at, end, "unreachable"));
}

#[test]
fn a_switched_off_blocking_service_is_a_gap_of_its_own_kind() {
    let mut rig = Rig::new(2);
    start_of_study(&mut rig);
    rig.step();
    let seq = rig.lease().expect("lease").seq;
    rig.beat(seq, true);
    rig.run_until(rig.now() + 15 * SEC, 15 * SEC);
    rig.beat(seq, false);
    let off_at = rig.now();
    rig.run_until(rig.now() + 15 * SEC, 15 * SEC);
    assert_eq!(rig.diag.view().service_enabled, Some(false));
    assert_eq!(rig.diag.view().open_gaps.iter().map(|g| g.kind).collect::<Vec<_>>(), vec!["service_disabled"]);
    rig.beat(seq, false);
    rig.run_until(rig.now() + 15 * SEC, 15 * SEC);
    rig.beat(seq, true);
    let on_at = rig.now();
    rig.run_until(rig.now() + 15 * SEC, 15 * SEC);
    let gaps = rig.gaps();
    assert_eq!(gaps.len(), 1);
    assert_eq!((gaps[0].kind.as_str(), gaps[0].start_ms, gaps[0].end_ms), ("service_disabled", off_at, on_at));
}

#[test]
fn a_lock_that_could_not_be_published_for_ninety_seconds_is_a_lease_gap() {
    let mut rig = Rig::new(2);
    let study = start_of_study(&mut rig);
    let begin = rig.now();
    rig.world.backend.set_outage(true);
    rig.run_until(begin + UNREACHABLE_AFTER_MS + 5 * SEC, 5 * SEC);
    assert_eq!(rig.diag.view().open_gaps.iter().map(|g| (g.kind, g.since_ms)).collect::<Vec<_>>(), vec![("lease_expired", begin - SEC)]);
    // While the server is away nothing is known about the phone: no "unreachable" is claimed.
    assert!(rig.diag.view().open_gaps.iter().all(|g| g.kind != "unreachable"));
    rig.world.backend.set_outage(false);
    rig.run_until(begin + UNREACHABLE_AFTER_MS + 2 * MIN, 5 * SEC);
    let gaps = rig.gaps();
    let lease_gaps: Vec<_> = gaps.iter().filter(|g| g.kind == "lease_expired").collect();
    assert_eq!(lease_gaps.len(), 1, "{gaps:?}");
    assert_eq!((lease_gaps[0].run_id, lease_gaps[0].start_ms), (study.run_id.expect("run"), begin - SEC));
    assert!(lease_gaps[0].end_ms > begin + UNREACHABLE_AFTER_MS && lease_gaps[0].end_ms <= begin + UNREACHABLE_AFTER_MS + 40 * SEC);
}

#[test]
fn gap_rows_are_append_only() {
    let rig = Rig::new(2);
    {
        let conn = rig.db.lock().expect("db");
        repo::insert_gap(&conn, &repo::GapRow { run_id: 1, start_ms: 10, end_ms: 20, kind: "unreachable".into() }).expect("insert");
        assert!(conn.execute("UPDATE phone_gap SET end_ms = 99", []).is_err());
        assert!(conn.execute("DELETE FROM phone_gap", []).is_err());
        assert!(conn.execute("INSERT INTO phone_gap (run_id, start_ms, end_ms, kind) VALUES (1, 1, 2, 'bogus')", []).is_err(), "the kind list is closed");
        assert!(conn.execute("INSERT INTO phone_gap (run_id, start_ms, end_ms, kind) VALUES (1, 5, 2, 'unreachable')", []).is_err(), "no negative span");
        assert!(!repo::insert_gap(&conn, &repo::GapRow { run_id: 1, start_ms: 10, end_ms: 30, kind: "unreachable".into() }).expect("again"), "the same gap twice is one row");
    }
    assert_eq!(rig.count("phone_gap"), 1);
}

// ---- the PC's view ----

#[test]
fn the_pc_view_says_what_the_publisher_knows_and_nothing_about_the_phones_content() {
    let mut rig = Rig::new(2);
    start_of_study(&mut rig);
    rig.step();
    let view = rig.diag.view();
    assert!(view.paired && view.want_locked && !view.publish_failing);
    assert_eq!(view.published.map(|p| (p.seq, p.state, p.reason)), Some((1, "locked", None)));
    let json = serde_json::to_value(rig.diag.view()).expect("json");
    let mut keys: Vec<&str> = json.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["lastBeatMs", "lastError", "openGaps", "paired", "publishFailing", "published", "serviceEnabled", "wantLocked"]);
}

// ---- pick-up counts ----

fn report(rig: &Rig, key: &str, count: i64) {
    rig.phone.report_pickups(&PickupReport { segment_id: key.into(), count }).expect("report");
}

fn pull(rig: &mut Rig) {
    rig.set(rig.now() + 31 * SEC);
    rig.step();
}

#[test]
fn pickup_reports_are_idempotent_per_segment_and_only_ever_added() {
    let mut rig = Rig::new(2);
    let study = start_of_study(&mut rig);
    let segment = study.segment_id.expect("segment");
    let key = segment_key(study.run_id.expect("run"), segment);
    rig.step();

    report(&rig, &key, 2);
    pull(&mut rig);
    assert_eq!(rig.count("phone_pickups"), 1);
    assert_eq!(repo::pickups_of(&rig.db.lock().expect("db"), segment).expect("count"), Some(2));

    // The same report again, however often it is read: nothing added.
    report(&rig, &key, 2);
    pull(&mut rig);
    pull(&mut rig);
    assert_eq!(rig.count("phone_pickups"), 1);

    // A later, higher count is appended; an older, lower one arriving late is not.
    report(&rig, &key, 5);
    pull(&mut rig);
    report(&rig, &key, 3);
    pull(&mut rig);
    assert_eq!(rig.count("phone_pickups"), 2);
    assert_eq!(repo::pickups_of(&rig.db.lock().expect("db"), segment).expect("count"), Some(5), "the segment's count is its largest report");

    let conn = rig.db.lock().expect("db");
    assert!(conn.execute("UPDATE phone_pickups SET count = 0", []).is_err());
    assert!(conn.execute("DELETE FROM phone_pickups", []).is_err());
}

#[test]
fn only_study_segments_of_this_pc_count_and_junk_is_ignored() {
    let mut rig = Rig::new(2);
    let study = start_of_study(&mut rig);
    let run = study.run_id.expect("run");
    let segment = study.segment_id.expect("segment");
    // The break's segment (a real segment, but not a study one) exists after the boundary.
    rig.set(study.planned_end_ms.expect("end") + 2 * SEC);
    let brk = rig.engine();
    assert_eq!(brk.phase, Phase::Break);
    let break_key = segment_key(run, brk.segment_id.expect("break segment"));
    rig.step();
    let other_run = segment_key(run + 1, segment);
    let trailing = format!("{run}:{segment}x");
    for junk in [break_key.as_str(), "99:99", "abc", "x", other_run.as_str(), trailing.as_str()] {
        report(&rig, junk, 4);
    }
    pull(&mut rig);
    assert_eq!(rig.count("phone_pickups"), 0);
    report(&rig, &segment_key(run, segment), 1);
    pull(&mut rig);
    assert_eq!(rig.count("phone_pickups"), 1);
}

#[test]
fn a_zero_count_is_a_real_answer_and_a_count_is_never_negative() {
    let mut rig = Rig::new(2);
    let study = start_of_study(&mut rig);
    let key = segment_key(study.run_id.expect("run"), study.segment_id.expect("segment"));
    rig.step();
    report(&rig, &key, 0);
    pull(&mut rig);
    assert_eq!(rig.count("phone_pickups"), 1, "a set without a single pick-up is recorded as such");
    let conn = rig.db.lock().expect("db");
    assert!(conn.execute("INSERT INTO phone_pickups (segment_id, count, reported_at_ms) VALUES (1, -1, 0)", []).is_err());
}

// ---- what leaves and what is stored: no app names, no usage ----

#[test]
fn a_report_carries_a_count_and_a_segment_id_and_nothing_a_phone_could_smuggle_in() {
    // The wire type refuses any extra field, so a phone build that adds an app name is rejected
    // by the receiving side instead of being stored.
    let smuggled = json!({ "segment_id": "1:2", "count": 3, "package": PACKAGE_CANARY });
    assert!(serde_json::from_value::<PickupReport>(smuggled).is_err());
    let sent = serde_json::to_value(PickupReport { segment_id: "1:2".into(), count: 3 }).expect("json");
    let mut keys: Vec<&str> = sent.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["count", "segment_id"]);

    // On the real adapter: the request body of the report names exactly the two RPC parameters.
    let mut rig = Rig::new(2);
    let study = start_of_study(&mut rig);
    rig.step();
    report(&rig, &segment_key(study.run_id.expect("run"), study.segment_id.expect("segment")), 4);
    pull(&mut rig);
    let bodies = rig.world.recorder.bodies_of("report_phone_pickups");
    assert_eq!(bodies.len(), 1);
    let body: Value = serde_json::from_str(&bodies[0]).expect("json body");
    let mut body_keys: Vec<&str> = body.as_object().expect("object").keys().map(String::as_str).collect();
    body_keys.sort_unstable();
    assert_eq!(body_keys, ["p_count", "p_segment_id"]);

    // Nothing that left the PC or the phone carries a canary, and the local tables have only the
    // columns of a count / a gap.
    for (name, text) in rig.world.recorder.bodies() {
        for canary in [PACKAGE_CANARY, CANARY_TITLE, CANARY_URL] {
            assert!(!text.contains(canary), "{name}");
        }
    }
    let columns = |table: &str| -> Vec<String> {
        let conn = rig.db.lock().expect("db");
        let mut stmt = conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}') ORDER BY cid")).expect("prepare");
        stmt.query_map([], |r| r.get(0)).expect("query").collect::<Result<_, _>>().expect("rows")
    };
    assert_eq!(columns("phone_pickups"), ["id", "segment_id", "count", "reported_at_ms"]);
    assert_eq!(columns("phone_gap"), ["id", "run_id", "start_ms", "end_ms", "kind"]);
    // The server's own row has the same shape: the PC reads counts and ids only.
    let rows = rig.world.backend.select(&Caller::Service, "phone_pickups", &[]).expect("server rows");
    assert_eq!(rows.len(), 1);
    let mut server_keys: Vec<&str> = rows[0].as_object().expect("row").keys().map(String::as_str).collect();
    server_keys.sort_unstable();
    for key in server_keys {
        assert!(["count", "owner", "phone", "segment_id", "at_ms", "owner_user_id", "phone_user_id", "reported_at"].contains(&key), "{key}");
    }
}

#[test]
fn the_publisher_logs_carry_ids_and_codes_only() {
    let mut rig = Rig::new(2);
    let logs = crate::testutil::capture_logs(|| {
        start_of_study(&mut rig);
        rig.step();
        rig.world.backend.set_outage(true);
        rig.grant(PassKind::EmergencyPass);
        rig.step();
    });
    for forbidden in ["http", "://", "title", PACKAGE_CANARY, "수학"] {
        assert!(!logs.contains(forbidden), "log leaked {forbidden:?}: {logs}");
    }
}
