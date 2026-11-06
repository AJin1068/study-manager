//! Calendar file import and export on top of the plan model. The `.ics` text itself is handled by
//! `plan_ics` (pure); this module decides what becomes a plan, and writes it the way every other
//! plan change is written: one transaction, lock check first, `plan_history` rows.
//!
//! Import is preview-then-apply. Both run the same classification, so the preview lists exactly
//! what apply will create. Each applied import is one history batch (`ics-<ms>-<first id>`) that
//! `undo_ics_import` reverts as a whole.
//!
//! Privacy: files come from a local picker and go back to a local save; nothing here touches the
//! network.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, NaiveDate, TimeZone};
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::plan::{
    self, fmt_date, first_date, is_base_date, plan_duration_min, plan_json, ExceptionKind, ModeLookup, Plan,
    PlanChange, PlanException, Repeat, MAX_SETS, OVERLAP_HORIZON_DAYS,
};
use crate::plan_ics::{self, ExportEvent, IcsEvent, IcsRepeat, SkipReason};
use crate::settings::{self, Outcome, SettingKey, SettingValue};
use crate::store;
use crate::study::{self, ModeConfig, StudyLevel};

type CandidateWithDates = (Candidate, Vec<(i64, i64)>, Vec<NaiveDate>);

/// History reason text of every imported plan; also how undo finds its batch's rows.
pub const IMPORT_REASON: &str = "ICS 가져오기";
const UID_DOMAIN: &str = "study-manager.local";

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkipCount {
    pub reason: SkipReason,
    pub count: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewItem {
    pub name: String,
    pub subject: String,
    pub start_date: String,
    pub start_time: String,
    pub sets: i64,
    pub repeat: Option<Repeat>,
    /// Occurrences the file removed with EXDATE.
    pub excluded_dates: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IcsPreview {
    pub items: Vec<PreviewItem>,
    pub skipped: Vec<SkipCount>,
    pub skipped_total: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IcsApplyResult {
    pub change: PlanChange,
    pub batch_id: Option<String>,
    pub imported: i64,
    pub skipped: Vec<SkipCount>,
    pub skipped_total: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IcsExport {
    pub content: String,
    pub event_count: i64,
}

/// Import and export stay off until the user turns the setting on (default off).
pub fn calendar_io_enabled(conn: &Connection) -> AppResult<bool> {
    match settings::read_value(conn, &SettingKey::CalendarIoEnabled.spec())? {
        SettingValue::Bool(on) => Ok(on),
        other => Err(AppError::Internal(format!("calendarIoEnabled is not a boolean: {other:?}"))),
    }
}

fn require_enabled(conn: &Connection) -> AppResult<()> {
    if calendar_io_enabled(conn)? {
        Ok(())
    } else {
        Err(AppError::Invalid("calendar import/export is turned off in settings".into()))
    }
}

fn skip_counts(skipped: &[SkipReason]) -> Vec<SkipCount> {
    let mut counts: BTreeMap<SkipReason, i64> = BTreeMap::new();
    for reason in skipped {
        *counts.entry(*reason).or_default() += 1;
    }
    counts.into_iter().map(|(reason, count)| SkipCount { reason, count }).collect()
}

// ---- export ----

fn uid(plan_id: i64, moved_from: Option<&str>) -> String {
    match moved_from {
        None => format!("sm-plan-{plan_id}@{UID_DOMAIN}"),
        Some(source) => format!("sm-plan-{plan_id}-moved-{source}@{UID_DOMAIN}"),
    }
}

/// Every live plan as VEVENTs. A repeating plan is one event with EXDATE for its removed or moved
/// occurrences; a moved occurrence is its own single event, so reading the file back gives the
/// same occurrences. Needs no lock: nothing is changed.
pub fn export_ics(conn: &Connection, at_ms: i64) -> AppResult<IcsExport> {
    require_enabled(conn)?;
    let plans = store::list_live_plans(conn)?;
    let exceptions = store::list_plan_exceptions(conn)?;
    let modes = ModeLookup::load(conn)?;
    let duration = |date: &str, sets: i64| -> AppResult<i64> { plan_duration_min(modes.config(modes.level_for(date))?, sets) };

    let mut events = Vec::new();
    for plan in &plans {
        let own: Vec<&PlanException> = exceptions.iter().filter(|e| e.plan_id == plan.id).collect();
        let mut exdates = Vec::new();
        let mut moved = Vec::new();
        for ex in &own {
            let day = plan::parse_date(&ex.date)?;
            if !is_base_date(plan, day)? {
                continue;
            }
            exdates.push(day);
            if let (ExceptionKind::Moved, Some(new_date), Some(new_time)) = (ex.kind, &ex.new_date, &ex.new_time) {
                moved.push(ExportEvent {
                    uid: uid(plan.id, Some(&ex.date)),
                    name: plan.name.clone(),
                    subject: plan.subject.clone(),
                    date: plan::parse_date(new_date)?,
                    time: plan::parse_time(new_time)?,
                    duration_min: duration(new_date, ex.sets_override.unwrap_or(plan.sets))?,
                    sets: ex.sets_override.unwrap_or(plan.sets),
                    repeat: None,
                    exdates: Vec::new(),
                });
            }
        }
        // The first day that really is an occurrence: a series continuation row may start on a
        // weekday its own mask does not include.
        let mut start = first_date(plan)?;
        let limit = start + Duration::days(7);
        while start < limit && !is_base_date(plan, start)? {
            start += Duration::days(1);
        }
        let cancelled_single = plan.repeat_weekdays.is_none() && !exdates.is_empty();
        if is_base_date(plan, start)? && !cancelled_single {
            events.push(ExportEvent {
                uid: uid(plan.id, None),
                name: plan.name.clone(),
                subject: plan.subject.clone(),
                date: start,
                time: plan::parse_time(&plan.start_time)?,
                duration_min: duration(&fmt_date(start), plan.sets)?,
                sets: plan.sets,
                repeat: match plan.repeat_weekdays {
                    Some(weekdays) => Some(IcsRepeat {
                        weekdays,
                        until: plan.repeat_until.as_deref().map(plan::parse_date).transpose()?,
                    }),
                    None => None,
                },
                exdates: if plan.repeat_weekdays.is_some() { exdates } else { Vec::new() },
            });
        }
        events.extend(moved);
    }
    let now = DateTime::from_timestamp_millis(at_ms)
        .ok_or_else(|| AppError::Internal("export time out of range".into()))?
        .naive_utc();
    let event_count = i64::try_from(events.len()).unwrap_or(i64::MAX);
    tracing::info!(event = "plans_ics_exported", events = event_count);
    Ok(IcsExport { content: plan_ics::generate_ics(&events, now), event_count })
}

// ---- import classification ----

struct Candidate {
    plan: Plan,
    /// Cancelled exceptions with `plan_id` 0 until the plan row exists.
    exceptions: Vec<PlanException>,
}

struct Classified {
    accepted: Vec<Candidate>,
    skipped: Vec<SkipReason>,
}

/// The set count whose derived length is closest to the file's event length (one set without one).
fn sets_for(cfg: &ModeConfig, minutes: Option<i64>) -> AppResult<i64> {
    let Some(minutes) = minutes else { return Ok(1) };
    let mut best = (1, i64::MAX);
    for sets in 1..=MAX_SETS {
        let diff = (plan_duration_min(cfg, sets)? - minutes).abs();
        if diff < best.1 {
            best = (sets, diff);
        }
    }
    Ok(best.0)
}

fn build_candidate(event: &IcsEvent, sets: i64, at_ms: i64) -> Candidate {
    let start_date = fmt_date(event.date);
    let plan = Plan {
        id: 0,
        series_id: 0,
        name: event.name.clone(),
        subject: event.subject.clone().unwrap_or_else(|| event.name.clone()),
        start_date: start_date.clone(),
        start_time: event.time.format("%H:%M").to_string(),
        sets,
        repeat_weekdays: event.repeat.as_ref().map(|r| r.weekdays),
        repeat_until: event.repeat.as_ref().and_then(|r| r.until).map(fmt_date),
        effective_from_date: start_date,
        created_at_ms: at_ms,
        deleted_at_ms: None,
    };
    let exceptions = event
        .exdates
        .iter()
        .filter(|day| is_base_date(&plan, **day).unwrap_or(false))
        .map(|day| PlanException {
            plan_id: 0,
            date: fmt_date(*day),
            kind: ExceptionKind::Cancelled,
            new_date: None,
            new_time: None,
            sets_override: None,
        })
        .collect();
    Candidate { plan, exceptions }
}

/// Local days of the candidate's occurrences that matter for the overlap rule: from today (past
/// occurrences cannot collide with anything that will start) up to the same horizon `create_plan`
/// checks.
fn overlap_days(plan: &Plan, today: NaiveDate) -> AppResult<Vec<NaiveDate>> {
    let start = plan::parse_date(&plan.start_date)?;
    let from = start.max(today);
    let Some(mask) = plan.repeat_weekdays else { return Ok(vec![start]) };
    let mut to = from + Duration::days(OVERLAP_HORIZON_DAYS);
    if let Some(until) = &plan.repeat_until {
        to = to.min(plan::parse_date(until)?);
    }
    let mut days = Vec::new();
    let mut day = from;
    while day <= to {
        if mask & (1 << chrono::Datelike::weekday(&day).num_days_from_monday()) != 0 {
            days.push(day);
        }
        day += Duration::days(1);
    }
    Ok(days)
}

/// Blocks of existing occurrences keyed by start; overlapping existing occurrences are merged, so
/// the blocks are disjoint and one predecessor lookup answers "does anything overlap [s, e)".
struct Busy(BTreeMap<i64, i64>);

impl Busy {
    fn from_intervals(mut intervals: Vec<(i64, i64)>) -> Busy {
        intervals.sort_unstable();
        let mut map: BTreeMap<i64, i64> = BTreeMap::new();
        let mut current: Option<(i64, i64)> = None;
        for (start, end) in intervals {
            current = match current {
                Some((s, e)) if start < e => Some((s, e.max(end))),
                Some((s, e)) => {
                    map.insert(s, e);
                    Some((start, end))
                }
                None => Some((start, end)),
            };
        }
        if let Some((s, e)) = current {
            map.insert(s, e);
        }
        Busy(map)
    }

    fn overlaps(&self, start: i64, end: i64) -> bool {
        self.0.range(..end).next_back().is_some_and(|(_, block_end)| *block_end > start)
    }
}

fn classify<Tz: TimeZone>(conn: &Connection, tz: &Tz, content: &str, at_ms: i64) -> AppResult<Classified> {
    let parsed = plan_ics::parse_ics(content, tz)?;
    let mut skipped = parsed.skipped;
    let modes = ModeLookup::load(conn)?;
    let cfg = modes.config(StudyLevel::DEFAULT)?;
    let today = DateTime::from_timestamp_millis(at_ms)
        .ok_or_else(|| AppError::Internal("import time out of range".into()))?
        .with_timezone(tz)
        .date_naive();

    // Candidates with their occurrence intervals, in file order.
    let mut candidates: Vec<CandidateWithDates> = Vec::new();
    for event in &parsed.events {
        let last = match &event.repeat {
            None => Some(event.date),
            Some(repeat) => repeat.until,
        };
        if last.is_some_and(|l| l < today) {
            skipped.push(SkipReason::Past);
            continue;
        }
        if event.repeat.is_none() && event.exdates.contains(&event.date) {
            skipped.push(SkipReason::Invalid);
            continue;
        }
        let candidate = build_candidate(event, event.sets.map_or_else(|| sets_for(cfg, event.duration_min), Ok)?, at_ms);
        let days = overlap_days(&candidate.plan, today)?;
        let mut intervals = Vec::new();
        for day in &days {
            let found = plan::occurrences_on(tz, std::slice::from_ref(&candidate.plan), &candidate.exceptions, &modes, *day)?;
            intervals.extend(found.iter().map(|o| (o.planned_start_ms, o.planned_end_ms)));
        }
        candidates.push((candidate, intervals, days));
    }

    // Existing occurrences around every day a candidate touches, computed once per distinct day.
    let mut needed: BTreeSet<NaiveDate> = BTreeSet::new();
    for (_, _, days) in &candidates {
        for day in days {
            needed.insert(*day - Duration::days(1));
            needed.insert(*day);
        }
    }
    let plans = store::list_live_plans(conn)?;
    let exceptions = store::list_plan_exceptions(conn)?;
    let mut existing = Vec::new();
    for day in needed {
        existing.extend(
            plan::occurrences_on(tz, &plans, &exceptions, &modes, day)?
                .into_iter()
                .map(|o| (o.planned_start_ms, o.planned_end_ms)),
        );
    }
    let mut busy = Busy::from_intervals(existing);

    // First come, first kept: a candidate that collides with the calendar or an earlier
    // candidate is dropped whole.
    let mut accepted = Vec::new();
    for (candidate, mut intervals, _) in candidates {
        intervals.sort_unstable();
        let self_overlap = intervals.windows(2).any(|w| w[1].0 < w[0].1);
        if self_overlap || intervals.iter().any(|(s, e)| busy.overlaps(*s, *e)) {
            skipped.push(SkipReason::Overlap);
            continue;
        }
        for (s, e) in intervals {
            busy.0.insert(s, e);
        }
        accepted.push(candidate);
    }
    Ok(Classified { accepted, skipped })
}

fn preview_item(candidate: &Candidate) -> PreviewItem {
    let plan = &candidate.plan;
    PreviewItem {
        name: plan.name.clone(),
        subject: plan.subject.clone(),
        start_date: plan.start_date.clone(),
        start_time: plan.start_time.clone(),
        sets: plan.sets,
        repeat: plan.repeat_weekdays.map(|weekdays| Repeat { weekdays, until: plan.repeat_until.clone() }),
        excluded_dates: i64::try_from(candidate.exceptions.len()).unwrap_or(i64::MAX),
    }
}

/// What apply would do with this file right now. Changes nothing and ignores the study lock.
pub fn preview_ics_import<Tz: TimeZone>(conn: &Connection, tz: &Tz, content: &str, at_ms: i64) -> AppResult<IcsPreview> {
    require_enabled(conn)?;
    let classified = classify(conn, tz, content, at_ms)?;
    Ok(IcsPreview {
        items: classified.accepted.iter().map(preview_item).collect(),
        skipped_total: i64::try_from(classified.skipped.len()).unwrap_or(i64::MAX),
        skipped: skip_counts(&classified.skipped),
    })
}

#[allow(clippy::too_many_arguments)] // mirrors the NewPlanHistory columns one to one
fn history_row<'a>(
    plan_id: Option<i64>,
    at_ms: i64,
    action: plan::Action,
    reason: plan::ReasonCode,
    reason_text: Option<&'a str>,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
    outcome: Outcome,
    batch_id: Option<&'a str>,
) -> plan::NewPlanHistory<'a> {
    plan::NewPlanHistory {
        plan_id,
        at_ms,
        action,
        reason,
        reason_text,
        before_json: before.map(|v| v.to_string()),
        after_json: after.map(|v| v.to_string()),
        outcome,
        batch_id,
    }
}

/// Creates the previewed plans as one batch. While a planned run holds the lock nothing is
/// created; the attempt is recorded like any other rejected plan change.
pub fn apply_ics_import<Tz: TimeZone>(
    conn: &mut Connection,
    tz: &Tz,
    content: &str,
    at_ms: i64,
) -> AppResult<IcsApplyResult> {
    require_enabled(conn)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if study::lock_held(&tx)? {
        let row = history_row(
            None,
            at_ms,
            plan::Action::Create,
            plan::ReasonCode::Custom,
            Some(IMPORT_REASON),
            None,
            Some(json!({ "import": "ics" })),
            Outcome::RejectedLocked,
            None,
        );
        store::append_plan_history(&tx, &row)?;
        tx.commit()?;
        return Ok(IcsApplyResult { change: PlanChange::rejected(None), batch_id: None, imported: 0, skipped: Vec::new(), skipped_total: 0 });
    }

    let classified = classify(&tx, tz, content, at_ms)?;
    let skipped_total = i64::try_from(classified.skipped.len()).unwrap_or(i64::MAX);
    let skipped = skip_counts(&classified.skipped);
    let mut batch_id: Option<String> = None;
    let mut first_id = None;
    for candidate in &classified.accepted {
        let id = store::insert_plan(&tx, &candidate.plan)?;
        for exception in &candidate.exceptions {
            store::put_plan_exception(&tx, &PlanException { plan_id: id, ..exception.clone() })?;
        }
        let batch = batch_id.get_or_insert_with(|| format!("ics-{at_ms}-{id}")).clone();
        first_id.get_or_insert(id);
        let row = history_row(
            Some(id),
            at_ms,
            plan::Action::Create,
            plan::ReasonCode::Custom,
            Some(IMPORT_REASON),
            None,
            Some(plan_json(&candidate.plan)),
            Outcome::Applied,
            Some(&batch),
        );
        store::append_plan_history(&tx, &row)?;
    }
    tx.commit()?;
    let imported = i64::try_from(classified.accepted.len()).unwrap_or(i64::MAX);
    tracing::info!(event = "plans_ics_imported", imported, skipped = skipped_total);
    let change = match first_id {
        Some(id) => PlanChange::applied(id),
        None => PlanChange { outcome: crate::settings::UpdateOutcome::Applied { key: "plan".into() }, plan_id: None },
    };
    Ok(IcsApplyResult { change, batch_id, imported, skipped, skipped_total })
}

/// Reverts one import batch: soft-deletes its plans and appends `undo` rows. Refused (and the
/// attempt recorded) while the lock is held; refused once any of its plans was edited, or has
/// already started a run, so history and past runs stay truthful.
pub fn undo_ics_import(conn: &mut Connection, batch_id: &str, at_ms: i64) -> AppResult<PlanChange> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let rows = store::list_plan_history_batch(&tx, batch_id)?;
    let created: Vec<i64> = rows
        .iter()
        .filter(|r| {
            r.action == plan::Action::Create.as_str() && r.outcome == "applied" && r.reason_text.as_deref() == Some(IMPORT_REASON)
        })
        .filter_map(|r| r.plan_id)
        .collect();
    let Some(&first) = created.first() else {
        return Err(AppError::Invalid("no calendar import has that batch id".into()));
    };
    if rows.iter().any(|r| r.action == plan::Action::Undo.as_str() && r.outcome == "applied") {
        return Err(AppError::Invalid("that calendar import was already undone".into()));
    }
    let undo_row = |plan_id, before, after, outcome| {
        history_row(plan_id, at_ms, plan::Action::Undo, plan::ReasonCode::Undo, None, before, after, outcome, Some(batch_id))
    };
    if study::lock_held(&tx)? {
        store::append_plan_history(&tx, &undo_row(Some(first), None, None, Outcome::RejectedLocked))?;
        tx.commit()?;
        return Ok(PlanChange::rejected(Some(first)));
    }

    let stale = || AppError::Invalid("an imported plan was changed since; undo no longer applies".into());
    let started: BTreeSet<i64> = store::list_runs_since(&tx, "0000-00-00")?.into_iter().map(|r| r.plan_id).collect();
    let mut plans = Vec::new();
    for id in &created {
        let plan = store::get_plan(&tx, *id)?.filter(|p| p.deleted_at_ms.is_none()).ok_or_else(stale)?;
        if started.contains(id) {
            return Err(AppError::Invalid("an imported plan has already started; undo no longer applies".into()));
        }
        // Only the batch's own create row may exist for it.
        let history = store::list_plan_history(&tx, Some(*id), i64::MAX)?;
        if history.iter().any(|h| h.plan_id == Some(*id) && h.batch_id.as_deref() != Some(batch_id)) {
            return Err(stale());
        }
        plans.push(plan);
    }
    for plan in &plans {
        store::soft_delete_plan(&tx, plan.id, at_ms)?;
        store::append_plan_history(&tx, &undo_row(Some(plan.id), Some(plan_json(plan)), Some(json!({ "deleted": true })), Outcome::Applied))?;
    }
    tx.commit()?;
    tracing::info!(event = "plans_ics_import_undone", plans = plans.len());
    Ok(PlanChange::applied(first))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::plan::{create_plan, list_occurrences, plan_history, CreatePlanInput, Occurrence};
    use chrono::Utc;

    const NOW: i64 = 1_767_225_600_000; // 2026-01-01T00:00:00Z

    fn enable(conn: &mut Connection) {
        let os = crate::settings::tests::FakeAutostart::new(false);
        settings::update_setting(conn, &os, SettingKey::CalendarIoEnabled, SettingValue::Bool(true), 1).expect("enable");
    }

    fn db() -> Connection {
        let mut conn = open_in_memory().expect("db");
        enable(&mut conn);
        conn
    }

    fn occurrences(conn: &Connection) -> Vec<(String, String, String, String, i64)> {
        let all: Vec<Occurrence> = list_occurrences(conn, &Utc, "2026-01-01", "2026-12-31").expect("list");
        all.into_iter().map(|o| (o.date, o.start_time, o.name, o.subject, o.sets)).collect()
    }

    fn input(name: &str, subject: &str, date: &str, time: &str, sets: i64, repeat: Option<Repeat>) -> CreatePlanInput {
        CreatePlanInput { name: name.into(), subject: subject.into(), start_date: date.into(), start_time: time.into(), sets, repeat }
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).expect("count")
    }

    fn ics(events: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n")
    }

    fn event(uid: &str, body: &str) -> String {
        format!("BEGIN:VEVENT\r\nUID:{uid}\r\n{body}END:VEVENT\r\n")
    }

    #[test]
    fn off_by_default_and_both_directions_refuse() {
        let mut conn = open_in_memory().expect("db");
        assert!(!calendar_io_enabled(&conn).expect("read"));
        assert!(export_ics(&conn, NOW).is_err());
        assert!(preview_ics_import(&conn, &Utc, &ics(""), NOW).is_err());
        assert!(apply_ics_import(&mut conn, &Utc, &ics(""), NOW).is_err());
        assert_eq!(count(&conn, "plan_history"), 0);
    }

    #[test]
    fn export_then_import_gives_the_same_occurrences() {
        let mut source = db();
        let weekly = Some(Repeat { weekdays: 0b101, until: Some("2026-03-16".into()) });
        let math = create_plan(&mut source, &Utc, input("수학, 심화", "수학", "2026-03-02", "20:00", 2, weekly), 1)
            .expect("create")
            .plan_id
            .expect("id");
        create_plan(&mut source, &Utc, input("국어", "국어", "2026-03-03", "09:05", 1, None), 2).expect("create");
        let mut moved = plan::UpdatePlanInput {
            plan_id: math,
            scope: plan::Scope::Occurrence,
            date: "2026-03-04".into(),
            new_date: Some("2026-03-05".into()),
            new_time: Some("19:00".into()),
            new_sets: None,
            repeat: None,
            reason_code: plan::ReasonCode::Postponed,
            reason_text: None,
        };
        plan::update_plan(&mut source, &Utc, moved.clone(), 3).expect("move");
        moved.date = "2026-03-09".into();
        moved.new_date = None;
        moved.new_time = None;
        moved.new_sets = Some(3);
        plan::update_plan(&mut source, &Utc, moved, 4).expect("resize");
        let delete = plan::DeletePlanInput {
            plan_id: math,
            scope: plan::Scope::Occurrence,
            date: "2026-03-11".into(),
            reason_code: plan::ReasonCode::Sick,
            reason_text: None,
        };
        plan::delete_plan(&mut source, delete, 5).expect("delete one");
        let before = occurrences(&source);
        assert_eq!(before.len(), 5, "{before:?}");

        let exported = export_ics(&source, NOW).expect("export");
        let mut target = db();
        let preview = preview_ics_import(&target, &Utc, &exported.content, NOW).expect("preview");
        assert_eq!((preview.skipped_total, preview.items.len() as i64), (0, exported.event_count));
        let applied = apply_ics_import(&mut target, &Utc, &exported.content, NOW).expect("apply");
        assert!(applied.change.is_applied());
        assert_eq!((applied.imported, applied.skipped_total), (exported.event_count, 0));
        assert_eq!(occurrences(&target), before);
    }

    #[test]
    fn unsupported_events_are_counted_by_reason_and_the_rest_imports() {
        let mut conn = db();
        let text = ics(&[
            event("a", "DTSTART:20260310T200000\r\nDTEND:20260310T230000\r\nSUMMARY:좋은 일정\r\n"),
            event("b", "DTSTART;VALUE=DATE:20260311\r\nSUMMARY:종일\r\n"),
            event("c", "DTSTART;TZID=Asia/Seoul:20260312T200000\r\nSUMMARY:tz\r\n"),
            event("d", "DTSTART:20260313T200000\r\nSUMMARY:daily\r\nRRULE:FREQ=DAILY\r\n"),
            event("e", "DTSTART:20250313T200000\r\nSUMMARY:지난 일정\r\n"),
        ].concat());
        let applied = apply_ics_import(&mut conn, &Utc, &text, NOW).expect("apply");
        assert_eq!((applied.imported, applied.skipped_total), (1, 4));
        let reasons: Vec<(SkipReason, i64)> = applied.skipped.iter().map(|s| (s.reason, s.count)).collect();
        assert_eq!(
            reasons,
            [(SkipReason::AllDay, 1), (SkipReason::Timezone, 1), (SkipReason::UnsupportedRule, 1), (SkipReason::Past, 1)]
        );
        // 180 minutes of events with no X-SM-SETS: 3 medium sets.
        assert_eq!(occurrences(&conn), [("2026-03-10".into(), "20:00".into(), "좋은 일정".into(), "좋은 일정".into(), 3)]);
    }

    #[test]
    fn overlaps_with_the_calendar_and_between_events_are_skipped() {
        let mut conn = db();
        create_plan(&mut conn, &Utc, input("기존", "기존", "2026-03-10", "20:00", 3, None), 1).expect("create");
        let text = ics(&[
            event("a", "DTSTART:20260310T210000\r\nSUMMARY:겹침\r\n"),
            event("b", "DTSTART:20260311T080000\r\nSUMMARY:앞\r\n"),
            event("c", "DTSTART:20260311T081000\r\nSUMMARY:뒤 겹침\r\n"),
            event("d", "DTSTART:20260310T230000\r\nSUMMARY:맞닿음\r\n"),
        ].concat());
        let applied = apply_ics_import(&mut conn, &Utc, &text, NOW).expect("apply");
        let reasons: Vec<(SkipReason, i64)> = applied.skipped.iter().map(|s| (s.reason, s.count)).collect();
        assert_eq!((applied.imported, reasons), (2, vec![(SkipReason::Overlap, 2)]));
        // Importing the same file again changes nothing.
        let again = apply_ics_import(&mut conn, &Utc, &text, NOW).expect("again");
        assert_eq!((again.imported, again.skipped_total), (0, 4));
        assert_eq!(again.batch_id, None);
    }

    #[test]
    fn lock_rejects_import_and_records_the_attempt() {
        let mut conn = db();
        crate::testutil::set_lock_for_test(&conn, true);
        let text = ics(&event("a", "DTSTART:20260310T200000\r\nSUMMARY:x\r\n"));
        let result = apply_ics_import(&mut conn, &Utc, &text, NOW).expect("apply");
        assert!(!result.change.is_applied());
        assert_eq!((result.imported, count(&conn, "plans")), (0, 0));
        let history = plan_history(&conn, None, 10).expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!((history[0].outcome.as_str(), history[0].reason_text.as_deref()), ("rejected_locked", Some(IMPORT_REASON)));
    }

    #[test]
    fn import_is_one_custom_create_batch_and_undo_reverts_it_once() {
        let mut conn = db();
        let text = ics(&[
            event("a", "DTSTART:20260310T200000\r\nSUMMARY:하나\r\n"),
            event("b", "DTSTART:20260311T200000\r\nSUMMARY:둘\r\n"),
        ].concat());
        let applied = apply_ics_import(&mut conn, &Utc, &text, NOW).expect("apply");
        let batch = applied.batch_id.clone().expect("batch");
        let history = plan_history(&conn, None, 10).expect("history");
        assert_eq!(history.len(), 2);
        for row in &history {
            assert_eq!(
                (row.action.as_str(), row.reason_code.as_str(), row.reason_text.as_deref(), row.batch_id.as_deref()),
                ("create", "custom", Some(IMPORT_REASON), Some(batch.as_str()))
            );
        }
        assert_eq!(occurrences(&conn).len(), 2);

        crate::testutil::set_lock_for_test(&conn, true);
        assert!(!undo_ics_import(&mut conn, &batch, NOW + 1).expect("locked").is_applied());
        assert_eq!(occurrences(&conn).len(), 2, "lock keeps the plans");
        crate::testutil::set_lock_for_test(&conn, false);

        assert!(undo_ics_import(&mut conn, &batch, NOW + 2).expect("undo").is_applied());
        assert!(occurrences(&conn).is_empty());
        assert!(undo_ics_import(&mut conn, &batch, NOW + 3).is_err(), "second undo is refused");
        assert!(undo_ics_import(&mut conn, "ics-0-0", NOW + 3).is_err());
    }

    #[test]
    fn undo_is_refused_after_an_imported_plan_was_edited() {
        let mut conn = db();
        let text = ics(&event("a", "DTSTART:20260310T200000\r\nSUMMARY:하나\r\n"));
        let applied = apply_ics_import(&mut conn, &Utc, &text, NOW).expect("apply");
        let id = applied.change.plan_id.expect("id");
        let edit = plan::UpdatePlanInput {
            plan_id: id,
            scope: plan::Scope::Occurrence,
            date: "2026-03-10".into(),
            new_date: None,
            new_time: Some("21:00".into()),
            new_sets: None,
            repeat: None,
            reason_code: plan::ReasonCode::Sick,
            reason_text: None,
        };
        plan::update_plan(&mut conn, &Utc, edit, 5).expect("edit");
        assert!(undo_ics_import(&mut conn, &applied.batch_id.expect("batch"), NOW + 1).is_err());
        assert_eq!(occurrences(&conn).len(), 1);
    }

    #[test]
    fn thousand_events_import_in_reasonable_time() {
        let mut conn = db();
        let mut body = String::new();
        // Distinct weekly slots: 7 days x 24 hours leaves 168 slots, so use one-off events.
        for i in 0..1000_i64 {
            let day = NaiveDate::from_ymd_opt(2026, 3, 1).expect("date") + Duration::days(i / 10);
            let hour = 5 + (i % 10) * 2;
            body.push_str(&event(
                &format!("e{i}"),
                &format!("DTSTART:{}T{hour:02}0000\r\nDTEND:{}T{hour:02}2500\r\nSUMMARY:일정 {i}\r\n", day.format("%Y%m%d"), day.format("%Y%m%d")),
            ));
        }
        let text = ics(&body);
        let started = std::time::Instant::now();
        let preview = preview_ics_import(&conn, &Utc, &text, NOW).expect("preview");
        let applied = apply_ics_import(&mut conn, &Utc, &text, NOW).expect("apply");
        assert_eq!((preview.items.len(), applied.imported, applied.skipped_total), (1000, 1000, 0));
        assert_eq!(count(&conn, "plan_history"), 1000);
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
    }

    #[test]
    fn serialized_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/ics-contract.json");
        let expected: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let preview = IcsPreview {
            items: vec![PreviewItem {
                name: "수학".into(),
                subject: "수학".into(),
                start_date: "2026-03-02".into(),
                start_time: "20:00".into(),
                sets: 3,
                repeat: Some(Repeat { weekdays: 5, until: Some("2026-12-31".into()) }),
                excluded_dates: 1,
            }],
            skipped: vec![SkipCount { reason: SkipReason::AllDay, count: 2 }],
            skipped_total: 2,
        };
        let applied = IcsApplyResult {
            change: PlanChange::applied(7),
            batch_id: Some("ics-1767225600000-7".into()),
            imported: 3,
            skipped: vec![SkipCount { reason: SkipReason::Overlap, count: 1 }],
            skipped_total: 1,
        };
        let export = IcsExport { content: "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".into(), event_count: 0 };
        let actual = json!({ "preview": preview, "applyResult": applied, "export": export });
        assert_eq!(actual, expected);
    }

    #[test]
    fn sets_follow_the_event_length() {
        let conn = open_in_memory().expect("db");
        let modes = ModeLookup::load(&conn).expect("modes");
        let cfg = modes.config(StudyLevel::DEFAULT).expect("cfg");
        assert_eq!(sets_for(cfg, None).expect("sets"), 1);
        assert_eq!(sets_for(cfg, Some(180)).expect("sets"), 3);
        assert_eq!(sets_for(cfg, Some(1)).expect("sets"), 1);
        assert_eq!(sets_for(cfg, Some(100_000)).expect("sets"), MAX_SETS);
    }
}
