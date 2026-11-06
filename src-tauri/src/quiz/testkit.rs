//! Test fixtures: a wiki subject with one page and a hand-made question bank.

use rusqlite::Connection;

use super::generate::NewItem;
use super::{ItemKind, QuizItem};
use crate::store::quiz as repo;
use crate::store::wiki as wiki_repo;

pub struct Bank {
    pub subject_id: i64,
    pub page_id: i64,
    pub items: Vec<QuizItem>,
}

/// `n` questions on one page of the subject "수학" (every third one is a blank; the rest are
/// four-choice). Blank answers are unique canaries that appear nowhere else.
pub fn seed_bank(conn: &Connection, n: usize) -> Bank {
    let subject_id = wiki_repo::insert_subject(conn, "수학", "수학", 1).expect("subject");
    wiki_repo::upsert_page(conn, subject_id, "미분", "pages/미분.md", "h", 1).expect("page");
    let page_id = wiki_repo::get_page(conn, subject_id, "미분").expect("page").expect("row").id;
    add_items(conn, page_id, 0..n);
    let items = (0..n).map(|i| find(conn, page_id, i)).collect();
    Bank { subject_id, page_id, items }
}

fn new_item(i: usize) -> NewItem {
    if i % 3 == 0 {
        NewItem {
            kind: ItemKind::Cloze,
            prompt: format!("문장 {i} 의 빈칸은 ____ 입니다"),
            choices: Vec::new(),
            answer_key: format!("정답카나리{i}"),
            evidence_quote: format!("문장 {i} 의 빈칸은 정답카나리{i} 입니다"),
        }
    } else {
        NewItem {
            kind: ItemKind::Mcq,
            prompt: format!("문제 {i} 의 답은?"),
            choices: vec![format!("보기{i}가"), format!("보기{i}나"), format!("보기{i}다"), format!("보기{i}라")],
            answer_key: format!("보기{i}다"),
            evidence_quote: format!("문제 {i} 의 근거"),
        }
    }
}

pub fn add_items(conn: &Connection, page_id: i64, range: std::ops::Range<usize>) {
    for i in range {
        repo::insert_item(conn, page_id, &new_item(i), 1).expect("item");
    }
}

fn find(conn: &Connection, page_id: i64, i: usize) -> QuizItem {
    let id: i64 = conn
        .query_row("SELECT id FROM quiz_items WHERE page_id = ?1 AND prompt = ?2", rusqlite::params![page_id, new_item(i).prompt], |r| r.get(0))
        .expect("id");
    repo::get_item(conn, id).expect("read").expect("item")
}

/// The right answer for each item, as the UI would submit it.
pub fn right_answers(items: &[QuizItem]) -> Vec<String> {
    items.iter().map(|i| i.answer_key.clone()).collect()
}

pub fn wrong_answer() -> String {
    "전혀 아닌 답".to_string()
}
