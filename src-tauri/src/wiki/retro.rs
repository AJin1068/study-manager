//! Pure parts of the session retrospective merge: reading the model's answer, the safety checks
//! that decide whether a merged page may replace the original, and the line diff shown in the
//! preview. Nothing here touches files, the database, or the model.

use serde::{Deserialize, Serialize};

use super::fs::MAX_PAGE_BYTES;
use super::ingest::json_object;
use super::names;

/// A merged page must keep at least this share of the original's characters (percent).
pub const MIN_KEEP_PERCENT: usize = 50;
/// Characters a merge may add beyond the original, per character of the retrospective, plus a
/// fixed allowance for headings and wording.
const GROWTH_PER_NOTE_CHAR: usize = 3;
const GROWTH_ALLOWANCE: usize = 2_000;
/// Lines compared by the diff; longer pages fall back to "everything replaced".
const MAX_DIFF_LINES: usize = 1_500;
pub const MAX_ADVICE_CHARS: usize = 200;
pub const MAX_SUGGESTIONS: usize = 8;

/// Why a merged page was thrown away. Carries no page text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MergeRejected {
    #[error("not_json")]
    NotJson,
    #[error("empty")]
    Empty,
    #[error("bad_text")]
    BadText,
    #[error("too_short")]
    TooShort,
    #[error("too_long")]
    TooLong,
}

impl MergeRejected {
    pub fn code(self) -> &'static str {
        match self {
            MergeRejected::NotJson => "merge_not_json",
            MergeRejected::Empty => "merge_empty",
            MergeRejected::BadText => "merge_bad_text",
            MergeRejected::TooShort => "merge_too_short",
            MergeRejected::TooLong => "merge_too_long",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMerge {
    markdown: String,
}

/// The page text a model answer proposes.
pub fn parse_merge(answer: &str) -> Result<String, MergeRejected> {
    let json = json_object(answer).ok_or(MergeRejected::NotJson)?;
    let raw: RawMerge = serde_json::from_str(json).map_err(|_| MergeRejected::NotJson)?;
    Ok(raw.markdown)
}

/// Accepts a merged page only if it cannot have destroyed the original: it keeps at least half of
/// the original's length, does not balloon past the retrospective's plausible contribution, fits a
/// page, and is plain text. An empty original (a new page) has no lower bound.
pub fn check_merge(original: &str, merged: &str, note: &str) -> Result<String, MergeRejected> {
    let merged = merged.trim().to_string();
    if merged.is_empty() {
        return Err(MergeRejected::Empty);
    }
    if merged.contains('\0') {
        return Err(MergeRejected::BadText);
    }
    let original_chars = original.trim().chars().count();
    let merged_chars = merged.chars().count();
    if merged_chars * 100 < original_chars * MIN_KEEP_PERCENT {
        return Err(MergeRejected::TooShort);
    }
    let ceiling = original_chars + note.chars().count() * GROWTH_PER_NOTE_CHAR + GROWTH_ALLOWANCE;
    if merged_chars > ceiling || merged.len() > MAX_PAGE_BYTES {
        return Err(MergeRejected::TooLong);
    }
    Ok(merged)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Same,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// Line diff by longest common subsequence.
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let line = |kind, text: &str| DiffLine {
        kind,
        text: text.to_string(),
    };
    if a.len() > MAX_DIFF_LINES || b.len() > MAX_DIFF_LINES {
        return a
            .iter()
            .map(|t| line(DiffKind::Removed, t))
            .chain(b.iter().map(|t| line(DiffKind::Added, t)))
            .collect();
    }
    // lcs[i][j] = length of the common subsequence of a[i..] and b[j..].
    let mut lcs = vec![vec![0_u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push(line(DiffKind::Same, a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(line(DiffKind::Removed, a[i]));
            i += 1;
        } else {
            out.push(line(DiffKind::Added, b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|t| line(DiffKind::Removed, t)));
    out.extend(b[j..].iter().map(|t| line(DiffKind::Added, t)));
    out
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub page_name: String,
    pub advice: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAdvice {
    suggestions: Vec<RawSuggestion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSuggestion {
    page_name: String,
    advice: String,
}

/// Improvement advice from a lint answer. Advice about a page that was not offered is dropped;
/// the rest is length-limited text for display only.
pub fn parse_advice(answer: &str, offered: &[String]) -> Option<Vec<Suggestion>> {
    let raw: RawAdvice = serde_json::from_str(json_object(answer)?).ok()?;
    if raw.suggestions.len() > MAX_SUGGESTIONS {
        return None;
    }
    let mut out = Vec::new();
    for item in raw.suggestions {
        let page_name = names::page_name(&item.page_name)?;
        let advice = item.advice.split_whitespace().collect::<Vec<_>>().join(" ");
        if advice.is_empty()
            || advice.chars().count() > MAX_ADVICE_CHARS
            || advice.chars().any(char::is_control)
        {
            return None;
        }
        if offered.contains(&page_name) {
            out.push(Suggestion { page_name, advice });
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_length_bounds_protect_the_original() {
        let original = "가".repeat(100);
        let note = "나".repeat(10);
        assert_eq!(
            check_merge(&original, &"가".repeat(49), &note),
            Err(MergeRejected::TooShort)
        );
        assert!(
            check_merge(&original, &"가".repeat(50), &note).is_ok(),
            "exactly half is allowed"
        );
        assert!(check_merge(&original, &"가".repeat(100 + 10 * 3 + 2_000), &note).is_ok());
        assert_eq!(
            check_merge(&original, &"가".repeat(100 + 10 * 3 + 2_001), &note),
            Err(MergeRejected::TooLong)
        );
        assert_eq!(
            check_merge(&original, "   ", &note),
            Err(MergeRejected::Empty)
        );
        assert_eq!(
            check_merge(&original, &format!("{original}\0"), &note),
            Err(MergeRejected::BadText)
        );
        assert!(
            check_merge("", "새 페이지", &note).is_ok(),
            "a new page has no lower bound"
        );
        assert_eq!(
            check_merge("", &"가".repeat(MAX_PAGE_BYTES), "x"),
            Err(MergeRejected::TooLong)
        );
    }

    #[test]
    fn answers_are_parsed_strictly() {
        assert_eq!(parse_merge("{\"markdown\":\"# a\"}"), Ok("# a".to_string()));
        assert_eq!(
            parse_merge("```json\n{\"markdown\":\"x\"}\n```"),
            Ok("x".to_string())
        );
        assert_eq!(
            parse_merge("{\"markdown\":\"x\",\"path\":\"../../etc\"}"),
            Err(MergeRejected::NotJson)
        );
        assert_eq!(parse_merge("no json"), Err(MergeRejected::NotJson));
    }

    #[test]
    fn the_diff_marks_added_and_removed_lines() {
        let diff = diff_lines("a\nb\nc", "a\nc\nd");
        let shown: Vec<(DiffKind, &str)> = diff.iter().map(|l| (l.kind, l.text.as_str())).collect();
        assert_eq!(
            shown,
            [
                (DiffKind::Same, "a"),
                (DiffKind::Removed, "b"),
                (DiffKind::Same, "c"),
                (DiffKind::Added, "d")
            ]
        );
        assert!(
            diff_lines("x", "x")
                .iter()
                .all(|l| l.kind == DiffKind::Same)
        );
        let huge = "l\n".repeat(MAX_DIFF_LINES + 1);
        assert_eq!(
            diff_lines(&huge, "n")
                .iter()
                .filter(|l| l.kind == DiffKind::Added)
                .count(),
            1
        );
    }

    #[test]
    fn advice_for_pages_that_were_not_offered_is_dropped_and_oversized_advice_rejects_the_answer() {
        let offered = vec!["미분".to_string()];
        let ok = "{\"suggestions\":[{\"page_name\":\"미분\",\"advice\":\"예시를 더하세요\"},{\"page_name\":\"다른\",\"advice\":\"x\"}]}";
        assert_eq!(
            parse_advice(ok, &offered),
            Some(vec![Suggestion {
                page_name: "미분".into(),
                advice: "예시를 더하세요".into()
            }])
        );
        let long = format!(
            "{{\"suggestions\":[{{\"page_name\":\"미분\",\"advice\":\"{}\"}}]}}",
            "가".repeat(MAX_ADVICE_CHARS + 1)
        );
        assert_eq!(parse_advice(&long, &offered), None);
        assert_eq!(parse_advice("{\"suggestions\":[],\"x\":1}", &offered), None);
    }
}
