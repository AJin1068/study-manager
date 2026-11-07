//! Mutual quiz from the room's shared pages. Questions are cut from the shared text by the local
//! template generator (`quiz::generate`), so no LLM is involved and nothing leaves the PC. Answers
//! are graded locally and are not stored: the personal quiz statistics are not touched.

use crate::quiz::generate::template_items;
use crate::quiz::grade::is_correct;
use crate::quiz::select::{hash_str, mix};
use crate::quiz::{QuizItem, QuizQuestion};

use super::model::{RevisionKind, WirePage};

/// Questions in one round.
pub const ROUND_QUESTIONS: usize = 5;

/// A round of questions with their answers; the answers stay in this struct.
pub struct SharedQuiz {
    items: Vec<(QuizItem, String)>,
    seed: u64,
}

impl std::fmt::Debug for SharedQuiz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SharedQuiz({} questions)", self.items.len())
    }
}

impl SharedQuiz {
    /// Draws up to `ROUND_QUESTIONS` questions from the shared pages, spread by `seed`. A page's
    /// merged revision is used when there is one, otherwise every member's newest edit.
    pub fn draw(pages: &[WirePage], seed: u64) -> Self {
        let mut pool: Vec<(QuizItem, String)> = Vec::new();
        for page in pages {
            let merged = page
                .revisions
                .iter()
                .filter(|r| r.kind == RevisionKind::Merge && !r.body.trim().is_empty())
                .max_by_key(|r| r.created_at_ms);
            let bodies: Vec<&str> = match merged {
                Some(m) => vec![m.body.as_str()],
                None => page
                    .revisions
                    .iter()
                    .filter(|r| !r.body.trim().is_empty())
                    .map(|r| r.body.as_str())
                    .collect(),
            };
            for body in bodies {
                for item in template_items(body) {
                    pool.push((
                        QuizItem {
                            id: 0,
                            page_id: 0,
                            kind: item.kind,
                            prompt: item.prompt,
                            choices: item.choices,
                            answer_key: item.answer_key,
                        },
                        page.title.clone(),
                    ));
                }
            }
        }
        pool.sort_by_key(|(item, _)| mix(seed ^ hash_str(&item.prompt)));
        pool.dedup_by(|a, b| a.0.prompt == b.0.prompt);
        pool.truncate(ROUND_QUESTIONS);
        for (i, (item, _)) in pool.iter_mut().enumerate() {
            item.id = i64::try_from(i + 1).unwrap_or(i64::MAX);
        }
        Self { items: pool, seed }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// What the UI shows: no answer, no evidence.
    pub fn questions(&self) -> Vec<(QuizQuestion, String)> {
        self.items
            .iter()
            .map(|(item, title)| (item.view(self.seed), title.clone()))
            .collect()
    }

    /// Whether `answer` is right for question `item_id`; `None` for an unknown question.
    pub fn grade(&self, item_id: i64, answer: &str) -> Option<bool> {
        self.items
            .iter()
            .find(|(item, _)| item.id == item_id)
            .map(|(item, _)| is_correct(item, answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_wiki::model::WireRevision;

    const BODY: &str = "- 도함수는 함수의 순간 변화율을 나타내는 함수이다.\n- 합성함수의 미분은 연쇄법칙으로 계산한다.\n- 극한값이 존재하면 함수는 그 점에서 수렴한다고 말한다.\n";

    fn page(title: &str, kind: RevisionKind, body: &str, at: i64) -> WirePage {
        WirePage {
            page_id: title.into(),
            title: title.into(),
            revisions: vec![WireRevision {
                revision_id: format!("r{at}"),
                kind,
                author: "가".into(),
                is_mine: false,
                body: body.into(),
                allow_llm_merge: false,
                branched: false,
                base_revision_id: None,
                contributors: vec!["가".into()],
                created_at_ms: at,
            }],
        }
    }

    #[test]
    fn a_round_is_cut_from_shared_text_and_is_deterministic() {
        let pages = [page("미분", RevisionKind::Edit, BODY, 1)];
        let a = SharedQuiz::draw(&pages, 7);
        let b = SharedQuiz::draw(&pages, 7);
        assert!(!a.is_empty() && a.len() <= ROUND_QUESTIONS);
        assert_eq!(a.questions(), b.questions());
        assert!(
            a.questions()
                .iter()
                .all(|(q, title)| !q.prompt.is_empty() && title == "미분")
        );
    }

    #[test]
    fn grading_is_local_and_unknown_questions_are_none() {
        let quiz = SharedQuiz::draw(&[page("미분", RevisionKind::Edit, BODY, 1)], 1);
        let (question, _) = &quiz.questions()[0];
        assert_eq!(quiz.grade(question.item_id, "전혀 아닌 답"), Some(false));
        assert_eq!(quiz.grade(999, "x"), None);
        let right = quiz
            .items
            .iter()
            .find(|(i, _)| i.id == question.item_id)
            .map(|(i, _)| i.answer_key.clone())
            .expect("item");
        assert_eq!(quiz.grade(question.item_id, &right), Some(true));
    }

    #[test]
    fn no_question_text_carries_the_answer_to_the_ui() {
        let quiz = SharedQuiz::draw(&[page("미분", RevisionKind::Edit, BODY, 1)], 3);
        for (item, _) in &quiz.items {
            let (q, _) = quiz
                .questions()
                .into_iter()
                .find(|(q, _)| q.item_id == item.id)
                .expect("question");
            if item.choices.is_empty() {
                assert!(!q.prompt.contains(&item.answer_key));
            }
        }
    }

    #[test]
    fn a_merged_revision_replaces_the_single_edits_as_the_source() {
        let mut p = page(
            "미분",
            RevisionKind::Edit,
            "- 단독 편집만 있는 문장은 여기에 하나 있다.\n",
            1,
        );
        p.revisions.push(
            page("미분", RevisionKind::Merge, BODY, 2)
                .revisions
                .remove(0),
        );
        let quiz = SharedQuiz::draw(&[p], 1);
        assert!(
            quiz.questions()
                .iter()
                .all(|(q, _)| !q.prompt.contains("단독"))
        );
    }

    #[test]
    fn pages_without_sentences_make_an_empty_round() {
        assert!(SharedQuiz::draw(&[page("제목", RevisionKind::Edit, "# 제목만", 1)], 1).is_empty());
        assert!(SharedQuiz::draw(&[], 1).is_empty());
    }
}
