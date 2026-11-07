//! Dashboard aggregates (stage 1-5): today, this week, and the first part of the weakness view.
//! The UI never reads the database; these functions fold recorded runs, minute buckets, events,
//! and the ledger into the numbers a screen shows. Loading is in `queries`, folding is pure.
//!
//! Definitions (D-25, D-29):
//! - *planned* = sets x study minutes of a run, plus occurrences that have no run row yet;
//! - *actual* = recognized study time (`focus_minutes.credited_ms`), which already excludes
//!   away, idle, break, and downtime time;
//! - a run belongs to the local date it was planned for, so a run past midnight stays on one day;
//! - a day is *achieved* when actual >= 80% of planned; the streak counts consecutive achieved
//!   days, skips days with nothing planned, ends at a planned day that was missed, and does not
//!   break on today while today is still in progress;
//! - focus score = recognized / studied x 100 over finished study segments.

mod queries;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike};
use rusqlite::Connection;
use serde::Serialize;

use crate::downtime::{self, Interval};
use crate::error::{AppError, AppResult};
use crate::focus::calc;
use crate::plan;
use crate::points::{self, PointsStatus};
use crate::store;
use queries::RunFact;

/// A day counts toward the streak at this share of its planned time (D-29).
pub const STREAK_PCT: i64 = 80;
/// The streak looks back this far; a longer run shows as this many days.
const STREAK_LOOKBACK_DAYS: i64 = 366;
/// The next plan is searched this many days ahead.
const NEXT_PLAN_HORIZON_DAYS: i64 = 14;
/// The weakness view covers this many days up to now.
pub const WEAKNESS_DAYS: i64 = 28;
/// A plan counts as "often postponed" from this many postponements in the window.
const POSTPONED_MIN: i64 = 2;
const LIST_LIMIT: i64 = 5;
const DAY_MS: i64 = 86_400_000;
const DATE_FMT: &str = "%Y-%m-%d";

// ---- wire types ----

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NextPlan {
    pub plan_id: i64,
    pub name: String,
    pub subject: String,
    pub planned_start_ms: i64,
    pub starts_in_ms: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TodayDashboard {
    pub date: String,
    pub next_plan: Option<NextPlan>,
    pub planned_ms: i64,
    pub actual_ms: i64,
    /// 0 to 100; `None` when nothing is planned today.
    pub achievement_pct: Option<i64>,
    pub streak_days: i64,
    pub streak_threshold_pct: i64,
    pub points: PointsStatus,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DayStat {
    pub date: String,
    /// Monday is 0.
    pub weekday: i64,
    pub planned_ms: i64,
    pub actual_ms: i64,
    pub achievement_pct: Option<i64>,
    pub achieved: bool,
    /// Recognized over studied time; `None` when no study segment of the day has finished.
    pub focus_score: Option<i64>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubjectTime {
    pub subject: String,
    pub actual_ms: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeekDashboard {
    /// Monday to Sunday of the local week that holds today.
    pub week_start: String,
    pub week_end: String,
    pub days: Vec<DayStat>,
    pub planned_ms: i64,
    pub actual_ms: i64,
    /// Per-day recognized time capped at the day's plan, over the week's plan; `None` without plans.
    pub achievement_pct: Option<i64>,
    pub achieved_days: i64,
    pub planned_days: i64,
    pub by_subject: Vec<SubjectTime>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppCount {
    pub app_id: String,
    pub count: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PostponedPlan {
    pub plan_id: i64,
    pub name: String,
    pub subject: String,
    pub count: i64,
}

/// Time the app was off (killed) or the machine asleep, shown as its own stat so it never reads as
/// a lack of effort.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppOffStat {
    pub sleep_ms: i64,
    pub forced_quit_ms: i64,
    pub forced_quit_count: i64,
    /// Union of sleep and forced-quit time that fell inside planned run windows.
    pub planned_overlap_ms: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseUsage {
    pub emergency_pass: i64,
    pub skip_today: i64,
    pub early_release: i64,
    pub aborted_runs: i64,
}

/// What the paired phone reported, as plain statistics. Time the phone was not under the lock is
/// shown on its own (like `AppOffStat`) and pick-ups are only counted: neither changes the focus
/// score or any point (decision O-23, revisited after real use).
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PhoneStat {
    /// Union of every gap (unreachable, lease expired, service disabled) inside the window.
    pub unreachable_ms: i64,
    pub gaps: i64,
    pub service_disabled_gaps: i64,
    /// Study sets of the window the phone sent a pick-up count for.
    pub pickup_sets_reported: i64,
    pub pickup_sets_with_pickups: i64,
    pub pickups_total: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Weakness {
    pub from_ms: i64,
    pub to_ms: i64,
    pub deviation_apps: Vec<AppCount>,
    /// Mean time from a study set's start to its first deviation, over sets that had one.
    pub avg_first_away_ms: Option<i64>,
    pub sets_with_away: i64,
    pub sets_total: i64,
    /// `heatmap[weekday][hour]` deviation counts; Monday is 0, hours are local.
    pub heatmap: Vec<Vec<i64>>,
    pub postponed_plans: Vec<PostponedPlan>,
    pub app_off: AppOffStat,
    pub releases: ReleaseUsage,
    pub phone: PhoneStat,
}

// ---- pure folds ----

fn pct(part: i64, whole: i64) -> Option<i64> {
    (whole > 0).then(|| ((part * 100 + whole / 2) / whole).clamp(0, 100))
}

/// Whether `actual` reaches the streak share of `planned`. Nothing planned is never achieved.
pub fn achieved(planned_ms: i64, actual_ms: i64) -> bool {
    planned_ms > 0 && actual_ms * 100 >= planned_ms * STREAK_PCT
}

/// Streak ending at `today` over `(planned, actual)` per date; see the module notes.
pub fn streak_days(days: &BTreeMap<NaiveDate, (i64, i64)>, today: NaiveDate) -> i64 {
    let Some(earliest) = days.keys().next().copied() else {
        return 0;
    };
    let mut streak = 0;
    let mut day = today;
    while day >= earliest && (today - day).num_days() <= STREAK_LOOKBACK_DAYS {
        if let Some((planned, actual)) = days.get(&day).copied().filter(|(planned, _)| *planned > 0)
        {
            if achieved(planned, actual) {
                streak += 1;
            } else if day != today {
                break;
            }
        }
        day -= Duration::days(1);
    }
    streak
}

/// Merges overlapping or touching intervals into a sorted disjoint list.
fn merge(mut spans: Vec<Interval>) -> Vec<Interval> {
    spans.retain(|s| s.end > s.start);
    spans.sort_unstable();
    let mut out: Vec<Interval> = Vec::new();
    for span in spans {
        match out.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => out.push(span),
        }
    }
    out
}

/// Total length of `a` that lies inside `b`, each side counted once however its rows overlap.
pub fn overlap_ms(a: Vec<Interval>, b: Vec<Interval>) -> i64 {
    let (a, b) = (merge(a), merge(b));
    let (mut i, mut j, mut total) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        total += (a[i].end.min(b[j].end) - a[i].start.max(b[j].start)).max(0);
        if a[i].end < b[j].end {
            i += 1;
        } else {
            j += 1;
        }
    }
    total
}

/// Weekday (Monday 0) and hour of a timestamp in `tz`.
fn weekday_hour<Tz: TimeZone>(tz: &Tz, at_ms: i64) -> Option<(usize, usize)> {
    let local: DateTime<Tz> = tz.timestamp_millis_opt(at_ms).single()?;
    Some((
        usize::try_from(local.weekday().num_days_from_monday()).ok()?,
        usize::try_from(local.hour()).ok()?,
    ))
}

/// Deviation counts per local weekday and hour. Timestamps that do not map to one local time
/// (the repeated hour of a DST change) are skipped rather than guessed.
pub fn heatmap<Tz: TimeZone>(tz: &Tz, times: &[i64]) -> Vec<Vec<i64>> {
    let mut grid = vec![vec![0_i64; 24]; 7];
    for at in times {
        if let Some((weekday, hour)) = weekday_hour(tz, *at) {
            grid[weekday][hour] += 1;
        }
    }
    grid
}

/// Planned time of an occurrence that has no run row (a future plan, or one the app never saw).
struct Extra {
    date: String,
    planned_ms: i64,
}

fn local_date<Tz: TimeZone>(tz: &Tz, at_ms: i64) -> AppResult<NaiveDate> {
    tz.timestamp_millis_opt(at_ms)
        .single()
        .map(|t| t.date_naive())
        .ok_or_else(|| AppError::Internal(format!("timestamp {at_ms} has no unique local time")))
}

fn date_str(date: NaiveDate) -> String {
    date.format(DATE_FMT).to_string()
}

/// Occurrences in `from..=to` that have no run row, as extra planned time. Occurrences from before
/// the plan existed are not planned time.
fn extras<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<(plan::Occurrence, Extra)>> {
    let keys = queries::run_keys(conn, &date_str(from), &date_str(to))?;
    let (occurrences, modes) = plan::occurrences_in(conn, tz, from, to)?;
    let mut out = Vec::new();
    for o in occurrences {
        if !o.eligible_for_missed || keys.contains(&(o.plan_id, o.date.clone())) {
            continue;
        }
        let planned_ms = o.sets * modes.config(o.level)?.study_min * 60_000;
        out.push((
            o.clone(),
            Extra {
                date: o.date,
                planned_ms,
            },
        ));
    }
    Ok(out)
}

struct DayAcc {
    planned_ms: i64,
    actual_ms: i64,
    studied_ms: i64,
    closed_credited_ms: i64,
}

fn fold_days(runs: &[RunFact], extras: &[Extra]) -> BTreeMap<String, DayAcc> {
    let mut days: BTreeMap<String, DayAcc> = BTreeMap::new();
    let empty = || DayAcc {
        planned_ms: 0,
        actual_ms: 0,
        studied_ms: 0,
        closed_credited_ms: 0,
    };
    for run in runs {
        let day = days.entry(run.date.clone()).or_insert_with(empty);
        day.planned_ms += run.planned_ms;
        day.actual_ms += run.credited_ms;
        day.studied_ms += run.studied_ms;
        day.closed_credited_ms += run.closed_credited_ms;
    }
    for extra in extras {
        days.entry(extra.date.clone())
            .or_insert_with(empty)
            .planned_ms += extra.planned_ms;
    }
    days
}

fn day_stat(date: NaiveDate, acc: Option<&DayAcc>) -> DayStat {
    let (planned, actual, studied, credited) = acc.map_or((0, 0, 0, 0), |a| {
        (
            a.planned_ms,
            a.actual_ms,
            a.studied_ms,
            a.closed_credited_ms,
        )
    });
    DayStat {
        date: date_str(date),
        weekday: i64::from(date.weekday().num_days_from_monday()),
        planned_ms: planned,
        actual_ms: actual,
        achievement_pct: pct(actual, planned),
        achieved: achieved(planned, actual),
        focus_score: calc::focus_score(credited, studied),
    }
}

// ---- today ----

/// Today's view. Settles the point ledger first so level and stage are current.
pub fn today<Tz: TimeZone>(
    conn: &mut Connection,
    tz: &Tz,
    now_ms: i64,
) -> AppResult<TodayDashboard> {
    let today = local_date(tz, now_ms)?;
    let lookback = today - Duration::days(STREAK_LOOKBACK_DAYS);
    let runs = queries::run_facts(conn, &date_str(lookback), &date_str(today))?;
    let past_extras = extras(conn, tz, lookback, today)?;
    let extra_rows: Vec<Extra> = past_extras.into_iter().map(|(_, e)| e).collect();
    let folded = fold_days(&runs, &extra_rows);

    let mut per_day = BTreeMap::new();
    for (date, acc) in &folded {
        per_day.insert(plan::parse_date(date)?, (acc.planned_ms, acc.actual_ms));
    }
    let (planned_ms, actual_ms) = per_day.get(&today).copied().unwrap_or((0, 0));

    Ok(TodayDashboard {
        date: date_str(today),
        next_plan: next_plan(conn, tz, today, now_ms)?,
        planned_ms,
        actual_ms,
        achievement_pct: pct(actual_ms, planned_ms),
        streak_days: streak_days(&per_day, today),
        streak_threshold_pct: STREAK_PCT,
        points: points::settled_status(conn, now_ms)?,
    })
}

/// The earliest plan that starts after `now_ms` and has not been run yet.
fn next_plan<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    today: NaiveDate,
    now_ms: i64,
) -> AppResult<Option<NextPlan>> {
    let horizon = today + Duration::days(NEXT_PLAN_HORIZON_DAYS);
    let mut upcoming: Vec<plan::Occurrence> = extras(conn, tz, today, horizon)?
        .into_iter()
        .map(|(o, _)| o)
        .filter(|o| o.planned_start_ms > now_ms)
        .collect();
    upcoming.sort_by_key(|o| (o.planned_start_ms, o.plan_id));
    Ok(upcoming.into_iter().next().map(|o| NextPlan {
        plan_id: o.plan_id,
        name: o.name,
        subject: o.subject,
        planned_start_ms: o.planned_start_ms,
        starts_in_ms: o.planned_start_ms - now_ms,
    }))
}

// ---- week ----

pub fn week<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<WeekDashboard> {
    let today = local_date(tz, now_ms)?;
    let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
    let sunday = monday + Duration::days(6);
    let runs = queries::run_facts(conn, &date_str(monday), &date_str(sunday))?;
    let extra_rows: Vec<Extra> = extras(conn, tz, monday, sunday)?
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    let folded = fold_days(&runs, &extra_rows);

    let days: Vec<DayStat> = (0..7)
        .map(|offset| monday + Duration::days(offset))
        .map(|d| day_stat(d, folded.get(&date_str(d))))
        .collect();
    let planned_ms: i64 = days.iter().map(|d| d.planned_ms).sum();
    let capped_actual: i64 = days.iter().map(|d| d.actual_ms.min(d.planned_ms)).sum();

    let mut subjects: HashMap<&str, i64> = HashMap::new();
    for run in &runs {
        *subjects.entry(run.subject.as_str()).or_insert(0) += run.credited_ms;
    }
    let mut by_subject: Vec<SubjectTime> = subjects
        .into_iter()
        .filter(|(_, ms)| *ms > 0)
        .map(|(subject, actual_ms)| SubjectTime {
            subject: subject.to_string(),
            actual_ms,
        })
        .collect();
    by_subject.sort_by(|a, b| {
        b.actual_ms
            .cmp(&a.actual_ms)
            .then_with(|| a.subject.cmp(&b.subject))
    });

    Ok(WeekDashboard {
        week_start: date_str(monday),
        week_end: date_str(sunday),
        planned_ms,
        actual_ms: days.iter().map(|d| d.actual_ms).sum(),
        achievement_pct: pct(capped_actual, planned_ms),
        achieved_days: i64::try_from(days.iter().filter(|d| d.achieved).count()).unwrap_or(0),
        planned_days: i64::try_from(days.iter().filter(|d| d.planned_ms > 0).count()).unwrap_or(0),
        days,
        by_subject,
    })
}

// ---- weakness ----

/// The last [`WEAKNESS_DAYS`] days up to `now_ms`.
pub fn weakness<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<Weakness> {
    let (from_ms, to_ms) = (now_ms - WEAKNESS_DAYS * DAY_MS, now_ms);

    let starts = queries::study_segment_starts(conn, from_ms, to_ms)?;
    let mut first_aways = Vec::new();
    for (segment_id, started_at_ms) in &starts {
        let events = store::list_focus_events(conn, *segment_id)?;
        if let Some(after) = calc::event_metrics(&events, *started_at_ms).first_away_after_ms {
            first_aways.push(after);
        }
    }
    let count = i64::try_from(first_aways.len()).unwrap_or(0);
    let avg_first_away_ms = (count > 0).then(|| first_aways.iter().sum::<i64>() / count);

    let records = store::list_downtime(conn, i64::MAX)?;
    let summary = downtime::summarize(&records, from_ms, to_ms);
    let off: Vec<Interval> = store::list_downtime_intervals(conn, from_ms, to_ms)?
        .into_iter()
        .map(|d| Interval {
            start: d.start.max(from_ms),
            end: d.end.min(to_ms),
        })
        .collect();
    let planned = queries::planned_windows(conn, from_ms, to_ms)?;

    let (emergency_pass, skip_today, early_release, aborted_runs) =
        queries::release_counts(conn, from_ms, to_ms)?;
    Ok(Weakness {
        from_ms,
        to_ms,
        deviation_apps: queries::deviation_apps(conn, from_ms, to_ms, LIST_LIMIT)?,
        avg_first_away_ms,
        sets_with_away: count,
        sets_total: i64::try_from(starts.len()).unwrap_or(0),
        heatmap: heatmap(tz, &queries::deviation_times(conn, from_ms, to_ms)?),
        postponed_plans: queries::postponed_plans(conn, from_ms, to_ms, POSTPONED_MIN, LIST_LIMIT)?,
        app_off: AppOffStat {
            sleep_ms: summary.sleep_ms,
            forced_quit_ms: summary.forced_quit_ms,
            forced_quit_count: summary.forced_quit_count,
            planned_overlap_ms: overlap_ms(off, planned),
        },
        releases: ReleaseUsage {
            emergency_pass,
            skip_today,
            early_release,
            aborted_runs,
        },
        phone: phone_stat(conn, from_ms, to_ms)?,
    })
}

/// The phone's gaps (clipped to the window, each stretch of time counted once) and pick-up counts.
fn phone_stat(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<PhoneStat> {
    let gaps = store::phone::gaps_between(conn, from_ms, to_ms)?;
    let spans: Vec<Interval> = gaps
        .iter()
        .map(|g| Interval {
            start: g.start_ms.max(from_ms),
            end: g.end_ms.min(to_ms),
        })
        .collect();
    let (pickup_sets_reported, pickup_sets_with_pickups, pickups_total) =
        store::phone::pickup_totals(conn, from_ms, to_ms)?;
    Ok(PhoneStat {
        unreachable_ms: merge(spans).iter().map(|i| (i.end - i.start).max(0)).sum(),
        gaps: i64::try_from(gaps.len()).unwrap_or(0),
        service_disabled_gaps: i64::try_from(
            gaps.iter().filter(|g| g.kind == "service_disabled").count(),
        )
        .unwrap_or(0),
        pickup_sets_reported,
        pickup_sets_with_pickups,
        pickups_total,
    })
}
