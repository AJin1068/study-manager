//! Which wiki page to study next. Pure functions of page facts, quiz accuracy, and focus by hour,
//! so a golden test pins the order; no LLM and no network are involved.
//!
//! Priority (most urgent first): an empty page (short body), a page the user often gets wrong,
//! a page not touched for a long time, then the rest. Focus decides which slot gets which: a slot
//! in an hour the user focuses well in gets the hardest page, a slot in a poorly focused hour
//! gets an easy page that already has questions (review and quiz), others follow the priority.

use chrono::{TimeZone, Timelike};
use rusqlite::Connection;
use serde::Serialize;

use crate::error::AppResult;
use crate::store::topic as repo;
use crate::store::wiki as wiki_repo;
use crate::wiki::service::WikiService;

/// A body shorter than this (characters) counts as empty.
pub const SHORT_BODY_CHARS: usize = 200;
/// Fewer answers than this say nothing about accuracy.
pub const MIN_ANSWERS: i64 = 3;
/// Accuracy (percent) below which a page counts as often wrong.
pub const LOW_ACCURACY_PCT: i64 = 60;
/// A page untouched this long counts as stale.
pub const STALE_MS: i64 = 14 * 24 * 60 * 60 * 1000;
/// Focus score (percent) from which an hour counts as well focused / below which as poorly focused.
pub const HIGH_FOCUS_PCT: i64 = 70;
pub const LOW_FOCUS_PCT: i64 = 50;
/// Recorded study time an hour needs before its score is trusted.
pub const MIN_FOCUS_SAMPLE_MS: i64 = 10 * 60 * 1000;
/// How far back focus data is read.
pub const FOCUS_LOOKBACK_MS: i64 = 28 * 24 * 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Empty,
    LowAccuracy,
    Stale,
    Hard,
    Review,
    /// The user picked it.
    Chosen,
}

impl Reason {
    #[cfg(test)]
    pub const ALL: [Reason; 6] = [Reason::Empty, Reason::LowAccuracy, Reason::Stale, Reason::Hard, Reason::Review, Reason::Chosen];

    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Empty => "empty",
            Reason::LowAccuracy => "low_accuracy",
            Reason::Stale => "stale",
            Reason::Hard => "hard",
            Reason::Review => "review",
            Reason::Chosen => "chosen",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [Reason::Empty, Reason::LowAccuracy, Reason::Stale, Reason::Hard, Reason::Review, Reason::Chosen].into_iter().find(|r| r.as_str() == raw)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageStat {
    pub page_id: i64,
    pub page_name: String,
    pub body_chars: usize,
    pub quiz_total: i64,
    pub quiz_correct: i64,
    pub updated_at_ms: i64,
}

/// Focus score of one local hour of the day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusHour {
    pub hour: u32,
    pub score_pct: i64,
}

/// A study time to give a page to: the hour it starts at (local).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub plan_id: i64,
    pub date: String,
    pub hour: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub plan_id: i64,
    pub date: String,
    pub page_id: i64,
    pub page_name: String,
    pub reason: Reason,
}

/// A recommended page without a slot.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Pick {
    pub page_id: i64,
    pub page_name: String,
    pub reason: Reason,
}

fn accuracy_pct(p: &PageStat) -> Option<i64> {
    (p.quiz_total >= MIN_ANSWERS).then(|| p.quiz_correct.saturating_mul(100) / p.quiz_total)
}

/// 0 empty, 1 often wrong, 2 stale, 3 nothing special.
fn tier(p: &PageStat, now_ms: i64) -> u8 {
    if p.body_chars < SHORT_BODY_CHARS {
        0
    } else if accuracy_pct(p).is_some_and(|a| a < LOW_ACCURACY_PCT) {
        1
    } else if now_ms.saturating_sub(p.updated_at_ms) >= STALE_MS {
        2
    } else {
        3
    }
}

fn priority_key(p: &PageStat, now_ms: i64) -> (u8, i64, i64, i64) {
    (tier(p, now_ms), accuracy_pct(p).unwrap_or(50), p.updated_at_ms, p.page_id)
}

/// Higher is harder: wrong answers and a long page. An unanswered page counts as middling.
pub fn difficulty(p: &PageStat) -> i64 {
    let wrong = 100 - accuracy_pct(p).unwrap_or(50);
    let length = i64::try_from(p.body_chars / 50).unwrap_or(i64::MAX).min(50);
    wrong + length
}

fn reason_for(p: &PageStat, now_ms: i64, fallback: Reason) -> Reason {
    match tier(p, now_ms) {
        0 => Reason::Empty,
        1 => Reason::LowAccuracy,
        2 => Reason::Stale,
        _ => fallback,
    }
}

/// Pages in the order they should be studied, with why.
pub fn rank(pages: &[PageStat], now_ms: i64) -> Vec<Pick> {
    let mut ordered: Vec<&PageStat> = pages.iter().collect();
    ordered.sort_by_key(|p| priority_key(p, now_ms));
    ordered
        .into_iter()
        .map(|p| Pick { page_id: p.page_id, page_name: p.page_name.clone(), reason: reason_for(p, now_ms, Reason::Review) })
        .collect()
}

/// Gives each slot (earliest first) one page, each page at most once. No pages means no
/// assignments; more slots than pages leaves the last slots without one.
pub fn recommend_next(pages: &[PageStat], focus_by_hour: &[FocusHour], slots: &[Slot], now_ms: i64) -> Vec<Assignment> {
    let mut pool: Vec<&PageStat> = pages.iter().collect();
    pool.sort_by_key(|p| priority_key(p, now_ms));
    let mut slots: Vec<&Slot> = slots.iter().collect();
    slots.sort_by(|a, b| (&a.date, a.hour, a.plan_id).cmp(&(&b.date, b.hour, b.plan_id)));

    let mut out = Vec::new();
    for slot in slots {
        if pool.is_empty() {
            break;
        }
        let score = focus_by_hour.iter().find(|f| f.hour == slot.hour).map(|f| f.score_pct);
        let (index, fallback) = match score {
            Some(s) if s >= HIGH_FOCUS_PCT => {
                // Hardest first; the priority order breaks ties because `pool` is sorted by it.
                let best = pool.iter().enumerate().max_by_key(|(i, p)| (difficulty(p), std::cmp::Reverse(*i))).map(|(i, _)| i);
                (best.unwrap_or(0), Reason::Hard)
            }
            Some(s) if s < LOW_FOCUS_PCT => {
                // Easiest page that already has questions to review; else the priority order.
                let easy = pool
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.quiz_total > 0 && p.body_chars >= SHORT_BODY_CHARS)
                    .min_by_key(|(i, p)| (difficulty(p), *i))
                    .map(|(i, _)| i);
                (easy.unwrap_or(0), Reason::Review)
            }
            _ => (0, Reason::Review),
        };
        let page = pool.remove(index);
        out.push(Assignment {
            plan_id: slot.plan_id,
            date: slot.date.clone(),
            page_id: page.page_id,
            page_name: page.page_name.clone(),
            reason: reason_for(page, now_ms, fallback),
        });
    }
    out
}

/// Focus score per local hour from `(minute_start_ms, credited, away, idle)` rows. An hour with
/// less than [`MIN_FOCUS_SAMPLE_MS`] of recorded study time is left out.
pub fn focus_by_hour<Tz: TimeZone>(tz: &Tz, minutes: &[(i64, i64, i64, i64)]) -> Vec<FocusHour> {
    let mut totals = [(0_i64, 0_i64); 24];
    for &(at_ms, credited, away, idle) in minutes {
        let Some(local) = tz.timestamp_millis_opt(at_ms).single() else { continue };
        let slot = &mut totals[usize::try_from(local.hour()).unwrap_or(0).min(23)];
        slot.0 += credited;
        slot.1 += credited + away + idle;
    }
    totals
        .iter()
        .enumerate()
        .filter(|(_, (_, all))| *all >= MIN_FOCUS_SAMPLE_MS)
        .map(|(hour, (credited, all))| FocusHour { hour: u32::try_from(hour).unwrap_or(0), score_pct: credited * 100 / all })
        .collect()
}

// ---- database wrappers ----

/// Facts about every page of a subject; the body is read from the wiki folder for its length.
/// A page whose file cannot be read counts as empty.
pub fn page_stats(conn: &Connection, subject_id: i64) -> AppResult<Vec<PageStat>> {
    let subject = wiki_repo::get_subject(conn, subject_id)?;
    let fs = WikiService::wiki_fs(conn)?;
    wiki_repo::list_pages(conn, subject_id)?
        .into_iter()
        .map(|page| {
            let body_chars = fs.read_page(&subject.slug, &page.page_name).ok().flatten().map_or(0, |b| b.chars().count());
            Ok(PageStat {
                page_id: page.id,
                page_name: page.page_name,
                body_chars,
                quiz_total: page.quiz_total,
                quiz_correct: page.quiz_correct,
                updated_at_ms: page.updated_at_ms,
            })
        })
        .collect()
}

/// Focus by local hour over the last four weeks.
pub fn focus_hours<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<Vec<FocusHour>> {
    Ok(focus_by_hour(tz, &repo::focus_minutes_since(conn, now_ms - FOCUS_LOOKBACK_MS)?))
}

/// The pages of a subject to study next, most urgent first (at most `limit`). Empty when the
/// subject has no pages.
pub fn recommend_for_subject(conn: &Connection, subject_id: i64, now_ms: i64) -> AppResult<Vec<Pick>> {
    let mut picks = rank(&page_stats(conn, subject_id)?, now_ms);
    picks.truncate(PICK_LIMIT);
    Ok(picks)
}

const PICK_LIMIT: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    const NOW: i64 = 1_800_000_000_000;
    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn page(id: i64, chars: usize, total: i64, correct: i64, age_days: i64) -> PageStat {
        PageStat { page_id: id, page_name: format!("p{id}"), body_chars: chars, quiz_total: total, quiz_correct: correct, updated_at_ms: NOW - age_days * DAY }
    }

    fn slot(plan: i64, hour: u32) -> Slot {
        Slot { plan_id: plan, date: "2026-11-07".into(), hour }
    }

    fn ids(a: &[Assignment]) -> Vec<i64> {
        a.iter().map(|x| x.page_id).collect()
    }

    #[test]
    fn empty_beats_often_wrong_beats_stale_beats_the_rest() {
        let pages = [page(1, 900, 10, 9, 1), page(2, 900, 10, 9, 30), page(3, 900, 10, 2, 1), page(4, 20, 0, 0, 1)];
        let picks = rank(&pages, NOW);
        assert_eq!(picks.iter().map(|p| p.page_id).collect::<Vec<_>>(), vec![4, 3, 2, 1]);
        assert_eq!(picks.iter().map(|p| p.reason).collect::<Vec<_>>(), vec![Reason::Empty, Reason::LowAccuracy, Reason::Stale, Reason::Review]);
    }

    #[test]
    fn a_page_with_too_few_answers_is_not_called_often_wrong() {
        let pages = [page(1, 900, 2, 0, 1), page(2, 900, 10, 5, 1)];
        assert_eq!(rank(&pages, NOW)[0].page_id, 2);
    }

    #[test]
    fn a_well_focused_hour_gets_the_hardest_page_and_a_poorly_focused_hour_gets_review() {
        // 1: easy reviewed page, 2: hard and long, 3: fine page with questions.
        let pages = [page(1, 400, 10, 10, 1), page(2, 2400, 10, 7, 1), page(3, 800, 10, 8, 1)];
        let focus = [FocusHour { hour: 9, score_pct: 85 }, FocusHour { hour: 22, score_pct: 30 }];
        let out = recommend_next(&pages, &focus, &[slot(10, 9), slot(11, 22)], NOW);
        assert_eq!(ids(&out), vec![2, 1]);
        assert_eq!((out[0].reason, out[1].reason), (Reason::Hard, Reason::Review));
    }

    #[test]
    fn an_unknown_hour_follows_the_priority_order_and_slots_go_earliest_first() {
        let pages = [page(1, 900, 10, 9, 1), page(2, 20, 0, 0, 1)];
        let out = recommend_next(&pages, &[], &[slot(11, 15), slot(10, 8)], NOW);
        assert_eq!(out.iter().map(|a| (a.plan_id, a.page_id)).collect::<Vec<_>>(), vec![(10, 2), (11, 1)]);
    }

    #[test]
    fn a_page_is_assigned_once_and_no_pages_gives_nothing() {
        let pages = [page(1, 900, 0, 0, 1)];
        assert_eq!(recommend_next(&pages, &[], &[slot(1, 9), slot(2, 10)], NOW).len(), 1);
        assert!(recommend_next(&[], &[], &[slot(1, 9)], NOW).is_empty());
    }

    #[test]
    fn the_result_is_deterministic() {
        let pages = [page(1, 900, 4, 2, 3), page(2, 900, 4, 2, 3), page(3, 900, 4, 2, 3)];
        let focus = [FocusHour { hour: 9, score_pct: 90 }];
        let a = recommend_next(&pages, &focus, &[slot(1, 9), slot(2, 9)], NOW);
        assert_eq!(a, recommend_next(&pages, &focus, &[slot(1, 9), slot(2, 9)], NOW));
        assert_eq!(ids(&a), vec![1, 2], "ties keep the priority order");
    }

    #[test]
    fn focus_hours_need_enough_recorded_time() {
        let hour_ms = 3_600_000;
        let base = (NOW / DAY) * DAY; // midnight UTC
        let mut rows = Vec::new();
        for m in 0..20 {
            rows.push((base + 9 * hour_ms + m * 60_000, 54_000, 6_000, 0)); // hour 9: 90%
        }
        rows.push((base + 14 * hour_ms, 60_000, 0, 0)); // hour 14: one minute only
        let out = focus_by_hour(&Utc, &rows);
        assert_eq!(out, vec![FocusHour { hour: 9, score_pct: 90 }]);
    }

    #[test]
    fn reasons_match_the_schema_check_list() {
        let conn = crate::db::open_in_memory().expect("db");
        let sql: String = conn.query_row("SELECT sql FROM sqlite_master WHERE name = 'topic_assignment'", [], |r| r.get(0)).expect("schema");
        for r in Reason::ALL {
            assert!(sql.contains(&format!("'{}'", r.as_str())), "{}", r.as_str());
            assert_eq!(Reason::parse(r.as_str()), Some(r));
        }
    }
}
