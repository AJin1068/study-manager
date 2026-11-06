//! Local wiki checks (PLAN 5 "점검"): orphan pages, broken links, duplicates, and empty pages.
//! Pure functions over `(page name, body)` pairs; no model and no files. The LLM only turns the
//! findings into improvement advice (`retro_service::lint`), and its advice is shown, never applied.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::names;

/// Body text (without headings and links) shorter than this many characters makes a page empty.
const EMPTY_BELOW_CHARS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LintKind {
    Empty,
    BrokenLink,
    Duplicate,
    Orphan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LintFinding {
    pub kind: LintKind,
    pub page_name: String,
    /// The missing target of a broken link, or the page a duplicate repeats.
    pub detail: Option<String>,
}

/// Targets of the `[[Name]]` links in a body, in order, with `|alias` and `#section` dropped.
pub fn link_targets(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        if !inner.contains('\n') {
            let target = inner.split(['|', '#']).next().unwrap_or("");
            if let Some(name) = names::page_name(target) {
                out.push(name);
            }
        }
        rest = &after[end + 2..];
    }
    out
}

fn visible_chars(body: &str) -> usize {
    body.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .map(|l| {
            let mut stripped = String::new();
            let mut rest = l;
            while let Some(start) = rest.find("[[") {
                stripped.push_str(&rest[..start]);
                rest = rest[start..].find("]]").map_or("", |end| &rest[start + end + 2..]);
            }
            stripped.push_str(rest);
            stripped.chars().filter(|c| c.is_alphanumeric()).count()
        })
        .sum()
}

fn fingerprint(body: &str) -> String {
    body.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Findings sorted by kind, then page name.
pub fn lint_pages(pages: &[(String, String)]) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let known: BTreeSet<&str> = pages.iter().map(|(n, _)| n.as_str()).collect();
    let mut inbound: BTreeMap<&str, usize> = BTreeMap::new();

    for (name, body) in pages {
        if visible_chars(body) < EMPTY_BELOW_CHARS {
            findings.push(LintFinding { kind: LintKind::Empty, page_name: name.clone(), detail: None });
        }
        let mut seen = BTreeSet::new();
        for target in link_targets(body) {
            if !seen.insert(target.clone()) {
                continue;
            }
            match known.get(target.as_str()) {
                Some(&known_name) => {
                    if known_name != name.as_str() {
                        *inbound.entry(known_name).or_default() += 1;
                    }
                }
                None => findings.push(LintFinding { kind: LintKind::BrokenLink, page_name: name.clone(), detail: Some(target) }),
            }
        }
    }

    // Duplicates: the same text under two names (an empty page is already reported as empty).
    let mut first_with: BTreeMap<String, &str> = BTreeMap::new();
    let mut ordered: Vec<&(String, String)> = pages.iter().collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, body) in ordered {
        if visible_chars(body) < EMPTY_BELOW_CHARS {
            continue;
        }
        let key = fingerprint(body);
        match first_with.get(&key) {
            Some(original) => findings.push(LintFinding {
                kind: LintKind::Duplicate,
                page_name: name.clone(),
                detail: Some((*original).to_string()),
            }),
            None => {
                first_with.insert(key, name);
            }
        }
    }

    if pages.len() > 1 {
        for (name, _) in pages {
            if inbound.get(name.as_str()).copied().unwrap_or(0) == 0 {
                findings.push(LintFinding { kind: LintKind::Orphan, page_name: name.clone(), detail: None });
            }
        }
    }
    findings.sort_by(|a, b| (a.kind, &a.page_name, &a.detail).cmp(&(b.kind, &b.page_name, &b.detail)));
    findings
}

/// One line per finding for the model: kinds and page names only, no page text.
pub fn describe(findings: &[LintFinding]) -> Vec<String> {
    findings
        .iter()
        .map(|f| {
            let kind = match f.kind {
                LintKind::Empty => "빈 페이지",
                LintKind::BrokenLink => "깨진 링크",
                LintKind::Duplicate => "중복",
                LintKind::Orphan => "고아 페이지",
            };
            match &f.detail {
                Some(detail) => format!("{kind}: {} -> {detail}", f.page_name),
                None => format!("{kind}: {}", f.page_name),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(name: &str, body: &str) -> (String, String) {
        (name.to_string(), body.to_string())
    }

    fn kinds(findings: &[LintFinding]) -> Vec<(LintKind, &str, Option<&str>)> {
        findings.iter().map(|f| (f.kind, f.page_name.as_str(), f.detail.as_deref())).collect()
    }

    #[test]
    fn finds_empty_broken_duplicate_and_orphan_pages_without_a_model() {
        let pages = vec![
            page("개요", "# 개요\n\n미분과 적분의 관계를 설명한다. [[미분]] [[없는 페이지]]"),
            page("미분", "# 미분\n\n도함수는 순간 변화율이다. [[개요]]"),
            page("미분 복사본", "# 미분\n\n도함수는   순간 변화율이다. [[개요]]"),
            page("빈 페이지", "# 빈 페이지\n"),
            page("외톨이", "# 외톨이\n\n아무도 링크하지 않는 충분히 긴 본문이다."),
        ];
        let found = lint_pages(&pages);
        assert_eq!(
            kinds(&found),
            [
                (LintKind::Empty, "빈 페이지", None),
                (LintKind::BrokenLink, "개요", Some("없는 페이지")),
                (LintKind::Duplicate, "미분 복사본", Some("미분")),
                (LintKind::Orphan, "미분 복사본", None),
                (LintKind::Orphan, "빈 페이지", None),
                (LintKind::Orphan, "외톨이", None),
            ]
        );
    }

    #[test]
    fn a_single_page_is_not_an_orphan_and_links_to_itself_do_not_count() {
        assert!(lint_pages(&[page("혼자", "# 혼자\n\n충분히 긴 본문이 여기에 있다. [[혼자]]")]).is_empty());
        let two = lint_pages(&[page("가", "# 가\n\n충분히 긴 본문이 여기에 있다. [[가]]"), page("나", "# 나\n\n충분히 긴 본문이 여기에 있다. [[가]]")]);
        assert_eq!(kinds(&two), [(LintKind::Orphan, "나", None)]);
    }

    #[test]
    fn link_parsing_drops_aliases_sections_and_unclosed_brackets() {
        assert_eq!(link_targets("[[a|별칭]] [[b#절]] [[c"), ["a", "b"]);
        assert_eq!(link_targets("[[bad/name]] [[줄\n바꿈]]"), Vec::<String>::new());
    }

    #[test]
    fn findings_are_deterministic_and_describe_names_only() {
        let pages = vec![page("가", "[[없음]]"), page("나", "")];
        let first = lint_pages(&pages);
        assert_eq!(first, lint_pages(&pages));
        let lines = describe(&first);
        assert!(lines.contains(&"깨진 링크: 가 -> 없음".to_string()), "{lines:?}");
        assert!(lines.iter().all(|l| !l.contains("본문")));
    }
}
