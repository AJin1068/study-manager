//! Dashboard tests run on a fixed clock (Thursday 2026-11-05 12:00 UTC) over a golden dataset
//! whose numbers are worked out by hand in the comments.

use chrono::{NaiveDate, TimeZone, Utc};
use rusqlite::{params, Connection};

use super::*;
use crate::db::open_in_memory;
use crate::enforce::model::{BlockAction, EnforceMode, NewBlockEvent, TargetKind};
use crate::focus::model::{FocusEventKind, MinuteDelta, NewFocusEvent};
use crate::plan::{create_plan, BlockKind, CreatePlanInput};
use crate::timer::model::{EndReason, NewRun, NewSegment, RunStatus};

const MIN: i64 = 60_000;

fn t(day: u32, h: i64, m: i64) -> i64 {
    Utc.with_ymd_and_hms(2026, 11, day, 0, 0, 0).single().expect("date").timestamp_millis() + (h * 60 + m) * MIN
}

fn now() -> i64 {
    t(5, 12, 0)
}

fn date(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, day).expect("date")
}

fn plan(conn: &mut Connection, name: &str, subject: &str, day: u32, time: &str, sets: i64) -> i64 {
    create_plan(
        conn,
        &Utc,
        CreatePlanInput {
            name: name.into(),
            subject: subject.into(),
            start_date: format!("2026-11-{day:02}"),
            start_time: time.into(),
            sets,
            repeat: None,
        },
        t(1, 0, 0),
    )
    .expect("plan")
    .plan_id
    .expect("id")
}

/// A run of `sets` x 50 minutes from `start_ms` with one study segment covering it, holding
/// `credited_min` one-minute buckets of recognized time. Returns `(run id, segment id)`.
fn run(
    conn: &Connection,
    plan_id: i64,
    day: u32,
    start_ms: i64,
    sets: i64,
    credited_min: i64,
    closed: bool,
) -> (i64, i64) {
    let end_ms = start_ms + sets * 50 * MIN;
    let run_id = store::insert_run(
        conn,
        &NewRun {
            plan_id,
            date: format!("2026-11-{day:02}"),
            planned_start_ms: start_ms,
            planned_end_ms: end_ms,
            actual_start_ms: None,
            mode_level: "medium".into(),
            study_min: 50,
            break_min: 10,
            long_break_min: None,
            sets_before_long_break: None,
            sets_total: sets,
            status: if closed { RunStatus::Completed } else { RunStatus::Active },
            ended_at_ms: closed.then_some(end_ms),
            detail_code: None,
        },
    )
    .expect("run");
    let segment = store::insert_segment(
        conn,
        run_id,
        &NewSegment {
            seq: 0,
            kind: BlockKind::Study,
            planned_start_ms: start_ms,
            planned_end_ms: end_ms,
            started_at_ms: start_ms,
            ended_at_ms: closed.then_some(end_ms),
            end_reason: closed.then_some(EndReason::Elapsed),
            late_join: false,
        },
    )
    .expect("segment");
    for minute in 0..credited_min {
        let minute_start_ms = start_ms + minute * MIN;
        store::add_focus_minute(
            conn,
            segment,
            &MinuteDelta { minute_start_ms, credited_ms: MIN, away_ms: 0, idle_ms: 0 },
        )
        .expect("minute");
    }
    (run_id, segment)
}

fn away(conn: &Connection, run_id: i64, segment_id: i64, at_ms: i64) {
    store::insert_focus_event(
        conn,
        &NewFocusEvent { run_id, segment_id, at_ms, kind: FocusEventKind::Away, app_id: Some("com.example.game") },
    )
    .expect("event");
}

fn block(conn: &Connection, at_ms: i64, kind: TargetKind, id: &str, action: BlockAction) {
    store::insert_block_event(
        conn,
        &NewBlockEvent {
            at_ms,
            run_id: None,
            segment_id: None,
            target_kind: kind,
            target_id: id,
            action,
            mode: EnforceMode::Enforce,
            detail_code: None,
        },
    )
    .expect("block event");
}

/// Mon 2 / Tue 3 / Wed 4 / Thu 5 of the week. Planned and recognized minutes per day:
///   Mon 수학  2 sets 20:00  planned 100, credited 90 (90%, achieved), studied 100, score 90
///   Tue 영어  2 sets 20:00  planned 100, credited 70 (70%, missed),   studied 100, score 70
///   Wed 수학  1 set  20:00  planned  50, credited 40 (80%, achieved), studied  50, score 80
///   Thu 영어  1 set  09:00  planned  50, credited 45 (90%), studied 50, score 90
///   Thu 수학  1 set  22:00  no run yet: +50 planned, so Thursday is 100 planned, 45 actual (45%)
/// Returns the connection and the Tuesday run/segment ids.
fn golden() -> (Connection, i64, i64) {
    let mut conn = open_in_memory().expect("db");
    let mon = plan(&mut conn, "수학 풀이", "수학", 2, "20:00", 2);
    let tue = plan(&mut conn, "영어 독해", "영어", 3, "20:00", 2);
    let wed = plan(&mut conn, "수학 복습", "수학", 4, "20:00", 1);
    let thu_am = plan(&mut conn, "영어 단어", "영어", 5, "09:00", 1);
    plan(&mut conn, "수학 저녁", "수학", 5, "22:00", 1);
    let (mon_run, mon_seg) = run(&conn, mon, 2, t(2, 20, 0), 2, 90, true);
    let (tue_run, tue_seg) = run(&conn, tue, 3, t(3, 20, 0), 2, 70, true);
    run(&conn, wed, 4, t(4, 20, 0), 1, 40, true);
    run(&conn, thu_am, 5, t(5, 9, 0), 1, 45, true);
    away(&conn, mon_run, mon_seg, t(2, 20, 30)); // 30 minutes into Monday's set
    away(&conn, tue_run, tue_seg, t(3, 20, 10)); // 10 minutes into Tuesday's set
    (conn, tue_run, tue_seg)
}

// ---- pure folds ----

#[test]
fn achieved_is_exactly_eighty_percent_and_never_for_an_empty_plan() {
    assert!(!achieved(100, 79));
    assert!(achieved(100, 80));
    assert!(achieved(100, 100));
    assert!(!achieved(0, 0));
    assert!(!achieved(0, 50));
}

#[test]
fn streak_counts_consecutive_achieved_days_and_skips_unplanned_ones() {
    let days = |rows: &[(u32, i64, i64)]| rows.iter().map(|(d, p, a)| (date(*d), (*p, *a))).collect::<BTreeMap<_, _>>();
    // (label, data, today, expected)
    let table = vec![
        ("no data", days(&[]), 5, 0),
        ("three in a row", days(&[(3, 100, 80), (4, 100, 100), (5, 100, 90)]), 5, 3),
        ("79% on a past day ends it", days(&[(2, 100, 100), (3, 100, 79), (4, 100, 100), (5, 100, 100)]), 5, 2),
        ("today in progress does not break", days(&[(3, 100, 100), (4, 100, 100), (5, 100, 10)]), 5, 2),
        ("today with nothing planned is skipped", days(&[(3, 100, 100), (4, 100, 100)]), 5, 2),
        ("a rest day between is skipped", days(&[(2, 100, 100), (3, 0, 0), (4, 100, 100)]), 4, 2),
        ("a missed day (planned, nothing done) ends it", days(&[(3, 100, 100), (4, 100, 0), (5, 100, 100)]), 5, 1),
    ];
    for (label, data, today, expected) in table {
        assert_eq!(streak_days(&data, date(today)), expected, "{label}");
    }
}

#[test]
fn overlap_counts_sleep_and_forced_quit_once_and_only_inside_plans() {
    let iv = |s: i64, e: i64| Interval { start: s, end: e };
    // Sleep 0-30 and forced quit 20-50 overlap each other (union 0-50); plans 10-40 and 45-60.
    assert_eq!(overlap_ms(vec![iv(0, 30), iv(20, 50)], vec![iv(10, 40), iv(45, 60)]), 30 + 5);
    assert_eq!(overlap_ms(vec![iv(0, 10)], vec![iv(10, 20)]), 0, "touching is no overlap");
    assert_eq!(overlap_ms(vec![], vec![iv(0, 5)]), 0);
}

#[test]
fn heatmap_buckets_by_local_weekday_and_hour() {
    // 2026-11-05 is a Thursday. 15:30 UTC is 00:30 the next day (Friday) in Seoul.
    let seoul = chrono_tz::Asia::Seoul;
    let times = [t(5, 15, 30), t(5, 15, 59), t(2, 0, 0)];
    let utc = heatmap(&Utc, &times);
    assert_eq!((utc[3][15], utc[0][0]), (2, 1));
    let kst = heatmap(&seoul, &times);
    assert_eq!((kst[4][0], kst[0][9]), (2, 1));
    assert_eq!(kst.iter().flatten().sum::<i64>(), 3);
}

// ---- today ----

#[test]
fn today_matches_the_hand_computed_golden() {
    let (mut conn, _, _) = golden();
    let got = today(&mut conn, &Utc, now()).expect("today");
    // Thursday: planned 100 min (50 run + 50 not yet run), recognized 45 min.
    assert_eq!((got.date.as_str(), got.planned_ms, got.actual_ms), ("2026-11-05", 100 * MIN, 45 * MIN));
    assert_eq!(got.achievement_pct, Some(45));
    // Next plan: the 22:00 evening plan, 10 hours away. The 09:00 plan already has a run.
    let next = got.next_plan.expect("next plan");
    assert_eq!((next.name.as_str(), next.planned_start_ms, next.starts_in_ms), ("수학 저녁", t(5, 22, 0), 10 * 60 * MIN));
    // Streak: Wednesday 80% counts, Tuesday 70% ends it; unfinished Thursday does not break it.
    assert_eq!((got.streak_days, got.streak_threshold_pct), (1, 80));
    // Windows of 30 recognized minutes: Mon 3 + Tue 2 + Wed 1 + Thu 1 = 7 x 20 points.
    assert_eq!((got.points.earned, got.points.balance, got.points.level), (140, 140, 5));
}

#[test]
fn empty_database_is_an_empty_state_not_an_error() {
    let mut conn = open_in_memory().expect("db");
    let got = today(&mut conn, &Utc, now()).expect("today");
    assert_eq!((got.planned_ms, got.actual_ms, got.achievement_pct, got.streak_days), (0, 0, None, 0));
    assert_eq!((got.next_plan, got.points.level, got.points.balance), (None, 1, 0));

    let week = week(&conn, &Utc, now()).expect("week");
    assert_eq!((week.days.len(), week.planned_ms, week.achievement_pct), (7, 0, None));
    assert!(week.days.iter().all(|d| d.actual_ms == 0 && d.focus_score.is_none() && !d.achieved));
    assert!(week.by_subject.is_empty());

    let weak = weakness(&conn, &Utc, now()).expect("weakness");
    assert!(weak.deviation_apps.is_empty() && weak.postponed_plans.is_empty());
    assert_eq!((weak.avg_first_away_ms, weak.sets_total), (None, 0));
    assert_eq!((weak.heatmap.len(), weak.heatmap[0].len()), (7, 24));
    assert_eq!(weak.heatmap.iter().flatten().sum::<i64>(), 0);
    assert_eq!(weak.app_off.planned_overlap_ms, 0);
}

#[test]
fn a_plan_with_no_attempt_and_no_study_yet_shows_planned_only() {
    let mut conn = open_in_memory().expect("db");
    plan(&mut conn, "수학", "수학", 5, "20:00", 2);
    let got = today(&mut conn, &Utc, now()).expect("today");
    assert_eq!((got.planned_ms, got.actual_ms, got.achievement_pct), (100 * MIN, 0, Some(0)));
    assert_eq!(got.next_plan.map(|n| n.starts_in_ms), Some(8 * 60 * MIN));
}

#[test]
fn a_run_past_midnight_stays_on_the_day_it_was_planned_for() {
    let mut conn = open_in_memory().expect("db");
    let id = plan(&mut conn, "야간", "수학", 4, "23:00", 2);
    // 23:00 to 00:40; 100 recognized minutes, 60 of them after midnight.
    run(&conn, id, 4, t(4, 23, 0), 2, 100, true);
    let got = today(&mut conn, &Utc, now()).expect("today");
    assert_eq!((got.planned_ms, got.actual_ms), (0, 0), "nothing is attributed to the 5th");
    assert_eq!(got.streak_days, 1, "the 4th was achieved");
    let w = week(&conn, &Utc, now()).expect("week");
    let wed = &w.days[2];
    assert_eq!((wed.date.as_str(), wed.planned_ms, wed.actual_ms, wed.achieved), ("2026-11-04", 100 * MIN, 100 * MIN, true));
    assert_eq!(w.days[3].actual_ms, 0);
}

#[test]
fn a_run_in_progress_counts_its_time_but_gives_no_score() {
    let mut conn = open_in_memory().expect("db");
    let id = plan(&mut conn, "수학", "수학", 5, "11:00", 2);
    run(&conn, id, 5, t(5, 11, 0), 2, 50, false);
    let got = today(&mut conn, &Utc, now()).expect("today");
    assert_eq!((got.planned_ms, got.actual_ms, got.achievement_pct), (100 * MIN, 50 * MIN, Some(50)));
    let w = week(&conn, &Utc, now()).expect("week");
    assert_eq!(w.days[3].focus_score, None);
}

// ---- week ----

#[test]
fn week_matches_the_hand_computed_golden() {
    let (conn, _, _) = golden();
    let w = week(&conn, &Utc, now()).expect("week");
    assert_eq!((w.week_start.as_str(), w.week_end.as_str()), ("2026-11-02", "2026-11-08"));
    let rows: Vec<_> = w
        .days
        .iter()
        .map(|d| (d.weekday, d.planned_ms / MIN, d.actual_ms / MIN, d.achievement_pct, d.achieved, d.focus_score))
        .collect();
    assert_eq!(
        rows,
        vec![
            (0, 100, 90, Some(90), true, Some(90)),
            (1, 100, 70, Some(70), false, Some(70)),
            (2, 50, 40, Some(80), true, Some(80)),
            (3, 100, 45, Some(45), false, Some(90)),
            (4, 0, 0, None, false, None),
            (5, 0, 0, None, false, None),
            (6, 0, 0, None, false, None),
        ]
    );
    // 350 planned, 245 recognized: 70%. Two of the four planned days were achieved.
    assert_eq!((w.planned_ms / MIN, w.actual_ms / MIN, w.achievement_pct), (350, 245, Some(70)));
    assert_eq!((w.achieved_days, w.planned_days), (2, 4));
    // 수학: Mon 90 + Wed 40; 영어: Tue 70 + Thu 45.
    let subjects: Vec<(&str, i64)> = w.by_subject.iter().map(|s| (s.subject.as_str(), s.actual_ms / MIN)).collect();
    assert_eq!(subjects, vec![("수학", 130), ("영어", 115)]);
}

#[test]
fn downtime_inside_a_set_is_removed_from_studied_time_so_the_score_is_not_punished() {
    let mut conn = open_in_memory().expect("db");
    let id = plan(&mut conn, "수학", "수학", 5, "09:00", 2);
    run(&conn, id, 5, t(5, 9, 0), 2, 70, true);
    // 20 minutes asleep inside the 100-minute set: studied 80, recognized 70, score 87.5 -> 88.
    conn.execute(
        "INSERT INTO downtime_records (kind, start_at_ms, end_at_ms, estimated) VALUES ('sleep', ?1, ?2, 0)",
        params![t(5, 9, 30), t(5, 9, 50)],
    )
    .expect("downtime");
    let w = week(&conn, &Utc, now()).expect("week");
    assert_eq!(w.days[3].focus_score, Some(88));
}

#[test]
fn week_boundary_is_monday_to_sunday_in_the_given_timezone() {
    let mut conn = open_in_memory().expect("db");
    let id = plan(&mut conn, "일요일", "수학", 1, "20:00", 1);
    run(&conn, id, 1, t(1, 20, 0), 1, 50, true);
    // Sunday the 1st belongs to the previous week.
    let w = week(&conn, &Utc, now()).expect("week");
    assert_eq!((w.planned_ms, w.actual_ms), (0, 0));
    let prev = week(&conn, &Utc, t(1, 12, 0)).expect("previous week");
    assert_eq!((prev.week_start.as_str(), prev.days[6].date.as_str(), prev.days[6].actual_ms), ("2026-10-26", "2026-11-01", 50 * MIN));
}

// ---- weakness ----

#[test]
fn weakness_matches_the_hand_computed_golden() {
    let (conn, _, _) = golden();
    // Attempts: game x4 (three quit requests, one dry-run hit), chat x1; a `terminated` outcome and
    // a protected skip are not attempts.
    block(&conn, t(3, 20, 40), TargetKind::App, "com.example.game", BlockAction::TerminateRequested);
    block(&conn, t(3, 20, 41), TargetKind::App, "com.example.game", BlockAction::Terminated);
    block(&conn, t(3, 20, 50), TargetKind::App, "com.example.game", BlockAction::TerminateRequested);
    block(&conn, t(4, 20, 5), TargetKind::App, "com.example.game", BlockAction::TerminateRequested);
    block(&conn, t(4, 21, 0), TargetKind::App, "com.example.game", BlockAction::WouldTerminate);
    block(&conn, t(5, 9, 15), TargetKind::App, "com.example.chat", BlockAction::TerminateRequested);
    block(&conn, t(5, 9, 16), TargetKind::App, "com.example.chat", BlockAction::SkippedProtected);
    block(&conn, t(2, 20, 45), TargetKind::Site, "news.example", BlockAction::SiteBlocked);
    // Outside the 28-day window: ignored.
    block(&conn, t(5, 12, 0) - 29 * 86_400_000, TargetKind::App, "com.example.old", BlockAction::TerminateRequested);

    let weak = weakness(&conn, &Utc, now()).expect("weakness");
    assert_eq!(
        weak.deviation_apps,
        vec![
            AppCount { app_id: "com.example.game".into(), count: 4 },
            AppCount { app_id: "com.example.chat".into(), count: 1 },
        ]
    );
    // Four study sets began in the window; two had a deviation, 30 and 10 minutes in: mean 20.
    assert_eq!((weak.sets_total, weak.sets_with_away, weak.avg_first_away_ms), (4, 2, Some(20 * MIN)));
    // Deviations: away Mon 20:30, away Tue 20:10, game x2 Tue 20:40/20:50, game Wed 20:05 and 21:00,
    // chat Thu 09:15, site Mon 20:45.
    let mut expected = vec![vec![0_i64; 24]; 7];
    expected[0][20] = 2;
    expected[1][20] = 3;
    expected[2][20] = 1;
    expected[2][21] = 1;
    expected[3][9] = 1;
    assert_eq!(weak.heatmap, expected);
}

#[test]
fn often_postponed_plans_come_from_applied_postponements_only() {
    let (conn, _, _) = golden();
    let ids: Vec<i64> = conn.prepare("SELECT id FROM plans ORDER BY id").expect("p").query_map([], |r| r.get(0)).expect("q").collect::<Result<_, _>>().expect("rows");
    let history = |plan_id: i64, at_ms: i64, reason: &str, outcome: &str| {
        conn.execute(
            "INSERT INTO plan_history (plan_id, at_ms, action, reason_code, outcome) VALUES (?1, ?2, 'move', ?3, ?4)",
            params![plan_id, at_ms, reason, outcome],
        )
        .expect("history");
    };
    history(ids[0], t(3, 8, 0), "postponed", "applied");
    history(ids[0], t(4, 8, 0), "postponed", "applied");
    history(ids[0], t(4, 9, 0), "postponed", "rejected_locked");
    history(ids[1], t(4, 8, 0), "postponed", "applied");
    history(ids[1], t(4, 9, 0), "sick", "applied");
    let weak = weakness(&conn, &Utc, now()).expect("weakness");
    assert_eq!(
        weak.postponed_plans,
        vec![PostponedPlan { plan_id: ids[0], name: "수학 풀이".into(), subject: "수학".into(), count: 2 }]
    );
}

#[test]
fn app_off_time_is_reported_separately_and_overlaps_plans_once() {
    let (conn, _, _) = golden();
    // Tuesday's run is 20:00-21:40. Sleep 19:50-20:20 and a forced quit 20:10-20:30 overlap each
    // other; their union 19:50-20:30 overlaps the plan for 30 minutes.
    conn.execute(
        "INSERT INTO downtime_records (kind, start_at_ms, end_at_ms, estimated) VALUES ('sleep', ?1, ?2, 0), ('forced_quit', ?3, ?4, 1)",
        params![t(3, 19, 50), t(3, 20, 20), t(3, 20, 10), t(3, 20, 30)],
    )
    .expect("downtime");
    let off = weakness(&conn, &Utc, now()).expect("weakness").app_off;
    assert_eq!(
        off,
        AppOffStat { sleep_ms: 30 * MIN, forced_quit_ms: 20 * MIN, forced_quit_count: 1, planned_overlap_ms: 30 * MIN }
    );
}

#[test]
fn release_usage_counts_each_kind_and_aborted_runs() {
    let (conn, tue_run, _) = golden();
    let pass = |kind: &str, at_ms: i64, expires: Option<i64>| {
        conn.execute(
            "INSERT INTO study_passes (at_ms, date, kind, expires_at_ms, challenge_len, typing_ms) VALUES (?1, '2026-11-03', ?2, ?3, 10, 5000)",
            params![at_ms, kind, expires],
        )
        .expect("pass");
    };
    pass("emergency_pass", t(3, 21, 0), Some(t(3, 21, 10)));
    pass("early_release", t(3, 21, 0), None);
    pass("early_release", t(4, 21, 0), None);
    pass("skip_today", t(4, 8, 0), None);
    pass("skip_today", t(5, 8, 0) - 40 * 86_400_000, None); // out of window
    store::finish_run(&conn, tue_run, RunStatus::Aborted, t(3, 21, 0), None).expect("abort");
    let r = weakness(&conn, &Utc, now()).expect("weakness").releases;
    assert_eq!(r, ReleaseUsage { emergency_pass: 1, skip_today: 1, early_release: 2, aborted_runs: 1 });
}

#[test]
fn dashboard_logs_carry_no_titles_or_urls() {
    let (mut conn, _, _) = golden();
    let logs = crate::testutil::capture_logs(|| {
        today(&mut conn, &Utc, now()).expect("today");
        week(&conn, &Utc, now()).expect("week");
        weakness(&conn, &Utc, now()).expect("weakness");
    });
    for forbidden in ["http", "title", "://"] {
        assert!(!logs.contains(forbidden), "log leaked {forbidden:?}: {logs}");
    }
}

// ---- the phone (stage 5): statistics only ----

fn segment_ids(conn: &Connection) -> Vec<i64> {
    conn.prepare("SELECT id FROM timer_segments ORDER BY id").expect("p").query_map([], |r| r.get(0)).expect("q").collect::<Result<_, _>>().expect("rows")
}

fn gap(conn: &Connection, run_id: i64, start_ms: i64, end_ms: i64, kind: &str) {
    store::phone::insert_gap(conn, &store::phone::GapRow { run_id, start_ms, end_ms, kind: kind.into() }).expect("gap");
}

#[test]
fn phone_unreachable_time_counts_each_stretch_once_clipped_to_the_window_and_pickups_count_study_sets() {
    let (conn, tue_run, _) = golden();
    let from = now() - WEAKNESS_DAYS * DAY_MS;
    // Tuesday: silence 20:10-20:40 and a switched-off service 20:30-20:50 overlap: 20:10-20:50.
    gap(&conn, tue_run, t(3, 20, 10), t(3, 20, 40), "unreachable");
    gap(&conn, tue_run, t(3, 20, 30), t(3, 20, 50), "service_disabled");
    // Half in the window, half before it: only the inside counts.
    gap(&conn, tue_run, from - 10 * MIN, from + 10 * MIN, "lease_expired");
    // Entirely before the window: not counted at all.
    gap(&conn, tue_run, from - 50 * MIN, from - 40 * MIN, "unreachable");

    let segments = segment_ids(&conn);
    let (mon, tue, wed) = (segments[0], segments[1], segments[2]);
    let at = t(5, 11, 0);
    // Monday reported 3 then 5 (the segment's count is 5), Tuesday 0, Wednesday 2.
    for (segment, count) in [(mon, 3), (mon, 5), (mon, 5), (tue, 0), (wed, 2)] {
        store::phone::record_pickups(&conn, segment, count, at).expect("pickups");
    }
    let phone = weakness(&conn, &Utc, now()).expect("weakness").phone;
    assert_eq!(
        phone,
        PhoneStat {
            unreachable_ms: 40 * MIN + 10 * MIN,
            gaps: 3,
            service_disabled_gaps: 1,
            pickup_sets_reported: 3,
            pickup_sets_with_pickups: 2,
            pickups_total: 7,
        }
    );
}

#[test]
fn without_a_phone_every_phone_number_is_zero() {
    let (conn, _, _) = golden();
    let phone = weakness(&conn, &Utc, now()).expect("weakness").phone;
    assert_eq!(
        phone,
        PhoneStat { unreachable_ms: 0, gaps: 0, service_disabled_gaps: 0, pickup_sets_reported: 0, pickup_sets_with_pickups: 0, pickups_total: 0 }
    );
}

#[test]
fn phone_data_never_changes_a_focus_score_a_streak_or_a_point() {
    let (mut conn, tue_run, _) = golden();
    let before = (today(&mut conn, &Utc, now()).expect("today"), week(&conn, &Utc, now()).expect("week"));
    let weak_before = weakness(&conn, &Utc, now()).expect("weakness");
    let ledger_before: i64 = conn.query_row("SELECT COUNT(*) FROM point_ledger", [], |r| r.get(0)).expect("ledger");

    // The worst the phone can say: pick-ups in every set and gaps across every run.
    for segment in segment_ids(&conn) {
        store::phone::record_pickups(&conn, segment, 500, now()).expect("pickups");
    }
    gap(&conn, tue_run, t(3, 20, 0), t(3, 21, 40), "unreachable");
    gap(&conn, tue_run, t(2, 20, 0), t(2, 21, 40), "service_disabled");

    let after = (today(&mut conn, &Utc, now()).expect("today"), week(&conn, &Utc, now()).expect("week"));
    assert_eq!(before, after, "today and week, including focus score, streak and points, are unchanged");
    let weak_after = weakness(&conn, &Utc, now()).expect("weakness");
    assert_eq!(weak_after.phone.pickups_total, 2_000);
    assert_eq!(Weakness { phone: weak_before.phone.clone(), ..weak_after }, weak_before, "everything but the phone block is unchanged");
    let ledger_after: i64 = conn.query_row("SELECT COUNT(*) FROM point_ledger", [], |r| r.get(0)).expect("ledger");
    assert_eq!(ledger_before, ledger_after, "no point was posted or deducted");
}
