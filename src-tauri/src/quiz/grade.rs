//! Local grading. A multiple-choice answer is the chosen choice text; a blank is the word typed.
//! Comparison ignores case and runs of whitespace, nothing else (a different word is wrong).

use super::{ItemKind, QuizItem};

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether `answer` is right for `item`. An empty answer is never right.
pub fn is_correct(item: &QuizItem, answer: &str) -> bool {
    let given = normalize(answer);
    if given.is_empty() {
        return false;
    }
    match item.kind {
        ItemKind::Mcq => {
            item.choices.iter().any(|c| normalize(c) == given)
                && given == normalize(&item.answer_key)
        }
        ItemKind::Cloze => given == normalize(&item.answer_key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: ItemKind, choices: &[&str], answer: &str) -> QuizItem {
        QuizItem {
            id: 1,
            page_id: 1,
            kind,
            prompt: "p".into(),
            choices: choices.iter().map(|c| c.to_string()).collect(),
            answer_key: answer.into(),
        }
    }

    #[test]
    fn a_blank_accepts_case_and_spacing_differences_only() {
        let it = item(ItemKind::Cloze, &[], "Chain Rule");
        assert!(is_correct(&it, "  chain   rule "));
        assert!(!is_correct(&it, "chain"));
        assert!(!is_correct(&it, ""));
        assert!(!is_correct(&it, "   "));
    }

    #[test]
    fn a_choice_must_be_one_of_the_offered_choices_and_the_right_one() {
        let it = item(ItemKind::Mcq, &["도함수", "적분", "극한", "연속"], "도함수");
        assert!(is_correct(&it, "도함수"));
        assert!(!is_correct(&it, "적분"));
        assert!(!is_correct(&it, "정의역"));
    }
}
