//! Calendar plans: the model, recurrence, occurrence derivation, overlap rule, and the
//! lock-aware change operations that write the append-only `plan_history`.
//!
//! A plan is a start time plus a set count. The end is never stored: it is derived from the
//! mode length (`plan_duration_min`), so a mode edit moves every derived end consistently.
//! Pure functions take a `TimeZone` so tests can pin DST behaviour; commands pass `Local`.

use std::collections::HashMap;

use chrono::{Datelike, Duration, LocalResult, NaiveDate, NaiveTime, TimeZone};
use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::settings::{Outcome, UpdateOutcome};
use crate::store;
use crate::study::{ModeConfig, StudyLevel};

pub const MAX_SETS: i64 = 24;
/// Overlap is checked this far ahead for repeating plans; later dates are checked when reached.
pub(crate) const OVERLAP_HORIZON_DAYS: i64 = 366;
const MAX_LIST_DAYS: i64 = 400;
pub(crate) const MAX_TEXT_CHARS: usize = 100;
const DATE_FMT: &str = "%Y-%m-%d";
const TIME_FMT: &str = "%H:%M";
const PLAN_KEY: &str = "plan";

// ---- enums mirrored by the plan_history CHECK constraints (drift-tested) ----

/// Variants beyond create/move/retime/resize/delete/repeat are written by later stories
/// (mode shift, undo, chatbot); they exist now because the DB CHECK reserves them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Action {
    Create,
    Move,
    Retime,
    ResizeSets,
    Delete,
    RepeatChange,
    ModeShift,
    Undo,
    ChatbotCreate,
    LlmAdjust,
}

impl Action {
    #[cfg(test)]
    pub const ALL: [Action; 10] = [
        Action::Create,
        Action::Move,
        Action::Retime,
        Action::ResizeSets,
        Action::Delete,
        Action::RepeatChange,
        Action::ModeShift,
        Action::Undo,
        Action::ChatbotCreate,
        Action::LlmAdjust,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Action::Create => "create",
            Action::Move => "move",
            Action::Retime => "retime",
            Action::ResizeSets => "resize_sets",
            Action::Delete => "delete",
            Action::RepeatChange => "repeat_change",
            Action::ModeShift => "mode_shift",
            Action::Undo => "undo",
            Action::ChatbotCreate => "chatbot_create",
            Action::LlmAdjust => "llm_adjust",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    Created,
    Postponed,
    Sick,
    Custom,
    ModeShift,
    Undo,
    Chatbot,
    LlmAdjust,
}

impl ReasonCode {
    #[cfg(test)]
    pub const ALL: [ReasonCode; 8] = [
        ReasonCode::Created,
        ReasonCode::Postponed,
        ReasonCode::Sick,
        ReasonCode::Custom,
        ReasonCode::ModeShift,
        ReasonCode::Undo,
        ReasonCode::Chatbot,
        ReasonCode::LlmAdjust,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ReasonCode::Created => "created",
            ReasonCode::Postponed => "postponed",
            ReasonCode::Sick => "sick",
            ReasonCode::Custom => "custom",
            ReasonCode::ModeShift => "mode_shift",
            ReasonCode::Undo => "undo",
            ReasonCode::Chatbot => "chatbot",
            ReasonCode::LlmAdjust => "llm_adjust",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Occurrence,
    Series,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExceptionKind {
    Moved,
    Cancelled,
}

impl ExceptionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ExceptionKind::Moved => "moved",
            ExceptionKind::Cancelled => "cancelled",
        }
    }

    pub fn parse(raw: &str) -> Option<ExceptionKind> {
        match raw {
            "moved" => Some(ExceptionKind::Moved),
            "cancelled" => Some(ExceptionKind::Cancelled),
            _ => None,
        }
    }
}

// ---- rows and wire types ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub id: i64,
    pub series_id: i64,
    pub name: String,
    pub subject: String,
    pub start_date: String,
    pub start_time: String,
    pub sets: i64,
    /// Bit 0 = Monday .. bit 6 = Sunday. `None` = single occurrence.
    pub repeat_weekdays: Option<i64>,
    pub repeat_until: Option<String>,
    pub effective_from_date: String,
    pub created_at_ms: i64,
    pub deleted_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanException {
    pub plan_id: i64,
    /// The occurrence's original date; also the key.
    pub date: String,
    pub kind: ExceptionKind,
    pub new_date: Option<String>,
    pub new_time: Option<String>,
    pub sets_override: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repeat {
    pub weekdays: i64,
    pub until: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePlanInput {
    pub name: String,
    pub subject: String,
    pub start_date: String,
    pub start_time: String,
    pub sets: i64,
    pub repeat: Option<Repeat>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlanInput {
    pub plan_id: i64,
    pub scope: Scope,
    /// The occurrence's original date (the one shown before the edit).
    pub date: String,
    /// Occurrence scope only: moves are per occurrence.
    pub new_date: Option<String>,
    pub new_time: Option<String>,
    pub new_sets: Option<i64>,
    /// Series scope only.
    pub repeat: Option<Repeat>,
    pub reason_code: ReasonCode,
    pub reason_text: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletePlanInput {
    pub plan_id: i64,
    pub scope: Scope,
    pub date: String,
    pub reason_code: ReasonCode,
    pub reason_text: Option<String>,
}

/// Result of a change attempt: the shared `UpdateOutcome` plus the affected plan id.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanChange {
    pub outcome: UpdateOutcome,
    pub plan_id: Option<i64>,
}

impl PlanChange {
    pub(crate) fn applied(plan_id: i64) -> PlanChange {
        PlanChange { outcome: UpdateOutcome::Applied { key: PLAN_KEY.into() }, plan_id: Some(plan_id) }
    }

    pub(crate) fn rejected(plan_id: Option<i64>) -> PlanChange {
        PlanChange { outcome: UpdateOutcome::RejectedLocked { key: PLAN_KEY.into() }, plan_id }
    }

    pub fn is_applied(&self) -> bool {
        matches!(self.outcome, UpdateOutcome::Applied { .. })
    }
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Occurrence {
    pub plan_id: i64,
    pub series_id: i64,
    pub name: String,
    pub subject: String,
    /// Where the occurrence happens (the move target for a moved occurrence).
    pub date: String,
    /// The original date; the key used to edit this occurrence.
    pub source_date: String,
    pub start_time: String,
    pub sets: i64,
    pub moved: bool,
    pub level: StudyLevel,
    pub duration_min: i64,
    pub planned_start_ms: i64,
    pub planned_end_ms: i64,
    /// False for occurrences that started before the plan row existed: the timer must never
    /// mark those `missed` retroactively.
    pub eligible_for_missed: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanHistoryEntry {
    pub id: i64,
    pub plan_id: Option<i64>,
    pub at_ms: i64,
    pub action: String,
    pub reason_code: String,
    pub reason_text: Option<String>,
    pub before_json: Option<String>,
    pub after_json: Option<String>,
    pub outcome: String,
    pub batch_id: Option<String>,
}

/// A history row to append. Built by this module only.
pub struct NewPlanHistory<'a> {
    pub plan_id: Option<i64>,
    pub at_ms: i64,
    pub action: Action,
    pub reason: ReasonCode,
    pub reason_text: Option<&'a str>,
    pub before_json: Option<String>,
    pub after_json: Option<String>,
    pub outcome: Outcome,
    pub batch_id: Option<&'a str>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubjectCount {
    pub subject: String,
    pub count: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanStats {
    pub month: String,
    pub postponed_count: i64,
    pub by_subject: Vec<SubjectCount>,
}

// ---- pure: time and blocks ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Study,
    Break,
    LongBreak,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub minutes: i64,
}

/// Study/break sequence for `sets` sets. A break sits between sets only; the last set has no
/// trailing break (D-3). A mode with a long break uses it after every `sets_before_long_break`-th
/// set, except the last.
pub fn block_sequence(cfg: &ModeConfig, sets: i64) -> AppResult<Vec<Block>> {
    check_sets(sets)?;
    let mut blocks = Vec::new();
    for set in 1..=sets {
        blocks.push(Block { kind: BlockKind::Study, minutes: cfg.study_min });
        if set == sets {
            break;
        }
        let long = match (cfg.long_break_min, cfg.sets_before_long_break) {
            (Some(minutes), Some(every)) if every > 0 && set % every == 0 => Some(minutes),
            _ => None,
        };
        blocks.push(match long {
            Some(minutes) => Block { kind: BlockKind::LongBreak, minutes },
            None => Block { kind: BlockKind::Break, minutes: cfg.break_min },
        });
    }
    Ok(blocks)
}

pub fn plan_duration_min(cfg: &ModeConfig, sets: i64) -> AppResult<i64> {
    Ok(block_sequence(cfg, sets)?.iter().map(|b| b.minutes).sum())
}

fn check_sets(sets: i64) -> AppResult<()> {
    if (1..=MAX_SETS).contains(&sets) {
        Ok(())
    } else {
        Err(AppError::Invalid(format!("sets must be between 1 and {MAX_SETS}")))
    }
}

/// Local wall time to UTC epoch ms. A time skipped by a DST jump moves forward to the first
/// valid minute; an ambiguous time (clocks fall back) takes the earlier instant.
pub fn local_to_ms<Tz: TimeZone>(tz: &Tz, date: NaiveDate, time: NaiveTime) -> AppResult<i64> {
    let mut naive = date.and_time(time);
    for _ in 0..=24 * 60 {
        match tz.from_local_datetime(&naive) {
            LocalResult::Single(dt) => return Ok(dt.timestamp_millis()),
            LocalResult::Ambiguous(earlier, _) => return Ok(earlier.timestamp_millis()),
            LocalResult::None => naive += Duration::minutes(1),
        }
    }
    Err(AppError::Internal("local time could not be resolved within a day".into()))
}

/// Parses `YYYY-MM-DD` strictly: the round trip must reproduce the input, so the stored text is
/// always canonical and sorts as a date.
pub fn parse_date(raw: &str) -> AppResult<NaiveDate> {
    NaiveDate::parse_from_str(raw, DATE_FMT)
        .ok()
        .filter(|d| d.format(DATE_FMT).to_string() == raw)
        .ok_or_else(|| AppError::Invalid(format!("date must be YYYY-MM-DD: {raw}")))
}

pub fn parse_time(raw: &str) -> AppResult<NaiveTime> {
    NaiveTime::parse_from_str(raw, TIME_FMT)
        .ok()
        .filter(|t| t.format(TIME_FMT).to_string() == raw)
        .ok_or_else(|| AppError::Invalid(format!("time must be HH:MM: {raw}")))
}

pub(crate) fn fmt_date(date: NaiveDate) -> String {
    date.format(DATE_FMT).to_string()
}

fn weekday_bit(date: NaiveDate) -> i64 {
    1 << date.weekday().num_days_from_monday()
}

fn validate_repeat(start: NaiveDate, repeat: &Repeat) -> AppResult<()> {
    if !(1..=127).contains(&repeat.weekdays) {
        return Err(AppError::Invalid("repeat weekdays must select at least one day".into()));
    }
    if repeat.weekdays & weekday_bit(start) == 0 {
        return Err(AppError::Invalid("repeat weekdays must include the start date's weekday".into()));
    }
    if let Some(until) = &repeat.until {
        if parse_date(until)? < start {
            return Err(AppError::Invalid("repeat end is before the start date".into()));
        }
    }
    Ok(())
}

fn clean_text(raw: &str, what: &str) -> AppResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_TEXT_CHARS {
        return Err(AppError::Invalid(format!("{what} must be 1-{MAX_TEXT_CHARS} characters")));
    }
    Ok(trimmed.to_string())
}

// ---- pure: modes and occurrences ----

/// Mode configs plus the per-day choices, loaded once per operation.
pub struct ModeLookup {
    configs: Vec<ModeConfig>,
    daily: HashMap<String, StudyLevel>,
}

impl ModeLookup {
    pub fn load(conn: &Connection) -> AppResult<ModeLookup> {
        Ok(ModeLookup { configs: store::list_mode_configs(conn)?, daily: store::list_daily_modes(conn)? })
    }

    pub(crate) fn level_for(&self, date: &str) -> StudyLevel {
        self.daily.get(date).copied().unwrap_or(StudyLevel::DEFAULT)
    }

    pub fn config(&self, level: StudyLevel) -> AppResult<&ModeConfig> {
        self.configs
            .iter()
            .find(|c| c.level == level)
            .ok_or_else(|| AppError::Internal(format!("mode config for {} is missing", level.as_str())))
    }
}

pub(crate) fn first_date(plan: &Plan) -> AppResult<NaiveDate> {
    Ok(parse_date(&plan.start_date)?.max(parse_date(&plan.effective_from_date)?))
}

/// Whether the plan itself (ignoring exceptions) has an occurrence on `date`.
pub fn is_base_date(plan: &Plan, date: NaiveDate) -> AppResult<bool> {
    if plan.deleted_at_ms.is_some() || date < first_date(plan)? {
        return Ok(false);
    }
    match plan.repeat_weekdays {
        None => Ok(date == parse_date(&plan.start_date)?),
        Some(mask) => {
            if let Some(until) = &plan.repeat_until {
                if date > parse_date(until)? {
                    return Ok(false);
                }
            }
            Ok(mask & weekday_bit(date) != 0)
        }
    }
}

/// Where and how one occurrence happens, before its length and start instant are derived.
struct Slot<'a> {
    date: &'a str,
    source_date: &'a str,
    time: &'a str,
    sets: i64,
    moved: bool,
}

fn build_occurrence<Tz: TimeZone>(tz: &Tz, modes: &ModeLookup, plan: &Plan, slot: &Slot<'_>) -> AppResult<Occurrence> {
    let level = modes.level_for(slot.date);
    let duration_min = plan_duration_min(modes.config(level)?, slot.sets)?;
    let planned_start_ms = local_to_ms(tz, parse_date(slot.date)?, parse_time(slot.time)?)?;
    Ok(Occurrence {
        plan_id: plan.id,
        series_id: plan.series_id,
        name: plan.name.clone(),
        subject: plan.subject.clone(),
        date: slot.date.to_string(),
        source_date: slot.source_date.to_string(),
        start_time: slot.time.to_string(),
        sets: slot.sets,
        moved: slot.moved,
        level,
        duration_min,
        planned_start_ms,
        planned_end_ms: planned_start_ms + duration_min * 60_000,
        eligible_for_missed: planned_start_ms >= plan.created_at_ms,
    })
}

/// Occurrences that happen on `date`, repeats and exceptions applied. An occurrence that spans
/// midnight belongs to its start date only.
pub fn occurrences_on<Tz: TimeZone>(
    tz: &Tz,
    plans: &[Plan],
    exceptions: &[PlanException],
    modes: &ModeLookup,
    date: NaiveDate,
) -> AppResult<Vec<Occurrence>> {
    let date_s = fmt_date(date);
    let mut found = Vec::new();
    for plan in plans.iter().filter(|p| p.deleted_at_ms.is_none()) {
        let base = is_base_date(plan, date)?;
        if base && !exceptions.iter().any(|e| e.plan_id == plan.id && e.date == date_s) {
            let slot = Slot { date: &date_s, source_date: &date_s, time: &plan.start_time, sets: plan.sets, moved: false };
            found.push(build_occurrence(tz, modes, plan, &slot)?);
        }
        for ex in exceptions.iter().filter(|e| e.plan_id == plan.id && e.kind == ExceptionKind::Moved) {
            if ex.new_date.as_deref() != Some(date_s.as_str()) || !is_base_date(plan, parse_date(&ex.date)?)? {
                continue;
            }
            let time = ex.new_time.as_deref().unwrap_or(&plan.start_time);
            let slot = Slot {
                date: &date_s,
                source_date: &ex.date,
                time,
                sets: ex.sets_override.unwrap_or(plan.sets),
                moved: true,
            };
            found.push(build_occurrence(tz, modes, plan, &slot)?);
        }
    }
    found.sort_by_key(|o| (o.planned_start_ms, o.plan_id));
    Ok(found)
}

fn occurrences_between<Tz: TimeZone>(
    tz: &Tz,
    plans: &[Plan],
    exceptions: &[PlanException],
    modes: &ModeLookup,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<Occurrence>> {
    let mut all = Vec::new();
    let mut day = from;
    while day <= to {
        all.extend(occurrences_on(tz, plans, exceptions, modes, day)?);
        day += Duration::days(1);
    }
    all.sort_by_key(|o| (o.planned_start_ms, o.plan_id));
    Ok(all)
}

/// Rejects the change if any occurrence of a touched plan overlaps another occurrence in
/// `from..=to`. Overlaps between untouched plans (for example after a mode length edit) are not
/// this change's fault and are ignored. Back-to-back occurrences are allowed.
fn check_overlap<Tz: TimeZone>(
    tz: &Tz,
    world: &World,
    from: NaiveDate,
    to: NaiveDate,
    touched: &[i64],
) -> AppResult<()> {
    // One day of margin each side: the previous day's occurrence can spill into `from`, and the
    // last day's can spill into the next.
    let all = occurrences_between(
        tz,
        &world.plans,
        &world.exceptions,
        &world.modes,
        from - Duration::days(1),
        to + Duration::days(1),
    )?;
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            if b.planned_start_ms >= a.planned_end_ms {
                break;
            }
            if touched.contains(&a.plan_id) || touched.contains(&b.plan_id) {
                return Err(AppError::Invalid(format!(
                    "plan overlaps another plan on {} and {}",
                    a.date, b.date
                )));
            }
        }
    }
    Ok(())
}

/// Plans, exceptions, and modes as they are (or would be) after a change.
struct World {
    plans: Vec<Plan>,
    exceptions: Vec<PlanException>,
    modes: ModeLookup,
}

fn load_world(conn: &Connection) -> AppResult<World> {
    Ok(World {
        plans: store::list_live_plans(conn)?,
        exceptions: store::list_plan_exceptions(conn)?,
        modes: ModeLookup::load(conn)?,
    })
}

fn overlap_end(first: NaiveDate, repeating: bool) -> NaiveDate {
    if repeating { first + Duration::days(OVERLAP_HORIZON_DAYS) } else { first }
}

// ---- queries ----

pub fn list_occurrences<Tz: TimeZone>(conn: &Connection, tz: &Tz, from: &str, to: &str) -> AppResult<Vec<Occurrence>> {
    let (from, to) = (parse_date(from)?, parse_date(to)?);
    if to < from || (to - from).num_days() >= MAX_LIST_DAYS {
        return Err(AppError::Invalid(format!("range must be 1-{MAX_LIST_DAYS} days")));
    }
    occurrences_in(conn, tz, from, to).map(|(occurrences, _)| occurrences)
}

/// Occurrences in `from..=to` together with the mode lookup that produced their lengths, so the
/// timer can snapshot the same configuration it planned with.
pub fn occurrences_in<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<(Vec<Occurrence>, ModeLookup)> {
    let world = load_world(conn)?;
    let occurrences = occurrences_between(tz, &world.plans, &world.exceptions, &world.modes, from, to)?;
    Ok((occurrences, world.modes))
}

pub fn plan_history(conn: &Connection, plan_id: Option<i64>, limit: i64) -> AppResult<Vec<PlanHistoryEntry>> {
    store::list_plan_history(conn, plan_id, limit)
}

/// Postponement statistics for a local month `YYYY-MM`: how many applied `postponed` changes
/// happened in it, and which subjects slip most.
pub fn plan_stats<Tz: TimeZone>(conn: &Connection, tz: &Tz, month: &str) -> AppResult<PlanStats> {
    let first = parse_date(&format!("{month}-01"))?;
    let next = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
    }
    .ok_or_else(|| AppError::Invalid(format!("month out of range: {month}")))?;
    let midnight = NaiveTime::MIN;
    let (from_ms, to_ms) = (local_to_ms(tz, first, midnight)?, local_to_ms(tz, next, midnight)?);
    Ok(PlanStats {
        month: month.to_string(),
        postponed_count: store::count_postponed(conn, from_ms, to_ms)?,
        by_subject: store::postponed_by_subject(conn, from_ms, to_ms)?,
    })
}

// ---- changes ----

pub(crate) fn plan_json(plan: &Plan) -> Value {
    json!({
        "name": plan.name,
        "subject": plan.subject,
        "startDate": plan.start_date,
        "startTime": plan.start_time,
        "sets": plan.sets,
        "repeatWeekdays": plan.repeat_weekdays,
        "repeatUntil": plan.repeat_until,
    })
}

fn occurrence_json(date: &str, time: &str, sets: i64) -> Value {
    json!({ "date": date, "time": time, "sets": sets })
}

/// What every history row of one operation shares.
struct Ctx<'a> {
    at_ms: i64,
    action: Action,
    reason: ReasonCode,
    reason_text: Option<&'a str>,
}

fn record(
    conn: &Connection,
    ctx: &Ctx<'_>,
    plan_id: Option<i64>,
    before: Option<Value>,
    after: Option<Value>,
    outcome: Outcome,
) -> AppResult<()> {
    store::append_plan_history(
        conn,
        &NewPlanHistory {
            plan_id,
            at_ms: ctx.at_ms,
            action: ctx.action,
            reason: ctx.reason,
            reason_text: ctx.reason_text,
            before_json: before.map(|v| v.to_string()),
            after_json: after.map(|v| v.to_string()),
            outcome,
            batch_id: None,
        },
    )
}

/// User edits must carry one of the user-selectable reasons; `custom` needs text (D-7).
fn check_user_reason(code: ReasonCode, text: Option<&str>) -> AppResult<Option<String>> {
    if !matches!(code, ReasonCode::Postponed | ReasonCode::Sick | ReasonCode::Custom) {
        return Err(AppError::Invalid("reason must be postponed, sick, or custom".into()));
    }
    let text = text.map(str::trim).filter(|t| !t.is_empty());
    if code == ReasonCode::Custom && text.is_none() {
        return Err(AppError::Invalid("a custom reason needs text".into()));
    }
    if let Some(t) = text {
        if t.chars().count() > 500 {
            return Err(AppError::Invalid("reason text is too long".into()));
        }
    }
    Ok(text.map(str::to_string))
}

fn update_action(input: &UpdatePlanInput) -> AppResult<Action> {
    match input.scope {
        Scope::Occurrence if input.repeat.is_some() => {
            return Err(AppError::Invalid("repeat can only change for the whole series".into()));
        }
        Scope::Series if input.new_date.is_some() => {
            return Err(AppError::Invalid("a date move applies to one occurrence".into()));
        }
        _ => {}
    }
    if input.repeat.is_some() {
        Ok(Action::RepeatChange)
    } else if input.new_date.is_some() {
        Ok(Action::Move)
    } else if input.new_time.is_some() {
        Ok(Action::Retime)
    } else if input.new_sets.is_some() {
        Ok(Action::ResizeSets)
    } else {
        Err(AppError::Invalid("nothing to change".into()))
    }
}

pub fn create_plan<Tz: TimeZone>(
    conn: &mut Connection,
    tz: &Tz,
    input: CreatePlanInput,
    at_ms: i64,
) -> AppResult<PlanChange> {
    let name = clean_text(&input.name, "name")?;
    let subject = clean_text(&input.subject, "subject")?;
    let start = parse_date(&input.start_date)?;
    parse_time(&input.start_time)?;
    check_sets(input.sets)?;
    if let Some(repeat) = &input.repeat {
        validate_repeat(start, repeat)?;
    }
    let plan = Plan {
        id: 0,
        series_id: 0,
        name,
        subject,
        start_date: input.start_date.clone(),
        start_time: input.start_time.clone(),
        sets: input.sets,
        repeat_weekdays: input.repeat.as_ref().map(|r| r.weekdays),
        repeat_until: input.repeat.as_ref().and_then(|r| r.until.clone()),
        effective_from_date: input.start_date.clone(),
        created_at_ms: at_ms,
        deleted_at_ms: None,
    };

    let ctx = Ctx { at_ms, action: Action::Create, reason: ReasonCode::Created, reason_text: None };
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if crate::study::lock_held(&tx)? {
        record(&tx, &ctx, None, None, Some(plan_json(&plan)), Outcome::RejectedLocked)?;
        tx.commit()?;
        return Ok(PlanChange::rejected(None));
    }
    let mut world = load_world(&tx)?;
    world.plans.push(plan.clone());
    check_overlap(tz, &world, start, overlap_end(start, plan.repeat_weekdays.is_some()), &[0])?;
    let id = store::insert_plan(&tx, &plan)?;
    record(&tx, &ctx, Some(id), None, Some(plan_json(&plan)), Outcome::Applied)?;
    tx.commit()?;
    tracing::info!(event = "plan_created", plan_id = id);
    Ok(PlanChange::applied(id))
}

pub fn update_plan<Tz: TimeZone>(
    conn: &mut Connection,
    tz: &Tz,
    input: UpdatePlanInput,
    at_ms: i64,
) -> AppResult<PlanChange> {
    let action = update_action(&input)?;
    let reason_text = check_user_reason(input.reason_code, input.reason_text.as_deref())?;
    let date = parse_date(&input.date)?;
    let new_date = input.new_date.as_deref().map(parse_date).transpose()?;
    if let Some(t) = &input.new_time {
        parse_time(t)?;
    }
    if let Some(s) = input.new_sets {
        check_sets(s)?;
    }
    let ctx = Ctx { at_ms, action, reason: input.reason_code, reason_text: reason_text.as_deref() };

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if crate::study::lock_held(&tx)? {
        let attempt = json!({
            "date": input.date, "newDate": input.new_date, "newTime": input.new_time,
            "newSets": input.new_sets, "repeat": input.repeat, "scope": input.scope,
        });
        record(&tx, &ctx, Some(input.plan_id), None, Some(attempt), Outcome::RejectedLocked)?;
        tx.commit()?;
        return Ok(PlanChange::rejected(Some(input.plan_id)));
    }

    let plan = live_plan(&tx, input.plan_id)?;
    if !is_base_date(&plan, date)? {
        return Err(AppError::Invalid(format!("plan has no occurrence on {}", input.date)));
    }
    let mut world = load_world(&tx)?;
    let affected = match input.scope {
        Scope::Occurrence => {
            let target = new_date.unwrap_or(date);
            update_occurrence(&tx, &mut world, &plan, &input, target)?
        }
        Scope::Series => update_series(&mut world, &plan, &input, date, at_ms)?,
    };
    check_overlap(tz, &world, affected.from, affected.to, &affected.touched)?;
    let id = affected.apply(&tx)?;
    record(&tx, &ctx, Some(id), Some(affected.before), Some(affected.after), Outcome::Applied)?;
    tx.commit()?;
    tracing::info!(event = "plan_updated", plan_id = id, action = action.as_str());
    Ok(PlanChange::applied(id))
}

pub fn delete_plan(conn: &mut Connection, input: DeletePlanInput, at_ms: i64) -> AppResult<PlanChange> {
    let reason_text = check_user_reason(input.reason_code, input.reason_text.as_deref())?;
    let date = parse_date(&input.date)?;
    let ctx = Ctx { at_ms, action: Action::Delete, reason: input.reason_code, reason_text: reason_text.as_deref() };

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if crate::study::lock_held(&tx)? {
        let attempt = json!({ "date": input.date, "scope": input.scope });
        record(&tx, &ctx, Some(input.plan_id), None, Some(attempt), Outcome::RejectedLocked)?;
        tx.commit()?;
        return Ok(PlanChange::rejected(Some(input.plan_id)));
    }

    let plan = live_plan(&tx, input.plan_id)?;
    if !is_base_date(&plan, date)? {
        return Err(AppError::Invalid(format!("plan has no occurrence on {}", input.date)));
    }
    match input.scope {
        Scope::Occurrence => {
            let (cur_date, cur_time, cur_sets) = match current_occurrence(&tx, &plan, &input.date)? {
                Some(cur) => cur,
                None => return Err(AppError::Invalid("occurrence is already deleted".into())),
            };
            store::put_plan_exception(
                &tx,
                &PlanException {
                    plan_id: plan.id,
                    date: input.date.clone(),
                    kind: ExceptionKind::Cancelled,
                    new_date: None,
                    new_time: None,
                    sets_override: None,
                },
            )?;
            record(
                &tx,
                &ctx,
                Some(plan.id),
                Some(occurrence_json(&cur_date, &cur_time, cur_sets)),
                Some(json!({ "deleted": true })),
                Outcome::Applied,
            )?;
        }
        Scope::Series => {
            // Everything from `date` on goes; the past stays so old runs keep their plan.
            let mut rows = vec![plan.clone()];
            for later in store::list_live_plans(&tx)? {
                if later.series_id == plan.series_id && later.id != plan.id && first_date(&later)? > date {
                    rows.push(later);
                }
            }
            let batch = (rows.len() > 1).then(|| format!("delete-{at_ms}-{}", plan.id));
            for row in &rows {
                if row.id == plan.id && first_date(row)? < date {
                    store::set_plan_repeat_until(&tx, row.id, Some(&fmt_date(date - Duration::days(1))))?;
                } else {
                    store::soft_delete_plan(&tx, row.id, at_ms)?;
                }
                store::append_plan_history(
                    &tx,
                    &NewPlanHistory {
                        plan_id: Some(row.id),
                        at_ms,
                        action: Action::Delete,
                        reason: input.reason_code,
                        reason_text: reason_text.as_deref(),
                        before_json: Some(plan_json(row).to_string()),
                        after_json: Some(json!({ "deletedFrom": input.date }).to_string()),
                        outcome: Outcome::Applied,
                        batch_id: batch.as_deref(),
                    },
                )?;
            }
        }
    }
    tx.commit()?;
    tracing::info!(event = "plan_deleted", plan_id = plan.id);
    Ok(PlanChange::applied(plan.id))
}

fn live_plan(conn: &Connection, id: i64) -> AppResult<Plan> {
    store::get_plan(conn, id)?
        .filter(|p| p.deleted_at_ms.is_none())
        .ok_or_else(|| AppError::Invalid(format!("plan {id} does not exist")))
}

/// Effective `(date, time, sets)` of the occurrence keyed by `source_date`, or `None` if it was
/// cancelled.
fn current_occurrence(conn: &Connection, plan: &Plan, source_date: &str) -> AppResult<Option<(String, String, i64)>> {
    Ok(match store::get_plan_exception(conn, plan.id, source_date)? {
        None => Some((source_date.to_string(), plan.start_time.clone(), plan.sets)),
        Some(ex) if ex.kind == ExceptionKind::Cancelled => None,
        Some(ex) => Some((
            ex.new_date.unwrap_or_else(|| source_date.to_string()),
            ex.new_time.unwrap_or_else(|| plan.start_time.clone()),
            ex.sets_override.unwrap_or(plan.sets),
        )),
    })
}

/// A validated update: the overlap window to check and the write to perform once it passes.
struct Prepared {
    touched: Vec<i64>,
    from: NaiveDate,
    to: NaiveDate,
    before: Value,
    after: Value,
    write: Write,
}

enum Write {
    /// `None` removes the exception (the occurrence is back to the plan's own values).
    Exception { plan_id: i64, source_date: String, exception: Option<PlanException> },
    InPlace(Plan),
    /// Cut `old_id` off the day before `next.start_date` and start `next` from there.
    Split { old_id: i64, cut_until: String, next: Plan },
}

impl Prepared {
    /// Performs the write and returns the plan id the history row belongs to.
    fn apply(&self, conn: &Connection) -> AppResult<i64> {
        match &self.write {
            Write::Exception { plan_id, source_date, exception } => {
                match exception {
                    Some(ex) => store::put_plan_exception(conn, ex)?,
                    None => store::delete_plan_exception(conn, *plan_id, source_date)?,
                }
                Ok(*plan_id)
            }
            Write::InPlace(plan) => {
                store::update_plan_fields(conn, plan)?;
                Ok(plan.id)
            }
            Write::Split { old_id, cut_until, next } => {
                store::set_plan_repeat_until(conn, *old_id, Some(cut_until))?;
                let id = store::insert_plan(conn, next)?;
                store::move_plan_exceptions_from(conn, *old_id, id, &next.start_date)?;
                Ok(id)
            }
        }
    }
}

fn update_occurrence(
    conn: &Connection,
    world: &mut World,
    plan: &Plan,
    input: &UpdatePlanInput,
    target_date: NaiveDate,
) -> AppResult<Prepared> {
    let (cur_date, cur_time, cur_sets) = current_occurrence(conn, plan, &input.date)?
        .ok_or_else(|| AppError::Invalid("occurrence is deleted".into()))?;
    let new_date_s = fmt_date(target_date);
    let new_time = input.new_time.clone().unwrap_or_else(|| cur_time.clone());
    let new_sets = input.new_sets.unwrap_or(cur_sets);
    if (new_date_s.as_str(), new_time.as_str(), new_sets) == (cur_date.as_str(), cur_time.as_str(), cur_sets) {
        return Err(AppError::Invalid("nothing to change".into()));
    }

    // Back to the plan's own values means the exception is no longer needed.
    let reverts = new_date_s == input.date && new_time == plan.start_time && new_sets == plan.sets;
    let exception = (!reverts).then(|| PlanException {
        plan_id: plan.id,
        date: input.date.clone(),
        kind: ExceptionKind::Moved,
        new_date: Some(new_date_s.clone()),
        new_time: Some(new_time.clone()),
        sets_override: (new_sets != plan.sets).then_some(new_sets),
    });
    world.exceptions.retain(|e| !(e.plan_id == plan.id && e.date == input.date));
    world.exceptions.extend(exception.clone());

    let source = parse_date(&input.date)?;
    Ok(Prepared {
        touched: vec![plan.id],
        from: source.min(target_date),
        to: source.max(target_date),
        before: occurrence_json(&cur_date, &cur_time, cur_sets),
        after: occurrence_json(&new_date_s, &new_time, new_sets),
        write: Write::Exception { plan_id: plan.id, source_date: input.date.clone(), exception },
    })
}

fn update_series(
    world: &mut World,
    plan: &Plan,
    input: &UpdatePlanInput,
    date: NaiveDate,
    at_ms: i64,
) -> AppResult<Prepared> {
    let mut next = plan.clone();
    if let Some(time) = &input.new_time {
        next.start_time = time.clone();
    }
    if let Some(sets) = input.new_sets {
        next.sets = sets;
    }
    if let Some(repeat) = &input.repeat {
        validate_repeat(date, repeat)?;
        next.repeat_weekdays = Some(repeat.weekdays);
        next.repeat_until = repeat.until.clone();
    }
    if next == *plan {
        return Err(AppError::Invalid("nothing to change".into()));
    }
    let to = overlap_end(date, next.repeat_weekdays.is_some());

    if date <= first_date(plan)? {
        // Nothing earlier to preserve: edit the row in place.
        replace_plan(&mut world.plans, &next);
        return Ok(Prepared {
            touched: vec![plan.id],
            from: date,
            to,
            before: plan_json(plan),
            after: plan_json(&next),
            write: Write::InPlace(next),
        });
    }

    // Past occurrences keep the old row: cut it off the day before, start a new row from `date`.
    let mut cut = plan.clone();
    let cut_until = fmt_date(date - Duration::days(1));
    cut.repeat_until = Some(cut_until.clone());
    next.id = 0;
    next.start_date = fmt_date(date);
    next.effective_from_date = fmt_date(date);
    next.created_at_ms = at_ms;
    replace_plan(&mut world.plans, &cut);
    world.plans.push(next.clone());
    // Edited-occurrence exceptions from `date` on follow the new row (id 0 until inserted).
    for ex in world.exceptions.iter_mut() {
        if ex.plan_id == plan.id && ex.date >= next.start_date {
            ex.plan_id = 0;
        }
    }
    Ok(Prepared {
        touched: vec![0, plan.id],
        from: date,
        to,
        before: plan_json(plan),
        after: plan_json(&next),
        write: Write::Split { old_id: plan.id, cut_until, next },
    })
}

fn replace_plan(plans: &mut [Plan], updated: &Plan) {
    if let Some(slot) = plans.iter_mut().find(|p| p.id == updated.id) {
        *slot = updated.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use chrono::Utc;
    use chrono_tz::America::New_York;

    const MON: i64 = 1;
    const WED: i64 = 4;

    fn ms(y: i32, m: u32, d: u32, h: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).single().expect("valid time").timestamp_millis()
    }

    fn day(raw: &str) -> NaiveDate {
        parse_date(raw).expect("date")
    }

    fn new_input(subject: &str, date: &str, time: &str, sets: i64, repeat: Option<Repeat>) -> CreatePlanInput {
        CreatePlanInput {
            name: format!("{subject} 공부"),
            subject: subject.into(),
            start_date: date.into(),
            start_time: time.into(),
            sets,
            repeat,
        }
    }

    fn weekly(weekdays: i64, until: Option<&str>) -> Option<Repeat> {
        Some(Repeat { weekdays, until: until.map(str::to_string) })
    }

    fn create(conn: &mut Connection, input: CreatePlanInput, at_ms: i64) -> AppResult<i64> {
        let change = create_plan(conn, &Utc, input, at_ms)?;
        Ok(change.plan_id.expect("applied create has an id"))
    }

    fn occurrence_update(plan_id: i64, date: &str) -> UpdatePlanInput {
        UpdatePlanInput {
            plan_id,
            scope: Scope::Occurrence,
            date: date.into(),
            new_date: None,
            new_time: None,
            new_sets: None,
            repeat: None,
            reason_code: ReasonCode::Postponed,
            reason_text: None,
        }
    }

    fn set_lock(conn: &Connection, held: bool) {
        conn.execute("UPDATE study_state SET studying = ?1 WHERE id = 1", [i64::from(held)]).expect("lock");
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).expect("count")
    }

    fn list(conn: &Connection, from: &str, to: &str) -> Vec<Occurrence> {
        list_occurrences(conn, &Utc, from, to).expect("list")
    }

    fn dates(occurrences: &[Occurrence]) -> Vec<&str> {
        occurrences.iter().map(|o| o.date.as_str()).collect()
    }

    #[test]
    fn block_sequence_durations_follow_the_plan_table() {
        let conn = open_in_memory().expect("db");
        let configs = store::list_mode_configs(&conn).expect("configs");
        let cfg = |level: StudyLevel| configs.iter().find(|c| c.level == level).expect("cfg");
        let table = [
            (StudyLevel::Medium, 3, 180),
            (StudyLevel::Easy, 4, 115),
            (StudyLevel::Easy, 5, 160),
            (StudyLevel::Easy, 8, 250),
            (StudyLevel::Hard, 2, 190),
            (StudyLevel::Easy, 1, 25),
        ];
        for (level, sets, minutes) in table {
            assert_eq!(plan_duration_min(cfg(level), sets).expect("duration"), minutes, "{level:?} x{sets}");
        }
        let kinds: Vec<BlockKind> = block_sequence(cfg(StudyLevel::Easy), 5).expect("blocks").iter().map(|b| b.kind).collect();
        use BlockKind::{Break, LongBreak, Study};
        assert_eq!(kinds, [Study, Break, Study, Break, Study, Break, Study, LongBreak, Study]);
        assert!(block_sequence(cfg(StudyLevel::Easy), 0).is_err());
        assert!(block_sequence(cfg(StudyLevel::Easy), MAX_SETS + 1).is_err());
    }

    #[test]
    fn repeats_and_single_occurrence_exceptions_shape_occurrences() {
        let mut conn = open_in_memory().expect("db");
        let id = create(&mut conn, new_input("수학", "2026-03-02", "20:00", 2, weekly(MON | WED, Some("2026-03-16"))), 1).expect("create");
        let all = list(&conn, "2026-03-01", "2026-03-31");
        assert_eq!(dates(&all), ["2026-03-02", "2026-03-04", "2026-03-09", "2026-03-11", "2026-03-16"]);
        assert!(all.iter().all(|o| o.duration_min == 115 && o.level == StudyLevel::Medium));

        let mut mv = occurrence_update(id, "2026-03-04");
        mv.new_date = Some("2026-03-05".into());
        mv.new_time = Some("19:00".into());
        assert!(update_plan(&mut conn, &Utc, mv, 2).expect("move").is_applied());
        let delete = DeletePlanInput { plan_id: id, scope: Scope::Occurrence, date: "2026-03-09".into(), reason_code: ReasonCode::Sick, reason_text: None };
        assert!(delete_plan(&mut conn, delete, 3).expect("delete").is_applied());

        let all = list(&conn, "2026-03-01", "2026-03-31");
        assert_eq!(dates(&all), ["2026-03-02", "2026-03-05", "2026-03-11", "2026-03-16"]);
        let moved = &all[1];
        assert!(moved.moved);
        assert_eq!((moved.source_date.as_str(), moved.start_time.as_str()), ("2026-03-04", "19:00"));

        // Moving back to the original values drops the exception instead of storing a no-op.
        let mut back = occurrence_update(id, "2026-03-04");
        back.new_date = Some("2026-03-04".into());
        back.new_time = Some("20:00".into());
        update_plan(&mut conn, &Utc, back, 4).expect("back");
        assert_eq!(count(&conn, "plan_exceptions"), 1, "only the cancellation remains");
        assert!(dates(&list(&conn, "2026-03-04", "2026-03-04")).contains(&"2026-03-04"));
    }

    #[test]
    fn overlapping_create_and_update_are_errors_that_write_nothing() {
        let mut conn = open_in_memory().expect("db");
        let first = create(&mut conn, new_input("수학", "2026-03-02", "20:00", 2, None), 1).expect("first");
        // 20:00 + 115 min ends 21:55: starting at 21:55 touches but does not overlap.
        create(&mut conn, new_input("영어", "2026-03-02", "21:55", 1, None), 2).expect("back to back");
        let (plans, history) = (count(&conn, "plans"), count(&conn, "plan_history"));

        assert!(create(&mut conn, new_input("과학", "2026-03-02", "20:30", 1, None), 3).is_err());
        assert!(create(&mut conn, new_input("과학", "2026-03-01", "20:00", 1, weekly(MON | 64, None)), 3).is_err(), "Sunday start; repeat reaches Monday");
        let mut push = occurrence_update(first, "2026-03-02");
        push.new_sets = Some(3);
        assert!(update_plan(&mut conn, &Utc, push, 4).is_err(), "3 sets run into the 21:55 plan");
        assert_eq!((count(&conn, "plans"), count(&conn, "plan_history")), (plans, history));
        assert_eq!(count(&conn, "plan_exceptions"), 0);
    }

    #[test]
    fn plans_spanning_midnight_belong_to_their_start_date_and_block_the_next_morning() {
        let mut conn = open_in_memory().expect("db");
        // 23:00 + 115 min ends 00:55 the next day.
        create(&mut conn, new_input("수학", "2026-03-02", "23:00", 2, None), 1).expect("late plan");
        assert!(list(&conn, "2026-03-03", "2026-03-03").is_empty());
        assert!(create(&mut conn, new_input("영어", "2026-03-03", "00:30", 1, None), 2).is_err());
        create(&mut conn, new_input("영어", "2026-03-03", "01:00", 1, None), 3).expect("after the end");
    }

    #[test]
    fn local_time_resolution_handles_dst_gap_and_overlap() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).expect("time");
        // 2026-03-08 02:00-03:00 does not exist in New York: 02:30 moves to 03:00 EDT = 07:00Z.
        assert_eq!(local_to_ms(&New_York, day("2026-03-08"), t(2, 30)).expect("gap"), ms(2026, 3, 8, 7));
        // 2026-11-01 01:30 happens twice; the earlier one is still EDT (UTC-4) = 05:30Z.
        assert_eq!(local_to_ms(&New_York, day("2026-11-01"), t(1, 30)).expect("overlap"), ms(2026, 11, 1, 5) + 30 * 60_000);
        assert_eq!(local_to_ms(&New_York, day("2026-03-09"), t(9, 0)).expect("plain"), ms(2026, 3, 9, 13));

        // A plan across the jump keeps its absolute length (180 min), not 180 wall-clock minutes.
        let mut conn = open_in_memory().expect("db");
        create_plan(&mut conn, &New_York, new_input("수학", "2026-03-08", "01:00", 3, None), 1).expect("create");
        let occ = list_occurrences(&conn, &New_York, "2026-03-08", "2026-03-08").expect("list");
        assert_eq!(occ[0].planned_start_ms, ms(2026, 3, 8, 6));
        assert_eq!(occ[0].planned_end_ms - occ[0].planned_start_ms, 180 * 60_000);
    }

    #[test]
    fn plan_history_rejects_update_and_delete() {
        let mut conn = open_in_memory().expect("db");
        create(&mut conn, new_input("수학", "2026-03-02", "20:00", 1, None), 1).expect("create");
        assert!(conn.execute("UPDATE plan_history SET reason_text = 'x'", []).is_err());
        assert!(conn.execute("DELETE FROM plan_history", []).is_err());
    }

    #[test]
    fn every_edit_appends_one_history_row_with_before_after_and_reason() {
        let mut conn = open_in_memory().expect("db");
        let id = create(&mut conn, new_input("수학", "2026-03-02", "20:00", 2, None), 10).expect("create");
        let mut mv = occurrence_update(id, "2026-03-02");
        mv.new_date = Some("2026-03-03".into());
        update_plan(&mut conn, &Utc, mv, 20).expect("move");
        let mut custom = occurrence_update(id, "2026-03-02");
        custom.new_time = Some("21:00".into());
        custom.reason_code = ReasonCode::Custom;
        custom.reason_text = Some("  시험 일정  ".into());
        update_plan(&mut conn, &Utc, custom, 30).expect("retime");
        let delete = DeletePlanInput { plan_id: id, scope: Scope::Occurrence, date: "2026-03-02".into(), reason_code: ReasonCode::Sick, reason_text: None };
        delete_plan(&mut conn, delete, 40).expect("delete");

        let rows = plan_history(&conn, Some(id), 50).expect("history");
        let summary: Vec<(i64, &str, &str)> = rows.iter().map(|r| (r.at_ms, r.action.as_str(), r.reason_code.as_str())).collect();
        assert_eq!(summary, [(40, "delete", "sick"), (30, "retime", "custom"), (20, "move", "postponed"), (10, "create", "created")]);
        assert_eq!(rows[1].reason_text.as_deref(), Some("시험 일정"));
        let before: Value = serde_json::from_str(rows[2].before_json.as_deref().expect("before")).expect("json");
        let after: Value = serde_json::from_str(rows[2].after_json.as_deref().expect("after")).expect("json");
        assert_eq!((before["date"].as_str(), after["date"].as_str()), (Some("2026-03-02"), Some("2026-03-03")));
        assert!(rows.iter().all(|r| r.outcome == "applied"));
    }

    #[test]
    fn user_edits_need_a_user_reason_and_a_valid_scope() {
        let mut conn = open_in_memory().expect("db");
        let id = create(&mut conn, new_input("수학", "2026-03-02", "20:00", 1, weekly(MON, None)), 1).expect("create");
        let rows = count(&conn, "plan_history");
        let mut bad = occurrence_update(id, "2026-03-02");
        bad.new_sets = Some(2);
        bad.reason_code = ReasonCode::ModeShift;
        assert!(update_plan(&mut conn, &Utc, bad.clone(), 2).is_err(), "system reasons are not user-selectable");
        bad.reason_code = ReasonCode::Custom;
        assert!(update_plan(&mut conn, &Utc, bad.clone(), 2).is_err(), "custom needs text");
        bad.reason_code = ReasonCode::Postponed;
        bad.scope = Scope::Series;
        bad.new_date = Some("2026-03-03".into());
        assert!(update_plan(&mut conn, &Utc, bad, 2).is_err(), "series scope cannot move one date");
        assert!(update_plan(&mut conn, &Utc, occurrence_update(id, "2026-03-02"), 2).is_err(), "no field to change");
        assert!(update_plan(&mut conn, &Utc, occurrence_update(id, "2026-03-03"), 2).is_err(), "no occurrence that day");
        assert_eq!(count(&conn, "plan_history"), rows);
    }

    #[test]
    fn locked_changes_are_rejected_recorded_and_leave_plans_untouched() {
        let mut conn = open_in_memory().expect("db");
        let id = create(&mut conn, new_input("수학", "2026-03-02", "20:00", 2, weekly(MON, None)), 1).expect("create");
        let plan_before = store::get_plan(&conn, id).expect("get");
        set_lock(&conn, true);

        let created = create_plan(&mut conn, &Utc, new_input("영어", "2026-03-03", "09:00", 1, None), 2).expect("create attempt");
        assert!(!created.is_applied());
        assert_eq!(created.plan_id, None);
        let mut edit = occurrence_update(id, "2026-03-02");
        edit.new_sets = Some(1);
        assert!(!update_plan(&mut conn, &Utc, edit.clone(), 3).expect("occurrence edit").is_applied());
        edit.scope = Scope::Series;
        assert!(!update_plan(&mut conn, &Utc, edit, 4).expect("series edit").is_applied());
        for scope in [Scope::Occurrence, Scope::Series] {
            let delete = DeletePlanInput { plan_id: id, scope, date: "2026-03-02".into(), reason_code: ReasonCode::Sick, reason_text: None };
            assert!(!delete_plan(&mut conn, delete, 5).expect("delete attempt").is_applied());
        }

        assert_eq!(store::get_plan(&conn, id).expect("get"), plan_before);
        assert_eq!((count(&conn, "plans"), count(&conn, "plan_exceptions")), (1, 0));
        let outcomes: Vec<String> = conn
            .prepare("SELECT outcome FROM plan_history WHERE id > 1 ORDER BY id")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(outcomes, ["rejected_locked"; 5]);
        let rejected_create: Option<i64> = conn
            .query_row("SELECT plan_id FROM plan_history WHERE id = 2", [], |r| r.get(0))
            .expect("row");
        assert_eq!(rejected_create, None);
    }

    #[test]
    fn unknown_lock_state_makes_every_change_an_error() {
        let mut conn = open_in_memory().expect("db");
        let id = create(&mut conn, new_input("수학", "2026-03-02", "20:00", 2, None), 1).expect("create");
        conn.execute("DELETE FROM study_state", []).expect("delete lock row");
        let rows = count(&conn, "plan_history");
        let mut edit = occurrence_update(id, "2026-03-02");
        edit.new_sets = Some(1);
        let delete = DeletePlanInput { plan_id: id, scope: Scope::Series, date: "2026-03-02".into(), reason_code: ReasonCode::Sick, reason_text: None };
        assert!(create_plan(&mut conn, &Utc, new_input("영어", "2026-03-03", "09:00", 1, None), 2).is_err());
        assert!(update_plan(&mut conn, &Utc, edit, 2).is_err());
        assert!(delete_plan(&mut conn, delete, 2).is_err());
        assert_eq!((count(&conn, "plans"), count(&conn, "plan_history")), (1, rows));
    }

    #[test]
    fn series_edit_leaves_the_past_alone_and_applies_from_the_edit_date() {
        let mut conn = open_in_memory().expect("db");
        let created_at = ms(2026, 3, 1, 0);
        let id = create(&mut conn, new_input("수학", "2026-03-02", "20:00", 2, weekly(MON, None)), created_at).expect("create");
        let past_before = list(&conn, "2026-03-02", "2026-03-15");

        let mut edit = occurrence_update(id, "2026-03-16");
        edit.scope = Scope::Series;
        edit.new_time = Some("21:00".into());
        let change = update_plan(&mut conn, &Utc, edit, ms(2026, 3, 10, 0)).expect("series edit");
        let new_id = change.plan_id.expect("id");
        assert_ne!(new_id, id, "the future moves to a new row");

        assert_eq!(list(&conn, "2026-03-02", "2026-03-15"), past_before, "past occurrences are unchanged");
        let all = list(&conn, "2026-03-02", "2026-03-30");
        let shape: Vec<(&str, &str, i64)> = all.iter().map(|o| (o.date.as_str(), o.start_time.as_str(), o.plan_id)).collect();
        assert_eq!(
            shape,
            [
                ("2026-03-02", "20:00", id),
                ("2026-03-09", "20:00", id),
                ("2026-03-16", "21:00", new_id),
                ("2026-03-23", "21:00", new_id),
                ("2026-03-30", "21:00", new_id),
            ]
        );
        let old = store::get_plan(&conn, id).expect("get").expect("old row");
        let new = store::get_plan(&conn, new_id).expect("get").expect("new row");
        assert_eq!(old.repeat_until.as_deref(), Some("2026-03-15"));
        assert_eq!((new.series_id, new.effective_from_date.as_str(), new.created_at_ms), (old.series_id, "2026-03-16", ms(2026, 3, 10, 0)));
        // One timeline for the series, whichever row the UI asks about.
        assert_eq!(plan_history(&conn, Some(id), 50).expect("a").len(), 2);
        assert_eq!(plan_history(&conn, Some(new_id), 50).expect("b").len(), 2);
    }

    #[test]
    fn series_edit_on_the_first_date_edits_in_place_and_series_delete_keeps_the_past() {
        let mut conn = open_in_memory().expect("db");
        let id = create(&mut conn, new_input("수학", "2026-03-02", "20:00", 1, weekly(MON, None)), 1).expect("create");
        let mut edit = occurrence_update(id, "2026-03-02");
        edit.scope = Scope::Series;
        edit.new_sets = Some(2);
        assert_eq!(update_plan(&mut conn, &Utc, edit, 2).expect("edit").plan_id, Some(id));
        assert_eq!(count(&conn, "plans"), 1);

        let delete = DeletePlanInput { plan_id: id, scope: Scope::Series, date: "2026-03-16".into(), reason_code: ReasonCode::Custom, reason_text: Some("그만".into()) };
        assert!(delete_plan(&mut conn, delete, 3).expect("delete").is_applied());
        assert_eq!(dates(&list(&conn, "2026-03-01", "2026-04-30")), ["2026-03-02", "2026-03-09"]);

        let all = DeletePlanInput { plan_id: id, scope: Scope::Series, date: "2026-03-02".into(), reason_code: ReasonCode::Sick, reason_text: None };
        assert!(delete_plan(&mut conn, all, 4).expect("delete all").is_applied());
        assert!(list(&conn, "2026-03-01", "2026-04-30").is_empty());
        assert_eq!(count(&conn, "plans"), 1, "soft delete keeps the row");
    }

    #[test]
    fn occurrences_before_the_plan_existed_are_not_eligible_for_missed() {
        let mut conn = open_in_memory().expect("db");
        // Created Wednesday 03-04 12:00 for a Monday series that starts 03-02.
        create(&mut conn, new_input("수학", "2026-03-02", "20:00", 1, weekly(MON, None)), ms(2026, 3, 4, 12)).expect("create");
        let eligible: Vec<(&str, bool)> = list(&conn, "2026-03-02", "2026-03-09")
            .iter()
            .map(|o| (if o.date == "2026-03-02" { "first" } else { "later" }, o.eligible_for_missed))
            .collect();
        assert_eq!(eligible, [("first", false), ("later", true)]);
    }

    #[test]
    fn monthly_stats_count_applied_postponements_by_month_and_subject() {
        let mut conn = open_in_memory().expect("db");
        let math = create(&mut conn, new_input("수학", "2026-03-02", "20:00", 1, weekly(MON, None)), 1).expect("math");
        let english = create(&mut conn, new_input("영어", "2026-03-02", "08:00", 1, weekly(MON, None)), 2).expect("english");
        let push = |conn: &mut Connection, plan: i64, to: &str, at: i64, reason: ReasonCode| {
            let mut u = occurrence_update(plan, "2026-03-02");
            u.new_date = Some(to.into());
            u.reason_code = reason;
            u.reason_text = Some("x".into());
            update_plan(conn, &Utc, u, at).expect("move");
        };
        push(&mut conn, math, "2026-03-03", ms(2026, 3, 5, 0), ReasonCode::Postponed);
        push(&mut conn, math, "2026-03-04", ms(2026, 3, 6, 0), ReasonCode::Postponed);
        push(&mut conn, math, "2026-03-05", ms(2026, 3, 31, 23), ReasonCode::Postponed);
        push(&mut conn, math, "2026-03-06", ms(2026, 3, 20, 0), ReasonCode::Sick);
        push(&mut conn, english, "2026-03-03", ms(2026, 3, 7, 0), ReasonCode::Postponed);
        push(&mut conn, english, "2026-03-04", ms(2026, 4, 1, 0), ReasonCode::Postponed);
        set_lock(&conn, true);
        let rejected = DeletePlanInput { plan_id: math, scope: Scope::Occurrence, date: "2026-03-09".into(), reason_code: ReasonCode::Postponed, reason_text: None };
        delete_plan(&mut conn, rejected, ms(2026, 3, 8, 0)).expect("rejected");

        let march = plan_stats(&conn, &Utc, "2026-03").expect("march");
        assert_eq!(march.postponed_count, 4, "sick, rejected, and April rows are excluded; the 03-31 23:00 row counts");
        assert_eq!(
            march.by_subject,
            [SubjectCount { subject: "수학".into(), count: 3 }, SubjectCount { subject: "영어".into(), count: 1 }]
        );
        let april = plan_stats(&conn, &Utc, "2026-04").expect("april");
        assert_eq!((april.postponed_count, april.by_subject.len()), (1, 1));
        assert!(plan_stats(&conn, &Utc, "2026-13").is_err());
    }

    #[test]
    fn check_constraints_accept_exactly_the_rust_enum_values() {
        let conn = open_in_memory().expect("db");
        let insert = |action: &str, reason: &str| {
            conn.execute(
                "INSERT INTO plan_history (plan_id, at_ms, action, reason_code, outcome) VALUES (NULL, 1, ?1, ?2, 'applied')",
                [action, reason],
            )
        };
        for action in Action::ALL {
            insert(action.as_str(), "created").unwrap_or_else(|e| panic!("action {} rejected: {e}", action.as_str()));
        }
        for reason in ReasonCode::ALL {
            insert("create", reason.as_str()).unwrap_or_else(|e| panic!("reason {} rejected: {e}", reason.as_str()));
        }
        assert!(insert("bogus", "created").is_err());
        assert!(insert("create", "bogus").is_err());

        // The inverse direction: the CHECK lists no value the enums lack.
        let sql: String = conn
            .query_row("SELECT sql FROM sqlite_master WHERE name = 'plan_history'", [], |r| r.get(0))
            .expect("schema");
        let listed = |column: &str| {
            let start = sql.find(&format!("{column} IN (")).expect("check list");
            let list = &sql[start..sql[start..].find("))").map_or(sql.len(), |end| start + end)];
            list.matches('\'').count() / 2
        };
        assert_eq!(listed("action"), Action::ALL.len());
        assert_eq!(listed("reason_code"), ReasonCode::ALL.len());
    }

    #[test]
    fn serialized_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/plan-contract.json");
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let occurrence = Occurrence {
            plan_id: 7,
            series_id: 7,
            name: "수학 문제풀이".into(),
            subject: "수학".into(),
            date: "2026-03-09".into(),
            source_date: "2026-03-02".into(),
            start_time: "20:00".into(),
            sets: 3,
            moved: true,
            level: StudyLevel::Medium,
            duration_min: 180,
            planned_start_ms: 1_773_054_000_000,
            planned_end_ms: 1_773_064_800_000,
            eligible_for_missed: true,
        };
        let entry = PlanHistoryEntry {
            id: 3,
            plan_id: Some(7),
            at_ms: 1_773_000_000_000,
            action: "move".into(),
            reason_code: "postponed".into(),
            reason_text: None,
            before_json: Some(r#"{"date":"2026-03-02","sets":3,"time":"20:00"}"#.into()),
            after_json: Some(r#"{"date":"2026-03-09","sets":3,"time":"20:00"}"#.into()),
            outcome: "applied".into(),
            batch_id: None,
        };
        let stats = PlanStats {
            month: "2026-03".into(),
            postponed_count: 3,
            by_subject: vec![
                SubjectCount { subject: "수학".into(), count: 2 },
                SubjectCount { subject: "영어".into(), count: 1 },
            ],
        };
        let actual = json!({
            "changes": [PlanChange::applied(7), PlanChange::rejected(None)],
            "occurrence": occurrence,
            "historyEntry": entry,
            "stats": stats,
        });
        assert_eq!(actual, expected);
    }
}
