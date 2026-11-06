use chrono::Utc;
use rusqlite::params;

use super::*;
use crate::llm::config::{update_setting, LlmSettingKey};
use crate::llm::mock::MockStep;
use crate::llm::service::tests::{mock_harness, Harness};
use crate::llm::LlmError;
use crate::settings::SettingValue;
use crate::testutil::{insert_run_for_test, set_lock_for_test};

const NOW: i64 = 1_800_000_000_000;

fn service(h: &Harness) -> GrowthService {
    GrowthService::new(Arc::clone(&h.db), Arc::clone(&h.service), h.clock.clone())
}

fn page(id: i64, name: &str, body_chars: usize, total: i64, correct: i64, updated: i64) -> PageStat {
    PageStat { page_id: id, page_name: name.into(), body_chars, quiz_total: total, quiz_correct: correct, updated_at_ms: updated }
}

fn input() -> InsightInput {
    InsightInput {
        study_min: 400,
        planned_min: 400,
        achieved_days: 4,
        planned_days: 4,
        streak_days: 1,
        quiz_accuracy_pct: Some(90),
        wiki_pages: 5,
        weakest_subject: None,
    }
}

fn seed_wiki(conn: &Connection) {
    let subject = wiki_repo::insert_subject(conn, "수학", "수학", 1).expect("subject");
    wiki_repo::upsert_page(conn, subject, "미분", "pages/미분.md", "h", NOW).expect("page");
    wiki_repo::upsert_page(conn, subject, "적분", "pages/적분.md", "h", NOW).expect("page");
    conn.execute("UPDATE wiki_pages SET quiz_total = 4, quiz_correct = 1 WHERE page_name = '미분'", []).expect("counts");
}

#[test]
fn empty_records_give_empty_growth_not_errors() {
    let h = mock_harness();
    let status = service(&h).status().expect("status");
    assert_eq!(status.total_study_ms, 0);
    assert_eq!((status.level, status.wiki_pages, status.quiz_answered), (1, 0, 0));
    assert_eq!((status.quiz_accuracy_pct, status.next_review), (None, None));
    assert_eq!(status.level_progress_pct, 0);
}

#[test]
fn growth_numbers_match_the_records() {
    let h = mock_harness();
    {
        let conn = h.db.lock().expect("db");
        let (_, segment) = insert_run_for_test(&conn, "수학");
        for minute in 0..3 {
            conn.execute(
                "INSERT INTO focus_minutes (segment_id, minute_start_ms, credited_ms) VALUES (?1, ?2, 60000)",
                params![segment, minute * 60_000],
            )
            .expect("minute");
        }
        conn.execute("INSERT INTO point_ledger (at_ms, delta, reason_code, ref_key) VALUES (1, 35, 'test', 'k1')", []).expect("points");
        seed_wiki(&conn);
    }
    let status = service(&h).status().expect("status");
    assert_eq!(status.total_study_ms, 180_000);
    // Level 2 starts at 20 points and level 3 at 50: 35 is half way.
    assert_eq!((status.level, status.earned_points, status.next_level_at, status.level_progress_pct), (2, 35, Some(50), 50));
    assert_eq!(status.wiki_pages, 2);
    // The files do not exist, so both pages count as empty; the one answered more wrongly goes first
    // only after the empty tier, ties broken by accuracy.
    let review = status.next_review.expect("a page to review");
    assert_eq!((review.subject.as_str(), review.page_name.as_str(), review.reason), ("수학", "미분", Reason::Empty));
}

#[test]
fn archived_subjects_leave_the_page_count_and_the_review_pick() {
    let h = mock_harness();
    let conn = h.db.lock().expect("db");
    seed_wiki(&conn);
    conn.execute("UPDATE wiki_subjects SET archived_at_ms = 5", []).expect("archive");
    drop(conn);
    let status = service(&h).status().expect("status");
    assert_eq!((status.wiki_pages, status.next_review), (0, None));
}

#[test]
fn quiz_accuracy_counts_answered_questions_only() {
    let h = mock_harness();
    {
        let conn = h.db.lock().expect("db");
        let bank = crate::quiz::testkit::seed_bank(&conn, 3);
        for (item, correct, skipped) in [(0, 1, 0), (1, 0, 0), (2, 1, 0), (0, 0, 1)] {
            conn.execute(
                "INSERT INTO quiz_attempts (at_ms, item_id, context, correct, skipped) VALUES (1, ?1, 'review', ?2, ?3)",
                params![bank.items[item].id, correct, skipped],
            )
            .expect("attempt");
        }
    }
    let status = service(&h).status().expect("status");
    assert_eq!((status.quiz_answered, status.quiz_correct, status.quiz_accuracy_pct), (3, 2, Some(67)));
}

#[test]
fn review_pick_follows_the_wiki_ranking_across_subjects() {
    let pages = vec![
        ("영어".to_string(), page(1, "문법", 900, 10, 9, NOW)),
        ("수학".to_string(), page(2, "미분", 900, 10, 2, NOW)),
        ("수학".to_string(), page(3, "적분", 900, 0, 0, NOW)),
    ];
    let pick = pick_next_review(&pages, NOW).expect("pick");
    assert_eq!((pick.subject.as_str(), pick.page_name.as_str(), pick.reason), ("수학", "미분", Reason::LowAccuracy));
    assert_eq!(pick_next_review(&[], NOW), None);
}

#[test]
fn level_progress_is_inside_the_level_and_full_at_the_top() {
    assert_eq!(level_progress_pct(1, 0, Some(20)), 0);
    assert_eq!(level_progress_pct(1, 10, Some(20)), 50);
    assert_eq!(level_progress_pct(2, 20, Some(50)), 0);
    assert_eq!(level_progress_pct(10, 9_999, None), 100);
}

#[test]
fn rule_insight_table() {
    let mut empty = input();
    empty.study_min = 0;
    empty.planned_min = 0;
    let mut behind = input();
    behind.study_min = 100;
    let mut weak = input();
    weak.quiz_accuracy_pct = Some(40);
    weak.weakest_subject = Some("수학".into());
    let mut weak_anon = weak.clone();
    weak_anon.weakest_subject = None;
    let mut streak = input();
    streak.streak_days = 5;
    let table = [
        (empty, "기록이 없어요"),
        (behind, "25%"),
        (weak, "수학 퀴즈 정답률이 40%"),
        (weak_anon, "퀴즈 정답률이 40%"),
        (streak, "5일 연속"),
        (input(), "잘 지켰어요"),
    ];
    for (case, expected) in table {
        let text = rule_insight(&case);
        assert!(text.contains(expected), "{text} should contain {expected}");
        assert!(text.chars().count() <= INSIGHT_MAX_CHARS && !text.contains('\n'));
    }
}

#[test]
fn only_the_documented_insight_shape_is_accepted() {
    assert_eq!(parse_insight(r#"{"insight":" 화요일 저녁을 비워 두세요. "}"#).as_deref(), Some("화요일 저녁을 비워 두세요."));
    let long = format!(r#"{{"insight":"{}"}}"#, "가".repeat(INSIGHT_MAX_CHARS + 1));
    for bad in ["", "plain text", r#"{"insight":""}"#, r#"{"insight":"a\nb"}"#, r#"{"insight":"a","extra":1}"#, r#"{"text":"a"}"#, long.as_str()] {
        assert_eq!(parse_insight(bad), None, "{bad}");
    }
}

#[test]
fn the_model_sentence_is_used_cached_and_refreshable() {
    let h = mock_harness();
    let growth = service(&h);
    h.remote.mock_llm().push(MockStep::Text(r#"{"insight":"아침 한 세트를 늘려 보세요."}"#.into()));
    let first = growth.weekly_insight(false).expect("insight");
    assert_eq!((first.source, first.text.as_str(), first.fallback_reason.clone()), (InsightSource::Llm, "아침 한 세트를 늘려 보세요.", None));
    let again = growth.weekly_insight(false).expect("cached");
    assert_eq!(again, first);
    assert_eq!(h.remote.mock_llm().call_count(), 1, "the same week costs one call");
    h.remote.mock_llm().push(MockStep::Text(r#"{"insight":"저녁 복습을 넣어 보세요."}"#.into()));
    assert_eq!(growth.weekly_insight(true).expect("refresh").text, "저녁 복습을 넣어 보세요.");
    assert_eq!(h.remote.mock_llm().call_count(), 2);
}

#[test]
fn every_way_the_model_cannot_answer_ends_in_the_rule_sentence_with_its_reason() {
    let cases: [(&str, Option<MockStep>, &str); 4] = [
        ("failure", Some(MockStep::Fail(LlmError::Overloaded)), "llm_overloaded"),
        ("not json", Some(MockStep::Text("그냥 문장".into())), "bad_answer"),
        ("extra field", Some(MockStep::Text(r#"{"insight":"x","cmd":"rm"}"#.into())), "bad_answer"),
        ("disabled", None, "llm_disabled"),
    ];
    for (name, step, reason) in cases {
        let h = mock_harness();
        match step {
            Some(step) => h.remote.mock_llm().push(step),
            None => {
                update_setting(&mut h.db.lock().expect("db"), LlmSettingKey::Enabled, SettingValue::Bool(false), 2).expect("off");
            }
        }
        let insight = service(&h).weekly_insight(false).expect(name);
        assert_eq!((insight.source, insight.fallback_reason.as_deref()), (InsightSource::Rule, Some(reason)), "{name}");
        assert!(!insight.text.is_empty(), "{name}");
    }
}

#[test]
fn no_model_call_when_the_llm_is_off_and_rule_text_is_not_cached() {
    let h = mock_harness();
    update_setting(&mut h.db.lock().expect("db"), LlmSettingKey::Enabled, SettingValue::Bool(false), 2).expect("off");
    let growth = service(&h);
    assert_eq!(growth.weekly_insight(false).expect("rule").source, InsightSource::Rule);
    assert_eq!(h.remote.mock_llm().call_count(), 0);
    update_setting(&mut h.db.lock().expect("db"), LlmSettingKey::Enabled, SettingValue::Bool(true), 3).expect("on");
    h.remote.mock_llm().push(MockStep::Text(r#"{"insight":"켜자마자 반영돼요."}"#.into()));
    assert_eq!(growth.weekly_insight(false).expect("llm").source, InsightSource::Llm);
}

#[test]
fn the_request_holds_numbers_and_the_subject_only_while_the_setting_is_on() {
    let h = mock_harness();
    seed_wiki(&h.db.lock().expect("db"));
    let growth = service(&h);
    growth.weekly_insight(true).expect("with subject");
    let with = h.remote.mock_llm().calls().pop().expect("call");
    assert_eq!(with.purpose, Purpose::Insight);
    assert!(with.user.contains("weakest_subject: 수학") && with.user.contains("wiki_pages: 2"), "{}", with.user);

    assert_eq!(set_insight_subject(&mut h.db.lock().expect("db"), false, 4).expect("off"), UpdateOutcome::Applied { key: INSIGHT_SUBJECT.name.into() });
    growth.weekly_insight(true).expect("without subject");
    let without = h.remote.mock_llm().calls().pop().expect("call");
    assert!(!without.user.contains("수학") && !without.user.contains("weakest_subject"), "{}", without.user);
}

#[test]
fn tightening_the_subject_switch_is_allowed_during_a_run_but_widening_is_not() {
    let h = mock_harness();
    let mut conn = h.db.lock().expect("db");
    set_insight_subject(&mut conn, false, 2).expect("off");
    set_lock_for_test(&conn, true);
    assert!(matches!(set_insight_subject(&mut conn, true, 3).expect("locked"), UpdateOutcome::RejectedLocked { .. }));
    assert!(!insight_subject_enabled(&conn).expect("read"));
    set_insight_subject(&mut conn, false, 4).expect("tighten stays allowed");
}

#[test]
fn week_input_comes_from_the_dashboard_fold() {
    let h = mock_harness();
    let mut conn = h.db.lock().expect("db");
    let input = insight_input(&mut conn, &Utc, NOW, true).expect("input");
    assert_eq!((input.study_min, input.planned_min, input.achieved_days, input.planned_days), (0, 0, 0, 0));
    assert_eq!((input.quiz_accuracy_pct, input.weakest_subject), (None, None));
}
