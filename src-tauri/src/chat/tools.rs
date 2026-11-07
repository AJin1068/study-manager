//! The chatbot's read-only tools. Each returns a small JSON value that goes to the model as a data
//! block, so every field here is something `llm::egress` lists as allowed for plan chat: dates,
//! times, counts, plan and subject names, wiki page names with accuracy. No window title, app
//! name, URL, or typed text exists in these results.

use chrono::{Datelike, Duration, NaiveDate, NaiveTime, TimeZone};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{Value, json};

use super::protocol::{FreeSlotArgs, RangeArgs, Tool};
use crate::dashboard;
use crate::error::{AppError, AppResult};
use crate::plan::{self, Occurrence, fmt_date, local_to_ms, parse_date};
use crate::store;
use crate::store::chat as repo;
use crate::store::wiki as wiki_repo;
use crate::wiki::recommend::{self, LOW_ACCURACY_PCT, MIN_ANSWERS};

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;
/// Longest range one tool call may cover.
pub const MAX_RANGE_DAYS: i64 = 31;
/// Free time is offered from this minute of the local day until midnight.
pub const DAY_START_MIN: i64 = 6 * 60;
const MIN_SLOT_MIN: i64 = 15;
const MAX_SLOT_MIN: i64 = 480;
const DEFAULT_SLOT_MIN: i64 = 60;
const MAX_SLOTS: usize = 40;
const MAX_PLANS: usize = 60;
const WEAK_PER_SUBJECT: usize = 5;
const FOCUS_WINDOW_DAYS: i64 = 28;

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct FreeSlot {
    pub date: String,
    pub start: String,
    pub end: String,
    #[serde(skip)]
    pub start_min: i64,
    #[serde(skip)]
    pub end_min: i64,
}

pub fn hhmm(minutes: i64) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn range(from: &str, to: &str) -> AppResult<(NaiveDate, NaiveDate)> {
    let (from, to) = (parse_date(from)?, parse_date(to)?);
    if to < from || (to - from).num_days() >= MAX_RANGE_DAYS {
        return Err(AppError::Invalid(format!(
            "range must be 1-{MAX_RANGE_DAYS} days"
        )));
    }
    Ok((from, to))
}

/// Free windows of at least `min_minutes` between 06:00 and midnight on each day of `from..=to`,
/// around the given occurrences and not before `now_ms`.
pub fn free_slots<Tz: TimeZone>(
    tz: &Tz,
    occurrences: &[Occurrence],
    from: NaiveDate,
    to: NaiveDate,
    now_ms: i64,
    min_minutes: i64,
) -> AppResult<Vec<FreeSlot>> {
    let mut spans: Vec<(i64, i64)> = occurrences
        .iter()
        .map(|o| (o.planned_start_ms, o.planned_end_ms))
        .collect();
    spans.sort_unstable();
    let now_floor = (now_ms + MINUTE_MS - 1).div_euclid(MINUTE_MS) * MINUTE_MS;
    let mut out = Vec::new();
    let mut day = from;
    while day <= to {
        let midnight = local_to_ms(tz, day, NaiveTime::MIN)?;
        let next_midnight = local_to_ms(tz, day + Duration::days(1), NaiveTime::MIN)?;
        let window_start = (midnight + DAY_START_MIN * MINUTE_MS).max(now_floor);
        let window_end = next_midnight;
        let mut cursor = window_start;
        let push = |start: i64, end: i64, out: &mut Vec<FreeSlot>| {
            if end - start >= min_minutes * MINUTE_MS {
                let (start_min, end_min) =
                    ((start - midnight) / MINUTE_MS, (end - midnight) / MINUTE_MS);
                out.push(FreeSlot {
                    date: fmt_date(day),
                    start: hhmm(start_min),
                    end: hhmm(end_min),
                    start_min,
                    end_min,
                });
            }
        };
        for &(span_start, span_end) in &spans {
            if span_end <= cursor || span_start >= window_end {
                continue;
            }
            if span_start > cursor {
                push(cursor, span_start.min(window_end), &mut out);
            }
            cursor = cursor.max(span_end);
        }
        if cursor < window_end {
            push(cursor, window_end, &mut out);
        }
        day += Duration::days(1);
    }
    Ok(out)
}

pub fn clamp_min_minutes(requested: Option<i64>) -> i64 {
    requested
        .unwrap_or(DEFAULT_SLOT_MIN)
        .clamp(MIN_SLOT_MIN, MAX_SLOT_MIN)
}

fn list_subjects(conn: &Connection) -> AppResult<Value> {
    let mut names: Vec<String> = wiki_repo::list_subjects(conn, false)?
        .into_iter()
        .map(|s| s.name)
        .collect();
    names.extend(repo::plan_subjects(conn)?);
    names.sort();
    names.dedup();
    Ok(json!({ "subjects": names }))
}

fn list_plans<Tz: TimeZone>(conn: &Connection, tz: &Tz, args: &RangeArgs) -> AppResult<Value> {
    let (from, to) = range(&args.from, &args.to)?;
    let (occurrences, _) = plan::occurrences_in(conn, tz, from, to)?;
    let plans: Vec<Value> = occurrences
        .iter()
        .take(MAX_PLANS)
        .map(|o| json!({ "plan_id": o.plan_id, "date": o.date, "time": o.start_time, "sets": o.sets, "minutes": o.duration_min, "name": o.name, "subject": o.subject }))
        .collect();
    Ok(json!({ "plans": plans }))
}

fn list_free_slots<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    args: &FreeSlotArgs,
    now_ms: i64,
) -> AppResult<Value> {
    let (from, to) = range(&args.from, &args.to)?;
    // A day before `from` can spill into it.
    let (occurrences, _) = plan::occurrences_in(conn, tz, from - Duration::days(1), to)?;
    let slots = free_slots(
        tz,
        &occurrences,
        from,
        to,
        now_ms,
        clamp_min_minutes(args.min_minutes),
    )?;
    Ok(json!({ "slots": slots.into_iter().take(MAX_SLOTS).collect::<Vec<_>>() }))
}

/// Deviation counts per weekday (Monday first), the share of focused time per hour, and how often
/// plans were postponed from each weekday, over the last four weeks. Counts only.
fn focus_summary<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<Value> {
    let hours: Vec<Value> = recommend::focus_hours(conn, tz, now_ms)?
        .into_iter()
        .map(|h| json!({ "hour": h.hour, "focus_pct": h.score_pct }))
        .collect();
    let weakness = dashboard::weakness(conn, tz, now_ms)?;
    let deviations: Vec<i64> = weakness
        .heatmap
        .iter()
        .map(|row| row.iter().sum())
        .collect();
    let mut postponed = vec![0_i64; 7];
    for raw in repo::postponed_before_json(conn, now_ms - FOCUS_WINDOW_DAYS * DAY_MS)? {
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let date = value
            .get("date")
            .or_else(|| value.get("startDate"))
            .and_then(Value::as_str);
        if let Some(day) = date.and_then(|d| parse_date(d).ok()) {
            if let Some(slot) = postponed
                .get_mut(usize::try_from(day.weekday().num_days_from_monday()).unwrap_or(0))
            {
                *slot += 1;
            }
        }
    }
    Ok(json!({
        "window_days": FOCUS_WINDOW_DAYS,
        "weekday_order": "mon,tue,wed,thu,fri,sat,sun",
        "focus_by_hour": hours,
        "deviations_by_weekday": deviations,
        "postponed_by_weekday": postponed,
    }))
}

fn weak_pages(conn: &Connection) -> AppResult<Value> {
    let mut subjects = Vec::new();
    for subject in wiki_repo::list_subjects(conn, false)? {
        let mut weak: Vec<(i64, String)> = wiki_repo::list_pages(conn, subject.id)?
            .into_iter()
            .filter(|p| p.quiz_total >= MIN_ANSWERS)
            .map(|p| (p.quiz_correct * 100 / p.quiz_total, p.page_name))
            .filter(|(pct, _)| *pct < LOW_ACCURACY_PCT)
            .collect();
        weak.sort();
        weak.truncate(WEAK_PER_SUBJECT);
        if !weak.is_empty() {
            let pages: Vec<Value> = weak
                .into_iter()
                .map(|(pct, name)| json!({ "page": name, "accuracy_pct": pct }))
                .collect();
            subjects.push(json!({ "subject": subject.name, "pages": pages }));
        }
    }
    Ok(json!({ "weak": subjects }))
}

/// Runs a read-only tool. `Propose` is handled by the service and is not a read.
pub fn run_read<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    tool: &Tool,
    now_ms: i64,
) -> AppResult<Value> {
    match tool {
        Tool::ListSubjects => list_subjects(conn),
        Tool::ListPlans(args) => list_plans(conn, tz, args),
        Tool::ListFreeSlots(args) => list_free_slots(conn, tz, args, now_ms),
        Tool::GetFocusSummary => focus_summary(conn, tz, now_ms),
        Tool::GetWikiWeakPages => weak_pages(conn),
        Tool::Propose(_) => Err(AppError::Internal("a proposal is not a read tool".into())),
    }
}

/// The plan's name and subject, for describing a move or a cancel in the preview.
pub fn plan_label(conn: &Connection, plan_id: i64) -> AppResult<Option<String>> {
    Ok(store::get_plan(conn, plan_id)?.map(|p| format!("{} ({})", p.name, p.subject)))
}
