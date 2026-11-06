use chrono::Utc;
use rusqlite::{params, Connection};

use super::protocol::{parse_step, DraftOp, Step, Tool};
use super::service::{ChatMessage, ChatService, ReplySource, Speaker, MAX_CLARIFICATIONS};
use crate::llm::config::{update_setting, LlmSettingKey};
use crate::llm::mock::MockStep;
use crate::llm::service::tests::{mock_harness, Harness};
use crate::llm::{LlmError, Purpose};
use crate::plan::{create_plan, plan_history, CreatePlanInput};
use crate::settings::SettingValue;
use crate::testutil::set_lock_for_test;

/// The harness clock: Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000_000;

fn chat(h: &Harness) -> ChatService {
    ChatService::new(h.db.clone(), h.service.clone(), h.clock.clone())
}

fn say(h: &Harness, step: &str) {
    h.remote.mock_llm().push(MockStep::Text(step.to_string()));
}

fn send(h: &Harness, message: &str) -> super::service::ChatReply {
    chat(h).send_in(&Utc, &[], message).expect("reply")
}

fn add_plan(conn: &mut Connection, date: &str, time: &str, sets: i64) -> i64 {
    let input = CreatePlanInput { name: "기존".into(), subject: "수학".into(), start_date: date.into(), start_time: time.into(), sets, repeat: None };
    create_plan(conn, &Utc, input, NOW - 1000).expect("plan").plan_id.expect("id")
}

fn plan_count(h: &Harness) -> i64 {
    h.db.lock().expect("db").query_row("SELECT COUNT(*) FROM plans", [], |r| r.get(0)).expect("count")
}

fn history_count(h: &Harness) -> usize {
    plan_history(&h.db.lock().expect("db"), None, 100).expect("history").len()
}

fn propose(date: &str, time: &str, sets: i64) -> String {
    format!(r#"{{"tool":"propose_plan","args":{{"name":"시험 대비","subject":"수학","date":"{date}","time":"{time}","sets":{sets}}}}}"#)
}

#[test]
fn the_exam_week_scenario_drafts_first_and_changes_nothing_until_applied() {
    let h = mock_harness();
    {
        let mut conn = h.db.lock().expect("db");
        add_plan(&mut conn, "2027-01-18", "09:00", 1);
    }
    let (plans_before, history_before) = (plan_count(&h), history_count(&h));
    say(&h, r#"{"tool":"list_free_slots","args":{"from":"2027-01-18","to":"2027-01-22","min_minutes":90}}"#);
    say(&h, r#"{"tool":"get_focus_summary"}"#);
    say(&h, &propose("2027-01-18", "19:00", 2));
    say(&h, &propose("2027-01-19", "19:00", 2));
    say(&h, r#"{"reply":"평일 저녁에 두 번 넣었어요. 토요일 시험 전까지 확인해 보세요."}"#);

    let reply = send(&h, "다음 주 토요일 시험인데 평일 저녁만 쓸 수 있어");
    assert_eq!(reply.source, ReplySource::Llm);
    let draft = reply.draft.expect("a draft");
    assert_eq!(draft.items.len(), 2);
    assert!(draft.items[0].label.contains("2027-01-18 19:00") && draft.items[0].kind == "create");
    // Preview only: no plan, no history row.
    assert_eq!((plan_count(&h), history_count(&h)), (plans_before, history_before));

    let calls = h.remote.mock_llm().calls();
    assert_eq!(calls.len(), 5);
    assert!(calls.iter().all(|c| c.purpose == Purpose::PlanChat));
    assert!(calls[0].user.contains("평일 저녁만") && calls[0].user.contains("today: 2027-01-15"));
    assert!(calls[1].user.contains("<document name=\"도구 결과\">") && calls[1].user.contains("\"slots\""), "{}", calls[1].user);
    assert!(calls[4].user.contains("\"ok\":true"));

    let applied = chat(&h).apply_draft_in(&Utc, draft.id).expect("apply");
    assert!(applied.applied && applied.changed == 2 && !applied.locked);
    assert_eq!(plan_count(&h), plans_before + 2);
    let history = plan_history(&h.db.lock().expect("db"), None, 100).expect("history");
    let chatbot: Vec<_> = history.iter().filter(|r| r.reason_code == "chatbot").collect();
    assert_eq!(chatbot.len(), 2);
    assert!(chatbot.iter().all(|r| r.action == "chatbot_create" && r.outcome == "applied" && r.batch_id.as_deref() == Some(&format!("chat-{}", draft.id)[..])));
    assert!(chat(&h).apply_draft_in(&Utc, draft.id).is_err(), "a draft applies once");
}

#[test]
fn a_move_and_a_cancel_go_through_the_plan_api_with_the_chatbot_reason() {
    let h = mock_harness();
    let (a, b) = {
        let mut conn = h.db.lock().expect("db");
        (add_plan(&mut conn, "2027-01-18", "09:00", 1), add_plan(&mut conn, "2027-01-19", "09:00", 1))
    };
    say(&h, &format!(r#"{{"tool":"propose_move","args":{{"plan_id":{a},"date":"2027-01-18","new_time":"20:00"}}}}"#));
    say(&h, &format!(r#"{{"tool":"propose_delete","args":{{"plan_id":{b},"date":"2027-01-19"}}}}"#));
    say(&h, r#"{"reply":"하나는 옮기고 하나는 취소해요."}"#);
    let draft = send(&h, "월요일은 저녁으로, 화요일은 취소").draft.expect("draft");
    assert_eq!(draft.items.iter().map(|i| i.kind).collect::<Vec<_>>(), ["move", "delete"]);
    assert!(draft.items[0].label.contains("기존 (수학)") && draft.items[0].label.contains("20:00"));
    assert!(chat(&h).apply_draft_in(&Utc, draft.id).expect("apply").applied);
    let history = plan_history(&h.db.lock().expect("db"), None, 100).expect("history");
    let mut rows: Vec<(String, String, String)> =
        history.iter().filter(|r| r.batch_id.is_some()).map(|r| (r.action.clone(), r.reason_code.clone(), r.outcome.clone())).collect();
    rows.sort();
    assert_eq!(rows, [("delete".to_string(), "chatbot".to_string(), "applied".to_string()), ("retime".to_string(), "chatbot".to_string(), "applied".to_string())]);
}

#[test]
fn an_overlapping_new_plan_is_moved_to_the_next_free_start_by_the_shift_layout_rule() {
    let h = mock_harness();
    {
        let mut conn = h.db.lock().expect("db");
        add_plan(&mut conn, "2027-01-18", "19:00", 2); // 115 minutes: until 20:55
    }
    say(&h, &propose("2027-01-18", "19:30", 1));
    say(&h, r#"{"reply":"겹쳐서 뒤로 미뤘어요."}"#);
    let draft = send(&h, "월요일 저녁에 한 세트").draft.expect("draft");
    assert!(draft.items[0].label.contains("2027-01-18 20:55"), "{:?}", draft.items);
    assert!(h.remote.mock_llm().calls()[1].user.contains("adjusted_time"));
}

#[test]
fn a_day_without_room_comes_back_as_an_error_with_free_slots_and_no_operation() {
    let h = mock_harness();
    {
        let mut conn = h.db.lock().expect("db");
        // 09:00 + 24 sets of 50+10 minutes fills the day until the evening.
        add_plan(&mut conn, "2027-01-18", "06:00", 24);
    }
    say(&h, &propose("2027-01-18", "10:00", 24));
    say(&h, r#"{"reply":"그날은 자리가 없어요."}"#);
    let reply = send(&h, "월요일에 하루 종일");
    assert!(reply.draft.is_none());
    assert!(h.remote.mock_llm().calls()[1].user.contains("no_room_that_day"));
}

#[test]
fn answers_outside_the_protocol_are_discarded_and_never_change_anything() {
    let bad = [
        "그냥 문장입니다",
        r#"{"tool":"delete_everything","args":{}}"#,
        r#"{"tool":"propose_plan","args":{"name":"a","subject":"b","date":"2027-01-19","time":"19:00","sets":1,"path":"/etc"}}"#,
        r#"{"tool":"propose_plan","args":{"name":"a","subject":"b","date":"2027-01-19","time":"19:00","sets":"two"}}"#,
        r#"{"reply":"ok","tool":"list_subjects"}"#,
        r#"{"reply":""}"#,
        r#"{"tool":"list_plans","args":{"from":"2027-01-18"}}"#,
        r#"{"tool":"get_focus_summary","args":{"x":1}}"#,
        r#"[{"reply":"ok"}]"#,
    ];
    for answer in bad {
        assert_eq!(parse_step(answer), None, "{answer}");
        let h = mock_harness();
        say(&h, answer);
        say(&h, r#"{"reply":"다시 말씀해 주세요."}"#);
        let reply = send(&h, "계획 만들어 줘");
        assert_eq!((reply.text.as_str(), reply.draft.is_some()), ("다시 말씀해 주세요.", false), "{answer}");
        assert_eq!((plan_count(&h), history_count(&h)), (0, 0));
    }
    // Three bad answers in one turn end it.
    let h = mock_harness();
    for _ in 0..3 {
        say(&h, "not json");
    }
    let reply = send(&h, "계획 만들어 줘");
    assert!(reply.text.contains("폐기") && reply.draft.is_none());
}

#[test]
fn the_documented_tool_shapes_parse() {
    assert_eq!(parse_step(r#"{"reply":" 안녕 "}"#), Some(Step::Reply("안녕".into())));
    assert_eq!(parse_step(r#"{"tool":"list_subjects"}"#), Some(Step::Tool(Tool::ListSubjects)));
    let Some(Step::Tool(Tool::Propose(DraftOp::Create(op)))) = parse_step(
        r#"{"tool":"propose_plan","args":{"name":"n","subject":"s","date":"2027-01-18","time":"19:00","sets":2,"repeat_weekdays":[1,3],"repeat_until":"2027-02-01"}}"#,
    ) else {
        panic!("a repeating plan parses");
    };
    assert_eq!((op.repeat_weekdays, op.repeat_until.as_deref()), (Some(vec![1, 3]), Some("2027-02-01")));
}

#[test]
fn a_repeating_plan_drafts_and_applies_as_one_series() {
    let h = mock_harness();
    say(&h, r#"{"tool":"propose_plan","args":{"name":"복습","subject":"수학","date":"2027-01-18","time":"19:00","sets":1,"repeat_weekdays":[1,2,3,4,5],"repeat_until":"2027-01-22"}}"#);
    say(&h, r#"{"reply":"평일 저녁 반복이에요."}"#);
    let draft = send(&h, "평일 저녁마다").draft.expect("draft");
    assert!(draft.items[0].label.contains("매주 월,화,수,목,금"));
    assert!(chat(&h).apply_draft_in(&Utc, draft.id).expect("apply").applied);
    let rows = crate::plan::list_occurrences(&h.db.lock().expect("db"), &Utc, "2027-01-18", "2027-01-24").expect("list");
    assert_eq!(rows.len(), 5);
}

#[test]
fn an_overlap_found_only_at_apply_time_rolls_the_whole_draft_back() {
    let h = mock_harness();
    say(&h, &propose("2027-01-18", "19:00", 1));
    say(&h, &propose("2027-01-19", "19:00", 1));
    say(&h, r#"{"reply":"두 개요."}"#);
    let draft = send(&h, "월화 저녁").draft.expect("draft");
    {
        let mut conn = h.db.lock().expect("db");
        add_plan(&mut conn, "2027-01-19", "19:30", 1); // appears after the preview
    }
    let before = (plan_count(&h), history_count(&h));
    assert!(chat(&h).apply_draft_in(&Utc, draft.id).is_err());
    assert_eq!((plan_count(&h), history_count(&h)), before, "the first operation was rolled back too");
    let still_open: i64 = h.db.lock().expect("db").query_row("SELECT COUNT(*) FROM chat_drafts WHERE applied_at_ms IS NULL", [], |r| r.get(0)).expect("drafts");
    assert_eq!(still_open, 1);
}

#[test]
fn during_a_run_cancels_and_moves_are_refused_and_recorded_like_a_locked_setting() {
    let h = mock_harness();
    let a = {
        let mut conn = h.db.lock().expect("db");
        let id = add_plan(&mut conn, "2027-01-18", "09:00", 1);
        set_lock_for_test(&conn, true);
        id
    };
    let history_before = history_count(&h);
    say(&h, &format!(r#"{{"tool":"propose_delete","args":{{"plan_id":{a},"date":"2027-01-18"}}}}"#));
    say(&h, &format!(r#"{{"tool":"propose_move","args":{{"plan_id":{a},"date":"2027-01-18","new_time":"22:00"}}}}"#));
    say(&h, r#"{"reply":"공부 중에는 계획을 취소하거나 옮길 수 없어요."}"#);
    let reply = send(&h, "오늘 계획 취소해 줘");
    assert!(reply.locked && reply.draft.is_none());
    let history = plan_history(&h.db.lock().expect("db"), None, 100).expect("history");
    assert_eq!(history.len(), history_before + 2);
    let refused: Vec<_> = history.iter().filter(|r| r.outcome == "rejected_locked").collect();
    assert_eq!(refused.len(), 2);
    assert!(refused.iter().all(|r| r.reason_code == "chatbot"));
    let occurrences = crate::plan::list_occurrences(&h.db.lock().expect("db"), &Utc, "2027-01-18", "2027-01-18").expect("list");
    assert_eq!((occurrences.len(), occurrences[0].start_time.as_str()), (1, "09:00"));
}

#[test]
fn an_addition_drafted_during_a_run_waits_and_applying_it_while_locked_is_recorded() {
    let h = mock_harness();
    set_lock_for_test(&h.db.lock().expect("db"), true);
    say(&h, &propose("2027-01-19", "19:00", 1));
    say(&h, r#"{"reply":"끝나면 적용하세요."}"#);
    let draft = send(&h, "내일 저녁 추가").draft.expect("draft");
    let refused = chat(&h).apply_draft_in(&Utc, draft.id).expect("refusal is a value");
    assert!(!refused.applied && refused.locked);
    assert_eq!(plan_count(&h), 0);
    let rows = plan_history(&h.db.lock().expect("db"), None, 10).expect("history");
    assert!(rows.iter().any(|r| r.outcome == "rejected_locked" && r.reason_code == "chatbot"));
    set_lock_for_test(&h.db.lock().expect("db"), false);
    assert!(chat(&h).apply_draft_in(&Utc, draft.id).expect("apply").applied);
    assert_eq!(plan_count(&h), 1);
}

#[test]
fn discarding_removes_the_draft_and_it_can_no_longer_be_applied() {
    let h = mock_harness();
    say(&h, &propose("2027-01-19", "19:00", 1));
    say(&h, r#"{"reply":"초안이에요."}"#);
    let draft = send(&h, "추가").draft.expect("draft");
    assert!(chat(&h).discard_draft(draft.id).expect("discard"));
    assert!(!chat(&h).discard_draft(draft.id).expect("twice"));
    assert!(chat(&h).apply_draft_in(&Utc, draft.id).is_err());
    assert_eq!(plan_count(&h), 0);
}

#[test]
fn the_model_must_draft_after_three_questions_back() {
    let history = |assistant: usize| -> Vec<ChatMessage> {
        (0..assistant).flat_map(|i| [ChatMessage { role: Speaker::User, text: format!("답 {i}") }, ChatMessage { role: Speaker::Assistant, text: format!("질문 {i}") }]).collect()
    };
    for (asked, must) in [(0, false), (MAX_CLARIFICATIONS - 1, false), (MAX_CLARIFICATIONS, true)] {
        let h = mock_harness();
        say(&h, r#"{"reply":"더 알려 주세요."}"#);
        let reply = chat(&h).send_in(&Utc, &history(asked), "그냥 알아서").expect("reply");
        assert!(h.remote.mock_llm().calls()[0].user.contains(&format!("must_draft: {must}")), "asked {asked}");
        assert_eq!(reply.over_asked, must, "asked {asked}");
    }
    // Complying at the limit produces a draft.
    let h = mock_harness();
    say(&h, &propose("2027-01-19", "19:00", 1));
    say(&h, r#"{"reply":"정해진 대로 넣었어요."}"#);
    let reply = chat(&h).send_in(&Utc, &history(MAX_CLARIFICATIONS), "그냥 알아서").expect("reply");
    assert!(reply.draft.is_some() && !reply.over_asked);
}

#[test]
fn without_a_model_the_chat_answers_locally_and_says_why() {
    let h = mock_harness();
    update_setting(&mut h.db.lock().expect("db"), LlmSettingKey::Enabled, SettingValue::Bool(false), 2).expect("off");
    let guide = send(&h, "다음 주 계획 짜줘");
    assert_eq!((guide.source, guide.fallback_reason.as_deref(), guide.draft.is_none()), (ReplySource::Local, Some("llm_disabled"), true));
    let free = send(&h, "이번 주 빈 시간 알려줘");
    assert!(free.text.starts_with("앞으로 비는 시간:") && free.text.contains("2027-01-15"), "{}", free.text);
    assert_eq!(h.remote.mock_llm().call_count(), 0);

    let failing = mock_harness();
    failing.remote.mock_llm().push(MockStep::Fail(LlmError::Overloaded));
    let reply = send(&failing, "빈 시간");
    assert_eq!((reply.source, reply.fallback_reason.as_deref()), (ReplySource::Local, Some("llm_overloaded")));
}

#[test]
fn a_failure_after_some_drafting_keeps_the_draft() {
    let h = mock_harness();
    say(&h, &propose("2027-01-19", "19:00", 1));
    h.remote.mock_llm().push(MockStep::Fail(LlmError::Timeout));
    let reply = send(&h, "내일 저녁");
    assert_eq!(reply.fallback_reason.as_deref(), Some("llm_timeout"));
    assert_eq!(reply.draft.expect("kept").items.len(), 1);
}

#[test]
fn tool_results_carry_names_and_counts_but_no_window_titles_apps_or_typed_text() {
    let h = mock_harness();
    {
        let conn = h.db.lock().expect("db");
        conn.execute(
            "INSERT INTO block_events (at_ms, target_kind, target_id, action, mode) VALUES (?1, 'app', 'CANARY_APP_7f3a', 'terminate_requested', 'enforce')",
            params![NOW - 3_600_000],
        )
        .expect("event");
        crate::store::wiki::insert_subject(&conn, "수학", "수학", 1).expect("subject");
    }
    say(&h, r#"{"tool":"get_focus_summary"}"#);
    say(&h, r#"{"tool":"get_wiki_weak_pages"}"#);
    say(&h, r#"{"tool":"list_subjects"}"#);
    say(&h, r#"{"reply":"요약했어요."}"#);
    send(&h, "내 집중 패턴이 어때?");
    let calls = h.remote.mock_llm().calls();
    let sent: String = calls.iter().map(|c| format!("{}\n{}", c.system, c.user)).collect();
    assert!(sent.contains("deviations_by_weekday") && sent.contains("수학"));
    for canary in ["CANARY_APP", crate::remote::payload::CANARY_TITLE, crate::remote::payload::CANARY_URL] {
        assert!(!sent.contains(canary), "{canary} leaked into the prompt");
    }
}

#[test]
fn weak_pages_report_only_pages_with_enough_wrong_answers() {
    let h = mock_harness();
    {
        let conn = h.db.lock().expect("db");
        let subject = crate::store::wiki::insert_subject(&conn, "수학", "수학", 1).expect("subject");
        for (name, total, correct) in [("미분", 5, 1), ("적분", 5, 5), ("극한", 2, 0)] {
            crate::store::wiki::upsert_page(&conn, subject, name, "p", "h", 1).expect("page");
            conn.execute("UPDATE wiki_pages SET quiz_total = ?2, quiz_correct = ?3 WHERE page_name = ?1", params![name, total, correct]).expect("counts");
        }
    }
    say(&h, r#"{"tool":"get_wiki_weak_pages"}"#);
    say(&h, r#"{"reply":"미분이 약해요."}"#);
    send(&h, "약한 부분은?");
    let result = &h.remote.mock_llm().calls()[1].user;
    assert!(result.contains("미분") && result.contains("\"accuracy_pct\":20"), "{result}");
    assert!(!result.contains("적분") && !result.contains("극한"), "{result}");
}

#[test]
fn drafting_changes_no_plan_table_even_when_the_model_proposes_many_operations() {
    let h = mock_harness();
    for day in 18..=22 {
        say(&h, &propose(&format!("2027-01-{day}"), "19:00", 1));
    }
    say(&h, r#"{"reply":"다섯 개."}"#);
    let tables = ["plans", "plan_exceptions", "plan_history"];
    let count = |h: &Harness| -> Vec<i64> {
        let conn = h.db.lock().expect("db");
        tables.iter().map(|t| conn.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0)).expect("count")).collect()
    };
    let before = count(&h);
    let reply = send(&h, "평일 저녁");
    assert_eq!(reply.draft.expect("draft").items.len(), 5);
    assert_eq!(count(&h), before);
}
