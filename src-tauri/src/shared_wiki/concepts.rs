//! "A friend wrote up a concept you have no page for." The comparison is local: the user's own
//! page names against the titles of the room's shared pages. No wiki text is sent anywhere for it.

use std::collections::HashSet;

use super::model::{RevisionKind, WirePage};

/// A concept compared ignoring case, spaces, hyphens and underscores.
pub fn normalize(name: &str) -> String {
    name.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').flat_map(char::to_lowercase).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewConcept {
    pub title: String,
    /// Nicknames of the friends who wrote it up, in contribution order.
    pub authors: Vec<String>,
}

/// Shared pages that someone else contributed to, that have no local page of the same name, and
/// that were not announced before (`seen` holds normalized names).
pub fn new_concepts(local_page_names: &[String], pages: &[WirePage], seen: &HashSet<String>) -> Vec<NewConcept> {
    let local: HashSet<String> = local_page_names.iter().map(|n| normalize(n)).collect();
    let mut out = Vec::new();
    for page in pages {
        let key = normalize(&page.title);
        if key.is_empty() || local.contains(&key) || seen.contains(&key) {
            continue;
        }
        let mut revisions: Vec<_> = page.revisions.iter().filter(|r| !r.is_mine && r.kind == RevisionKind::Edit && !r.body.trim().is_empty()).collect();
        revisions.sort_by_key(|r| r.created_at_ms);
        let mut authors: Vec<String> = Vec::new();
        for r in revisions {
            if !authors.contains(&r.author) {
                authors.push(r.author.clone());
            }
        }
        if !authors.is_empty() {
            out.push(NewConcept { title: page.title.clone(), authors });
        }
    }
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_wiki::model::WireRevision;

    fn rev(author: &str, mine: bool, at: i64) -> WireRevision {
        WireRevision {
            revision_id: format!("r{at}"),
            kind: RevisionKind::Edit,
            author: author.into(),
            is_mine: mine,
            body: "본문".into(),
            allow_llm_merge: false,
            branched: false,
            base_revision_id: None,
            contributors: vec![author.into()],
            created_at_ms: at,
        }
    }

    fn page(title: &str, revs: Vec<WireRevision>) -> WirePage {
        WirePage { page_id: title.into(), title: title.into(), revisions: revs }
    }

    #[test]
    fn a_friends_page_without_a_local_page_is_new_and_names_the_friends() {
        let pages = [page("연쇄 법칙", vec![rev("나나", false, 2), rev("가가", false, 1), rev("나나", false, 3)])];
        let found = new_concepts(&["적분".into()], &pages, &HashSet::new());
        assert_eq!(found, vec![NewConcept { title: "연쇄 법칙".into(), authors: vec!["가가".into(), "나나".into()] }]);
    }

    #[test]
    fn matching_ignores_case_spaces_hyphens_and_underscores() {
        let pages = [page("Chain Rule", vec![rev("가", false, 1)])];
        assert!(new_concepts(&["chain-rule".into()], &pages, &HashSet::new()).is_empty());
        assert!(new_concepts(&["chain_rule".into()], &pages, &HashSet::new()).is_empty());
        assert_eq!(new_concepts(&["chain rules".into()], &pages, &HashSet::new()).len(), 1);
    }

    #[test]
    fn my_own_pages_and_already_announced_concepts_are_not_news() {
        let pages = [page("내가 올림", vec![rev("나", true, 1)]), page("알림 끝", vec![rev("가", false, 1)])];
        let seen: HashSet<String> = [normalize("알림 끝")].into_iter().collect();
        assert!(new_concepts(&[], &pages, &seen).is_empty());
    }

    #[test]
    fn a_page_where_a_friend_and_i_both_wrote_counts_as_news_for_the_friend_part() {
        let pages = [page("공동", vec![rev("나", true, 1), rev("가", false, 2)])];
        assert_eq!(new_concepts(&[], &pages, &HashSet::new())[0].authors, ["가"]);
    }
}
