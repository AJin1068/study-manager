//! Turning wiki page text into questions, and refusing bad ones. Two sources produce the same
//! `NewItem`: the local template generator (fill-in-the-blank and multiple choice cut from the
//! page's own sentences, no LLM, works offline) and a model answer parsed by `parse_items`. A
//! model-made question is kept only if it is well-formed, has exactly one right choice, and quotes
//! the page: `evidence_quote` must be a substring of the page text, otherwise the question is a
//! hallucination and is dropped.

use serde::Deserialize;

use super::ItemKind;
use crate::wiki::ingest::json_object;

/// The gap in a fill-in-the-blank prompt.
pub const CLOZE_MARK: &str = "____";
/// Questions kept per page and generation run.
pub const MAX_ITEMS_PER_PAGE: usize = 8;
const MIN_PROMPT_CHARS: usize = 5;
const MAX_PROMPT_CHARS: usize = 300;
const MAX_ANSWER_CHARS: usize = 60;
const MAX_CHOICE_CHARS: usize = 80;
const MAX_EVIDENCE_CHARS: usize = 400;
const MIN_SENTENCE_CHARS: usize = 15;
const MIN_SENTENCE_WORDS: usize = 4;
const MCQ_CHOICES: usize = 4;

/// A question ready to be stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewItem {
    pub kind: ItemKind,
    pub prompt: String,
    /// Empty for a blank.
    pub choices: Vec<String>,
    pub answer_key: String,
    pub evidence_quote: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawItems {
    items: Vec<RawItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawItem {
    kind: String,
    prompt: String,
    #[serde(default)]
    choices: Vec<String>,
    answer: String,
    evidence_quote: String,
}

fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalized(text: &str) -> String {
    squeeze(text).to_lowercase()
}

fn clean_text(text: &str, max: usize) -> Option<String> {
    let t = squeeze(text);
    (!t.is_empty() && t.chars().count() <= max && !t.contains('\0')).then_some(t)
}

fn validate(raw: RawItem, body: &str) -> Option<NewItem> {
    let kind = ItemKind::parse(raw.kind.trim())?;
    let prompt = clean_text(&raw.prompt, MAX_PROMPT_CHARS)
        .filter(|p| p.chars().count() >= MIN_PROMPT_CHARS)?;
    let answer = clean_text(&raw.answer, MAX_ANSWER_CHARS)?;
    let evidence = clean_text(&raw.evidence_quote, MAX_EVIDENCE_CHARS)?;
    if !squeeze(body).contains(&evidence) {
        return None;
    }
    match kind {
        ItemKind::Cloze => {
            let leaks = normalized(&prompt).contains(&normalized(&answer));
            let in_evidence = normalized(&evidence).contains(&normalized(&answer));
            (prompt.matches(CLOZE_MARK).count() == 1 && !leaks && in_evidence).then_some(NewItem {
                kind,
                prompt,
                choices: Vec::new(),
                answer_key: answer,
                evidence_quote: evidence,
            })
        }
        ItemKind::Mcq => {
            let choices: Vec<String> = raw
                .choices
                .iter()
                .map(|c| clean_text(c, MAX_CHOICE_CHARS))
                .collect::<Option<_>>()?;
            if !(4..=5).contains(&choices.len()) {
                return None;
            }
            let mut seen: Vec<String> = choices.iter().map(|c| normalized(c)).collect();
            seen.sort();
            seen.dedup();
            if seen.len() != choices.len() {
                return None;
            }
            let right: Vec<&String> = choices
                .iter()
                .filter(|c| normalized(c) == normalized(&answer))
                .collect();
            let [only] = right.as_slice() else {
                return None;
            };
            let answer_key = (*only).clone();
            Some(NewItem {
                kind,
                prompt,
                choices,
                answer_key,
                evidence_quote: evidence,
            })
        }
    }
}

/// Reads a model answer (`{"items":[...]}`) against the page text it was made from. Returns the
/// valid questions (at most [`MAX_ITEMS_PER_PAGE`], no repeated prompt) and how many were dropped.
/// An answer that is not the expected JSON yields nothing.
pub fn parse_items(text: &str, body: &str) -> (Vec<NewItem>, usize) {
    let Some(json) = json_object(text) else {
        return (Vec::new(), 0);
    };
    let Ok(raw) = serde_json::from_str::<RawItems>(json) else {
        return (Vec::new(), 0);
    };
    let total = raw.items.len();
    let mut kept: Vec<NewItem> = Vec::new();
    for item in raw.items {
        if kept.len() >= MAX_ITEMS_PER_PAGE {
            break;
        }
        if let Some(valid) = validate(item, body) {
            if !kept
                .iter()
                .any(|k| k.prompt == valid.prompt && k.kind == valid.kind)
            {
                kept.push(valid);
            }
        }
    }
    let dropped = total - kept.len();
    (kept, dropped)
}

// ---- local template generator ----

fn sentences(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with("관련:")
            || line.contains("[[")
            || line.contains(['*', '`', '|'])
        {
            continue;
        }
        let line = line.trim_start_matches(['-', '•']).trim_start();
        let line = match line.split_once(". ") {
            Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest,
            _ => line,
        };
        for piece in line.split(['.', '!', '?', '。']) {
            let piece = piece.trim();
            if piece.chars().count() >= MIN_SENTENCE_CHARS
                && piece.split_whitespace().count() >= MIN_SENTENCE_WORDS
            {
                out.push(piece.to_string());
            }
        }
    }
    out
}

fn words(sentence: &str) -> Vec<String> {
    sentence
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| (2..=20).contains(&w.chars().count()) && w.chars().all(char::is_alphanumeric))
        .map(str::to_string)
        .collect()
}

/// The blanked word of a sentence: the longest word that occurs once in it.
fn blank_word(sentence: &str) -> Option<String> {
    let mut candidates = words(sentence);
    candidates.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    candidates
        .into_iter()
        .find(|w| sentence.matches(w.as_str()).count() == 1)
}

/// Fill-in-the-blank questions cut from the page's own sentences, plus a multiple-choice version
/// of every second one when the page has enough other words to use as wrong choices. Deterministic.
pub fn template_items(body: &str) -> Vec<NewItem> {
    let all_words: Vec<String> = {
        let mut w: Vec<String> = sentences(body).iter().flat_map(|s| words(s)).collect();
        w.sort();
        w.dedup();
        w
    };
    let mut out = Vec::new();
    for (index, sentence) in sentences(body).into_iter().enumerate() {
        let Some(answer) = blank_word(&sentence) else {
            continue;
        };
        let blanked = sentence.replacen(&answer, CLOZE_MARK, 1);
        out.push(NewItem {
            kind: ItemKind::Cloze,
            prompt: blanked.clone(),
            choices: Vec::new(),
            answer_key: answer.clone(),
            evidence_quote: sentence.clone(),
        });
        if index % 2 == 1 {
            let mut distractors: Vec<&String> = all_words
                .iter()
                .filter(|w| normalized(w) != normalized(&answer) && !sentence.contains(w.as_str()))
                .collect();
            distractors.sort_by_key(|w| {
                (
                    w.chars().count().abs_diff(answer.chars().count()),
                    w.to_string(),
                )
            });
            if distractors.len() >= MCQ_CHOICES - 1 {
                let mut choices: Vec<String> = distractors
                    .into_iter()
                    .take(MCQ_CHOICES - 1)
                    .cloned()
                    .collect();
                choices.push(answer.clone());
                out.push(NewItem {
                    kind: ItemKind::Mcq,
                    prompt: format!("빈칸에 들어갈 말은? {blanked}"),
                    choices,
                    answer_key: answer,
                    evidence_quote: sentence,
                });
            }
        }
        if out.len() >= MAX_ITEMS_PER_PAGE {
            break;
        }
    }
    out.truncate(MAX_ITEMS_PER_PAGE);
    out
}

/// The template questions in the model's answer format; what the mock LLM answers a bank request
/// with, so the mock path exercises the same parser as a real model.
pub fn template_json(body: &str) -> String {
    let items: Vec<_> = template_items(body)
        .into_iter()
        .map(|i| serde_json::json!({"kind": i.kind.as_str(), "prompt": i.prompt, "choices": i.choices, "answer": i.answer_key, "evidence_quote": i.evidence_quote}))
        .collect();
    serde_json::json!({"items": items}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "# 미분\n\n- 도함수는 함수의 순간 변화율을 나타내는 함수이다.\n- 합성함수의 미분은 연쇄법칙으로 계산한다.\n- 극한값이 존재하면 함수는 그 점에서 수렴한다고 말한다.\n\n관련: [[적분]]\n";

    fn model_item(
        kind: &str,
        prompt: &str,
        choices: &[&str],
        answer: &str,
        quote: &str,
    ) -> serde_json::Value {
        serde_json::json!({"kind": kind, "prompt": prompt, "choices": choices, "answer": answer, "evidence_quote": quote})
    }

    fn reply(items: Vec<serde_json::Value>) -> String {
        serde_json::json!({"items": items}).to_string()
    }

    #[test]
    fn a_well_formed_question_quoting_the_page_is_kept() {
        let text = reply(vec![
            model_item(
                "mcq",
                "도함수는 무엇을 나타내나?",
                &["순간 변화율", "넓이", "평균", "누적"],
                "순간 변화율",
                "도함수는 함수의 순간 변화율을 나타내는 함수이다",
            ),
            model_item(
                "cloze",
                "합성함수의 미분은 ____으로 계산한다",
                &[],
                "연쇄법칙",
                "합성함수의 미분은 연쇄법칙으로 계산한다",
            ),
        ]);
        let (items, dropped) = parse_items(&text, PAGE);
        assert_eq!((items.len(), dropped), (2, 0));
        assert_eq!(items[0].answer_key, "순간 변화율");
    }

    #[test]
    fn a_quote_that_is_not_on_the_page_discards_the_question() {
        let text = reply(vec![model_item(
            "cloze",
            "미분은 ____이다",
            &[],
            "마법",
            "미분은 마법이다",
        )]);
        assert_eq!(parse_items(&text, PAGE), (Vec::new(), 1));
    }

    #[test]
    fn exactly_one_right_choice_and_four_or_five_choices_are_required() {
        let quote = "도함수는 함수의 순간 변화율을 나타내는 함수이다";
        for (choices, answer) in [
            (
                vec!["순간 변화율", "순간 변화율", "넓이", "평균"],
                "순간 변화율",
            ),
            (vec!["넓이", "평균", "누적", "합"], "순간 변화율"),
            (vec!["순간 변화율", "넓이", "평균"], "순간 변화율"),
            (vec!["순간 변화율", "a", "b", "c", "d", "e"], "순간 변화율"),
        ] {
            let text = reply(vec![model_item(
                "mcq",
                "도함수는?",
                &choices,
                answer,
                quote,
            )]);
            assert!(parse_items(&text, PAGE).0.is_empty(), "{choices:?}");
        }
    }

    #[test]
    fn a_blank_must_have_one_gap_and_must_not_show_its_answer() {
        let quote = "합성함수의 미분은 연쇄법칙으로 계산한다";
        for prompt in [
            "합성함수의 미분은 연쇄법칙으로 계산한다",
            "____의 ____은 연쇄법칙",
            "연쇄법칙은 ____ 이다",
        ] {
            let text = reply(vec![model_item("cloze", prompt, &[], "연쇄법칙", quote)]);
            assert!(parse_items(&text, PAGE).0.is_empty(), "{prompt}");
        }
    }

    #[test]
    fn non_json_unknown_fields_and_unknown_kinds_yield_nothing() {
        assert_eq!(parse_items("그냥 문장", PAGE), (Vec::new(), 0));
        assert_eq!(
            parse_items("{\"items\":[],\"path\":\"/etc\"}", PAGE),
            (Vec::new(), 0)
        );
        let text = reply(vec![model_item(
            "essay",
            "서술하시오",
            &[],
            "x",
            "도함수는 함수의 순간 변화율을 나타내는 함수이다",
        )]);
        assert_eq!(parse_items(&text, PAGE), (Vec::new(), 1));
    }

    #[test]
    fn template_questions_are_valid_by_the_same_rules_and_deterministic() {
        let items = template_items(PAGE);
        assert!(!items.is_empty());
        assert_eq!(items, template_items(PAGE));
        // Round-trips through the model format and the validator unchanged.
        let (parsed, dropped) = parse_items(&template_json(PAGE), PAGE);
        assert_eq!((parsed, dropped), (items.clone(), 0));
        for item in &items {
            assert!(PAGE.contains(&item.evidence_quote), "{item:?}");
            if item.kind == ItemKind::Cloze {
                assert!(
                    item.prompt.contains(CLOZE_MARK) && !item.prompt.contains(&item.answer_key)
                );
            }
        }
    }

    #[test]
    fn a_page_without_sentences_makes_no_questions() {
        assert!(template_items("# 제목만\n\n관련: [[다른]]").is_empty());
        assert!(template_items("").is_empty());
    }

    #[test]
    fn a_long_page_is_capped() {
        let body: String = (0..30)
            .map(|i| format!("- 항목{i}번 문서는 서로 다른 개념{i}을 설명하는 문장이다.\n"))
            .collect();
        assert!(template_items(&body).len() <= MAX_ITEMS_PER_PAGE);
    }
}
