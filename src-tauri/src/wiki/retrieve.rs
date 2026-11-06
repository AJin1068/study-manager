//! Local page search for "ask": keyword counts, no model. Words are lowercase alphanumeric runs;
//! Hangul runs also contribute character pairs, so a particle ("미분이", "미분은") still matches
//! the stem. A page scores on how often question words appear in it, plus a bonus for its name.

use std::collections::{BTreeMap, BTreeSet};

/// Pages whose name contains a question word get this many extra points per word.
const NAME_BONUS: usize = 5;
/// A word counts at most this often per page, so one long page cannot drown the rest.
const TF_CAP: usize = 5;
/// Only the start of a page is scored.
const SCORED_CHARS: usize = 20_000;

fn is_hangul(c: char) -> bool {
    ('\u{ac00}'..='\u{d7a3}').contains(&c)
}

/// Search terms of a text, with repeats (term frequency is the count).
pub fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        let chars: Vec<char> = word.chars().collect();
        if chars.is_empty() || (chars.len() == 1 && !is_hangul(chars[0]) && !chars[0].is_numeric()) {
            continue;
        }
        if chars.len() >= 2 && chars.iter().all(|c| is_hangul(*c)) {
            for pair in chars.windows(2) {
                out.push(pair.iter().collect());
            }
        }
        out.push(word.to_string());
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    pub index: usize,
    pub score: usize,
}

/// Pages with a positive score, best first (ties by name), at most `limit`. `pages` is
/// `(name, body)`; the result indexes into it.
pub fn rank_pages(question: &str, pages: &[(String, String)], limit: usize) -> Vec<Ranked> {
    let wanted: BTreeSet<String> = terms(question).into_iter().collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut ranked = Vec::new();
    for (index, (name, body)) in pages.iter().enumerate() {
        let head: String = body.chars().take(SCORED_CHARS).collect();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for term in terms(&head) {
            if wanted.contains(&term) {
                *counts.entry(term).or_default() += 1;
            }
        }
        let name_terms: BTreeSet<String> = terms(name).into_iter().collect();
        let body_score: usize = counts.values().map(|n| (*n).min(TF_CAP)).sum();
        let name_score = wanted.intersection(&name_terms).count() * NAME_BONUS;
        let score = body_score + name_score;
        if score > 0 {
            ranked.push(Ranked { index, score });
        }
    }
    ranked.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| pages[a.index].0.cmp(&pages[b.index].0)));
    ranked.truncate(limit);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(name: &str, body: &str) -> (String, String) {
        (name.to_string(), body.to_string())
    }

    #[test]
    fn particles_do_not_hide_a_hangul_stem() {
        let q = terms("미분이 뭐야?");
        assert!(q.contains(&"미분".to_string()) && q.contains(&"미분이".to_string()) && q.contains(&"뭐야".to_string()));
        let pages = [page("적분", "넓이를 구한다"), page("미분", "미분은 함수의 변화율이다")];
        let ranked = rank_pages("미분이 뭐야?", &pages, 3);
        assert_eq!(ranked.iter().map(|r| r.index).collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn names_count_more_than_one_mention_and_ties_break_by_name() {
        let pages = [
            page("b-page", "limit limit"),
            page("limit", "x"),
            page("a-page", "limit limit"),
        ];
        let ranked = rank_pages("limit", &pages, 5);
        let order: Vec<&str> = ranked.iter().map(|r| pages[r.index].0.as_str()).collect();
        assert_eq!(order, ["limit", "a-page", "b-page"]);
        assert_eq!((ranked[0].score, ranked[1].score), (NAME_BONUS, 2));
    }

    #[test]
    fn a_page_cannot_win_by_repeating_a_word_endlessly() {
        let spam = page("spam", &"limit ".repeat(500));
        let real = page("real", "limit derivative integral series vector matrix");
        let ranked = rank_pages("limit derivative integral series vector matrix", &[spam, real], 2);
        assert_eq!(ranked[0].index, 1, "six distinct matches beat one capped word: {ranked:?}");
        assert_eq!((ranked[0].score, ranked[1].score), (6, TF_CAP));
    }

    #[test]
    fn nothing_matching_or_nothing_searchable_ranks_nothing() {
        let pages = [page("a", "alpha beta")];
        assert!(rank_pages("gamma", &pages, 3).is_empty());
        assert!(rank_pages("? ! ,", &pages, 3).is_empty());
        assert!(rank_pages("x", &pages, 3).is_empty(), "single latin letters are not terms");
        assert!(rank_pages("alpha", &[], 3).is_empty());
        assert_eq!(rank_pages("alpha", &pages, 0), []);
    }

    #[test]
    fn the_limit_keeps_the_best_pages() {
        // Scores 1..=5, with the sixth page capped at 5 and tying the fifth (name order decides).
        let pages: Vec<_> = (0..6).map(|i| page(&format!("p{i}"), &"term ".repeat(i + 1))).collect();
        let ranked = rank_pages("term", &pages, 3);
        assert_eq!(ranked.iter().map(|r| r.index).collect::<Vec<_>>(), [4, 5, 3]);
    }
}
