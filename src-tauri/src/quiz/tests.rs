use chrono::Utc;
use rusqlite::{params, Connection};

use super::gate::QuizGate;
use super::generate::NewItem;
use super::testkit::{right_answers, seed_bank, wrong_answer};
use super::{Context, ItemKind, QuizItem};
use crate::db::open_in_memory;
use crate::points::rewards;
use crate::store::quiz as repo;

const NOW: i64 = 1_800_000_000_000;

fn list_sql(conn: &Connection, table: &str) -> String {
    conn.query_row("SELECT sql FROM sqlite_master WHERE name = ?1", params![table], |r| r.get(0)).expect("schema")
}

/// The quoted values of the first `column IN (...)` list of a table's DDL.
fn check_list(sql: &str, column: &str) -> Vec<String> {
    let from = sql.find(&format!("{column} IN (")).expect("list") + column.len() + 5;
    let body = &sql[from..from + sql[from..].find(')').expect("end")];
    body.split(',').map(|v| v.trim().trim_matches('\'').to_string()).collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn the_enums_match_the_schema_check_lists() {
    let conn = open_in_memory().expect("db");
    let contexts = check_list(&list_sql(&conn, "quiz_attempts"), "context");
    assert_eq!(sorted(contexts), sorted(Context::ALL.iter().map(|c| c.as_str().to_string()).collect()));
    let kinds = check_list(&list_sql(&conn, "quiz_items"), "kind");
    assert_eq!(sorted(kinds), sorted(ItemKind::ALL.iter().map(|k| k.as_str().to_string()).collect()));
}

#[test]
fn attempts_are_append_only_and_the_release_tag_is_early_release_only() {
    let conn = open_in_memory().expect("db");
    let bank = seed_bank(&conn, 3);
    let attempt = |context: &str, release_kind: Option<&str>, correct: bool, skipped: bool| {
        repo::insert_attempt(
            &conn,
            &repo::NewAttempt { at_ms: NOW, item_id: bank.items[0].id, context, release_kind, run_id: None, segment_id: None, correct, skipped, response_ms: 5 },
        )
    };
    assert!(attempt("release", Some("early_release"), true, false).is_ok());
    assert!(attempt("session_end", None, false, true).is_ok());
    assert!(attempt("distraction", Some("early_release"), true, false).is_err(), "only the release context carries the tag");
    assert!(attempt("release", Some("skip_today"), true, false).is_err());
    assert!(attempt("session_end", None, true, true).is_err(), "a skipped question is never correct");
    assert!(attempt("homework", None, true, false).is_err());
    assert!(conn.execute("UPDATE quiz_attempts SET correct = 1", []).is_err());
    assert!(conn.execute("DELETE FROM quiz_attempts", []).is_err());
}

#[test]
fn a_skipped_attempt_is_recorded_but_not_counted_on_the_page() {
    let conn = open_in_memory().expect("db");
    let bank = seed_bank(&conn, 3);
    let add = |correct: bool, skipped: bool| {
        repo::insert_attempt(
            &conn,
            &repo::NewAttempt { at_ms: NOW, item_id: bank.items[0].id, context: "session_end", release_kind: None, run_id: None, segment_id: None, correct, skipped, response_ms: 1 },
        )
        .expect("attempt");
    };
    add(true, false);
    add(false, false);
    add(false, true);
    let (total, correct): (i64, i64) =
        conn.query_row("SELECT quiz_total, quiz_correct FROM wiki_pages WHERE id = ?1", params![bank.page_id], |r| Ok((r.get(0)?, r.get(1)?))).expect("page");
    assert_eq!((total, correct), (2, 1));
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM quiz_attempts", [], |r| r.get(0)).expect("rows");
    assert_eq!(rows, 3);
}

#[test]
fn a_reported_question_is_never_drawn_again_and_the_report_is_refused_under_lock() {
    let conn = open_in_memory().expect("db");
    let bank = seed_bank(&conn, 4);
    crate::testutil::set_lock_for_test(&conn, true);
    assert!(super::gate::flag_item(&conn, bank.items[0].id, "wrong_answer", NOW).is_err(), "no reports from inside a locked block");
    assert_eq!(repo::drawable_count(&conn).expect("count"), 4);
    crate::testutil::set_lock_for_test(&conn, false);
    super::gate::flag_item(&conn, bank.items[0].id, "wrong_answer", NOW).expect("flag");
    assert!(super::gate::flag_item(&conn, 9_999, "other", NOW).is_err(), "unknown item");
    assert_eq!(repo::drawable_count(&conn).expect("count"), 3);
    assert!(repo::candidates(&conn, None).expect("candidates").iter().all(|c| c.item.id != bank.items[0].id));
    for seed in 0..20 {
        let gate = QuizGate::default();
        let card = gate.offer_review(&conn, NOW + seed, None, 3).expect("card").expect("card");
        assert!(card.questions.iter().all(|q| q.item_id != bank.items[0].id));
    }
}

fn answer_all(gate: &QuizGate, conn: &mut Connection, card: &super::gate::QuizCard, now: i64, right: bool) -> super::gate::CardResult {
    let answers: Vec<String> = card
        .questions
        .iter()
        .map(|q| {
            if right {
                repo::get_item(conn, q.item_id).expect("read").expect("item").answer_key
            } else {
                wrong_answer()
            }
        })
        .collect();
    gate.answer(conn, &Utc, card.card_id, &answers, false, now).expect("answer").expect("result")
}

fn ledger(conn: &Connection) -> (i64, i64) {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(delta), 0) FROM point_ledger WHERE reason_code = ?1",
        params![rewards::REASON_QUIZ_BONUS],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .expect("ledger")
}

#[test]
fn bonus_points_are_idempotent_by_attempt_and_capped_per_day() {
    let mut conn = open_in_memory().expect("db");
    seed_bank(&conn, 12);
    let gate = QuizGate::default();
    let first = gate.offer_review(&conn, NOW, None, 3).expect("card").expect("card");
    let result = answer_all(&gate, &mut conn, &first, NOW + 1_000, true);
    assert_eq!(result.bonus_points, 3 * rewards::QUIZ_BONUS_POINTS);
    assert!(result.results.iter().all(|r| r.correct));
    // The same card cannot be answered twice.
    assert!(gate.answer(&mut conn, &Utc, first.card_id, &[], false, NOW + 2_000).expect("again").is_none());
    assert_eq!(ledger(&conn), (3, 3 * rewards::QUIZ_BONUS_POINTS));
    // Every row is keyed by its attempt, and posting a key twice adds nothing.
    let key: String = conn.query_row("SELECT ref_key FROM point_ledger WHERE reason_code = 'quiz_bonus' LIMIT 1", [], |r| r.get(0)).expect("key");
    assert!(key.starts_with("quiz:attempt:"));
    assert!(!crate::store::insert_ledger_entry(&conn, NOW, 99, rewards::REASON_QUIZ_BONUS, &key).expect("repeat"));

    // 3 more right answers: only 2 more fit under the cap of 5 a day.
    let second = gate.offer_review(&conn, NOW + 3_000, None, 3).expect("card").expect("card");
    let result = answer_all(&gate, &mut conn, &second, NOW + 4_000, true);
    assert_eq!(result.bonus_points, 2 * rewards::QUIZ_BONUS_POINTS);
    assert_eq!(ledger(&conn).0, rewards::QUIZ_BONUS_DAILY_CAP);
    let third = gate.offer_review(&conn, NOW + 5_000, None, 3).expect("card").expect("card");
    assert_eq!(answer_all(&gate, &mut conn, &third, NOW + 6_000, true).bonus_points, 0, "over the cap pays nothing");
    assert_eq!(ledger(&conn).0, rewards::QUIZ_BONUS_DAILY_CAP);
}

#[test]
fn wrong_answers_pay_nothing_and_a_dismissed_review_card_leaves_no_rows() {
    let mut conn = open_in_memory().expect("db");
    seed_bank(&conn, 8);
    let gate = QuizGate::default();
    let card = gate.offer_review(&conn, NOW, None, 3).expect("card").expect("card");
    let result = answer_all(&gate, &mut conn, &card, NOW + 1, false);
    assert_eq!((result.bonus_points, result.results.iter().filter(|r| r.correct).count()), (0, 0));
    assert_eq!(ledger(&conn), (0, 0));
    let card = gate.offer_review(&conn, NOW + 10, None, 1).expect("card").expect("card");
    let dismissed = gate.answer(&mut conn, &Utc, card.card_id, &[], true, NOW + 11).expect("dismiss").expect("result");
    assert!(dismissed.skipped && dismissed.results.is_empty());
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM quiz_attempts", [], |r| r.get(0)).expect("rows");
    assert_eq!(rows, 3, "only the three graded answers of the first card");
}

#[test]
fn the_card_the_ui_receives_has_no_answer_and_review_does_not_repeat_within_a_day() {
    let conn = open_in_memory().expect("db");
    let bank = seed_bank(&conn, 6);
    let gate = QuizGate::default();
    let card = gate.offer_review(&conn, NOW, None, 3).expect("card").expect("card");
    let json = serde_json::to_string(&card).expect("json");
    for item in bank.items.iter().filter(|i| i.kind == ItemKind::Cloze) {
        assert!(!json.contains(&item.answer_key), "{json}");
    }
    assert!(!json.contains("answerKey") && !json.contains("evidence") && !json.contains("\"answer\""));
    let mcq = card.questions.iter().find(|q| q.kind == ItemKind::Mcq);
    if let Some(q) = mcq {
        assert_eq!(q.choices.len(), 4);
    }
}

#[test]
fn drawing_prefers_fresh_questions_and_never_repeats_inside_a_day() {
    let mut conn = open_in_memory().expect("db");
    seed_bank(&conn, 6);
    let gate = QuizGate::default();
    let a = gate.offer_review(&conn, NOW, None, 3).expect("card").expect("card");
    let first_ids: Vec<i64> = a.questions.iter().map(|q| q.item_id).collect();
    answer_all(&gate, &mut conn, &a, NOW + 1, false);
    let b = gate.offer_review(&conn, NOW + 10, None, 3).expect("card").expect("card");
    assert!(b.questions.iter().all(|q| !first_ids.contains(&q.item_id)), "the other three come first");
    answer_all(&gate, &mut conn, &b, NOW + 11, false);
    assert!(gate.offer_review(&conn, NOW + 20, None, 3).expect("card").is_none(), "nothing fresh is left within 24 hours");
}

#[test]
fn bank_generation_works_with_the_llm_off_and_through_the_mock_and_rejects_bad_model_output() {
    use crate::llm::config::{update_setting, LlmSettingKey};
    use crate::llm::service::tests::mock_harness;
    use crate::settings::SettingValue;
    use crate::testutil::TempDir;
    use crate::wiki::ingest::SourceKind;
    use crate::wiki::pdf::NoPdfExtractor;
    use crate::wiki::service::WikiService;
    use std::sync::Arc;

    let page = "# 미분\n\n- 도함수는 함수의 순간 변화율을 나타내는 함수이다.\n- 합성함수의 미분은 연쇄법칙으로 계산한다.\n- 극한값이 존재하면 함수는 그 점에서 수렴한다고 말한다.\n";
    let build = |llm_on: bool| {
        let h = mock_harness();
        let dir = TempDir::new("quiz-bank");
        {
            let mut conn = h.db.lock().expect("db");
            let root = dir.path().join("wiki");
            update_setting(&mut conn, LlmSettingKey::WikiDir, SettingValue::Text(root.to_string_lossy().into_owned()), 1).expect("dir");
        }
        let wiki = Arc::new(WikiService::new(Arc::clone(&h.db), Arc::clone(&h.service), h.clock.clone(), Arc::new(NoPdfExtractor)));
        let subject = wiki.create_subject("수학").expect("subject");
        let view = wiki.ingest_text(subject.id, SourceKind::Paste, "", page).expect("ingest");
        wiki.apply_ingest(&view.preview_id.expect("id"), None).expect("apply");
        if !llm_on {
            let mut conn = h.db.lock().expect("db");
            update_setting(&mut conn, LlmSettingKey::Enabled, SettingValue::Bool(false), 2).expect("off");
        }
        let quiz = super::service::QuizService::new(Arc::clone(&h.db), wiki, Arc::clone(&h.service), h.clock.clone());
        (h, dir, quiz, subject.id)
    };

    // LLM off: the template generator alone makes a bank, and no quiz_gen call is recorded.
    let (h, _dir, quiz, subject_id) = build(false);
    let report = quiz.generate_for_subject(subject_id).expect("generate");
    assert!(report.added >= 3 && report.from_model == 0 && report.from_template > 0, "{report:?}");
    assert_eq!(h.db.lock().expect("db").query_row("SELECT COUNT(*) FROM llm_usage WHERE purpose = 'quiz_gen'", [], |r| r.get::<_, i64>(0)).expect("n"), 0);
    assert_eq!(quiz.generate_for_subject(subject_id).expect("again").added, 0, "pages that have questions are skipped");

    // Mock LLM on: the mock answers in the model format and the same validator accepts it.
    let (h, _dir, quiz, subject_id) = build(true);
    let report = quiz.generate_for_subject(subject_id).expect("generate");
    assert!(report.added >= 3 && report.from_model >= 3 && report.dropped == 0, "{report:?}");
    assert_eq!(h.db.lock().expect("db").query_row("SELECT COUNT(*) FROM llm_usage WHERE purpose = 'quiz_gen'", [], |r| r.get::<_, i64>(0)).expect("n"), 1);
    let stats = quiz.stats().expect("stats");
    assert_eq!(stats.drawable, i64::try_from(report.added).expect("count"));
}

#[test]
fn the_unique_prompt_per_page_keeps_regeneration_from_duplicating() {
    let conn = open_in_memory().expect("db");
    let bank = seed_bank(&conn, 3);
    let again = NewItem {
        kind: ItemKind::Cloze,
        prompt: bank.items[0].prompt.clone(),
        choices: Vec::new(),
        answer_key: "x".into(),
        evidence_quote: "y".into(),
    };
    assert!(!repo::insert_item(&conn, bank.page_id, &again, NOW).expect("insert"));
    assert_eq!(repo::count_for_page(&conn, bank.page_id).expect("count"), 3);
    let items: Vec<QuizItem> = bank.items.clone();
    assert_eq!(right_answers(&items).len(), 3);
}
