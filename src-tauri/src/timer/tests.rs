//! Timer tests run on a fake clock: every function takes the time as a number, so a test is a
//! table of `(now, expected rows)`. Bounds below are for a medium 3-set plan at 20:00:
//! study 20:00-20:50, break -21:05, study -21:55, break -22:10, study -23:00.

use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use chrono_tz::America::New_York;
use rusqlite::Connection;

use super::derive::LATE_TOLERANCE_MS;
use super::engine::{tick_once, Emission, EngineHealth, Supervised, WakeReason, DEGRADED_AFTER};
use super::model::{EndReason, Phase, RunRow, RunStatus, SegmentRow};
use super::release::{ReleaseDesk, ReleaseOutcome};
use super::study_pass::{self, Grant, PassKind, Typing};
use super::*;
use crate::commands::{exit_decision, lock_db};
use crate::db::open_in_memory;
use crate::downtime::{DowntimeKind, Interval};
use crate::plan::{create_plan, BlockKind, CreatePlanInput};
use crate::study::{ModeConfig, StudyLevel};
use crate::testutil::{set_lock_for_test, FakeNotifier};

const DAY: i64 = 24 * 3600 * 1000;

/// Midnight UTC on 2026-11-05 plus the given clock time; `h` may pass 24 for the next day.
fn t(h: i64, m: i64, s: i64) -> i64 {
    let base = Utc.with_ymd_and_hms(2026, 11, 5, 0, 0, 0).single().expect("date").timestamp_millis();
    base + (h * 3600 + m * 60 + s) * 1000
}

fn new_plan(conn: &mut Connection, date: &str, time: &str, sets: i64, created_at: i64) -> i64 {
    let input = CreatePlanInput {
        name: "수학 문제풀이".into(),
        subject: "수학".into(),
        start_date: date.into(),
        start_time: time.into(),
        sets,
        repeat: None,
    };
    create_plan(conn, &Utc, input, created_at).expect("create").plan_id.expect("id")
}

/// The usual fixture: a 3-set medium plan on 2026-11-05 20:00 created the day before.
fn db_with_plan() -> (Connection, i64) {
    let mut conn = open_in_memory().expect("db");
    let id = new_plan(&mut conn, "2026-11-05", "20:00", 3, t(0, 0, 0) - DAY);
    (conn, id)
}

fn tick(conn: &mut Connection, now: i64) -> Reconciled {
    reconcile(conn, &Utc, now).expect("reconcile")
}

fn runs(conn: &Connection) -> Vec<RunRow> {
    store::list_runs_since(conn, "0000-00-00").expect("runs")
}

fn segs(conn: &Connection, run_id: i64) -> Vec<SegmentRow> {
    store::list_segments(conn, run_id).expect("segments")
}

fn lock(conn: &Connection) -> bool {
    store::read_study_state(conn).expect("lock").0
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).expect("count")
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

// ---- AC1: transitions at the planned boundaries, lock held across breaks ----

#[test]
fn transitions_happen_on_planned_boundaries_and_lock_spans_the_breaks() {
    let (mut conn, _) = db_with_plan();

    let idle = tick(&mut conn, t(19, 59, 59));
    assert_eq!((idle.state.phase, idle.state.lock_held), (Phase::Idle, false));
    assert_eq!(idle.state.next.as_ref().map(|n| n.planned_start_ms), Some(t(20, 0, 0)));
    assert!(!lock(&conn) && runs(&conn).is_empty());

    // One second late: the segment still starts on the planned boundary.
    let started = tick(&mut conn, t(20, 0, 1));
    assert_eq!((started.state.phase, started.state.set_index, started.state.sets_total), (Phase::Study, 1, 3));
    assert_eq!(started.state.planned_end_ms, Some(t(20, 50, 0)));
    assert!(lock(&conn));
    let run = &runs(&conn)[0];
    assert_eq!((run.status, run.planned_start_ms, run.planned_end_ms), (RunStatus::Active, t(20, 0, 0), t(23, 0, 0)));
    let first = &segs(&conn, run.id)[0];
    assert_eq!((first.kind, first.started_at_ms, first.late_join), (BlockKind::Study, t(20, 0, 0), false));

    // (now, phase, set, planned end of the open segment); the lock must be 1 in every row.
    let table = [
        (t(20, 49, 59), Phase::Study, 1, t(20, 50, 0)),
        (t(20, 50, 2), Phase::Break, 1, t(21, 5, 0)),
        (t(21, 5, 1), Phase::Study, 2, t(21, 55, 0)),
        (t(21, 55, 1), Phase::Break, 2, t(22, 10, 0)),
        (t(22, 10, 1), Phase::Study, 3, t(23, 0, 0)),
        (t(22, 59, 59), Phase::Study, 3, t(23, 0, 0)),
    ];
    for (now, phase, set, end) in table {
        let r = tick(&mut conn, now);
        assert_eq!((r.state.phase, r.state.set_index, r.state.planned_end_ms), (phase, set, Some(end)), "{now}");
        assert!(r.state.lock_held && lock(&conn), "lock must hold at {now}");
    }

    let done = tick(&mut conn, t(23, 0, 1));
    assert_eq!((done.state.phase, done.state.lock_held), (Phase::Idle, false));
    assert!(!lock(&conn));
    let run = &runs(&conn)[0];
    assert_eq!((run.status, run.ended_at_ms), (RunStatus::Completed, Some(t(23, 0, 0))));
    let all = segs(&conn, run.id);
    assert_eq!(all.len(), 5);
    let boundaries: Vec<i64> = all.iter().map(|s| s.started_at_ms).collect();
    assert_eq!(boundaries, [t(20, 0, 0), t(20, 50, 0), t(21, 5, 0), t(21, 55, 0), t(22, 10, 0)]);
    assert!(all.iter().all(|s| s.end_reason == Some(EndReason::Elapsed) && !s.late_join));
    assert_eq!(all[4].ended_at_ms, Some(t(23, 0, 0)));
}

#[test]
fn easy_mode_long_break_comes_from_the_run_snapshot_not_the_live_config() {
    // Easy: 25/5, long break 20 after 4 sets; 5 sets put the long break after the 4th set.
    let mut conn = open_in_memory().expect("db");
    conn.execute("INSERT INTO daily_mode (date, level, selected_at_ms) VALUES ('2026-11-05', 'easy', 0)", [])
        .expect("daily mode");
    new_plan(&mut conn, "2026-11-05", "08:00", 5, t(0, 0, 0) - DAY);
    tick(&mut conn, t(8, 0, 1));
    // A mode edit after the run started must not change it.
    conn.execute("UPDATE study_mode_config SET study_min = 99 WHERE level = 'easy'", []).expect("edit");
    // 4 studies of 25 plus 3 short breaks of 5 = 115 minutes, then the long break.
    let at = tick(&mut conn, t(8, 0, 0) + 116 * 60_000);
    assert_eq!((at.state.phase, at.state.set_index), (Phase::LongBreak, 4));
    assert_eq!(at.state.planned_end_ms, Some(t(8, 0, 0) + 135 * 60_000));
}

// ---- AC3: idempotent reconcile ----

#[test]
fn applying_the_same_time_twice_changes_nothing() {
    let (mut conn, _) = db_with_plan();
    let first = tick(&mut conn, t(21, 20, 0));
    assert!(first.changed);
    let counts = (count(&conn, "plan_runs"), count(&conn, "timer_segments"));
    let snapshot: Vec<SegmentRow> = segs(&conn, runs(&conn)[0].id);
    let second = tick(&mut conn, t(21, 20, 0));
    assert!(!second.changed);
    assert_eq!((count(&conn, "plan_runs"), count(&conn, "timer_segments")), counts);
    assert_eq!(segs(&conn, runs(&conn)[0].id), snapshot);
    assert_eq!(first.state.phase, second.state.phase);
    assert_eq!(second.state.run_id, runs(&conn).first().map(|r| r.id), "second pass sees written ids");
}

// ---- AC4: recovery scenarios 1-9 ----

#[test]
fn scenario_1_sleep_inside_a_block_continues_the_same_segment() {
    let (mut conn, _) = db_with_plan();
    tick(&mut conn, t(20, 10, 0));
    let before = segs(&conn, runs(&conn)[0].id);
    let woke = tick(&mut conn, t(20, 40, 0));
    assert_eq!((woke.state.phase, woke.state.lock_held), (Phase::Study, true));
    assert_eq!(segs(&conn, runs(&conn)[0].id), before, "no row is rewritten for a sleep inside a block");
}

#[test]
fn scenario_2_wake_after_blocks_ended_marks_elapsed_while_down_and_joins_late() {
    let (mut conn, _) = db_with_plan();
    tick(&mut conn, t(20, 10, 0));
    let woke = tick(&mut conn, t(21, 20, 0));
    assert_eq!((woke.state.phase, woke.state.set_index), (Phase::Study, 2));
    let run = &runs(&conn)[0];
    let all = segs(&conn, run.id);
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].end_reason, Some(EndReason::ElapsedWhileDown));
    assert_eq!(all[0].ended_at_ms, Some(t(20, 50, 0)));
    assert_eq!((all[1].end_reason, all[1].late_join), (Some(EndReason::ElapsedWhileDown), true));
    // The segment found already running starts when it was found.
    assert_eq!((all[2].started_at_ms, all[2].planned_start_ms, all[2].late_join), (t(21, 20, 0), t(21, 5, 0), true));
    assert!(all[2].ended_at_ms.is_none());
    assert!(lock(&conn));
}

#[test]
fn scenario_2_wake_after_the_whole_run_completes_it_and_frees_the_lock() {
    let (mut conn, _) = db_with_plan();
    tick(&mut conn, t(20, 10, 0));
    let woke = tick(&mut conn, t(23, 30, 0));
    assert!(!woke.state.lock_held && !lock(&conn));
    let run = &runs(&conn)[0];
    assert_eq!((run.status, run.detail_code.as_deref()), (RunStatus::Completed, Some("ended_while_down")));
    assert!(segs(&conn, run.id).iter().all(|s| s.ended_at_ms.is_some()));
}

#[test]
fn scenario_3_restart_resumes_the_active_run() {
    let dir = TempDir::new("restart");
    let path = dir.0.join("t.sqlite");
    let id;
    {
        let mut conn = crate::db::open(&path).expect("open");
        id = new_plan(&mut conn, "2026-11-05", "20:00", 3, t(0, 0, 0) - DAY);
        tick(&mut conn, t(21, 0, 0));
    }
    let mut reopened = crate::db::open(&path).expect("reopen");
    assert!(lock(&reopened), "the lock survives the restart");
    let r = tick(&mut reopened, t(21, 2, 0));
    assert_eq!((r.state.phase, r.state.set_index), (Phase::Break, 1));
    let run = &runs(&reopened)[0];
    assert_eq!((run.plan_id, run.status), (id, RunStatus::Active));
    assert_eq!(count(&reopened, "plan_runs"), 1, "no duplicate run after a restart");
}

#[test]
fn scenario_4_first_launch_after_the_start_joins_late_and_keeps_both_start_times() {
    let (mut conn, _) = db_with_plan();
    let r = tick(&mut conn, t(20, 30, 0));
    assert_eq!(r.state.phase, Phase::Study);
    let run = &runs(&conn)[0];
    assert_eq!((run.planned_start_ms, run.actual_start_ms), (t(20, 0, 0), Some(t(20, 30, 0))));
    assert_eq!(run.detail_code.as_deref(), Some("joined_late"));
    assert!(segs(&conn, run.id)[0].late_join);
    assert!(lock(&conn));
}

#[test]
fn scenario_4_a_run_nobody_saw_is_missed_once_and_never_locks() {
    let (mut conn, _) = db_with_plan();
    let r = tick(&mut conn, t(23, 30, 0));
    assert!(!r.state.lock_held && !lock(&conn));
    let run = &runs(&conn)[0];
    assert_eq!((run.status, run.actual_start_ms, run.detail_code.as_deref()), (RunStatus::Missed, None, Some("not_started")));
    assert_eq!(count(&conn, "timer_segments"), 0);
    tick(&mut conn, t(23, 31, 0));
    assert_eq!(count(&conn, "plan_runs"), 1, "missed is recorded once");
}

#[test]
fn a_plan_created_after_its_start_is_neither_missed_nor_started() {
    let mut conn = open_in_memory().expect("db");
    new_plan(&mut conn, "2026-11-05", "20:00", 3, t(20, 10, 0));
    tick(&mut conn, t(20, 30, 0));
    tick(&mut conn, t(23, 30, 0));
    assert_eq!(count(&conn, "plan_runs"), 0);
    assert!(!lock(&conn));
}

#[test]
fn scenario_5_a_clock_set_backwards_neither_frees_the_lock_nor_rewinds() {
    let (conn, _) = db_with_plan();
    let db = Mutex::new(conn);
    let hw = HighWater::new(0);
    let mut last = None;
    tick_once(&db, &Utc, &hw, t(20, 30, 0), WakeReason::Tick, &mut last, &FakeNotifier::default()).expect("tick");
    let before = {
        let conn = lock_db(&db).expect("lock");
        (segs(&conn, runs(&conn)[0].id), count(&conn, "timer_segments"))
    };
    let out = testutil_capture(|| {
        tick_once(&db, &Utc, &hw, t(19, 0, 0), WakeReason::Tick, &mut last, &FakeNotifier::default()).expect("tick");
    });
    assert!(out.contains("clock_regression_held"), "logged: {out}");
    let conn = lock_db(&db).expect("lock");
    assert!(lock(&conn), "a past clock must not release the lock");
    assert_eq!((segs(&conn, runs(&conn)[0].id), count(&conn, "timer_segments")), before);
    assert_eq!(last.expect("state").now_ms, t(20, 30, 0), "state is derived at the held time");
}

fn testutil_capture(f: impl FnOnce()) -> String {
    crate::testutil::capture_logs(f)
}

#[test]
fn high_water_never_moves_back_and_honors_the_persisted_floor() {
    let hw = HighWater::new(t(21, 0, 0));
    assert_eq!(hw.observe(t(20, 0, 0)), t(21, 0, 0), "a clock behind the floor is held at the floor");
    assert_eq!(hw.observe(t(22, 0, 0)), t(22, 0, 0));
    assert_eq!(hw.observe(t(21, 30, 0)), t(22, 0, 0));
    assert_eq!(hw.get(), t(22, 0, 0));
}

#[test]
fn heartbeat_floor_comes_from_the_highest_session_and_never_decreases() {
    let conn = open_in_memory().expect("db");
    let a = store::insert_session(&conn, 1_000).expect("a");
    let b = store::insert_session(&conn, 500).expect("b");
    store::touch_heartbeat(&conn, a, 5_000).expect("touch");
    store::touch_heartbeat(&conn, a, 2_000).expect("regressed touch");
    assert_eq!(store::session_last_heartbeat(&conn, a).expect("hb"), 5_000);
    assert_eq!(store::max_heartbeat(&conn).expect("max"), 5_000);
    store::touch_heartbeat(&conn, b, 9_000).expect("touch");
    assert_eq!(store::max_heartbeat(&conn).expect("max"), 9_000);
}

#[test]
fn scenario_6_a_forward_jump_is_allowed_and_only_unexplained_time_counts_as_clock_jump() {
    let (mut conn, _) = db_with_plan();
    tick(&mut conn, t(20, 10, 0));
    tick(&mut conn, t(22, 20, 0));
    let unexplained = clock_jump_intervals(&conn, t(20, 0, 0), t(23, 0, 0)).expect("jump");
    // Block 0 was watched from 20:10 (when the engine joined); blocks 0..=3 then passed unobserved.
    let total: i64 = unexplained.iter().map(|i| i.end - i.start).sum();
    assert_eq!(total, (40 + 15 + 50 + 15) * 60_000, "unobserved stretches: {unexplained:?}");

    // A recorded sleep explains the same stretch: nothing is left over.
    conn.execute(
        "INSERT INTO downtime_records (kind, start_at_ms, end_at_ms, estimated) VALUES ('sleep', ?1, ?2, 0)",
        [t(20, 10, 0), t(22, 20, 0)],
    )
    .expect("sleep");
    assert_eq!(clock_jump_ms(&conn, t(20, 0, 0), t(23, 0, 0)).expect("jump"), 0);
}

#[test]
fn scenario_7_a_run_crossing_midnight_stays_one_active_run() {
    let mut conn = open_in_memory().expect("db");
    // Medium 5 sets = 310 min: 22:00 until 03:10 the next day.
    new_plan(&mut conn, "2026-11-05", "22:00", 5, t(0, 0, 0) - DAY);
    tick(&mut conn, t(22, 5, 0));
    let past_midnight = tick(&mut conn, t(24, 30, 0));
    assert!(past_midnight.state.lock_held);
    assert_eq!(count(&conn, "plan_runs"), 1);
    assert_eq!(runs(&conn)[0].date, "2026-11-05");
    assert_eq!((past_midnight.state.phase, past_midnight.state.set_index), (Phase::Study, 3));
    tick(&mut conn, t(27, 11, 0));
    assert_eq!(runs(&conn)[0].status, RunStatus::Completed);
    assert!(!lock(&conn));
}

#[test]
fn scenario_8_a_plan_deleted_during_a_run_does_not_cut_the_run() {
    let (mut conn, id) = db_with_plan();
    tick(&mut conn, t(20, 10, 0));
    // The lock normally forbids this; the engine must still behave if it happens anyway.
    conn.execute("UPDATE plans SET deleted_at_ms = 1 WHERE id = ?1", [id]).expect("delete");
    let mid = tick(&mut conn, t(21, 0, 0));
    assert!(mid.state.lock_held, "the frozen run keeps going");
    tick(&mut conn, t(23, 0, 1));
    assert_eq!(runs(&conn)[0].status, RunStatus::Completed);
    assert_eq!(count(&conn, "plan_runs"), 1);
    assert!(!lock(&conn));
}

#[test]
fn scenario_9_a_time_zone_change_does_not_move_a_frozen_run() {
    let (mut conn, _) = db_with_plan();
    tick(&mut conn, t(20, 10, 0));
    let run_before = runs(&conn)[0].clone();
    let segments_before = segs(&conn, run_before.id);
    // In New York the same plan would start at a different instant; the run must not follow.
    reconcile(&mut conn, &New_York, t(20, 20, 0)).expect("reconcile in another zone");
    assert_eq!(runs(&conn), [run_before]);
    assert_eq!(segs(&conn, runs(&conn)[0].id), segments_before);
    assert_eq!(count(&conn, "plan_runs"), 1);
    assert!(lock(&conn));
}

#[test]
fn an_overlapping_later_run_starts_when_the_earlier_one_ends_and_the_lock_never_drops() {
    let mut conn = open_in_memory().expect("db");
    new_plan(&mut conn, "2026-11-05", "20:00", 1, t(0, 0, 0) - DAY); // 50 min, ends 20:50
    new_plan(&mut conn, "2026-11-05", "20:55", 1, t(0, 0, 0) - DAY);
    set_medium(&conn, 60, 15); // the first now ends 21:00 and overlaps the second
    tick(&mut conn, t(20, 0, 1));
    tick(&mut conn, t(20, 59, 59));
    let handover = tick(&mut conn, t(21, 0, 1));
    assert!(handover.state.lock_held && lock(&conn));
    let all = runs(&conn);
    assert_eq!(all.len(), 2);
    let second = &all[1];
    assert_eq!((second.planned_start_ms, second.detail_code.as_deref()), (t(21, 0, 0), Some("shifted_by_overlap")));
    let first_seg = &segs(&conn, second.id)[0];
    assert_eq!((first_seg.started_at_ms, first_seg.late_join), (t(21, 0, 0), true));
    assert_eq!(all[0].status, RunStatus::Completed);
}

// ---- AC5: get_timer_state / resumed ----

#[test]
fn state_after_resume_shows_the_current_phase_and_next_plan() {
    let (mut conn, _) = db_with_plan();
    new_plan(&mut conn, "2026-11-06", "09:00", 1, t(0, 0, 0) - DAY);
    tick(&mut conn, t(20, 10, 0));
    let db = Mutex::new(conn);
    let hw = HighWater::new(0);
    let mut last = None;
    let emitted =
        tick_once(&db, &Utc, &hw, t(21, 10, 0), WakeReason::Resumed, &mut last, &FakeNotifier::default()).expect("resume reconcile");
    assert!(matches!(emitted.as_slice(), [Emission::TimerChanged(_), Emission::PlanRechecked(_)]), "{emitted:?}");

    let conn = lock_db(&db).expect("lock");
    let state = current_state(&conn, &Utc, t(21, 10, 0)).expect("state");
    assert_eq!((state.phase, state.set_index, state.sets_total), (Phase::Study, 2, 3));
    assert_eq!(state.planned_end_ms, Some(t(21, 55, 0)));
    let next = state.next.expect("next plan");
    assert_eq!((next.date.as_str(), next.planned_start_ms), ("2026-11-06", t(33, 0, 0)));
    assert!(state.run_id.is_some() && state.segment_id.is_some());
}

#[test]
fn reading_the_state_never_writes() {
    let (conn, _) = db_with_plan();
    let before = (count(&conn, "plan_runs"), count(&conn, "timer_segments"), lock(&conn));
    let state = current_state(&conn, &Utc, t(21, 0, 0)).expect("state");
    assert_eq!((state.phase, state.lock_held), (Phase::Break, true), "projection shows what the engine will write");
    assert!(state.run_id.is_none(), "unwritten rows have no id yet");
    assert_eq!((count(&conn, "plan_runs"), count(&conn, "timer_segments"), lock(&conn)), before);
}

#[test]
fn an_unchanged_tick_emits_nothing_but_a_resume_always_does() {
    let (conn, _) = db_with_plan();
    let db = Mutex::new(conn);
    let hw = HighWater::new(0);
    let mut last = None;
    let first = tick_once(&db, &Utc, &hw, t(20, 10, 0), WakeReason::Tick, &mut last, &FakeNotifier::default()).expect("tick");
    assert!(matches!(first.as_slice(), [Emission::TimerChanged(_)]));
    let quiet = tick_once(&db, &Utc, &hw, t(20, 10, 1), WakeReason::Tick, &mut last, &FakeNotifier::default()).expect("tick");
    assert!(quiet.is_empty(), "{quiet:?}");
    let resumed = tick_once(&db, &Utc, &hw, t(20, 10, 2), WakeReason::Resumed, &mut last, &FakeNotifier::default()).expect("resume");
    assert_eq!(resumed.len(), 2);
}

// ---- AC6 / AC7: fail-closed and immutability ----

#[test]
fn a_missing_lock_row_stops_the_engine_before_it_writes_anything() {
    let (mut conn, _) = db_with_plan();
    conn.execute("DELETE FROM study_state", []).expect("delete");
    assert!(reconcile(&mut conn, &Utc, t(20, 30, 0)).is_err());
    assert!(current_state(&conn, &Utc, t(20, 30, 0)).is_err());
    assert_eq!(count(&conn, "plan_runs"), 0);
    assert_eq!(count(&conn, "timer_segments"), 0);
}

#[test]
fn closed_segments_are_immutable_and_never_deleted() {
    let (mut conn, _) = db_with_plan();
    tick(&mut conn, t(21, 0, 0));
    let all = segs(&conn, runs(&conn)[0].id);
    let (closed, open) = (&all[0], &all[1]);
    assert!(closed.ended_at_ms.is_some() && open.ended_at_ms.is_none());
    assert!(conn.execute("UPDATE timer_segments SET late_join = 1 WHERE id = ?1", [closed.id]).is_err());
    assert!(conn.execute("UPDATE timer_segments SET ended_at_ms = 1 WHERE id = ?1", [closed.id]).is_err());
    assert!(conn.execute("DELETE FROM timer_segments WHERE id = ?1", [closed.id]).is_err());
    assert!(conn.execute("DELETE FROM timer_segments WHERE id = ?1", [open.id]).is_err());
    // Marking the notice on an open segment is the one allowed change.
    conn.execute("UPDATE timer_segments SET notice_sent_at_ms = 5 WHERE id = ?1", [open.id]).expect("notice");
}

#[test]
fn check_constraints_accept_exactly_the_rust_enum_values() {
    let (conn, plan_id) = db_with_plan();
    let run = |status: &str| {
        conn.execute(
            "INSERT INTO plan_runs (plan_id, date, planned_start_ms, planned_end_ms, mode_level, study_min, break_min, sets_total, status)
             VALUES (?1, ?2, 1, 2, 'medium', 50, 15, 1, ?3)",
            rusqlite::params![plan_id, format!("d-{status}"), status],
        )
    };
    for status in RunStatus::ALL {
        run(status.as_str()).unwrap_or_else(|e| panic!("status {} rejected: {e}", status.as_str()));
    }
    assert!(run("bogus").is_err());
    let run_id = conn.last_insert_rowid();

    let segment = |seq: i64, reason: &str| {
        conn.execute(
            "INSERT INTO timer_segments (run_id, seq, kind, planned_start_ms, planned_end_ms, started_at_ms, ended_at_ms, end_reason)
             VALUES (?1, ?2, 'study', 1, 2, 1, 2, ?3)",
            rusqlite::params![run_id, seq, reason],
        )
    };
    for (seq, reason) in (0..).zip(EndReason::ALL) {
        segment(seq, reason.as_str()).unwrap_or_else(|e| panic!("reason {} rejected: {e}", reason.as_str()));
    }
    assert!(segment(99, "bogus").is_err());

    let sql: String = conn
        .query_row("SELECT sql FROM sqlite_master WHERE name = 'plan_runs'", [], |r| r.get(0))
        .expect("schema");
    let status_list = &sql[sql.find("status IN (").expect("list")..];
    assert_eq!(status_list[..status_list.find(')').expect("end")].matches('\'').count() / 2, RunStatus::ALL.len());
    let sql: String = conn
        .query_row("SELECT sql FROM sqlite_master WHERE name = 'timer_segments'", [], |r| r.get(0))
        .expect("schema");
    let reason_list = &sql[sql.find("end_reason IN (").expect("list")..];
    assert_eq!(reason_list[..reason_list.find(')').expect("end")].matches('\'').count() / 2, EndReason::ALL.len());
}

// ---- AC8: release procedures (early release, emergency pass, skip today) ----

fn locked_run() -> (Connection, i64) {
    let (mut conn, plan_id) = db_with_plan();
    tick(&mut conn, t(21, 0, 0));
    assert!(lock(&conn));
    (conn, plan_id)
}

/// Runs the whole challenge: issue, type the exact sentence one minute later, submit.
fn release(conn: &mut Connection, kind: PassKind, at: i64) -> ReleaseOutcome {
    let desk = ReleaseDesk::default();
    let challenge = desk.begin(conn, &Utc, kind, at - 60_000).expect("challenge");
    desk.submit(conn, &Utc, challenge.challenge_id, &challenge.sentence, at).expect("submit")
}

fn typing() -> Typing {
    Typing { challenge_len: 90, typing_ms: 1 }
}

fn passes(conn: &Connection) -> Vec<study_pass::PassRecord> {
    study_pass::list_passes(conn, 100).expect("passes")
}

#[test]
fn the_exact_sentence_releases_early_and_records_it_without_the_text() {
    let (mut conn, _) = locked_run();
    let desk = ReleaseDesk::default();
    let challenge = desk.begin(&conn, &Utc, PassKind::EarlyRelease, t(21, 0, 0)).expect("challenge");
    let outcome = desk.submit(&mut conn, &Utc, challenge.challenge_id, &challenge.sentence, t(21, 1, 0)).expect("submit");
    assert_eq!(outcome, ReleaseOutcome::Released { kind: PassKind::EarlyRelease, expires_at_ms: None });
    assert!(!lock(&conn));
    let run = &runs(&conn)[0];
    assert_eq!(
        (run.status, run.ended_at_ms, run.detail_code.as_deref()),
        (RunStatus::Aborted, Some(t(21, 1, 0)), Some("early_release"))
    );
    let closed = segs(&conn, run.id).into_iter().find(|s| s.end_reason == Some(EndReason::EarlyRelease)).expect("segment");
    assert_eq!(closed.ended_at_ms, Some(t(21, 1, 0)));

    let record = &passes(&conn)[0];
    assert_eq!((record.kind.as_str(), record.run_id, record.date.as_str()), ("early_release", Some(run.id), "2026-11-05"));
    assert_eq!(
        (record.challenge_len, record.typing_ms, record.expires_at_ms, record.penalty_points),
        (i64::try_from(challenge.sentence.chars().count()).expect("len"), 60_000, None, None)
    );
    let dump: String = conn
        .query_row("SELECT group_concat(at_ms || date || kind || challenge_len || typing_ms, '|') FROM study_passes", [], |r| r.get(0))
        .expect("dump");
    assert!(!dump.contains(&challenge.sentence));
    let session = store::insert_session(&conn, 1).expect("session");
    assert!(exit_decision(&mut conn, session, t(21, 2, 0)).expect("exit"));
}

#[test]
fn a_wrong_sentence_changes_nothing_and_counts_attempts() {
    let (mut conn, _) = locked_run();
    let desk = ReleaseDesk::default();
    let challenge = desk.begin(&conn, &Utc, PassKind::EarlyRelease, t(21, 0, 0)).expect("challenge");
    let almost = format!("{} ", challenge.sentence);
    let first = desk.submit(&mut conn, &Utc, challenge.challenge_id, &almost, t(21, 1, 0)).expect("submit");
    let second = desk.submit(&mut conn, &Utc, challenge.challenge_id, "", t(21, 1, 1)).expect("submit");
    assert_eq!((first, second), (ReleaseOutcome::Mismatch { attempts: 1 }, ReleaseOutcome::Mismatch { attempts: 2 }));
    assert!(lock(&conn));
    assert_eq!(runs(&conn)[0].status, RunStatus::Active);
    assert!(passes(&conn).is_empty(), "a rejected release leaves no record");
    // A stale id does not pass either, even with the right text.
    let stale = desk.submit(&mut conn, &Utc, challenge.challenge_id + 1, &challenge.sentence, t(21, 1, 2)).expect("submit");
    assert_eq!(stale, ReleaseOutcome::NoChallenge);
    assert!(lock(&conn));
}

#[test]
fn a_paste_with_different_normalization_or_a_prefix_is_not_a_match() {
    let (mut conn, _) = locked_run();
    let desk = ReleaseDesk::default();
    let challenge = desk.begin(&conn, &Utc, PassKind::EmergencyPass, t(21, 0, 0)).expect("challenge");
    let half: String = challenge.sentence.chars().take(40).collect();
    let decomposed: String = challenge.sentence.replace(' ', "  ");
    for typed in [half, decomposed, challenge.sentence.to_uppercase() + "!"] {
        let outcome = desk.submit(&mut conn, &Utc, challenge.challenge_id, &typed, t(21, 1, 0)).expect("submit");
        assert!(matches!(outcome, ReleaseOutcome::Mismatch { .. }));
    }
    assert!(passes(&conn).is_empty());
}

#[test]
fn a_challenge_is_refused_when_there_is_nothing_to_release() {
    let (conn, _) = db_with_plan();
    let desk = ReleaseDesk::default();
    assert!(desk.begin(&conn, &Utc, PassKind::EarlyRelease, t(10, 0, 0)).is_err());
    assert!(desk.begin(&conn, &Utc, PassKind::EmergencyPass, t(10, 0, 0)).is_err());
    assert!(desk.begin(&conn, &Utc, PassKind::SkipToday, t(10, 0, 0)).is_ok(), "today's plan can be skipped before it starts");
}

#[test]
fn a_challenge_is_single_use() {
    let (mut conn, _) = locked_run();
    let desk = ReleaseDesk::default();
    let challenge = desk.begin(&conn, &Utc, PassKind::EarlyRelease, t(21, 0, 0)).expect("challenge");
    desk.submit(&mut conn, &Utc, challenge.challenge_id, &challenge.sentence, t(21, 1, 0)).expect("submit");
    let again = desk.submit(&mut conn, &Utc, challenge.challenge_id, &challenge.sentence, t(21, 1, 1)).expect("submit");
    assert_eq!(again, ReleaseOutcome::NoChallenge);
    assert_eq!(passes(&conn).len(), 1);
}

#[test]
fn release_works_with_the_engine_thread_dead_and_the_db_lock_poisoned() {
    let (conn, _) = locked_run();
    let db = Arc::new(Mutex::new(conn));
    let held = Arc::clone(&db);
    // The engine panics while holding the connection: this poisons the mutex.
    let crashed = std::thread::spawn(move || {
        let _guard = held.lock().expect("lock");
        panic!("injected engine panic");
    })
    .join();
    assert!(crashed.is_err() && db.is_poisoned());

    let desk = ReleaseDesk::default();
    let challenge = desk.begin(&lock_db(&db).expect("recovered lock"), &Utc, PassKind::EarlyRelease, t(21, 2, 0)).expect("challenge");
    let outcome = {
        let mut conn = lock_db(&db).expect("lock");
        desk.submit(&mut conn, &Utc, challenge.challenge_id, &challenge.sentence, t(21, 3, 0)).expect("submit")
    };
    assert_eq!(outcome, ReleaseOutcome::Released { kind: PassKind::EarlyRelease, expires_at_ms: None });
    assert!(!db.is_poisoned(), "the poison is cleared so the rest of the app works again");
    assert!(!lock(&lock_db(&db).expect("lock")));
}

#[test]
fn an_early_released_run_is_not_resumed_but_the_next_plan_of_the_day_starts_on_time() {
    let (mut conn, _) = locked_run();
    new_plan_unlocked(&mut conn, "2026-11-05", "23:30", 1);
    release(&mut conn, PassKind::EarlyRelease, t(21, 1, 0));

    let later = tick(&mut conn, t(21, 30, 0));
    assert!(!later.state.lock_held && !lock(&conn), "the released run does not come back");
    assert_eq!(runs(&conn)[0].status, RunStatus::Aborted);
    assert_eq!(later.state.next.as_ref().map(|n| n.planned_start_ms), Some(t(23, 30, 0)));

    let next = tick(&mut conn, t(23, 30, 1));
    assert!(next.state.lock_held && lock(&conn));
    assert_eq!(runs(&conn).len(), 2);
}

/// Adds a second plan while the lock is free by briefly clearing it, as the real flow would
/// allow only between runs.
fn new_plan_unlocked(conn: &mut Connection, date: &str, time: &str, sets: i64) {
    set_lock_for_test(conn, false);
    new_plan(conn, date, time, sets, t(0, 0, 0) - DAY);
    set_lock_for_test(conn, true);
}

#[test]
fn early_release_frees_a_stuck_lock_that_has_no_active_run() {
    let mut conn = open_in_memory().expect("db");
    set_lock_for_test(&conn, true);
    assert_eq!(
        study_pass::grant(&mut conn, &Utc, PassKind::EarlyRelease, t(10, 0, 0), typing()).expect("grant"),
        Grant::Granted { expires_at_ms: None }
    );
    assert!(!lock(&conn));
    assert_eq!(
        study_pass::grant(&mut conn, &Utc, PassKind::EarlyRelease, t(10, 0, 1), typing()).expect("grant"),
        Grant::NothingToRelease
    );
    assert_eq!(passes(&conn).len(), 1, "a refused release is not recorded");
}

#[test]
fn a_release_does_nothing_when_the_lock_row_is_missing() {
    let (mut conn, _) = locked_run();
    conn.execute("DELETE FROM study_state", []).expect("delete");
    for kind in PassKind::ALL {
        assert!(study_pass::grant(&mut conn, &Utc, kind, t(21, 1, 0), typing()).is_err());
    }
    assert_eq!(runs(&conn)[0].status, RunStatus::Active, "fail closed: the transaction rolled back");
    assert!(passes(&conn).is_empty());
}

#[test]
fn an_emergency_pass_keeps_the_lock_and_run_and_records_the_penalty() {
    let (mut conn, _) = locked_run();
    let outcome = release(&mut conn, PassKind::EmergencyPass, t(21, 1, 0));
    let expires = t(21, 1, 0) + study_pass::EMERGENCY_PASS_MS;
    assert_eq!(outcome, ReleaseOutcome::Released { kind: PassKind::EmergencyPass, expires_at_ms: Some(expires) });
    assert!(lock(&conn), "a pass pauses enforcement only; the lock stays");
    assert_eq!(runs(&conn)[0].status, RunStatus::Active);
    let record = &passes(&conn)[0];
    assert_eq!(
        (record.kind.as_str(), record.expires_at_ms, record.penalty_points),
        ("emergency_pass", Some(expires), Some(study_pass::EMERGENCY_PASS_PENALTY_POINTS))
    );
    assert_eq!(study_pass::active_until(&conn, expires - 1).expect("active"), Some(expires));
    assert_eq!(study_pass::active_until(&conn, expires).expect("active"), None, "expires exactly at the end");
    let session = store::insert_session(&conn, 1).expect("session");
    assert!(!exit_decision(&mut conn, session, t(21, 2, 0)).expect("exit"), "the gate still refuses during a pass");
}

#[test]
fn the_daily_pass_limit_is_enforced_per_local_day_and_follows_the_setting() {
    let (mut conn, _) = locked_run();
    release(&mut conn, PassKind::EmergencyPass, t(21, 1, 0));
    let after_expiry = t(21, 20, 0);
    let desk = ReleaseDesk::default();
    assert!(desk.begin(&conn, &Utc, PassKind::EmergencyPass, after_expiry).is_err(), "default limit is one a day");
    assert_eq!(
        study_pass::grant(&mut conn, &Utc, PassKind::EmergencyPass, after_expiry, typing()).expect("grant"),
        Grant::LimitReached { limit: 1 }
    );
    assert_eq!(passes(&conn).len(), 1);

    store::put_setting(&conn, "emergencyPassDailyLimit", "release", "2", 1).expect("setting");
    assert!(matches!(
        study_pass::grant(&mut conn, &Utc, PassKind::EmergencyPass, after_expiry, typing()).expect("grant"),
        Grant::Granted { .. }
    ));
    assert_eq!(
        study_pass::grant(&mut conn, &Utc, PassKind::EmergencyPass, after_expiry + 1, typing()).expect("grant"),
        Grant::AlreadyActive { expires_at_ms: after_expiry + study_pass::EMERGENCY_PASS_MS },
        "a running pass cannot be stacked"
    );
    let status = study_pass::status(&conn, &Utc, after_expiry + 1).expect("status");
    assert_eq!((status.daily_limit, status.used_today, status.remaining_today), (2, 2, 0));

    // The next local day starts a fresh allowance.
    let next_day = study_pass::status(&conn, &Utc, t(24, 5, 0)).expect("status");
    assert_eq!((next_day.used_today, next_day.remaining_today, next_day.active_until_ms), (0, 2, None));
}

#[test]
fn skip_today_closes_the_run_and_every_remaining_plan_and_leaves_tomorrow_alone() {
    let (mut conn, _) = locked_run();
    new_plan_unlocked(&mut conn, "2026-11-05", "23:30", 1);
    new_plan_unlocked(&mut conn, "2026-11-06", "20:00", 1);
    let outcome = release(&mut conn, PassKind::SkipToday, t(21, 1, 0));
    assert_eq!(outcome, ReleaseOutcome::Released { kind: PassKind::SkipToday, expires_at_ms: None });
    assert!(!lock(&conn));

    let all = runs(&conn);
    assert_eq!(all.len(), 2, "tomorrow's plan has no run yet");
    assert!(all.iter().all(|r| r.status == RunStatus::Skipped), "{all:?}");
    assert!(all.iter().all(|r| r.date == "2026-11-05"));
    let first = &all[0];
    assert!(segs(&conn, first.id).iter().any(|s| s.end_reason == Some(EndReason::Skipped)));

    let evening = tick(&mut conn, t(23, 30, 5));
    assert!(!evening.state.lock_held && !lock(&conn), "the skipped 23:30 plan never starts");
    assert_eq!(runs(&conn).len(), 2);
    assert_eq!(evening.state.next.as_ref().map(|n| n.date.as_str()), Some("2026-11-06"));

    let tomorrow = tick(&mut conn, t(44, 0, 5));
    assert!(tomorrow.state.lock_held && lock(&conn), "the next day's plan runs normally");
    let session = store::insert_session(&conn, 1).expect("session");
    assert!(!exit_decision(&mut conn, session, t(44, 1, 0)).expect("exit"));
    assert_eq!(passes(&conn)[0].kind, "skip_today");
}

#[test]
fn skip_today_before_the_first_plan_works_once_and_a_second_skip_has_nothing_to_do() {
    let (mut conn, _) = db_with_plan();
    let outcome = release(&mut conn, PassKind::SkipToday, t(10, 0, 0));
    assert!(matches!(outcome, ReleaseOutcome::Released { kind: PassKind::SkipToday, .. }));
    assert_eq!(runs(&conn)[0].status, RunStatus::Skipped);
    assert!(!tick(&mut conn, t(20, 0, 5)).state.lock_held, "the skipped plan does not lock at its time");
    assert_eq!(
        study_pass::grant(&mut conn, &Utc, PassKind::SkipToday, t(10, 0, 1), typing()).expect("grant"),
        Grant::NothingToRelease
    );
    assert_eq!(passes(&conn).len(), 1);
}

#[test]
fn study_passes_are_append_only_and_the_kind_list_matches_the_rust_enum() {
    let (mut conn, _) = locked_run();
    release(&mut conn, PassKind::EarlyRelease, t(21, 1, 0));
    assert!(conn.execute("UPDATE study_passes SET typing_ms = 0", []).is_err());
    assert!(conn.execute("DELETE FROM study_passes", []).is_err());
    for kind in PassKind::ALL {
        let expires = (kind == PassKind::EmergencyPass).then_some(5);
        conn.execute(
            "INSERT INTO study_passes (at_ms, date, kind, expires_at_ms, challenge_len, typing_ms) VALUES (1, 'd', ?1, ?2, 80, 0)",
            rusqlite::params![kind.as_str(), expires],
        )
        .unwrap_or_else(|e| panic!("{} rejected: {e}", kind.as_str()));
    }
    assert!(conn
        .execute("INSERT INTO study_passes (at_ms, date, kind, challenge_len, typing_ms) VALUES (1, 'd', 'surprise', 80, 0)", [])
        .is_err());
    assert!(
        conn.execute("INSERT INTO study_passes (at_ms, date, kind, challenge_len, typing_ms) VALUES (1, 'd', 'emergency_pass', 80, 0)", [])
            .is_err(),
        "an emergency pass without an expiry is invalid"
    );
}

#[test]
fn every_sentence_is_long_enough() {
    for sentence in release::sentences() {
        assert!(sentence.chars().count() >= release::MIN_SENTENCE_CHARS, "too short: {sentence}");
    }
    let distinct: std::collections::HashSet<_> = release::sentences().iter().collect();
    assert_eq!(distinct.len(), release::sentences().len());
}

#[test]
fn logs_carry_no_typed_text_plan_names_or_subjects() {
    let (mut conn, _) = locked_run();
    let desk = ReleaseDesk::default();
    let challenge = desk.begin(&conn, &Utc, PassKind::EarlyRelease, t(21, 0, 0)).expect("challenge");
    let wrong = "비밀 입력 내용 XYZ";
    let logs = crate::testutil::capture_logs(|| {
        desk.submit(&mut conn, &Utc, challenge.challenge_id, wrong, t(21, 1, 0)).expect("wrong");
        desk.submit(&mut conn, &Utc, challenge.challenge_id, &challenge.sentence, t(21, 1, 1)).expect("right");
    });
    assert!(logs.contains("release_rejected") && logs.contains("pass_granted"), "logged: {logs}");
    for forbidden in [wrong, challenge.sentence.as_str(), "수학"] {
        assert!(!logs.contains(forbidden), "log leaked `{forbidden}`: {logs}");
    }
}

// ---- AC9: supervision ----

#[test]
fn a_panicking_tick_is_caught_and_the_next_one_runs() {
    let health = EngineHealth::default();
    let first: Supervised<i32> = health.run(|| panic!("injected tick panic"));
    assert_eq!(first, Supervised::Failed { just_degraded: false });
    assert_eq!(health.snapshot().restarts, 1);
    let second = health.run(|| Ok(7));
    assert_eq!(second, Supervised::Done { value: 7, recovered: false });
    assert_eq!(health.snapshot().consecutive_failures, 0);
}

#[test]
fn five_consecutive_failures_degrade_once_and_recovery_is_reported() {
    let health = EngineHealth::default();
    let mut degraded_signals = 0;
    for i in 0..DEGRADED_AFTER + 3 {
        let result: Supervised<()> = if i % 2 == 0 {
            health.run(|| panic!("injected"))
        } else {
            health.run(|| Err(crate::error::AppError::Internal("injected error".into())))
        };
        if matches!(result, Supervised::Failed { just_degraded: true }) {
            degraded_signals += 1;
        }
    }
    assert_eq!(degraded_signals, 1, "degraded is announced once, not on every failure");
    assert!(health.snapshot().degraded);
    let recovered = health.run(|| Ok(()));
    assert_eq!(recovered, Supervised::Done { value: (), recovered: true });
    assert!(!health.snapshot().degraded);
}

#[test]
fn a_tick_that_panics_while_holding_the_db_does_not_stop_later_ticks() {
    let (conn, _) = db_with_plan();
    let db = Mutex::new(conn);
    let hw = HighWater::new(0);
    let health = EngineHealth::default();
    let mut last = None;
    let crashed: Supervised<()> = health.run(|| {
        let _guard = db.lock().expect("lock");
        panic!("injected panic inside a tick");
    });
    assert!(matches!(crashed, Supervised::Failed { .. }));
    let next = health.run(|| tick_once(&db, &Utc, &hw, t(20, 30, 0), WakeReason::Tick, &mut last, &FakeNotifier::default()));
    assert!(matches!(next, Supervised::Done { .. }), "{next:?}");
    assert!(lock(&lock_db(&db).expect("lock")));
}

// ---- AC10: exit gate ----

#[test]
fn exit_is_refused_during_a_break_and_allowed_after_the_run() {
    let (mut conn, _) = db_with_plan();
    let session = store::insert_session(&conn, 1).expect("session");
    let r = tick(&mut conn, t(20, 55, 0));
    assert_eq!(r.state.phase, Phase::Break);
    assert!(!exit_decision(&mut conn, session, t(20, 55, 1)).expect("exit"));
    let latest = &crate::settings::read_history(&conn, 1).expect("history")[0];
    assert_eq!((latest.key.as_str(), latest.outcome.as_str()), ("appExit", "rejected_locked"));
    tick(&mut conn, t(23, 0, 1));
    assert!(exit_decision(&mut conn, session, t(23, 0, 2)).expect("exit"));
}

// ---- AC11: accelerated end-to-end on a file database ----

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("study-manager-timer-{tag}-{}", crate::clock::now_ms().expect("clock")));
        std::fs::create_dir_all(&dir).expect("dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn two_minute_plan_runs_start_to_finish_across_a_restart() {
    let dir = TempDir::new("e2e");
    let path = dir.0.join("e2e.sqlite");
    let mut conn = crate::db::open(&path).expect("open");
    set_medium(&conn, 2, 2);
    new_plan(&mut conn, "2026-11-05", "20:00", 2, t(0, 0, 0) - DAY); // 2 + 2 + 2 = 6 minutes

    let mut locked_during = Vec::new();
    for second in (-10..=380).step_by(5) {
        let now = t(20, 0, 0) + second * 1000;
        if second == 150 {
            // Mid-break restart.
            drop(conn);
            conn = crate::db::open(&path).expect("reopen");
        }
        tick(&mut conn, now);
        locked_during.push((second, lock(&conn)));
    }
    for (second, held) in locked_during {
        let expected = (0..360).contains(&second);
        assert_eq!(held, expected, "lock at +{second}s");
    }
    let run = &runs(&conn)[0];
    assert_eq!((run.status, run.ended_at_ms, run.actual_start_ms), (RunStatus::Completed, Some(t(20, 6, 0)), Some(t(20, 0, 0))));
    let all = segs(&conn, run.id);
    let shape: Vec<(BlockKind, i64, Option<EndReason>)> = all.iter().map(|s| (s.kind, s.started_at_ms, s.end_reason)).collect();
    assert_eq!(
        shape,
        [
            (BlockKind::Study, t(20, 0, 0), Some(EndReason::Elapsed)),
            (BlockKind::Break, t(20, 2, 0), Some(EndReason::Elapsed)),
            (BlockKind::Study, t(20, 4, 0), Some(EndReason::Elapsed)),
        ]
    );
    assert!(all.iter().all(|s| !s.late_join), "a restart inside a block is not a late join");
    assert_eq!(count(&conn, "plan_runs"), 1);
}

#[test]
fn late_tolerance_separates_tick_jitter_from_missed_boundaries() {
    let (mut conn, _) = db_with_plan();
    tick(&mut conn, t(20, 0, 0));
    // Within tolerance the boundary counts as seen live.
    tick(&mut conn, t(20, 50, 0) + LATE_TOLERANCE_MS);
    let first = &segs(&conn, runs(&conn)[0].id)[0];
    assert_eq!(first.end_reason, Some(EndReason::Elapsed));
    // One millisecond past it, the next boundary counts as unobserved.
    tick(&mut conn, t(21, 5, 0) + LATE_TOLERANCE_MS + 1);
    let second = &segs(&conn, runs(&conn)[0].id)[1];
    assert_eq!(second.end_reason, Some(EndReason::ElapsedWhileDown));
}

// ---- 2-S5: break notice, set/run summaries ----

const MIN: i64 = 60_000;

fn emitted_notices(out: &[Emission]) -> usize {
    out.iter().filter(|e| matches!(e, Emission::BreakEndingSoon(_))).count()
}

#[test]
fn break_notice_is_sent_once_at_one_minute_left_and_not_again_after_a_restart() {
    let (conn, _) = db_with_plan();
    set_medium(&conn, 10, 4);
    let db = Mutex::new(conn);
    let hw = HighWater::new(0);
    let notifier = FakeNotifier::default();
    let mut last = None;
    let run_tick = |now: i64, last: &mut Option<_>| {
        tick_once(&db, &Utc, &hw, now, WakeReason::Tick, last, &notifier).expect("tick")
    };

    // Break is 20:10-20:14; one minute before the end is 20:13:00.
    for now in [t(20, 10, 1), t(20, 12, 59)] {
        assert_eq!(emitted_notices(&run_tick(now, &mut last)), 0, "too early at {now}");
    }
    assert!(notifier.sent().is_empty());

    let due = run_tick(t(20, 13, 0), &mut last);
    let soon = due.iter().find_map(|e| match e {
        Emission::BreakEndingSoon(s) => Some(s.clone()),
        _ => None,
    });
    let soon = soon.expect("event at exactly one minute left");
    assert_eq!((soon.remaining_ms, soon.planned_end_ms), (MIN, t(20, 14, 0)));
    assert_eq!(notifier.sent().len(), 1);
    // The text names no subject: it can show on a lock screen.
    let (title, body) = &notifier.sent()[0];
    assert!(!title.contains("수학") && !body.contains("수학"), "{title} {body}");

    assert_eq!(emitted_notices(&run_tick(t(20, 13, 1), &mut last)), 0);
    // A restart loses `last`; the sent mark in the database still prevents a repeat.
    let mut restarted = None;
    assert_eq!(emitted_notices(&run_tick(t(20, 13, 30), &mut restarted)), 0);
    assert_eq!(emitted_notices(&run_tick(t(20, 14, 1), &mut restarted)), 0, "never after the break ended");
    assert_eq!(notifier.sent().len(), 1);
}

#[test]
fn a_break_of_one_minute_or_less_sends_no_notice() {
    let (conn, _) = db_with_plan();
    set_medium(&conn, 10, 1);
    let db = Mutex::new(conn);
    let hw = HighWater::new(0);
    let notifier = FakeNotifier::default();
    let mut last = None;
    for now in [t(20, 10, 1), t(20, 10, 30), t(20, 10, 59)] {
        let out = tick_once(&db, &Utc, &hw, now, WakeReason::Tick, &mut last, &notifier).expect("tick");
        assert_eq!(emitted_notices(&out), 0, "{now}");
    }
    assert!(notifier.sent().is_empty());
}

#[test]
fn notice_rule_edges() {
    let (mut conn, _) = db_with_plan();
    set_medium(&conn, 10, 4);
    tick(&mut conn, t(20, 10, 1));
    let state = current_state(&conn, &Utc, t(20, 10, 1)).expect("state");
    let seg = store::get_segment(&conn, state.segment_id.expect("segment")).expect("get").expect("row");
    let end = seg.planned_end_ms;
    assert!(!notice::is_due(&seg, end - MIN - 1));
    assert!(notice::is_due(&seg, end - MIN));
    assert!(notice::is_due(&seg, end - 1));
    assert!(!notice::is_due(&seg, end), "nothing at or after the end");
    assert!(!notice::is_due(&SegmentRow { notice_sent_at_ms: Some(1), ..seg.clone() }, end - 5));
    assert!(!notice::is_due(&SegmentRow { kind: BlockKind::Study, ..seg }, end - 5));
}

#[test]
fn set_and_run_summaries_exclude_downtime_and_count_block_attempts() {
    use crate::enforce::model::{BlockAction as A, EnforceMode as M, NewBlockEvent, TargetKind as K};
    let (conn, _) = db_with_plan();
    set_medium(&conn, 10, 5);
    // Sleep from 20:02 to 20:05 lies inside the first study block (20:00-20:10).
    store::insert_downtime(&conn, DowntimeKind::Sleep, Interval { start: t(20, 2, 0), end: t(20, 5, 0) }, false)
        .expect("downtime");
    let db = Mutex::new(conn);
    let hw = HighWater::new(0);
    let notifier = FakeNotifier::default();
    let mut last = None;
    let mut run_tick = |now: i64| {
        tick_once(&db, &Utc, &hw, now, WakeReason::Tick, &mut last, &notifier).expect("tick")
    };
    // Blocks: study 20:00-10, break -15, study -25, break -30, study -40.
    assert!(run_tick(t(20, 0, 1)).iter().all(|e| matches!(e, Emission::TimerChanged(_))));
    let record = |study_nth: usize, specs: &[(K, &str, A, M)]| {
        let conn = db.lock().expect("lock");
        let run_id = runs(&conn)[0].id;
        let study: Vec<i64> =
            segs(&conn, run_id).iter().filter(|s| s.kind == BlockKind::Study).map(|s| s.id).collect();
        for (kind, id, action, mode) in specs {
            let event = NewBlockEvent {
                at_ms: t(20, 3, 0),
                run_id: Some(run_id),
                segment_id: Some(study[study_nth]),
                target_kind: *kind,
                target_id: id,
                action: *action,
                mode: *mode,
                detail_code: None,
            };
            store::insert_block_event(&conn, &event).expect("event");
        }
    };
    // Set 1: three attempts (both modes, app and site); everything else is not an attempt.
    let mut first_set = vec![
        (K::App, "a", A::TerminateRequested, M::Enforce),
        (K::App, "b", A::WouldTerminate, M::DryRun),
        (K::Site, "x.example", A::SiteBlocked, M::Enforce),
        (K::Extension, "chrome", A::ExtensionDisconnected, M::Enforce),
        (K::Extension, "chrome", A::ExtensionConnected, M::Enforce),
    ];
    for action in [A::SkippedProtected, A::SkippedPass, A::Terminated, A::ForceKilled, A::Failed, A::SiteAllowedPass] {
        first_set.push((K::App, "c", action, M::Enforce));
    }
    record(0, &first_set);
    {
        // Another run's attempt is never counted.
        let conn = db.lock().expect("lock");
        let run_id = runs(&conn)[0].id;
        let event = NewBlockEvent {
            at_ms: t(20, 3, 0),
            run_id: Some(run_id + 100),
            segment_id: None,
            target_kind: K::App,
            target_id: "a",
            action: A::WouldTerminate,
            mode: M::DryRun,
            detail_code: None,
        };
        store::insert_block_event(&conn, &event).expect("event");
    }

    let first = run_tick(t(20, 10, 1));
    let set = first.iter().find_map(|e| match e {
        Emission::SetSummary(s) => Some(s.clone()),
        _ => None,
    });
    let set = set.expect("set summary when the study block ends");
    assert_eq!((set.set_index, set.set_studied_ms, set.today_studied_ms), (1, 7 * MIN, 7 * MIN));
    assert_eq!(set.attempt_count, 3);
    assert!(!first.iter().any(|e| matches!(e, Emission::RunSummary(_))));
    let sent = notifier.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].1.contains("7분"), "{sent:?}");

    run_tick(t(20, 15, 1));
    record(1, &[(K::App, "a", A::WouldTerminate, M::DryRun)]);
    let second = run_tick(t(20, 25, 1));
    assert!(second.iter().any(|e| matches!(e, Emission::SetSummary(s) if s.set_index == 2 && s.set_studied_ms == 10 * MIN && s.today_studied_ms == 17 * MIN && s.attempt_count == 1)));

    let end = run_tick(t(20, 40, 1));
    let run = end.iter().find_map(|e| match e {
        Emission::RunSummary(r) => Some(r.clone()),
        _ => None,
    });
    let run = run.expect("run summary when the run completes");
    assert_eq!((run.sets_total, run.run_studied_ms, run.today_studied_ms), (3, 27 * MIN, 27 * MIN));
    assert_eq!((run.status.as_str(), run.attempt_count), ("completed", 4));
    assert!(end.iter().any(|e| matches!(e, Emission::SetSummary(s) if s.set_index == 3 && s.attempt_count == 0)));
}

#[test]
fn studied_time_counts_overlapping_downtime_once() {
    let seg = SegmentRow {
        id: 1,
        run_id: 1,
        seq: 1,
        kind: BlockKind::Study,
        planned_start_ms: 0,
        planned_end_ms: 100,
        started_at_ms: 0,
        ended_at_ms: Some(100),
        end_reason: Some(EndReason::Elapsed),
        late_join: false,
        notice_sent_at_ms: None,
    };
    let down = |start, end| Interval { start, end };
    // (downtime rows, expected studied)
    let table: [(&[Interval], i64); 5] = [
        (&[], 100),
        (&[down(10, 30)], 80),
        (&[down(10, 30), down(20, 40)], 70),
        (&[down(-50, 20), down(90, 500)], 70),
        (&[down(0, 100)], 0),
    ];
    for (rows, expected) in table {
        assert_eq!(summary::studied_ms(&seg, rows), expected, "{rows:?}");
    }
    assert_eq!(summary::studied_ms(&SegmentRow { ended_at_ms: None, end_reason: None, ..seg }, &[]), 0);
}

#[test]
fn an_aborted_study_block_is_summarized_with_its_partial_time() {
    let (conn, _) = db_with_plan();
    let db = Mutex::new(conn);
    let hw = HighWater::new(0);
    let notifier = FakeNotifier::default();
    let mut last = None;
    tick_once(&db, &Utc, &hw, t(20, 0, 1), WakeReason::Tick, &mut last, &notifier).expect("tick");
    {
        let mut conn = lock_db(&db).expect("lock");
        release(&mut conn, PassKind::EarlyRelease, t(20, 30, 0));
    }
    let out = tick_once(&db, &Utc, &hw, t(20, 30, 1), WakeReason::Tick, &mut last, &notifier).expect("tick");
    assert!(out.iter().any(|e| matches!(e, Emission::SetSummary(s) if s.set_studied_ms == 30 * MIN)), "{out:?}");
    assert!(out.iter().any(|e| matches!(e, Emission::RunSummary(r) if r.status == "aborted")), "{out:?}");
}
