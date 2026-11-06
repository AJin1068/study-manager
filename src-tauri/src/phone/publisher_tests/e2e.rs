//! The whole chain on one fake clock: the desktop timer and publisher, the in-memory server, the
//! phone's lease polling, the `BlockerDriver` and a `MockBlocker`. The phone's monotonic clock is
//! the fake clock's reading minus its start; the phone's wall clock is not an input anywhere.

use link_protocol::{FreeReason, ReleaseCause};

use super::*;
use crate::phone::allow_list::AllowList;
use crate::phone::blocker::BlockerStatus;
use crate::phone::blocker_port::{BlockerCall, BlockerDriver, MockBlocker, PhoneBlocker, TickOutcome};
use crate::phone::model::PhoneCode;

/// A paired phone running the driver over the mock blocker.
struct PhoneApp {
    driver: BlockerDriver<MockBlocker>,
}

impl PhoneApp {
    fn new() -> PhoneApp {
        let allow = AllowList::new(true, true, [PACKAGE_CANARY]).expect("allow list");
        PhoneApp { driver: BlockerDriver::new(MockBlocker::default(), allow) }
    }

    /// One poll of the service: fetch (a failed fetch changes nothing), offer, enforce. Device time
    /// passes for the mock blocker as it does for the fake clock.
    fn poll(&mut self, rig: &Rig, last_mono: &mut i64) -> Result<TickOutcome, String> {
        let mono = rig.now() - T0;
        self.driver.blocker().advance(mono - *last_mono);
        *last_mono = mono;
        match rig.phone.fetch_lease() {
            Ok(fetched) => {
                if let Some(lease) = fetched.lease {
                    // A repeated lease is refused by the holder; that is the normal case.
                    let _ = self.driver.offer(lease, fetched.server_now_ms, mono);
                }
            }
            Err(PhoneError::Rejected(PhoneCode::NotPaired)) => self.driver.pair_revoked(),
            Err(_) => {} // server unreachable: the lease in force keeps deciding
        }
        self.driver.tick(mono).map_err(|e| e.code().to_string())
    }
}

#[test]
fn study_start_locks_the_phone_and_an_emergency_pass_frees_it() {
    let mut rig = Rig::new(2);
    let mut phone = PhoneApp::new();
    let mut mono = rig.now() - T0;

    // Before the plan starts: nothing to enforce.
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Unchanged));
    assert_eq!(phone.driver.blocker().status(), BlockerStatus::Inactive);

    // PC: the study segment begins and the publisher tells the server.
    let study = start_of_study(&mut rig);
    rig.step();
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Engaged { seq: 1 }));
    let blocker = phone.driver.blocker();
    assert_eq!(blocker.status(), BlockerStatus::Active);
    let engaged = blocker.engaged().expect("engaged");
    let end = study.planned_end_ms.expect("end");
    assert_eq!(engaged.remaining_ms, end + GRACE_MS - rig.now(), "the device gets a duration, not a wall-clock instant");
    // Polling again changes nothing and does not engage twice.
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Unchanged));
    assert_eq!(phone.driver.blocker().calls().len(), 1);
    let blocker = phone.driver.blocker();
    // The emergency call stays allowed.
    let BlockerCall::Engage { allow, .. } = &blocker.calls()[0] else { panic!("first call is an engage") };
    assert_eq!(allow["roles"][0], json!("emergency_call"));

    // PC: an emergency pass. The run goes on; the phone is freed at its next poll.
    rig.set(rig.now() + 5 * MIN);
    rig.grant(PassKind::EmergencyPass);
    rig.step();
    assert!(rig.lock_held());
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Released { reason: FreeReason::EmergencyPass }));
    assert_eq!(phone.driver.blocker().status(), BlockerStatus::Inactive);
    assert_eq!(phone.driver.blocker().calls().last(), Some(&BlockerCall::Release));

    // The pass ends: the phone is locked again for the rest of the segment.
    rig.run_until(rig.now() + 10 * MIN + 2 * SEC, 5 * SEC);
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Engaged { seq: 3 }));
    assert_eq!(phone.driver.blocker().engaged().expect("engaged").remaining_ms, end + GRACE_MS - rig.now());

    // The segment ends: free with the run's end.
    rig.set(end + 2 * SEC);
    rig.engine();
    rig.step();
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Released { reason: FreeReason::RunEnded }));
    assert_eq!(phone.driver.blocker().status(), BlockerStatus::Inactive);
}

#[test]
fn a_server_outage_keeps_the_lease_until_it_runs_out_and_then_frees_the_phone() {
    let mut rig = Rig::new(2);
    let mut phone = PhoneApp::new();
    let mut mono = rig.now() - T0;

    let study = start_of_study(&mut rig);
    rig.step();
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Engaged { seq: 1 }));
    let end = study.planned_end_ms.expect("end");

    // The server (and so the PC's signals) go away; the desktop timer is unaffected.
    rig.world.backend.set_outage(true);
    rig.set(rig.now() + 20 * MIN);
    rig.step();
    assert!(rig.engine().lock_held, "the PC keeps its own lock");
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Unchanged));
    assert_eq!(phone.driver.blocker().status(), BlockerStatus::Active, "offline with a valid lease stays locked");

    // The PC passes the end of the segment while the server is away: the Free lease cannot reach
    // the phone. The lease still ends by itself at its own end plus the grace.
    rig.set(end + GRACE_MS - SEC);
    rig.step();
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Unchanged));
    assert_eq!(phone.driver.blocker().status(), BlockerStatus::Active, "one second of the lease is left");
    rig.set(end + GRACE_MS + SEC);
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Released { reason: FreeReason::LeaseExpired }));
    assert_eq!(phone.driver.blocker().status(), BlockerStatus::Inactive, "never locked forever");

    // The server returns: the publisher catches up and the phone hears the Free lease, which
    // changes nothing (it is already free).
    rig.world.backend.set_outage(false);
    rig.engine();
    rig.step();
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Unchanged));
    assert!(rig.gaps().iter().all(|g| g.kind != "service_disabled"));
}

#[test]
fn ending_the_pairing_during_a_study_run_frees_the_phone_and_leaves_the_pc_lock_alone() {
    let mut rig = Rig::new(2);
    let mut phone = PhoneApp::new();
    let mut mono = rig.now() - T0;

    let study = start_of_study(&mut rig);
    rig.step();
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Engaged { seq: 1 }));

    // The owner ends the pairing mid-run, as the service does: server first, then the local row.
    rig.set(rig.now() + 2 * MIN);
    rig.owner.revoke_pair().expect("revoke");
    repo::save_pair(&rig.db.lock().expect("db"), false, None, rig.now()).expect("local pair row");
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Released { reason: FreeReason::PairRevoked }));
    assert_eq!(phone.driver.blocker().status(), BlockerStatus::Inactive);

    // The PC study run is untouched, and no later segment change is published or enforced.
    let before = rig.publications();
    assert!(rig.lock_held());
    rig.set(study.planned_end_ms.expect("end") + 2 * SEC);
    rig.engine();
    rig.step();
    assert_eq!(rig.publications(), before);
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Unchanged));
    assert_eq!(phone.driver.blocker().status(), BlockerStatus::Inactive);
    // The old token reads nothing and writes nothing any more.
    assert!(matches!(rig.phone.fetch_lease(), Err(PhoneError::Rejected(PhoneCode::NotPaired))));
    assert!(rig.phone.beat(&PhoneBeat { lease_seq: 1, locked: true, service_enabled: true }).is_err());
}

#[test]
fn nothing_the_chain_sends_names_an_app_even_with_a_custom_allow_list() {
    let mut rig = Rig::new(2);
    let mut phone = PhoneApp::new(); // allow list holds the canary package
    let mut mono = rig.now() - T0;
    let study = start_of_study(&mut rig);
    rig.step();
    assert_eq!(phone.poll(&rig, &mut mono), Ok(TickOutcome::Engaged { seq: 1 }));
    // The package reached the blocker (device side) ...
    let BlockerCall::Engage { allow, .. } = &phone.driver.blocker().calls()[0] else { panic!("engage") };
    assert!(allow.to_string().contains(PACKAGE_CANARY));
    // ... and nothing else: beats, pick-up reports, publications, the gaps and the mirrors.
    rig.beat(1, true);
    report(&rig, &segment_key(study.run_id.expect("run"), study.segment_id.expect("segment")), 2);
    pull(&mut rig);
    rig.grant(PassKind::EmergencyPass);
    rig.step();
    for (name, text) in rig.world.recorder.bodies() {
        for canary in [PACKAGE_CANARY, CANARY_TITLE, CANARY_URL] {
            assert!(!text.contains(canary), "{name} carries a canary");
        }
    }
    for table in ["phone_lock_state", "phone_beats", "phone_pickups", "phone_pairs"] {
        let rows = rig.world.backend.select(&Caller::Service, table, &[]).expect("server rows");
        assert!(!serde_json::to_string(&rows).expect("json").contains(PACKAGE_CANARY), "{table}");
    }
    assert_eq!(ReleaseCause::EmergencyPass, rig.lease().and_then(|l| l.release_cause).expect("cause"));
}
