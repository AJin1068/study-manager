//! Checking a model's answer to a question about wiki pages. The model may only cite pages it was
//! given; a citation outside that set is dropped, and an answer left with no valid citation is
//! discarded (a confident answer that points at nothing is the hallucination this guards against).

use serde::Deserialize;

use super::ingest::json_object;

pub const MAX_ANSWER_CHARS: usize = 4_000;
/// What the model says when the pages do not contain the answer.
pub const NOT_IN_WIKI: &str = "위키에 없음";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAnswer {
    answer: String,
    #[serde(default)]
    citations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnswerCheck {
    Grounded { answer: String, citations: Vec<String>, dropped: usize },
    /// The model said the pages do not answer it.
    NotInWiki,
    /// An answer with no citation inside the retrieved set.
    Ungrounded,
    /// Not the expected JSON, empty, or too long.
    Malformed,
}

/// `retrieved` are the names of the pages that were sent with the question.
pub fn check_answer(text: &str, retrieved: &[String]) -> AnswerCheck {
    let Some(json) = json_object(text) else { return AnswerCheck::Malformed };
    let Ok(raw) = serde_json::from_str::<RawAnswer>(json) else { return AnswerCheck::Malformed };
    let answer = raw.answer.trim().to_string();
    if answer.is_empty() || answer.chars().count() > MAX_ANSWER_CHARS || answer.contains('\0') {
        return AnswerCheck::Malformed;
    }
    let mut citations: Vec<String> = Vec::new();
    let mut dropped = 0;
    for cited in &raw.citations {
        if retrieved.contains(cited) {
            if !citations.contains(cited) {
                citations.push(cited.clone());
            }
        } else {
            dropped += 1;
        }
    }
    if citations.is_empty() {
        return if answer.starts_with(NOT_IN_WIKI) { AnswerCheck::NotInWiki } else { AnswerCheck::Ungrounded };
    }
    AnswerCheck::Grounded { answer, citations, dropped }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retrieved() -> Vec<String> {
        vec!["미분".into(), "적분".into()]
    }

    fn reply(answer: &str, citations: &[&str]) -> String {
        serde_json::json!({"answer": answer, "citations": citations}).to_string()
    }

    #[test]
    fn citations_inside_the_retrieved_set_are_kept_and_others_dropped() {
        let check = check_answer(&reply("도함수는 변화율이다.", &["미분", "존재하지 않는 페이지", "미분", "../etc"]), &retrieved());
        assert_eq!(check, AnswerCheck::Grounded { answer: "도함수는 변화율이다.".into(), citations: vec!["미분".into()], dropped: 2 });
    }

    #[test]
    fn an_answer_citing_only_unknown_pages_is_discarded() {
        assert_eq!(check_answer(&reply("자신 있는 답", &["지어낸 페이지"]), &retrieved()), AnswerCheck::Ungrounded);
        assert_eq!(check_answer(&reply("자신 있는 답", &[]), &retrieved()), AnswerCheck::Ungrounded);
    }

    #[test]
    fn saying_the_wiki_lacks_it_is_its_own_outcome() {
        assert_eq!(check_answer(&reply("위키에 없음", &[]), &retrieved()), AnswerCheck::NotInWiki);
        assert_eq!(check_answer(&reply("위키에 없음. 다른 자료가 필요해요", &["없는 페이지"]), &retrieved()), AnswerCheck::NotInWiki);
    }

    #[test]
    fn malformed_answers_are_rejected() {
        for text in [
            "그냥 문장".to_string(),
            "{\"answer\": ".to_string(),
            serde_json::json!({"answer": "x", "citations": [], "path": "/etc"}).to_string(),
            serde_json::json!({"citations": ["미분"]}).to_string(),
            reply("   ", &["미분"]),
            reply(&"가".repeat(MAX_ANSWER_CHARS + 1), &["미분"]),
        ] {
            assert_eq!(check_answer(&text, &retrieved()), AnswerCheck::Malformed, "{text}");
        }
    }

    #[test]
    fn a_fenced_answer_is_read() {
        let fenced = format!("```json\n{}\n```", reply("답", &["적분"]));
        assert!(matches!(check_answer(&fenced, &retrieved()), AnswerCheck::Grounded { .. }));
    }
}
