//! Point tests run on a fake clock: every function takes the time as a number, and a scenario is
//! a table of minute buckets written straight into the database.

use chrono::{TimeZone, Utc};
use rusqlite::Connection;

use super::rewards::{self, LEVEL_THRESHOLDS, Stage};
use super::*;
use crate::db::open_in_memory;
use crate::enforce::model::{BlockAction, EnforceMode, NewBlockEvent, TargetKind};
use crate::plan::{BlockKind, CreatePlanInput, create_plan};
use crate::timer::model::{EndReason, NewRun, NewSegment};

fn t(h: i64, m: i64, s: i64) -> i64 {
    let base = Utc
        .with_ymd_and_hms(2026, 11, 5, 0, 0, 0)
        .single()
        .expect("date")
        .timestamp_millis();
    base + (h * 3600 + m * 60 + s) * 1000
}

fn bucket(
    minute_start_ms: i64,
    credited_ms: i64,
    away_ms: i64,
    idle_ms: i64,
) -> (i64, MinuteDelta) {
    (
        minute_start_ms,
        MinuteDelta {
            minute_start_ms,
            credited_ms,
            away_ms,
            idle_ms,
        },
    )
}

/// `count` clean minutes starting at `from_minute` after 20:00.
fn clean(from_minute: i64, count: i64) -> Vec<(i64, MinuteDelta)> {
    (from_minute..from_minute + count)
        .map(|m| bucket(t(20, 0, 0) + m * 60_000, 60_000, 0, 0))
        .collect()
}

// ---- pure window splitting ----

#[test]
fn thirty_clean_minutes_make_one_window_and_twenty_nine_make_none() {
    let now = t(23, 0, 0);
    assert_eq!(windows(&clean(0, 29), &[], now), vec![]);
    assert_eq!(
        windows(&clean(0, 30), &[], now),
        vec![Window {
            index: 0,
            end_ms: t(20, 30, 0),
            clean: true
        }]
    );
    let two = windows(&clean(0, 61), &[], now);
    assert_eq!(
        two.iter().map(|w| (w.index, w.end_ms)).collect::<Vec<_>>(),
        vec![(0, t(20, 30, 0)), (1, t(21, 0, 0))]
    );
}

#[test]
fn a_minute_still_being_written_does_not_close_a_window() {
    let minutes = clean(0, 30);
    // The 30th minute (20:29) ends at 20:30:00.
    assert_eq!(windows(&minutes, &[], t(20, 29, 59)), vec![]);
    assert_eq!(windows(&minutes, &[], t(20, 30, 0)).len(), 1);
}

#[test]
fn away_idle_or_an_attempt_in_any_minute_makes_the_window_unpaid_but_the_next_one_starts_fresh() {
    let now = t(23, 0, 0);
    for (label, spoil) in [("away", (0, 1_000, 0)), ("idle", (0, 0, 1_000))] {
        let mut minutes = clean(0, 60);
        minutes[12] = bucket(minutes[12].0, 59_000, spoil.1, spoil.2);
        // The lost second makes this window need one more minute; the later minutes shift the end.
        let got = windows(&minutes, &[], now);
        assert!(!got[0].clean, "{label}");
        assert!(
            got.iter().skip(1).all(|w| w.clean),
            "{label}: later windows are judged on their own"
        );
    }
    let attempt = t(20, 10, 30);
    let got = windows(&clean(0, 60), &[attempt], now);
    assert_eq!(
        got.iter().map(|w| w.clean).collect::<Vec<_>>(),
        vec![false, true]
    );
    // An attempt exactly at the start of the first minute counts; one at the end of the last minute does not.
    assert!(!windows(&clean(0, 30), &[t(20, 0, 0)], now)[0].clean);
    assert!(windows(&clean(0, 30), &[t(20, 30, 0)], now)[0].clean);
}

#[test]
fn recognized_time_carries_across_a_break_between_sets() {
    // 25 minutes, a 5 minute break (no buckets), 5 more minutes: the window closes at the 30th recognized minute.
    let mut minutes = clean(0, 25);
    minutes.extend(clean(30, 5));
    let got = windows(&minutes, &[], t(23, 0, 0));
    assert_eq!(
        got,
        vec![Window {
            index: 0,
            end_ms: t(20, 35, 0),
            clean: true
        }]
    );
}

// ---- level curve ----

#[test]
fn level_boundaries_hold_at_n_minus_1_and_n_for_every_threshold() {
    assert_eq!(rewards::level_for(0), 1);
    assert_eq!(rewards::level_for(-50), 1);
    for (index, needed) in LEVEL_THRESHOLDS.iter().enumerate().skip(1) {
        let level = i64::try_from(index).expect("small") + 1;
        assert_eq!(
            rewards::level_for(needed - 1),
            level - 1,
            "{} points",
            needed - 1
        );
        assert_eq!(rewards::level_for(*needed), level, "{needed} points");
    }
    let top = LEVEL_THRESHOLDS[LEVEL_THRESHOLDS.len() - 1];
    assert_eq!(
        rewards::level_for(top * 100),
        i64::try_from(LEVEL_THRESHOLDS.len()).expect("small")
    );
}

#[test]
fn thresholds_rise_strictly_and_early_levels_come_faster_than_later_ones() {
    let gaps: Vec<i64> = LEVEL_THRESHOLDS.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps.iter().all(|g| *g > 0));
    assert!(
        gaps.windows(2).all(|g| g[0] <= g[1]),
        "gaps never shrink: {gaps:?}"
    );
    // The very first level-up is one reward window.
    assert_eq!(LEVEL_THRESHOLDS[1], rewards::WINDOW_POINTS);
}

#[test]
fn growth_stages_change_exactly_at_their_first_level() {
    let table = [
        (1, Stage::Seed, "씨앗"),
        (2, Stage::Seed, "씨앗"),
        (3, Stage::Sprout, "새싹"),
        (5, Stage::Sprout, "새싹"),
        (6, Stage::SmallTree, "작은 나무"),
        (8, Stage::SmallTree, "작은 나무"),
        (9, Stage::ForestSpirit, "숲의 정령"),
        (10, Stage::ForestSpirit, "숲의 정령"),
    ];
    for (level, stage, label) in table {
        assert_eq!(rewards::stage_for_level(level), stage, "level {level}");
        assert_eq!(stage.label(), label);
    }
    assert_eq!(rewards::next_level_at(1), Some(LEVEL_THRESHOLDS[1]));
    assert_eq!(rewards::next_level_at(10), None);
    let status = status_for(10, 140);
    assert_eq!(
        (status.level, status.stage, status.next_level_at),
        (5, Stage::Sprout, Some(200))
    );
}

// ---- ledger and settlement ----

struct Rig {
    conn: Connection,
    plan_id: i64,
}

fn rig() -> Rig {
    let mut conn = open_in_memory().expect("db");
    create_plan(
        &mut conn,
        &Utc,
        CreatePlanInput {
            name: "수학".into(),
            subject: "수학".into(),
            start_date: "2026-11-05".into(),
            start_time: "20:00".into(),
            sets: 4,
            repeat: None,
        },
        t(0, 0, 0) - 86_400_000,
    )
    .expect("plan");
    let plan_id = store::list_live_plans(&conn).expect("plans")[0].id;
    Rig { conn, plan_id }
}

impl Rig {
    /// A run planned for 20:00; returns `(run id, segment id)`.
    fn run(
        &self,
        date: &str,
        actual_start_ms: Option<i64>,
        status: RunStatus,
        ended_at_ms: Option<i64>,
    ) -> (i64, i64) {
        let run = store::insert_run(
            &self.conn,
            &NewRun {
                plan_id: self.plan_id,
                date: date.into(),
                planned_start_ms: t(20, 0, 0),
                planned_end_ms: t(22, 0, 0),
                actual_start_ms,
                mode_level: "medium".into(),
                study_min: 50,
                break_min: 10,
                long_break_min: None,
                sets_before_long_break: None,
                sets_total: 4,
                status,
                ended_at_ms,
                detail_code: None,
            },
        )
        .expect("run");
        let seg = store::insert_segment(
            &self.conn,
            run,
            &NewSegment {
                seq: 0,
                kind: BlockKind::Study,
                planned_start_ms: t(20, 0, 0),
                planned_end_ms: t(22, 0, 0),
                started_at_ms: t(20, 0, 0),
                ended_at_ms: None,
                end_reason: None::<EndReason>,
                late_join: false,
            },
        )
        .expect("segment");
        (run, seg)
    }

    fn minutes(&self, segment: i64, minutes: &[(i64, MinuteDelta)]) {
        for (_, delta) in minutes {
            store::add_focus_minute(&self.conn, segment, delta).expect("minute");
        }
    }

    fn attempt(&self, run: i64, segment: i64, at_ms: i64) {
        store::insert_block_event(
            &self.conn,
            &NewBlockEvent {
                at_ms,
                run_id: Some(run),
                segment_id: Some(segment),
                target_kind: TargetKind::App,
                target_id: "com.example.game",
                action: BlockAction::TerminateRequested,
                mode: EnforceMode::Enforce,
                detail_code: None,
            },
        )
        .expect("event");
    }

    fn pass(&self, at_ms: i64, penalty: Option<i64>) {
        self.conn
            .execute(
                "INSERT INTO study_passes (at_ms, date, kind, expires_at_ms, challenge_len, typing_ms, penalty_points)
                 VALUES (?1, '2026-11-05', 'emergency_pass', ?2, 10, 5000, ?3)",
                rusqlite::params![at_ms, at_ms + 600_000, penalty],
            )
            .expect("pass");
    }

    fn ledger(&self) -> Vec<(String, i64)> {
        let mut rows: Vec<(String, i64)> = store::list_ledger(&self.conn, 1000)
            .expect("ledger")
            .into_iter()
            .map(|e| (e.ref_key, e.delta))
            .collect();
        rows.sort();
        rows
    }
}

#[test]
fn a_clean_hour_pays_two_windows_and_the_start_bonus_exactly_once_however_often_it_is_settled() {
    let mut rig = rig();
    let (_, seg) = rig.run("2026-11-05", Some(t(20, 0, 0)), RunStatus::Active, None);
    rig.minutes(seg, &clean(0, 61));
    assert_eq!(settle(&mut rig.conn, t(21, 5, 0)).expect("settle"), 3);
    assert_eq!(settle(&mut rig.conn, t(21, 5, 0)).expect("again"), 0);
    assert_eq!(settle(&mut rig.conn, t(23, 0, 0)).expect("later"), 0);
    let status = status(&rig.conn).expect("status");
    assert_eq!(
        (status.balance, status.earned),
        (2 * rewards::WINDOW_POINTS + rewards::START_BONUS_POINTS, 50)
    );
    assert_eq!(status.level, 3);
    let keys: Vec<String> = rig.ledger().into_iter().map(|(k, _)| k).collect();
    assert_eq!(
        keys,
        vec!["focus30:run:1:win:0", "focus30:run:1:win:1", "start:run:1"]
    );
}

#[test]
fn a_window_with_an_attempt_pays_nothing_and_the_next_clean_window_still_pays() {
    let mut rig = rig();
    let (run, seg) = rig.run("2026-11-05", None, RunStatus::Active, None);
    rig.minutes(seg, &clean(0, 60));
    rig.attempt(run, seg, t(20, 12, 0));
    settle(&mut rig.conn, t(22, 0, 0)).expect("settle");
    assert_eq!(
        rig.ledger(),
        vec![("focus30:run:1:win:1".to_string(), rewards::WINDOW_POINTS)]
    );
}

#[test]
fn the_start_bonus_needs_a_start_within_the_tolerance() {
    let mut rig = rig();
    let on_time = t(20, 0, 0) + rewards::START_TOLERANCE_MS;
    rig.run(
        "2026-11-05",
        Some(on_time),
        RunStatus::Completed,
        Some(t(21, 0, 0)),
    );
    rig.run(
        "2026-11-06",
        Some(on_time + 1),
        RunStatus::Completed,
        Some(t(21, 0, 0)),
    );
    rig.run("2026-11-07", None, RunStatus::Missed, Some(t(22, 0, 0)));
    settle(&mut rig.conn, t(23, 0, 0)).expect("settle");
    assert_eq!(
        rig.ledger(),
        vec![("start:run:1".to_string(), rewards::START_BONUS_POINTS)]
    );
}

#[test]
fn emergency_passes_and_aborted_runs_are_deducted_once_and_do_not_lower_the_level() {
    let mut rig = rig();
    let (_, seg) = rig.run(
        "2026-11-05",
        Some(t(20, 0, 0)),
        RunStatus::Aborted,
        Some(t(21, 0, 0)),
    );
    rig.minutes(seg, &clean(0, 60));
    rig.pass(t(20, 40, 0), Some(rewards::EMERGENCY_PASS_PENALTY));
    rig.pass(t(20, 50, 0), None); // other releases carry no penalty
    settle(&mut rig.conn, t(22, 0, 0)).expect("settle");
    settle(&mut rig.conn, t(22, 0, 0)).expect("again");
    let status = status(&rig.conn).expect("status");
    let earned = 2 * rewards::WINDOW_POINTS + rewards::START_BONUS_POINTS;
    assert_eq!(status.earned, earned);
    assert_eq!(
        status.balance,
        earned - rewards::EMERGENCY_PASS_PENALTY - rewards::ABORT_PENALTY
    );
    assert_eq!(status.level, rewards::level_for(earned));
    let keys: Vec<String> = rig.ledger().into_iter().map(|(k, _)| k).collect();
    assert!(keys.contains(&"pass:1".to_string()) && keys.contains(&"abort:run:1".to_string()));
    assert!(!keys.contains(&"pass:2".to_string()));
}

#[test]
fn the_balance_equals_the_ledger_sum_and_a_rebuild_from_the_same_data_gives_the_same_ledger() {
    let scenario = |rig: &Rig| {
        let (run, seg) = rig.run(
            "2026-11-05",
            Some(t(20, 2, 0)),
            RunStatus::Aborted,
            Some(t(21, 30, 0)),
        );
        rig.minutes(seg, &clean(0, 90));
        rig.attempt(run, seg, t(20, 45, 0));
        rig.pass(t(20, 20, 0), Some(rewards::EMERGENCY_PASS_PENALTY));
    };
    let mut one_shot = rig();
    scenario(&one_shot);
    settle(&mut one_shot.conn, t(23, 0, 0)).expect("settle");

    // Same data, settled in pieces while it was still arriving, then again after a restart.
    let mut piecemeal = rig();
    scenario(&piecemeal);
    for now in [
        t(20, 10, 0),
        t(20, 31, 0),
        t(20, 31, 0),
        t(21, 5, 0),
        t(23, 0, 0),
        t(23, 0, 0),
    ] {
        settle(&mut piecemeal.conn, now).expect("settle");
    }
    assert_eq!(one_shot.ledger(), piecemeal.ledger());

    let rows = store::list_ledger(&one_shot.conn, 1000).expect("ledger");
    let sum: i64 = rows.iter().map(|e| e.delta).sum();
    let positive: i64 = rows.iter().filter(|e| e.delta > 0).map(|e| e.delta).sum();
    let status = status(&one_shot.conn).expect("status");
    assert_eq!((status.balance, status.earned), (sum, positive));
    // Window 1 (20:30-21:00) has the attempt; windows 0 and 2 pay, plus start bonus, pass, abort.
    assert_eq!(
        sum,
        2 * rewards::WINDOW_POINTS + rewards::START_BONUS_POINTS - 2 * 30
    );
}

#[test]
fn the_ledger_is_append_only_unique_per_ref_key_and_rejects_zero_deltas() {
    let rig = rig();
    assert!(store::insert_ledger_entry(&rig.conn, 1, 20, "focus_window", "k").expect("insert"));
    assert!(!store::insert_ledger_entry(&rig.conn, 2, 99, "focus_window", "k").expect("duplicate"));
    assert_eq!(rig.ledger(), vec![("k".to_string(), 20)]);
    assert!(store::insert_ledger_entry(&rig.conn, 3, 0, "focus_window", "zero").is_err());
    assert!(
        rig.conn
            .execute("UPDATE point_ledger SET delta = 1", [])
            .is_err()
    );
    assert!(rig.conn.execute("DELETE FROM point_ledger", []).is_err());
}
