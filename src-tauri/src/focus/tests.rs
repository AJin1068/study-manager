//! Focus tests run on fake sensors and a fake clock: every function takes the time as a number,
//! so a scenario is a table of `(second, what is in front, idle seconds)`.

use std::sync::Mutex;

use chrono::{TimeZone, Utc};
use rusqlite::Connection;

use super::calc::{
    Foreground, IDLE_SUSPECT_MS, MAX_GAP_MS, Observation, SAMPLE_INTERVAL_MS, Tracker,
    event_metrics, focus_score,
};
use super::model::{FocusEventKind, FocusEventRow, FocusTotals, MinuteDelta};
use super::*;
use crate::db::open_in_memory;
use crate::enforce::model::{BlockAction, EnforceMode, NewBlockEvent, TargetKind};
use crate::plan::{CreatePlanInput, create_plan};
use crate::platform::PlatformError;
use crate::study::{ModeConfig, StudyLevel};
use crate::testutil::{capture_logs, set_lock_for_test};
use crate::timer::model::TimerState;

const SELF_PID: u32 = 999;
const IDE: &str = "com.example.ide";
const DOCS: &str = "com.example.docs";
const GAME: &str = "com.example.game";
const SECRET_NAME: &str = "Secret Diary - bank.example.com/account?token=abc";

fn t(h: i64, m: i64, s: i64) -> i64 {
    let base = Utc
        .with_ymd_and_hms(2026, 11, 5, 0, 0, 0)
        .single()
        .expect("date")
        .timestamp_millis();
    base + (h * 3600 + m * 60 + s) * 1000
}

fn app(id: &str, pid: u32) -> AppIdentity {
    AppIdentity {
        id: id.into(),
        display_name: SECRET_NAME.into(),
        pid,
        is_protected: false,
    }
}

// ---- pure calculator ----

fn study() -> Foreground {
    Foreground::Study
}

fn other(id: &str) -> Foreground {
    Foreground::Other(id.into())
}

/// Feeds `(second, foreground, idle seconds)` rows and returns every event and the summed buckets.
fn run(
    rows: &[(i64, Foreground, i64)],
) -> (Vec<(i64, FocusEventKind, Option<String>)>, FocusTotals) {
    let mut tracker = Tracker::default();
    let mut events = Vec::new();
    let mut totals = FocusTotals::default();
    for (second, foreground, idle) in rows {
        let obs = Observation {
            at_ms: second * 1000,
            foreground: foreground.clone(),
            idle_ms: idle * 1000,
        };
        let step = tracker.step(&obs, 0);
        events.extend(
            step.events
                .into_iter()
                .map(|e| (e.at_ms / 1000, e.kind, e.app_id)),
        );
        for m in step.minutes {
            totals.credited_ms += m.credited_ms;
            totals.away_ms += m.away_ms;
            totals.idle_ms += m.idle_ms;
        }
    }
    (events, totals)
}

#[test]
fn only_a_study_to_non_study_switch_is_a_deviation_and_return_pairs_with_it() {
    let (events, totals) = run(&[
        (0, study(), 0),
        (5, study(), 0),
        (10, other(GAME), 0),
        (15, other("com.example.chat"), 0), // still away: no second event
        (20, study(), 0),
        (25, study(), 0),
    ]);
    assert_eq!(
        events,
        vec![
            (10, FocusEventKind::Away, Some(GAME.to_owned())),
            (20, FocusEventKind::Return, None),
        ]
    );
    // [0,10) credited, [10,20) away, [20,25) credited.
    assert_eq!(
        (totals.credited_ms, totals.away_ms, totals.idle_ms),
        (15_000, 10_000, 0)
    );
}

#[test]
fn moving_between_study_apps_is_not_a_deviation() {
    // Both IDE and docs classify as `Study`, so the calculator sees an unbroken study stretch.
    let (events, totals) = run(&[(0, study(), 0), (5, study(), 0), (10, study(), 0)]);
    assert!(events.is_empty());
    assert_eq!(totals.credited_ms, 10_000);
}

#[test]
fn starting_in_a_non_study_app_records_no_deviation_and_no_return() {
    let (events, totals) = run(&[
        (0, other(GAME), 0),
        (5, other(GAME), 0),
        (10, study(), 0),
        (15, study(), 0),
    ]);
    assert!(
        events.is_empty(),
        "never left a study app, so nothing to return from: {events:?}"
    );
    assert_eq!((totals.credited_ms, totals.away_ms), (5_000, 10_000));
}

#[test]
fn each_trip_away_is_its_own_pair_and_unknown_foreground_changes_nothing() {
    let (events, totals) = run(&[
        (0, study(), 0),
        (5, Foreground::Unknown, 0), // locked screen: no event, no credit, no away time
        (10, other(GAME), 0),
        (15, study(), 0),
        (20, other(GAME), 0),
        (25, study(), 0),
    ]);
    let kinds: Vec<_> = events.iter().map(|e| e.1).collect();
    assert_eq!(
        kinds,
        [
            FocusEventKind::Away,
            FocusEventKind::Return,
            FocusEventKind::Away,
            FocusEventKind::Return
        ]
    );
    // [0,5) credited, [5,10) unattributed, [10,15) away, [15,20) credited, [20,25) away.
    assert_eq!((totals.credited_ms, totals.away_ms), (10_000, 10_000));
}

#[test]
fn idle_boundary_9_59_is_credited_and_10_00_is_a_zoning_out_suspicion() {
    let (events, totals) = run(&[(0, study(), 599), (5, study(), 599), (10, study(), 599)]);
    assert!(events.is_empty());
    assert_eq!((totals.credited_ms, totals.idle_ms), (10_000, 0));

    let (events, totals) = run(&[(0, study(), 600), (5, study(), 600), (10, study(), 600)]);
    assert_eq!(events, vec![(0, FocusEventKind::IdleStart, None)]);
    assert_eq!((totals.credited_ms, totals.idle_ms), (0, 10_000));

    // Crossing the line mid-stream: the span before the first 600 s reading is still credited.
    let (events, totals) = run(&[
        (0, study(), 598),
        (5, study(), 599),
        (10, study(), 600),
        (15, study(), 601),
    ]);
    assert_eq!(events, vec![(10, FocusEventKind::IdleStart, None)]);
    assert_eq!((totals.credited_ms, totals.idle_ms), (10_000, 5_000));
    assert_eq!(IDLE_SUSPECT_MS, 600_000);
}

#[test]
fn input_after_a_long_idle_ends_the_suspicion_and_credit_resumes() {
    let (events, totals) = run(&[
        (0, study(), 700),
        (5, study(), 705),
        (10, study(), 0),
        (15, study(), 5),
    ]);
    assert_eq!(
        events,
        vec![
            (0, FocusEventKind::IdleStart, None),
            (10, FocusEventKind::IdleEnd, None)
        ]
    );
    assert_eq!((totals.idle_ms, totals.credited_ms), (10_000, 5_000));
}

#[test]
fn idle_in_a_non_study_app_is_away_time_not_idle_time() {
    let (_, totals) = run(&[
        (0, study(), 0),
        (5, other(GAME), 700),
        (10, other(GAME), 705),
    ]);
    assert_eq!(
        (totals.credited_ms, totals.away_ms, totals.idle_ms),
        (5_000, 5_000, 0)
    );
}

#[test]
fn a_sleep_gap_gets_no_time_and_idle_that_accrued_while_asleep_is_not_believed() {
    let eight_hours = 8 * 3600;
    let (events, totals) = run(&[
        (0, study(), 0),
        (5, study(), 5),
        // The lid was closed; the OS idle counter kept growing across the gap.
        (5 + eight_hours, study(), eight_hours),
        (10 + eight_hours, study(), eight_hours + 5),
    ]);
    assert!(events.is_empty(), "sleep is not zoning out: {events:?}");
    // [0,5) before the gap and [5+8h, 10+8h) after it; nothing for the 8 hours between.
    assert_eq!((totals.credited_ms, totals.idle_ms), (10_000, 0));
    const { assert!(8 * 3600 * 1000 > MAX_GAP_MS) };

    // 10 observed minutes of no input after waking is a suspicion again.
    let mut rows = vec![(0, study(), 0), (5 + eight_hours, study(), eight_hours)];
    for s in (10..=620).step_by(5) {
        rows.push((s + eight_hours, study(), eight_hours + s));
    }
    let (events, _) = run(&rows);
    assert_eq!(
        events,
        vec![(5 + eight_hours + 600, FocusEventKind::IdleStart, None)]
    );
}

#[test]
fn a_gap_just_over_three_samples_is_unobserved_and_exactly_three_is_not() {
    let (_, totals) = run(&[(0, study(), 0), (15, study(), 0)]);
    assert_eq!(totals.credited_ms, 15_000);
    let (_, totals) = run(&[(0, study(), 0), (16, study(), 0)]);
    assert_eq!(totals.credited_ms, 0);
    assert_eq!(MAX_GAP_MS, 3 * SAMPLE_INTERVAL_MS);
}

#[test]
fn time_before_the_segment_started_and_out_of_order_samples_are_ignored() {
    let mut tracker = Tracker::default();
    let obs = |s: i64| Observation {
        at_ms: s * 1000,
        foreground: Foreground::Study,
        idle_ms: 0,
    };
    assert!(tracker.step(&obs(7), 10_000).minutes.is_empty());
    let step = tracker.step(&obs(12), 10_000);
    assert_eq!(
        step.minutes.iter().map(|m| m.credited_ms).sum::<i64>(),
        2_000,
        "only 10 s to 12 s"
    );
    let before = tracker.step(&obs(11), 10_000);
    assert!(before.events.is_empty() && before.minutes.is_empty());
    let same = tracker.step(&obs(12), 10_000);
    assert!(same.events.is_empty() && same.minutes.is_empty());
}

#[test]
fn a_span_over_a_minute_boundary_is_split_between_both_minutes() {
    let mut tracker = Tracker::default();
    let obs = |ms: i64| Observation {
        at_ms: ms,
        foreground: Foreground::Study,
        idle_ms: 0,
    };
    tracker.step(&obs(58_000), 0);
    let step = tracker.step(&obs(62_000), 0);
    let by_minute: Vec<_> = step
        .minutes
        .iter()
        .map(|m| (m.minute_start_ms, m.credited_ms))
        .collect();
    assert_eq!(by_minute, vec![(0, 2_000), (60_000, 2_000)]);
}

#[test]
fn focus_score_is_recognized_over_studied_with_no_attempt_penalty() {
    assert_eq!(focus_score(0, 0), None);
    assert_eq!(focus_score(5, -1), None);
    assert_eq!(focus_score(0, 1_000), Some(0));
    assert_eq!(focus_score(1_500_000, 3_000_000), Some(50));
    assert_eq!(focus_score(3_000_000, 3_000_000), Some(100));
    assert_eq!(focus_score(530_000, 600_000), Some(88));
    assert_eq!(focus_score(2, 3), Some(67), "rounds half up");
    assert_eq!(
        focus_score(9_000_000, 3_000_000),
        Some(100),
        "never above 100"
    );
    assert_eq!(focus_score(-5, 1_000), Some(0));
}

fn ev(at_ms: i64, kind: FocusEventKind) -> FocusEventRow {
    FocusEventRow {
        at_ms,
        kind,
        app_id: None,
    }
}

#[test]
fn event_metrics_measure_first_deviation_and_return_from_the_first_away() {
    let events = [
        ev(1_000, FocusEventKind::IdleStart),
        ev(130_000, FocusEventKind::Away),
        ev(190_000, FocusEventKind::Return),
        ev(300_000, FocusEventKind::Away),
        ev(310_000, FocusEventKind::Return),
        ev(400_000, FocusEventKind::SelfReport),
    ];
    let m = event_metrics(&events, 100_000);
    assert_eq!(
        (
            m.away_count,
            m.idle_suspect_count,
            m.first_away_after_ms,
            m.return_after_ms
        ),
        (2, 1, Some(30_000), Some(60_000))
    );
    let none = event_metrics(&[], 0);
    assert_eq!(
        (
            none.first_away_after_ms,
            none.return_after_ms,
            none.away_count
        ),
        (None, None, 0)
    );
    let unreturned = event_metrics(&[ev(5_000, FocusEventKind::Away)], 0);
    assert_eq!(
        (unreturned.first_away_after_ms, unreturned.return_after_ms),
        (Some(5_000), None)
    );
}

#[test]
fn classification_treats_study_apps_and_this_app_as_studying() {
    let ids = vec![IDE.to_owned(), DOCS.to_owned()];
    assert_eq!(
        classify(Some(&app(IDE, 1)), &ids, SELF_PID),
        Foreground::Study
    );
    assert_eq!(
        classify(Some(&app(DOCS, 2)), &ids, SELF_PID),
        Foreground::Study
    );
    assert_eq!(
        classify(Some(&app(GAME, 3)), &ids, SELF_PID),
        Foreground::Other(GAME.into())
    );
    assert_eq!(
        classify(Some(&app("com.studymanager.app", SELF_PID)), &[], SELF_PID),
        Foreground::Study
    );
    assert_eq!(classify(None, &ids, SELF_PID), Foreground::Unknown);
    assert_eq!(
        classify(Some(&app(GAME, 3)), &[], SELF_PID),
        Foreground::Other(GAME.into()),
        "empty list: nothing recognized"
    );
}

// ---- recording against a database ----

#[derive(Default)]
struct ProbeState {
    front: Option<AppIdentity>,
    idle: u64,
    fail: bool,
}

#[derive(Default)]
struct FakeProbe(Mutex<ProbeState>);

impl FakeProbe {
    fn set(&self, front: Option<AppIdentity>, idle: u64) {
        let mut state = self.0.lock().expect("probe");
        state.front = front;
        state.idle = idle;
    }

    fn fail(&self, fail: bool) {
        self.0.lock().expect("probe").fail = fail;
    }
}

impl FocusProbe for FakeProbe {
    fn frontmost_app(&self) -> Result<Option<AppIdentity>, PlatformError> {
        let state = self.0.lock().expect("probe");
        if state.fail {
            return Err(PlatformError::Unsupported("test"));
        }
        Ok(state.front.clone())
    }

    fn idle_seconds(&self) -> Result<u64, PlatformError> {
        let state = self.0.lock().expect("probe");
        if state.fail {
            return Err(PlatformError::Unsupported("test"));
        }
        Ok(state.idle)
    }

    fn window_title_access(&self) -> TitleAccess {
        TitleAccess::Denied
    }
}

fn set_medium(conn: &Connection, study: i64, brk: i64) {
    let config = ModeConfig {
        level: StudyLevel::Medium,
        study_min: study,
        break_min: brk,
        long_break_min: None,
        sets_before_long_break: None,
    };
    store::put_mode_config(conn, &config).expect("mode");
}

/// A database with one plan on 2026-11-05 20:00 and the IDE on the study-app list.
fn rig(sets: i64, study_min: i64, break_min: i64) -> Mutex<Connection> {
    let mut conn = open_in_memory().expect("db");
    set_medium(&conn, study_min, break_min);
    let input = CreatePlanInput {
        name: "수학".into(),
        subject: "수학".into(),
        start_date: "2026-11-05".into(),
        start_time: "20:00".into(),
        sets,
        repeat: None,
    };
    create_plan(&mut conn, &Utc, input, t(0, 0, 0) - 86_400_000).expect("plan");
    store::insert_study_app(&conn, EntryKind::App, IDE, 1).expect("study app");
    Mutex::new(conn)
}

fn state_at(db: &Mutex<Connection>, now: i64) -> TimerState {
    let mut conn = db.lock().expect("db");
    timer::reconcile(&mut conn, &Utc, now)
        .expect("reconcile")
        .state
}

fn count(db: &Mutex<Connection>, table: &str) -> i64 {
    let conn = db.lock().expect("db");
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .expect("count")
}

fn sample_at(
    db: &Mutex<Connection>,
    state: &TimerState,
    probe: &FakeProbe,
    rec: &mut Recorder,
    at_ms: i64,
) {
    sample(
        db,
        enforce::study_window(state),
        probe,
        rec,
        SELF_PID,
        at_ms,
    )
    .expect("sample");
}

#[test]
fn a_study_block_is_recorded_into_events_minutes_and_the_set_summary_without_double_counting_attempts()
 {
    let db = rig(3, 10, 5);
    let probe = FakeProbe::default();
    let mut rec = Recorder::default();
    let study_state = state_at(&db, t(20, 0, 1));
    assert!(study_state.lock_held);

    // 20:00:05 .. 20:09:55 every 5 s: IDE, then the game from 3:00 to 3:55, then the IDE again.
    for second in (5..600).step_by(5) {
        let front = if (180..240).contains(&second) {
            app(GAME, 7)
        } else {
            app(IDE, 8)
        };
        probe.set(Some(front), 0);
        if second == 190 {
            // The enforcement engine logs the same trip into the game as one block attempt.
            let event = NewBlockEvent {
                at_ms: t(20, 3, 10),
                run_id: study_state.run_id,
                segment_id: study_state.segment_id,
                target_kind: TargetKind::App,
                target_id: GAME,
                action: BlockAction::WouldTerminate,
                mode: EnforceMode::DryRun,
                detail_code: None,
            };
            store::insert_block_event(&db.lock().expect("db"), &event).expect("attempt");
        }
        sample_at(&db, &study_state, &probe, &mut rec, t(20, 0, second));
    }

    let after = state_at(&db, t(20, 10, 1));
    let settled = {
        let conn = db.lock().expect("db");
        timer::summary::settle(&conn, &study_state, &after).expect("settle")
    };
    let set = settled.sets.first().expect("set summary");
    assert_eq!(set.set_studied_ms, 600_000);
    // [5 s, 180 s) and [240 s, 595 s) are recognized: 175 + 355 seconds.
    assert_eq!(set.focus_credited_ms, 530_000);
    assert_eq!(set.focus_score, Some(88));
    assert_eq!(
        (set.first_away_after_ms, set.return_after_ms),
        (Some(180_000), Some(60_000))
    );
    assert_eq!(
        set.attempt_count, 1,
        "one trip into a blocked app is one attempt; the deviation is not added to it"
    );

    let conn = db.lock().expect("db");
    let events =
        store::list_focus_events(&conn, study_state.segment_id.expect("segment")).expect("events");
    let kinds: Vec<_> = events
        .iter()
        .map(|e| (e.kind, e.app_id.as_deref()))
        .collect();
    assert_eq!(
        kinds,
        [
            (FocusEventKind::Away, Some(GAME)),
            (FocusEventKind::Return, None)
        ]
    );
    let totals =
        store::focus_totals(&conn, study_state.run_id.expect("run"), None).expect("totals");
    assert_eq!(
        (totals.credited_ms, totals.away_ms, totals.idle_ms),
        (530_000, 60_000, 0)
    );
}

#[test]
fn nothing_is_recorded_during_breaks_when_idle_or_when_the_lock_is_not_held() {
    let db = rig(3, 10, 5);
    let probe = FakeProbe::default();
    probe.set(Some(app(IDE, 8)), 0);
    let mut rec = Recorder::default();

    // Before the plan starts: no run, no window.
    let idle_state = state_at(&db, t(19, 0, 0));
    sample_at(&db, &idle_state, &probe, &mut rec, t(19, 0, 5));
    sample_at(&db, &idle_state, &probe, &mut rec, t(19, 0, 10));

    // Break 20:10 - 20:15.
    state_at(&db, t(20, 0, 1));
    let break_state = state_at(&db, t(20, 10, 30));
    assert!(break_state.lock_held, "the lock spans breaks");
    for s in (35..300).step_by(5) {
        sample_at(&db, &break_state, &probe, &mut rec, t(20, 10, s));
    }
    assert_eq!(
        (count(&db, "focus_minutes"), count(&db, "focus_events")),
        (0, 0)
    );

    // A window for a study block whose lock was released in between records nothing either.
    let study_state = state_at(&db, t(20, 15, 1));
    set_lock_for_test(&db.lock().expect("db"), false);
    sample_at(&db, &study_state, &probe, &mut rec, t(20, 15, 5));
    sample_at(&db, &study_state, &probe, &mut rec, t(20, 15, 10));
    assert_eq!(
        (count(&db, "focus_minutes"), count(&db, "focus_events")),
        (0, 0)
    );
}

#[test]
fn a_new_segment_starts_a_fresh_tracker() {
    let db = rig(3, 10, 5);
    let probe = FakeProbe::default();
    let mut rec = Recorder::default();
    let first = state_at(&db, t(20, 0, 1));
    probe.set(Some(app(IDE, 8)), 0);
    sample_at(&db, &first, &probe, &mut rec, t(20, 9, 50));
    // The break clears the tracker; the next study block's first sample has no earlier sample to span from.
    let brk = state_at(&db, t(20, 10, 1));
    sample_at(&db, &brk, &probe, &mut rec, t(20, 10, 5));
    let second = state_at(&db, t(20, 15, 1));
    sample_at(&db, &second, &probe, &mut rec, t(20, 15, 5));
    assert_eq!(count(&db, "focus_minutes"), 0);
    probe.set(Some(app(IDE, 8)), 0);
    sample_at(&db, &second, &probe, &mut rec, t(20, 15, 10));
    let conn = db.lock().expect("db");
    let totals =
        store::focus_totals(&conn, second.run_id.expect("run"), second.segment_id).expect("totals");
    assert_eq!(totals.credited_ms, 5_000);
}

#[test]
fn eight_hours_of_study_is_480_minute_buckets_not_thousands_of_rows() {
    // Two 4-hour blocks with a 5-minute break.
    let db = rig(2, 240, 5);
    let probe = FakeProbe::default();
    probe.set(Some(app(IDE, 8)), 0);
    let mut rec = Recorder::default();
    let first = state_at(&db, t(20, 0, 1));
    for second in (5..240 * 60).step_by(5) {
        sample_at(&db, &first, &probe, &mut rec, t(20, 0, 0) + second * 1000);
    }
    let brk = state_at(&db, t(24, 0, 1));
    sample_at(&db, &brk, &probe, &mut rec, t(24, 0, 5));
    let second_block = state_at(&db, t(24, 5, 1));
    assert_eq!(second_block.set_index, 2);
    for second in (5..240 * 60).step_by(5) {
        sample_at(
            &db,
            &second_block,
            &probe,
            &mut rec,
            t(24, 5, 0) + second * 1000,
        );
    }
    assert_eq!(count(&db, "focus_minutes"), 480);
    let conn = db.lock().expect("db");
    let totals = store::focus_totals(&conn, first.run_id.expect("run"), None).expect("totals");
    assert_eq!(totals.credited_ms, 2 * (240 * 60 - 10) * 1000);
}

#[test]
fn zoning_out_is_recorded_as_idle_events_and_idle_time() {
    let db = rig(3, 30, 5);
    let probe = FakeProbe::default();
    let mut rec = Recorder::default();
    let state = state_at(&db, t(20, 0, 1));
    for second in (5..=1500).step_by(5) {
        // No input from the start: the OS idle counter equals the elapsed time.
        probe.set(Some(app(IDE, 8)), second as u64 + 595);
        sample_at(&db, &state, &probe, &mut rec, t(20, 0, 0) + second * 1000);
    }
    let conn = db.lock().expect("db");
    let events =
        store::list_focus_events(&conn, state.segment_id.expect("segment")).expect("events");
    assert_eq!(
        events.iter().map(|e| e.kind).collect::<Vec<_>>(),
        [FocusEventKind::IdleStart]
    );
    assert_eq!(
        events[0].at_ms,
        t(20, 0, 5),
        "595 + 5 = 600 s at the first sample"
    );
    let totals = store::focus_totals(&conn, state.run_id.expect("run"), None).expect("totals");
    assert_eq!((totals.credited_ms, totals.idle_ms), (0, 1_495_000));
}

#[test]
fn sensor_failure_records_nothing_and_is_logged_once() {
    let db = rig(3, 10, 5);
    let probe = FakeProbe::default();
    probe.fail(true);
    let mut rec = Recorder::default();
    let state = state_at(&db, t(20, 0, 1));
    let logs = capture_logs(|| {
        for s in [5, 10, 15] {
            sample_at(&db, &state, &probe, &mut rec, t(20, 0, s));
        }
    });
    assert_eq!(logs.matches("focus_probe_failed").count(), 1, "{logs}");
    assert_eq!(
        (count(&db, "focus_minutes"), count(&db, "focus_events")),
        (0, 0)
    );
}

#[test]
fn logs_and_rows_carry_app_ids_only_never_names_or_titles() {
    let db = rig(3, 10, 5);
    let probe = FakeProbe::default();
    let mut rec = Recorder::default();
    let state = state_at(&db, t(20, 0, 1));
    let logs = capture_logs(|| {
        probe.set(Some(app(IDE, 8)), 0);
        sample_at(&db, &state, &probe, &mut rec, t(20, 0, 5));
        probe.set(Some(app(GAME, 9)), 0);
        sample_at(&db, &state, &probe, &mut rec, t(20, 0, 10));
    });
    assert!(logs.contains(GAME) && logs.contains("away"), "{logs}");
    for secret in ["Secret Diary", "bank.example.com", "token=abc"] {
        assert!(!logs.contains(secret), "log leaked {secret}: {logs}");
    }
    let conn = db.lock().expect("db");
    let dump: String = conn
        .prepare("SELECT app_id FROM focus_events")
        .expect("prepare")
        .query_map([], |r| r.get::<_, Option<String>>(0))
        .expect("query")
        .filter_map(|r| r.ok().flatten())
        .collect();
    assert_eq!(dump, GAME);
}

// ---- study-app list ----

fn locked_db() -> Connection {
    let conn = open_in_memory().expect("db");
    set_lock_for_test(&conn, true);
    conn
}

#[test]
fn study_apps_are_normalized_unique_and_removable_when_unlocked() {
    let mut conn = open_in_memory().expect("db");
    assert!(
        store::list_study_apps(&conn).expect("list").is_empty(),
        "the list starts empty (D-31)"
    );
    let applied = add_study_app(&mut conn, "  Com.Example.IDE ", 5).expect("add");
    assert_eq!(
        applied,
        UpdateOutcome::Applied {
            key: STUDY_APP_LIST_KEY.into()
        }
    );
    let apps = store::list_study_apps(&conn).expect("list");
    assert_eq!(
        (
            apps[0].app_id.as_str(),
            apps[0].kind.as_str(),
            apps[0].enabled
        ),
        ("com.example.ide", "app", true)
    );
    assert!(
        add_study_app(&mut conn, "com.example.ide", 6).is_err(),
        "duplicate"
    );
    assert!(add_study_app(&mut conn, "   ", 6).is_err(), "empty");
    assert!(
        add_study_app(&mut conn, "a\nb", 6).is_err(),
        "control character"
    );

    set_study_app_enabled(&mut conn, apps[0].id, false, 7).expect("disable");
    assert!(
        store::list_enabled_study_app_ids(&conn, EntryKind::App)
            .expect("ids")
            .is_empty()
    );
    set_study_app_enabled(&mut conn, apps[0].id, true, 8).expect("enable");
    assert_eq!(
        store::list_enabled_study_app_ids(&conn, EntryKind::App).expect("ids"),
        ["com.example.ide"]
    );

    assert!(remove_study_app(&mut conn, 999, 9).is_err(), "unknown id");
    remove_study_app(&mut conn, apps[0].id, 10).expect("remove");
    assert!(store::list_study_apps(&conn).expect("list").is_empty());
}

#[test]
fn the_study_app_list_cannot_change_while_the_lock_is_held_and_the_attempt_is_recorded() {
    let mut conn = locked_db();
    let outcome = add_study_app(&mut conn, GAME, 5).expect("add");
    assert_eq!(
        outcome,
        UpdateOutcome::RejectedLocked {
            key: STUDY_APP_LIST_KEY.into()
        }
    );
    assert!(store::list_study_apps(&conn).expect("list").is_empty());
    let latest = &crate::settings::read_history(&conn, 1).expect("history")[0];
    assert_eq!(
        (
            latest.key.as_str(),
            latest.outcome.as_str(),
            latest.after.as_str()
        ),
        (
            STUDY_APP_LIST_KEY,
            "rejected_locked",
            "add:com.example.game"
        )
    );

    set_lock_for_test(&conn, false);
    add_study_app(&mut conn, IDE, 6).expect("add");
    let id = store::list_study_apps(&conn).expect("list")[0].id;
    set_lock_for_test(&conn, true);
    assert!(matches!(
        remove_study_app(&mut conn, id, 7).expect("remove"),
        UpdateOutcome::RejectedLocked { .. }
    ));
    assert!(matches!(
        set_study_app_enabled(&mut conn, id, false, 8).expect("toggle"),
        UpdateOutcome::RejectedLocked { .. }
    ));
    assert_eq!(store::list_study_apps(&conn).expect("list").len(), 1);
    assert!(store::list_study_apps(&conn).expect("list")[0].enabled);
}

#[test]
fn the_empty_list_warning_shows_exactly_when_no_study_app_is_enabled() {
    assert!(needs_study_apps(0));
    assert!(!needs_study_apps(1));

    let mut conn = open_in_memory().expect("db");
    let probe = FakeProbe::default();
    let empty = setup_status(&conn, &probe).expect("setup");
    assert_eq!(
        (
            empty.study_app_count,
            empty.needs_study_apps,
            empty.sensors_available
        ),
        (0, true, true)
    );
    assert_eq!(empty.title_access, TitleAccess::Denied);

    add_study_app(&mut conn, IDE, 5).expect("add");
    let id = store::list_study_apps(&conn).expect("list")[0].id;
    let one = setup_status(&conn, &probe).expect("setup");
    assert_eq!((one.study_app_count, one.needs_study_apps), (1, false));

    set_study_app_enabled(&mut conn, id, false, 6).expect("disable");
    assert!(
        setup_status(&conn, &probe).expect("setup").needs_study_apps,
        "a disabled entry recognizes nothing"
    );

    probe.fail(true);
    assert!(
        !setup_status(&conn, &probe)
            .expect("setup")
            .sensors_available
    );
}

// ---- schema ----

fn check_list(conn: &Connection, table: &str, column: &str) -> Vec<String> {
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = ?1",
            [table],
            |r| r.get(0),
        )
        .expect("sql");
    let start = sql
        .find(&format!("{column} "))
        .and_then(|i| sql[i..].find("IN (").map(|j| i + j + 4))
        .expect("check");
    let list = &sql[start..start + sql[start..].find(')').expect("end")];
    let mut values: Vec<String> = list
        .split(',')
        .map(|v| v.trim().trim_matches('\'').to_owned())
        .collect();
    values.sort();
    values
}

#[test]
fn check_constraints_list_exactly_the_rust_enum_values() {
    let conn = open_in_memory().expect("db");
    let sorted = |all: &[&str]| {
        let mut v: Vec<String> = all.iter().map(|s| (*s).to_owned()).collect();
        v.sort();
        v
    };
    assert_eq!(
        check_list(&conn, "focus_events", "kind"),
        sorted(&FocusEventKind::ALL.map(FocusEventKind::as_str))
    );
    assert_eq!(
        check_list(&conn, "study_apps", "kind"),
        sorted(&EntryKind::ALL.map(EntryKind::as_str))
    );
    for kind in FocusEventKind::ALL {
        assert_eq!(FocusEventKind::parse(kind.as_str()), Some(kind));
    }
}

#[test]
fn every_event_kind_is_accepted_by_the_real_check_and_events_are_append_only() {
    let db = rig(3, 10, 5);
    let state = state_at(&db, t(20, 0, 1));
    let conn = db.lock().expect("db");
    for kind in FocusEventKind::ALL {
        let row = NewFocusEvent {
            run_id: state.run_id.expect("run"),
            segment_id: state.segment_id.expect("segment"),
            at_ms: 1,
            kind,
            app_id: None,
        };
        store::insert_focus_event(&conn, &row).expect("accepted");
    }
    assert!(
        conn.execute("UPDATE focus_events SET kind = 'away'", [])
            .is_err()
    );
    assert!(conn.execute("DELETE FROM focus_events", []).is_err());
    assert!(
        conn.execute(
            "INSERT INTO focus_events (run_id, segment_id, at_ms, kind) VALUES (1, 1, 1, 'other')",
            []
        )
        .is_err()
    );
}

#[test]
fn a_minute_bucket_cannot_exceed_a_minute_and_is_never_deleted() {
    let db = rig(3, 10, 5);
    let state = state_at(&db, t(20, 0, 1));
    let conn = db.lock().expect("db");
    let segment = state.segment_id.expect("segment");
    let delta = |credited, away| MinuteDelta {
        minute_start_ms: 0,
        credited_ms: credited,
        away_ms: away,
        idle_ms: 0,
    };
    store::add_focus_minute(&conn, segment, &delta(40_000, 0)).expect("first");
    store::add_focus_minute(&conn, segment, &delta(0, 20_000)).expect("fills the minute");
    assert!(
        store::add_focus_minute(&conn, segment, &delta(1, 0)).is_err(),
        "61 s in one minute"
    );
    assert!(conn.execute("DELETE FROM focus_minutes", []).is_err());
    let totals = store::focus_totals(&conn, state.run_id.expect("run"), None).expect("totals");
    assert_eq!((totals.credited_ms, totals.away_ms), (40_000, 20_000));
}
