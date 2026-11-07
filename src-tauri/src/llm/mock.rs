//! Mock LLM: deterministic template answers derived from the prompt material, plus a scripted
//! queue for tests. Nothing leaves the device. Used in mock mode (the UI shows "모의 LLM") and by
//! every stage-3 test that needs a model.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::json;

use super::anthropic::interpret;
use super::egress::{AdjustItem, PromptInput, PromptPage};
use super::{LlmClient, LlmError, LlmRequest, LlmResponse, PageDraft, Purpose};
use crate::plan_shift::{ShiftItem, ShiftPlan, compute_shift};
use crate::remote::http::{HttpResponse, TransportError};

/// Calls kept for inspection. Older ones are dropped.
const KEEP_CALLS: usize = 64;

/// One scripted outcome, consumed by the next call.
#[derive(Clone, Debug)]
pub enum MockStep {
    /// The model answers with exactly this text.
    Text(String),
    /// A failure, as the port reports it.
    Fail(LlmError),
    /// A raw wire outcome, classified by the same function the real adapter uses.
    Raw(Result<HttpResponse, TransportError>),
}

/// What a call carried, for assertions about what would have been sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedCall {
    pub purpose: Purpose,
    pub system: String,
    pub user: String,
}

#[derive(Default)]
pub struct MockLlm {
    script: Mutex<VecDeque<MockStep>>,
    calls: Mutex<Vec<RecordedCall>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl MockLlm {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_script(steps: impl IntoIterator<Item = MockStep>) -> Self {
        Self {
            script: Mutex::new(steps.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn push(&self, step: MockStep) {
        lock(&self.script).push_back(step);
    }

    pub fn calls(&self) -> Vec<RecordedCall> {
        lock(&self.calls).clone()
    }

    pub fn call_count(&self) -> usize {
        lock(&self.calls).len()
    }
}

impl LlmClient for MockLlm {
    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let (system, user) = request.prompt.render();
        {
            let mut calls = lock(&self.calls);
            if calls.len() >= KEEP_CALLS {
                calls.remove(0);
            }
            calls.push(RecordedCall {
                purpose: request.purpose,
                system,
                user,
            });
        }
        let step = lock(&self.script).pop_front();
        let in_tokens = u32::try_from(request.prompt.estimated_input_tokens()).unwrap_or(u32::MAX);
        let text = match step {
            Some(MockStep::Fail(err)) => return Err(err),
            Some(MockStep::Raw(result)) => return interpret(result),
            Some(MockStep::Text(text)) => text,
            None => template_answer(request.prompt.input()),
        };
        let out_tokens = u32::try_from(text.chars().count() / 2 + 1).unwrap_or(u32::MAX);
        Ok(LlmResponse {
            text,
            in_tokens,
            out_tokens,
        })
    }
}

/// A page name safe for a file name: invalid characters dropped, whitespace collapsed, short.
fn label(raw: &str, fallback: &str) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| !c.is_control() && !"/\\:*?\"<>|".contains(*c))
        .collect();
    let squeezed = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = squeezed.trim_start_matches('.').chars().take(30).collect();
    let short = short.trim().to_string();
    if short.is_empty() {
        fallback.to_string()
    } else {
        short
    }
}

fn template_answer(input: &PromptInput) -> String {
    match input {
        PromptInput::Probe => "ok".to_string(),
        PromptInput::QuizGen { body, .. } => crate::quiz::generate::template_json(body),
        PromptInput::Outline { subject } => pages_json(outline_drafts(subject)),
        PromptInput::Ingest { text, .. } => pages_json(section_drafts(text)),
        PromptInput::Ask { pages, .. } => ask(pages),
        PromptInput::Merge { existing, note, .. } => merge(existing, note),
        PromptInput::SharedMerge { parts, .. } => shared_merge(parts),
        PromptInput::Lint { pages, .. } => lint(pages),
        PromptInput::TabJudge { domain, label, .. } => tab_verdict(domain, label),
        PromptInput::PlanAdjust {
            items,
            now_min,
            day_end_min,
        } => plan_adjust(items, *now_min, *day_end_min),
        PromptInput::PlanChat { .. } => {
            json!({ "reply": "어떤 계획을 만들까요? 날짜와 과목을 알려 주세요." }).to_string()
        }
        PromptInput::CharacterNames { looks } => {
            crate::character::compose::mock_names_json(looks.len())
        }
        PromptInput::Insight(input) => {
            json!({ "insight": crate::growth::rule_insight(input) }).to_string()
        }
    }
}

/// The page followed by the retrospective as a list under one heading: nothing of the page is lost.
fn merge(existing: &str, note: &str) -> String {
    let bullets: Vec<String> = note
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| format!("- {l}"))
        .collect();
    let base = existing.trim_end();
    let merged = if base.is_empty() {
        format!("## 회고\n\n{}", bullets.join("\n"))
    } else {
        format!("{base}\n\n## 회고\n\n{}", bullets.join("\n"))
    };
    json!({"markdown": merged}).to_string()
}

/// Every part, one after the other under its own heading: nothing of any contribution is lost.
fn shared_merge(parts: &[PromptPage]) -> String {
    let merged: Vec<String> = parts
        .iter()
        .enumerate()
        .map(|(i, p)| format!("## 정리 {}\n\n{}", i + 1, p.body.trim()))
        .collect();
    json!({"markdown": merged.join("\n\n")}).to_string()
}

fn lint(pages: &[PromptPage]) -> String {
    let suggestions: Vec<_> =
        pages.iter().take(8).map(|p| json!({"page_name": p.name, "advice": "핵심 개념을 한두 문장으로 보강하고 관련 페이지에 링크를 걸어 보세요."})).collect();
    json!({"suggestions": suggestions}).to_string()
}

const DISTRACTION_WORDS: &[&str] = &[
    "game", "게임", "shop", "쇼핑", "webtoon", "웹툰", "sns", "예능",
];

fn tab_verdict(domain: &str, label: &str) -> String {
    let haystack = format!("{domain} {label}").to_lowercase();
    let verdict = if DISTRACTION_WORDS.iter().any(|w| haystack.contains(w)) {
        "distraction"
    } else {
        "study"
    };
    json!({"verdict": verdict}).to_string()
}

/// What the rule-based shift would do, in the model's output format.
fn plan_adjust(items: &[AdjustItem], now_min: i64, day_end_min: i64) -> String {
    const MIN_MS: i64 = 60_000;
    let shift_items: Vec<ShiftItem> = items
        .iter()
        .map(|i| ShiftItem {
            old_start_ms: i.start_min * MIN_MS,
            old_duration_min: i.old_min,
            new_duration_min: i.new_min,
            pending: i.pending,
        })
        .collect();
    let starts: Vec<serde_json::Value> =
        match compute_shift(&shift_items, now_min * MIN_MS, day_end_min * MIN_MS) {
            ShiftPlan::Moves(moves) => moves
                .into_iter()
                .filter_map(|(index, start)| {
                    items
                        .get(index)
                        .map(|i| json!({"index": i.index, "start_min": start / MIN_MS}))
                })
                .collect(),
            ShiftPlan::OverflowsMidnight => Vec::new(),
        };
    json!({"starts": starts}).to_string()
}

/// Three starter pages for a subject.
pub fn outline_drafts(subject: &str) -> Vec<PageDraft> {
    let name = label(subject, "과목");
    let overview = format!("{name} 개요");
    let concepts = format!("{name} 핵심 개념");
    let review = format!("{name} 복습 질문");
    vec![
        PageDraft {
            markdown: format!("# {overview}\n\n- 이 과목에서 배울 것 (초안)\n- 먼저 알아야 할 것"),
            links: vec![concepts.clone(), review.clone()],
            page_name: overview.clone(),
        },
        PageDraft {
            markdown: format!("# {concepts}\n\n- 핵심 용어 목록 (초안)"),
            links: vec![overview.clone()],
            page_name: concepts,
        },
        PageDraft {
            markdown: format!("# {review}\n\n- 스스로 설명해 볼 질문 (초안)"),
            links: vec![overview],
            page_name: review,
        },
    ]
}

/// One page per heading section; without headings, one page named after the first line.
pub fn section_drafts(text: &str) -> Vec<PageDraft> {
    let mut sections: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') {
            let title = label(line.trim_start_matches('#'), "메모");
            sections.push((title, format!("{line}\n")));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if sections.is_empty() {
        let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        sections.push((label(first, "메모"), format!("{}\n", text.trim())));
    }
    sections.truncate(8);
    let names: Vec<String> = sections.iter().map(|(n, _)| n.clone()).collect();
    sections
        .into_iter()
        .map(|(page_name, body)| {
            let links = names.iter().filter(|n| **n != page_name).cloned().collect();
            PageDraft {
                page_name,
                markdown: body.trim().to_string(),
                links,
            }
        })
        .collect()
}

fn pages_json(drafts: Vec<PageDraft>) -> String {
    let pages: Vec<_> = drafts
        .into_iter()
        .map(|d| json!({"page_name": d.page_name, "markdown": d.markdown, "links": d.links}))
        .collect();
    json!({"pages": pages}).to_string()
}

fn ask(pages: &[PromptPage]) -> String {
    let Some(first) = pages.first() else {
        return json!({"answer": "위키에 없음", "citations": []}).to_string();
    };
    let gist: String = first
        .body
        .lines()
        .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .unwrap_or("")
        .chars()
        .take(80)
        .collect();
    let citations: Vec<&String> = pages.iter().take(2).map(|p| &p.name).collect();
    json!({"answer": format!("[{}] {}", first.name, gist), "citations": citations}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::anthropic::tests::{contract_cases, ok_body};
    use crate::llm::egress::Prompt;

    fn req(purpose: Purpose, prompt: Prompt) -> LlmRequest {
        LlmRequest {
            purpose,
            prompt,
            max_tokens: 100,
        }
    }

    #[test]
    fn answers_are_deterministic_and_derived_from_the_material() {
        let mock = MockLlm::new();
        let prompt = Prompt::ingest(
            "수학",
            "# 미분\n도함수는 변화율이다.\n# 적분\n넓이를 구한다.",
        );
        let first = mock
            .complete(&req(Purpose::WikiIngest, prompt.clone()))
            .expect("reply");
        let second = mock
            .complete(&req(Purpose::WikiIngest, prompt))
            .expect("reply");
        assert_eq!(first, second);
        let value: serde_json::Value = serde_json::from_str(&first.text).expect("json");
        let names: Vec<&str> = value["pages"]
            .as_array()
            .expect("pages")
            .iter()
            .filter_map(|p| p["page_name"].as_str())
            .collect();
        assert_eq!(names, ["미분", "적분"]);
        assert_eq!(value["pages"][0]["links"], json!(["적분"]));
        assert!(first.in_tokens > 0 && first.out_tokens > 0);
        assert_eq!(mock.call_count(), 2);
        assert_eq!(mock.calls()[0].purpose, Purpose::WikiIngest);
    }

    #[test]
    fn plain_text_becomes_one_page_named_after_its_first_line_and_names_are_file_safe() {
        let mock = MockLlm::new();
        let reply = mock
            .complete(&req(
                Purpose::WikiIngest,
                Prompt::ingest("s", "a/b: c?\n본문"),
            ))
            .expect("reply");
        let value: serde_json::Value = serde_json::from_str(&reply.text).expect("json");
        assert_eq!(value["pages"][0]["page_name"], "ab c");
        let outline = mock
            .complete(&req(Purpose::WikiIngest, Prompt::outline("한/글")))
            .expect("outline");
        assert!(outline.text.contains("한글 개요"));
    }

    #[test]
    fn ask_cites_the_pages_it_was_given_and_says_so_when_given_none() {
        let mock = MockLlm::new();
        let pages = vec![PromptPage {
            name: "미분".into(),
            body: "# 미분\n도함수는 변화율이다.".into(),
        }];
        let reply = mock
            .complete(&req(
                Purpose::WikiAsk,
                Prompt::ask("수학", "도함수?", pages),
            ))
            .expect("reply");
        let value: serde_json::Value = serde_json::from_str(&reply.text).expect("json");
        assert_eq!(value["citations"], json!(["미분"]));
        assert!(
            value["answer"]
                .as_str()
                .is_some_and(|a| a.contains("변화율"))
        );
        let empty = mock
            .complete(&req(Purpose::WikiAsk, Prompt::ask("수학", "?", Vec::new())))
            .expect("reply");
        assert!(empty.text.contains("위키에 없음"));
    }

    #[test]
    fn the_script_comes_first_in_order_then_templates_resume() {
        let mock = MockLlm::with_script([
            MockStep::Text("one".into()),
            MockStep::Fail(LlmError::Overloaded),
        ]);
        let probe = || req(Purpose::Probe, Prompt::probe());
        assert_eq!(mock.complete(&probe()).expect("one").text, "one");
        assert_eq!(mock.complete(&probe()), Err(LlmError::Overloaded));
        assert_eq!(mock.complete(&probe()).expect("template").text, "ok");
        mock.push(MockStep::Text("later".into()));
        assert_eq!(mock.complete(&probe()).expect("pushed").text, "later");
    }

    /// The same wire outcomes give the same result through the mock and the real adapter.
    #[test]
    fn contract_suite_mock_and_anthropic_agree_on_every_failure() {
        for (name, raw, expected) in contract_cases() {
            let mock = MockLlm::with_script([MockStep::Raw(raw)]);
            assert_eq!(
                mock.complete(&req(Purpose::Probe, Prompt::probe())),
                Err(expected),
                "{name}"
            );
        }
        let ok = MockLlm::with_script([MockStep::Raw(Ok(HttpResponse::ok_json(
            200,
            &ok_body("hi", 3, 4),
        )))]);
        assert_eq!(
            ok.complete(&req(Purpose::Probe, Prompt::probe())),
            Ok(LlmResponse {
                text: "hi".into(),
                in_tokens: 3,
                out_tokens: 4
            })
        );
    }

    #[test]
    fn recorded_calls_are_bounded() {
        let mock = MockLlm::new();
        for _ in 0..(KEEP_CALLS + 5) {
            mock.complete(&req(Purpose::Probe, Prompt::probe()))
                .expect("reply");
        }
        assert_eq!(mock.call_count(), KEEP_CALLS);
    }
}
