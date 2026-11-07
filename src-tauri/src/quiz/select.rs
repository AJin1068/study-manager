//! Which questions to ask. A pure function of the candidates, the clock, and a seed, so a golden
//! test can pin it. Order of preference: not asked in the last 24 hours, then the page the user
//! answers worst, then the longest-unasked, then a seeded tiebreak.

use super::{ItemKind, QuizItem, REASK_MS};

/// A drawable question with what the draw needs to know about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub item: QuizItem,
    pub page_total: i64,
    pub page_correct: i64,
    pub last_asked_ms: Option<i64>,
}

/// SplitMix64 finalizer: a well-spread, stable integer mix (not a security hash).
pub fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

pub fn hash_str(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Accuracy of the page in per-mille; a page never answered counts as middling so new material
/// is neither buried nor always first.
fn accuracy_permille(c: &Candidate) -> i64 {
    if c.page_total <= 0 {
        500
    } else {
        c.page_correct.saturating_mul(1000) / c.page_total
    }
}

fn is_fresh(c: &Candidate, now_ms: i64) -> bool {
    c.last_asked_ms
        .is_none_or(|at| now_ms.saturating_sub(at) >= REASK_MS)
}

fn rank(c: &Candidate, now_ms: i64, seed: u64) -> (u8, i64, i64, u64) {
    let stale = u8::from(!is_fresh(c, now_ms));
    let age_key = c.last_asked_ms.unwrap_or(i64::MIN);
    (
        stale,
        accuracy_permille(c),
        age_key,
        mix(seed ^ mix(u64::try_from(c.item.id).unwrap_or(0))),
    )
}

/// Picks up to `n` candidates (indexes into `candidates`). With `fresh_only`, a question asked in
/// the last 24 hours is never picked; otherwise such questions fill in only after fresh ones run
/// out. With `want_cloze`, at least one fill-in-the-blank is included whenever one exists.
pub fn pick(
    candidates: &[Candidate],
    n: usize,
    now_ms: i64,
    seed: u64,
    fresh_only: bool,
    want_cloze: bool,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..candidates.len())
        .filter(|&i| !fresh_only || is_fresh(&candidates[i], now_ms))
        .collect();
    order.sort_by_key(|&i| rank(&candidates[i], now_ms, seed));
    let mut chosen: Vec<usize> = order.iter().copied().take(n).collect();
    if want_cloze
        && !chosen.is_empty()
        && chosen
            .iter()
            .all(|&i| candidates[i].item.kind != ItemKind::Cloze)
    {
        if let Some(&cloze) = order
            .iter()
            .find(|&&i| candidates[i].item.kind == ItemKind::Cloze)
        {
            if let Some(last) = chosen.last_mut() {
                *last = cloze;
            }
        }
    }
    chosen.sort_by_key(|&i| {
        mix(seed ^ mix(u64::try_from(candidates[i].item.id).unwrap_or(0)) ^ 0x51)
    });
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: i64, kind: ItemKind, total: i64, correct: i64, last: Option<i64>) -> Candidate {
        Candidate {
            item: QuizItem {
                id,
                page_id: id,
                kind,
                prompt: format!("q{id}"),
                choices: vec![],
                answer_key: "a".into(),
            },
            page_total: total,
            page_correct: correct,
            last_asked_ms: last,
        }
    }

    fn ids(cands: &[Candidate], picked: &[usize]) -> Vec<i64> {
        let mut v: Vec<i64> = picked.iter().map(|&i| cands[i].item.id).collect();
        v.sort_unstable();
        v
    }

    const NOW: i64 = 10 * REASK_MS;

    #[test]
    fn the_draw_is_deterministic_for_a_seed_and_golden() {
        let cands: Vec<Candidate> = (1..=6)
            .map(|i| cand(i, ItemKind::Cloze, 0, 0, None))
            .collect();
        let a = pick(&cands, 3, NOW, 7, true, false);
        let b = pick(&cands, 3, NOW, 7, true, false);
        assert_eq!(a, b);
        assert_eq!(a.len(), 3);
        let c = pick(&cands, 3, NOW, 8, true, false);
        assert!(
            (0..30).any(
                |seed| ids(&cands, &pick(&cands, 3, NOW, seed, true, false)) != ids(&cands, &c)
            ),
            "the seed changes the draw"
        );
    }

    #[test]
    fn weak_pages_and_long_unasked_questions_come_first() {
        let cands = vec![
            cand(1, ItemKind::Cloze, 10, 10, None),
            cand(2, ItemKind::Cloze, 10, 2, None),
            cand(3, ItemKind::Cloze, 10, 5, None),
            cand(4, ItemKind::Cloze, 10, 5, Some(NOW - 2 * REASK_MS)),
            cand(5, ItemKind::Cloze, 10, 5, Some(NOW - 3 * REASK_MS)),
        ];
        assert_eq!(
            ids(&cands, &pick(&cands, 3, NOW, 1, true, false)),
            vec![2, 3, 5]
        );
    }

    #[test]
    fn a_question_asked_within_24_hours_is_excluded_or_last() {
        let cands = vec![
            cand(1, ItemKind::Cloze, 0, 0, Some(NOW - 1_000)),
            cand(2, ItemKind::Cloze, 0, 0, None),
            cand(3, ItemKind::Cloze, 0, 0, None),
        ];
        assert_eq!(
            ids(&cands, &pick(&cands, 3, NOW, 1, true, false)),
            vec![2, 3],
            "fresh_only never repeats"
        );
        let filled = pick(&cands, 3, NOW, 1, false, false);
        assert_eq!(
            ids(&cands, &filled),
            vec![1, 2, 3],
            "without fresh_only the recent one only fills the gap"
        );
        let two = pick(&cands, 2, NOW, 1, false, false);
        assert_eq!(ids(&cands, &two), vec![2, 3]);
    }

    #[test]
    fn a_blank_is_included_when_one_exists() {
        let mut cands: Vec<Candidate> = (1..=5)
            .map(|i| cand(i, ItemKind::Mcq, 0, 0, None))
            .collect();
        cands.push(cand(9, ItemKind::Cloze, 10, 10, None));
        for seed in 0..20 {
            let picked = pick(&cands, 3, NOW, seed, true, true);
            assert!(picked.iter().any(|&i| cands[i].item.id == 9), "seed {seed}");
        }
        let no_cloze: Vec<Candidate> = (1..=4)
            .map(|i| cand(i, ItemKind::Mcq, 0, 0, None))
            .collect();
        assert_eq!(pick(&no_cloze, 3, NOW, 1, true, true).len(), 3);
    }

    #[test]
    fn fewer_candidates_than_requested_returns_what_exists() {
        let cands = vec![cand(1, ItemKind::Cloze, 0, 0, None)];
        assert_eq!(pick(&cands, 3, NOW, 1, true, true).len(), 1);
        assert!(pick(&[], 3, NOW, 1, true, true).is_empty());
    }
}
