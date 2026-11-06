//! The room's daily wiki recap, written locally from the pages the room shared: counts, titles and
//! nicknames that every member can already see. No model, no new data leaves the PC.

use super::concepts::NewConcept;
use super::model::WirePage;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DailySummary {
    pub pages_touched: usize,
    pub revisions: usize,
    pub contributors: Vec<String>,
    pub new_concepts: Vec<String>,
    pub text: String,
}

/// Summarizes what was added in `[from_ms, to_ms)`.
pub fn daily(pages: &[WirePage], from_ms: i64, to_ms: i64, news: &[NewConcept]) -> DailySummary {
    let mut touched = 0;
    let mut revisions = 0;
    let mut contributors: Vec<String> = Vec::new();
    for page in pages {
        let today: Vec<_> = page.revisions.iter().filter(|r| r.created_at_ms >= from_ms && r.created_at_ms < to_ms).collect();
        if today.is_empty() {
            continue;
        }
        touched += 1;
        revisions += today.len();
        for r in today {
            for name in &r.contributors {
                if !contributors.contains(name) {
                    contributors.push(name.clone());
                }
            }
        }
    }
    let new_concepts: Vec<String> = news.iter().map(|n| n.title.clone()).collect();
    let text = if revisions == 0 {
        "오늘은 방에 새로 올라온 위키 정리가 없어요.".to_string()
    } else {
        let mut text = format!("오늘 {touched}개 페이지에 정리가 {revisions}건 올라왔어요. 함께한 사람: {}.", contributors.join(", "));
        if !new_concepts.is_empty() {
            text.push_str(&format!(" 내 위키에 없는 개념: {}.", new_concepts.join(", ")));
        }
        text
    };
    DailySummary { pages_touched: touched, revisions, contributors, new_concepts, text }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_wiki::model::{RevisionKind, WireRevision};

    fn page(title: &str, revs: &[(&str, i64)]) -> WirePage {
        WirePage {
            page_id: title.into(),
            title: title.into(),
            revisions: revs
                .iter()
                .map(|(who, at)| WireRevision {
                    revision_id: format!("{title}{at}"),
                    kind: RevisionKind::Edit,
                    author: (*who).into(),
                    is_mine: false,
                    body: "본문".into(),
                    allow_llm_merge: false,
                    branched: false,
                    base_revision_id: None,
                    contributors: vec![(*who).into()],
                    created_at_ms: *at,
                })
                .collect(),
        }
    }

    #[test]
    fn counts_only_revisions_inside_the_half_open_window() {
        let pages = [page("가", &[("A", 10), ("B", 20)]), page("나", &[("A", 5), ("C", 25)])];
        let s = daily(&pages, 10, 25, &[]);
        assert_eq!((s.pages_touched, s.revisions), (1, 2));
        assert_eq!(s.contributors, ["A", "B"]);
        assert!(s.text.contains("1개 페이지") && s.text.contains("2건"));
    }

    #[test]
    fn an_empty_day_says_so_and_new_concepts_are_named() {
        assert!(daily(&[], 0, 100, &[]).text.contains("없어요"));
        let news = [NewConcept { title: "연쇄 법칙".into(), authors: vec!["A".into()] }];
        let s = daily(&[page("가", &[("A", 1)])], 0, 100, &news);
        assert!(s.text.contains("연쇄 법칙"));
        assert_eq!(s.new_concepts, ["연쇄 법칙"]);
    }
}
