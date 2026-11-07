//! Rule-based plan shift on a mode change (feature 17) and its undo.
//!
//! Changing today's mode, or the lengths of the mode in force today, changes the derived length
//! of today's plans. Plans that have not started yet are moved by the accumulated length
//! difference so the gaps between plans stay as they were. The shift runs inside the caller's
//! Immediate transaction: the mode write, every moved occurrence, and the history rows commit
//! together or not at all. The shift itself is rule-based; an `LlmProposal` (computed by
//! `plan_adjust` before the transaction, never inside it) may replace the rule's new start times
//! when it passes `validate_layout` and still matches the day the transaction sees. Anything else
//! falls back to the rule, and the history rows say which one produced them (`mode_shift` or
//! `llm_adjust`).

use std::collections::HashSet;

use chrono::{Duration, NaiveTime, TimeZone};
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{AppError, AppResult};
use crate::plan::{
    Action, ExceptionKind, ModeLookup, NewPlanHistory, Occurrence, PlanChange, PlanException,
    ReasonCode, fmt_date, local_to_ms, occurrences_on, parse_date, plan_duration_min,
};
use crate::settings::{Outcome, Reason, UpdateOutcome};
use crate::store;
use crate::study::{self, ModeConfig, StudyLevel};

const MINUTE_MS: i64 = 60_000;
const TIME_FMT: &str = "%H:%M";

// ---- pure rule ----

/// One occurrence on (or spilling into) the day being shifted, ordered by `old_start_ms`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShiftItem {
    pub old_start_ms: i64,
    pub old_duration_min: i64,
    pub new_duration_min: i64,
    /// Not started yet: no run exists and the start is still ahead. Everything else is frozen.
    pub pending: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ShiftPlan {
    /// `(item index, new start ms)` for each item that actually moves.
    Moves(Vec<(usize, i64)>),
    /// Some pending plan would end after local midnight: nothing moves (D-10).
    OverflowsMidnight,
}

fn ceil_minute(ms: i64) -> i64 {
    (ms + MINUTE_MS - 1).div_euclid(MINUTE_MS) * MINUTE_MS
}

/// Each pending plan keeps the gap to its predecessor: its displacement is the previous plan's
/// displacement plus that plan's length difference. A plan is never pulled before `now_ms` or
/// before the end of a frozen plan; the clamp carries on to the plans after it. All-or-nothing
/// on midnight, because moving only some plans would let them overlap the ones left behind.
pub fn compute_shift(items: &[ShiftItem], now_ms: i64, day_end_ms: i64) -> ShiftPlan {
    let now_floor = ceil_minute(now_ms);
    let mut frozen_end = i64::MIN;
    let mut displacement = 0_i64;
    let mut moves = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if !item.pending {
            frozen_end = frozen_end.max(item.old_start_ms + item.old_duration_min * MINUTE_MS);
            continue;
        }
        let lower = if frozen_end == i64::MIN {
            now_floor
        } else {
            now_floor.max(ceil_minute(frozen_end))
        };
        let start = (item.old_start_ms + displacement).max(lower);
        if start + item.new_duration_min * MINUTE_MS > day_end_ms {
            return ShiftPlan::OverflowsMidnight;
        }
        if start != item.old_start_ms {
            moves.push((index, start));
        }
        displacement =
            start - item.old_start_ms + (item.new_duration_min - item.old_duration_min) * MINUTE_MS;
    }
    ShiftPlan::Moves(moves)
}

/// Start times (ms) the LLM proposed for the items of a day, in the order of `items`. Built by
/// `plan_adjust` from a read-only preview; `shift_after_mode_change_with` applies it only if the
/// day it sees in the real transaction is identical to `items`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmProposal {
    pub items: Vec<ShiftItem>,
    pub starts: Vec<i64>,
}

/// Whether a proposed layout is acceptable: one start per item; frozen items stay where they are;
/// a pending item starts on a whole minute, not before now, and ends before the day does (its new
/// length is fixed, so the total set count cannot change); and no two items overlap, frozen ones
/// included.
pub fn validate_layout(items: &[ShiftItem], starts: &[i64], now_ms: i64, day_end_ms: i64) -> bool {
    if items.len() != starts.len() {
        return false;
    }
    let now_floor = ceil_minute(now_ms);
    let mut spans: Vec<(i64, i64)> = Vec::with_capacity(items.len());
    for (item, &start) in items.iter().zip(starts) {
        if item.pending {
            let end = start + item.new_duration_min * MINUTE_MS;
            if start % MINUTE_MS != 0 || start < now_floor || end > day_end_ms {
                return false;
            }
            spans.push((start, end));
        } else {
            if start != item.old_start_ms {
                return false;
            }
            spans.push((start, start + item.old_duration_min * MINUTE_MS));
        }
    }
    spans.sort_unstable();
    spans.windows(2).all(|pair| pair[0].1 <= pair[1].0)
}

// ---- wire types ----

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShiftSummary {
    /// Set only when at least one plan moved; the key for `undo_plan_batch`.
    pub batch_id: Option<String>,
    pub moved_count: usize,
    /// The new lengths would run past midnight, so no plan was moved.
    pub overflows_midnight: bool,
    /// Who decided the new start times.
    pub source: ShiftSource,
    /// Why the LLM did not decide (a stable code), when it was asked or could have been.
    pub fallback_code: Option<String>,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ShiftSource {
    /// The fixed rule: later plans move by the accumulated length difference.
    Rule,
    /// The LLM's layout, validated.
    Llm,
}

impl ShiftSummary {
    fn empty(source: ShiftSource, overflows_midnight: bool) -> ShiftSummary {
        ShiftSummary {
            batch_id: None,
            moved_count: 0,
            overflows_midnight,
            source,
            fallback_code: None,
        }
    }
}

/// Result of a mode change: the usual outcome plus what happened to today's plans.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModeChange {
    #[serde(flatten)]
    pub outcome: UpdateOutcome,
    /// `None` when the change was rejected.
    pub shift: Option<ShiftSummary>,
}

impl ModeChange {
    pub fn rejected(key: String) -> ModeChange {
        ModeChange {
            outcome: UpdateOutcome::RejectedLocked { key },
            shift: None,
        }
    }

    /// Whether plan data changed, so the UI must refetch and the timer must reconcile.
    pub fn moved_plans(&self) -> bool {
        self.shift.as_ref().is_some_and(|s| s.moved_count > 0)
    }
}

/// What caused the shift; recorded in every history row so undo can restore it.
pub enum ModeTrigger {
    Daily {
        date: String,
        before: StudyLevel,
        after: StudyLevel,
    },
    Config {
        before: ModeConfig,
        after: ModeConfig,
    },
}

impl ModeTrigger {
    fn to_json(&self) -> AppResult<Value> {
        let ser = |c: &ModeConfig| {
            serde_json::to_value(c)
                .map_err(|e| AppError::Internal(format!("mode config serialization failed: {e}")))
        };
        Ok(match self {
            ModeTrigger::Daily {
                date,
                before,
                after,
            } => {
                json!({ "kind": "daily", "date": date, "before": before.as_str(), "after": after.as_str() })
            }
            ModeTrigger::Config { before, after } => {
                json!({ "kind": "config", "before": ser(before)?, "after": ser(after)? })
            }
        })
    }
}

// ---- shift ----

fn exception_json(ex: Option<&PlanException>) -> Value {
    match ex {
        None => Value::Null,
        Some(e) => {
            json!({ "newDate": e.new_date, "newTime": e.new_time, "setsOverride": e.sets_override })
        }
    }
}

fn state_json(
    source_date: &str,
    date: &str,
    time: &str,
    ex: Option<&PlanException>,
    mode: &Value,
) -> Value {
    json!({ "sourceDate": source_date, "date": date, "time": time, "exception": exception_json(ex), "mode": mode })
}

/// The day being shifted: items in start order, and for each the occurrence it came from
/// (`None` for yesterday's plan that spills into today, which is frozen).
struct Day {
    items: Vec<ShiftItem>,
    origin: Vec<Option<Occurrence>>,
    day_end_ms: i64,
}

fn collect_day<Tz: TimeZone>(
    tx: &Connection,
    tz: &Tz,
    today: &str,
    old: &ModeLookup,
    at_ms: i64,
) -> AppResult<Day> {
    let date = parse_date(today)?;
    let new = ModeLookup::load(tx)?;
    let plans = store::list_live_plans(tx)?;
    let exceptions = store::list_plan_exceptions(tx)?;
    let day_start = local_to_ms(tz, date, NaiveTime::MIN)?;
    let day_end_ms = local_to_ms(tz, date + Duration::days(1), NaiveTime::MIN)?;
    let started: HashSet<(i64, String)> =
        store::list_runs_since(tx, &fmt_date(date - Duration::days(1)))?
            .into_iter()
            .map(|run| (run.plan_id, run.date))
            .collect();

    let mut items = Vec::new();
    let mut origin: Vec<Option<Occurrence>> = Vec::new();
    // Yesterday's occurrence that runs past midnight blocks the start of today.
    for o in occurrences_on(tz, &plans, &exceptions, old, date - Duration::days(1))? {
        if o.planned_end_ms > day_start {
            items.push(ShiftItem {
                old_start_ms: o.planned_start_ms,
                old_duration_min: o.duration_min,
                new_duration_min: o.duration_min,
                pending: false,
            });
            origin.push(None);
        }
    }
    for o in occurrences_on(tz, &plans, &exceptions, old, date)? {
        let pending = o.planned_start_ms > at_ms && !started.contains(&(o.plan_id, o.date.clone()));
        let new_duration_min = if pending {
            plan_duration_min(new.config(new.level_for(&o.date))?, o.sets)?
        } else {
            o.duration_min
        };
        items.push(ShiftItem {
            old_start_ms: o.planned_start_ms,
            old_duration_min: o.duration_min,
            new_duration_min,
            pending,
        });
        origin.push(Some(o));
    }
    Ok(Day {
        items,
        origin,
        day_end_ms,
    })
}

/// What the LLM is asked to place: today's items with their new lengths, and the day's end.
/// `apply_mode` writes the new mode inside a transaction that is rolled back afterwards, so the
/// lengths are exactly those the real change will produce. `None` while the lock is held.
pub fn preview_day<Tz: TimeZone>(
    conn: &mut Connection,
    tz: &Tz,
    today: &str,
    at_ms: i64,
    apply_mode: impl FnOnce(&Connection) -> AppResult<()>,
) -> AppResult<Option<(Vec<ShiftItem>, i64)>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if study::lock_held(&tx)? {
        return Ok(None);
    }
    let old = ModeLookup::load(&tx)?;
    apply_mode(&tx)?;
    let day = collect_day(&tx, tz, today, &old, at_ms)?;
    // Dropping the transaction without a commit undoes the trial mode write.
    drop(tx);
    Ok(Some((day.items, day.day_end_ms)))
}

/// Moves today's not-yet-started plans after a mode write and records one history row per moved
/// plan under one batch id. `old` is the mode lookup from before the write; `tx` already holds the
/// new mode and must be the caller's Immediate transaction.
///
/// Without a `proposal` the rule decides (`mode_shift` rows). A valid `proposal` decides the new
/// start times instead and the rows are `llm_adjust`. A proposal that is invalid, or computed for
/// a different day than the one this transaction sees, is ignored and the rule runs;
/// `fallback_code` says which of the two happened.
pub fn shift_after_mode_change_with<Tz: TimeZone>(
    tx: &Connection,
    tz: &Tz,
    today: &str,
    old: &ModeLookup,
    trigger: &ModeTrigger,
    at_ms: i64,
    proposal: Option<&LlmProposal>,
) -> AppResult<ShiftSummary> {
    let Day {
        items,
        origin,
        day_end_ms,
    } = collect_day(tx, tz, today, old, at_ms)?;

    let mut source = ShiftSource::Rule;
    let mut fallback_code = None;
    let mut moves = None;
    if let Some(proposal) = proposal {
        if proposal.items != items {
            fallback_code = Some("llm_day_changed".to_string());
        } else if !validate_layout(&items, &proposal.starts, at_ms, day_end_ms) {
            fallback_code = Some("llm_layout_invalid".to_string());
        } else {
            source = ShiftSource::Llm;
            moves = Some(
                items
                    .iter()
                    .zip(&proposal.starts)
                    .enumerate()
                    .filter(|(_, (item, start))| item.old_start_ms != **start)
                    .map(|(i, (_, start))| (i, *start))
                    .collect::<Vec<_>>(),
            );
        }
    }
    let moves = match moves {
        Some(moves) => moves,
        None => match compute_shift(&items, at_ms, day_end_ms) {
            ShiftPlan::OverflowsMidnight => {
                return Ok(ShiftSummary {
                    fallback_code,
                    ..ShiftSummary::empty(ShiftSource::Rule, true)
                });
            }
            ShiftPlan::Moves(moves) => moves,
        },
    };
    if moves.is_empty() {
        return Ok(ShiftSummary {
            fallback_code,
            ..ShiftSummary::empty(source, false)
        });
    }

    let (action, reason, prefix) = match source {
        ShiftSource::Rule => (Action::ModeShift, ReasonCode::ModeShift, "modeshift"),
        ShiftSource::Llm => (Action::LlmAdjust, ReasonCode::LlmAdjust, "llmadjust"),
    };
    let batch = format!("{prefix}-{}", store::next_plan_history_id(tx)?);
    let mode = trigger.to_json()?;
    for &(index, new_start) in &moves {
        let occ = origin
            .get(index)
            .and_then(Option::as_ref)
            .ok_or_else(|| AppError::Internal("shift moved a frozen occurrence".into()))?;
        let plan = store::get_plan(tx, occ.plan_id)?
            .ok_or_else(|| AppError::Internal("shifted plan is missing".into()))?;
        let existing = store::get_plan_exception(tx, occ.plan_id, &occ.source_date)?;
        let local = tz
            .timestamp_millis_opt(new_start)
            .single()
            .ok_or_else(|| AppError::Internal("shifted start is not a valid local time".into()))?;
        let new_time = local.time().format(TIME_FMT).to_string();
        let sets_override = existing.as_ref().and_then(|e| e.sets_override);

        let reverts =
            occ.source_date == occ.date && new_time == plan.start_time && sets_override.is_none();
        let after_ex = (!reverts).then(|| PlanException {
            plan_id: occ.plan_id,
            date: occ.source_date.clone(),
            kind: ExceptionKind::Moved,
            new_date: Some(occ.date.clone()),
            new_time: Some(new_time.clone()),
            sets_override,
        });
        match &after_ex {
            Some(ex) => store::put_plan_exception(tx, ex)?,
            None => store::delete_plan_exception(tx, occ.plan_id, &occ.source_date)?,
        }
        store::append_plan_history(
            tx,
            &NewPlanHistory {
                plan_id: Some(occ.plan_id),
                at_ms,
                action,
                reason,
                reason_text: None,
                before_json: Some(
                    state_json(
                        &occ.source_date,
                        &occ.date,
                        &occ.start_time,
                        existing.as_ref(),
                        &mode,
                    )
                    .to_string(),
                ),
                after_json: Some(
                    state_json(
                        &occ.source_date,
                        &occ.date,
                        &new_time,
                        after_ex.as_ref(),
                        &mode,
                    )
                    .to_string(),
                ),
                outcome: Outcome::Applied,
                batch_id: Some(&batch),
            },
        )?;
    }
    tracing::info!(event = "plans_mode_shifted", batch_id = %batch, moved = moves.len(), source = ?source);
    Ok(ShiftSummary {
        batch_id: Some(batch),
        moved_count: moves.len(),
        overflows_midnight: false,
        source,
        fallback_code,
    })
}

// ---- undo ----

fn text<'a>(value: &'a Value, key: &str) -> AppResult<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Internal(format!("history row is missing {key}")))
}

fn parse_json(raw: Option<&str>) -> AppResult<Value> {
    let raw = raw.ok_or_else(|| AppError::Internal("history row has no snapshot".into()))?;
    serde_json::from_str(raw)
        .map_err(|e| AppError::Internal(format!("history snapshot is malformed: {e}")))
}

fn stale() -> AppError {
    AppError::Invalid("plans or mode changed after the shift; undo no longer applies".into())
}

fn level_from(value: &Value, key: &str) -> AppResult<StudyLevel> {
    StudyLevel::parse(text(value, key)?)
        .ok_or_else(|| AppError::Internal("history row has an unknown mode".into()))
}

fn config_from(value: &Value, key: &str) -> AppResult<ModeConfig> {
    let raw = value
        .get(key)
        .cloned()
        .ok_or_else(|| AppError::Internal("history row is missing a config".into()))?;
    serde_json::from_value(raw)
        .map_err(|e| AppError::Internal(format!("history config is malformed: {e}")))
}

fn current_config(conn: &Connection, level: StudyLevel) -> AppResult<ModeConfig> {
    store::list_mode_configs(conn)?
        .into_iter()
        .find(|c| c.level == level)
        .ok_or_else(|| {
            AppError::Internal(format!("mode config row missing for {}", level.as_str()))
        })
}

/// One mode-shift row paired with its parsed snapshots.
struct ShiftRow {
    plan_id: i64,
    before: Value,
    after: Value,
}

/// Reverts a whole mode-shift batch: the moved occurrences and the mode change that caused them.
/// Appends `undo` rows (history is never edited). Rejected while the lock is held, recorded.
/// Refused once any moved plan has started or its time has passed, or if anything it touched was
/// changed since.
pub fn undo_plan_batch<Tz: TimeZone>(
    conn: &mut Connection,
    tz: &Tz,
    batch_id: &str,
    at_ms: i64,
) -> AppResult<PlanChange> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let rows = store::list_plan_history_batch(&tx, batch_id)?;
    let shifts: Vec<_> = rows
        .iter()
        .filter(|r| {
            (r.action == Action::ModeShift.as_str() || r.action == Action::LlmAdjust.as_str())
                && r.outcome == "applied"
        })
        .collect();
    let Some(first) = shifts.first() else {
        return Err(AppError::Invalid("no mode shift has that batch id".into()));
    };
    if rows
        .iter()
        .any(|r| r.action == Action::Undo.as_str() && r.outcome == "applied")
    {
        return Err(AppError::Invalid(
            "that mode shift was already undone".into(),
        ));
    }
    let first_plan = first.plan_id;

    let undo_row = |plan_id: Option<i64>,
                    before: Option<String>,
                    after: Option<String>,
                    outcome: Outcome| NewPlanHistory {
        plan_id,
        at_ms,
        action: Action::Undo,
        reason: ReasonCode::Undo,
        reason_text: None,
        before_json: before,
        after_json: after,
        outcome,
        batch_id: Some(batch_id),
    };
    if study::lock_held(&tx)? {
        store::append_plan_history(
            &tx,
            &undo_row(first_plan, None, None, Outcome::RejectedLocked),
        )?;
        tx.commit()?;
        return Ok(PlanChange::rejected(first_plan));
    }

    let mut parsed = Vec::new();
    for row in &shifts {
        parsed.push(ShiftRow {
            plan_id: row
                .plan_id
                .ok_or_else(|| AppError::Internal("mode shift row has no plan".into()))?,
            before: parse_json(row.before_json.as_deref())?,
            after: parse_json(row.after_json.as_deref())?,
        });
    }

    let started: HashSet<(i64, String)> = store::list_runs_since(&tx, "0000-00-00")?
        .into_iter()
        .map(|run| (run.plan_id, run.date))
        .collect();
    for row in &parsed {
        let source_date = text(&row.after, "sourceDate")?;
        let date = text(&row.after, "date")?;
        let plan = store::get_plan(&tx, row.plan_id)?
            .filter(|p| p.deleted_at_ms.is_none())
            .ok_or_else(stale)?;
        let current = store::get_plan_exception(&tx, plan.id, source_date)?;
        if exception_json(current.as_ref()) != row.after["exception"] {
            return Err(stale());
        }
        if started.contains(&(plan.id, date.to_string())) {
            return Err(AppError::Invalid(
                "a moved plan has already started; undo no longer applies".into(),
            ));
        }
        // Both the current and the restored start must still lie ahead, or the plan would be
        // missed the moment it was restored.
        for snapshot in [&row.after, &row.before] {
            let start = local_to_ms(
                tz,
                parse_date(text(snapshot, "date")?)?,
                crate::plan::parse_time(text(snapshot, "time")?)?,
            )?;
            if start <= at_ms {
                return Err(AppError::Invalid(
                    "a moved plan's time has passed; undo no longer applies".into(),
                ));
            }
        }
    }

    // The mode is identical in every row of the batch.
    let lead = parsed
        .first()
        .ok_or_else(|| AppError::Internal("mode shift batch is empty".into()))?;
    let lead_plan = lead.plan_id;
    let mode = &lead.after["mode"];
    enum Restore {
        Daily {
            date: String,
            current: StudyLevel,
            back: StudyLevel,
        },
        Config {
            current: ModeConfig,
            back: ModeConfig,
        },
    }
    let restore = match text(mode, "kind")? {
        "daily" => {
            let date = text(mode, "date")?.to_string();
            let current = store::read_daily_mode(&tx, &date)?.unwrap_or(StudyLevel::DEFAULT);
            if current != level_from(mode, "after")? {
                return Err(stale());
            }
            Restore::Daily {
                date,
                current,
                back: level_from(mode, "before")?,
            }
        }
        "config" => {
            let back = config_from(mode, "before")?;
            let current = current_config(&tx, back.level)?;
            if current != config_from(mode, "after")? {
                return Err(stale());
            }
            Restore::Config { current, back }
        }
        other => return Err(AppError::Internal(format!("unknown mode trigger {other}"))),
    };

    for row in &parsed {
        let source_date = text(&row.after, "sourceDate")?;
        match &row.before["exception"] {
            Value::Null => store::delete_plan_exception(&tx, row.plan_id, source_date)?,
            ex => store::put_plan_exception(
                &tx,
                &PlanException {
                    plan_id: row.plan_id,
                    date: source_date.to_string(),
                    kind: ExceptionKind::Moved,
                    new_date: Some(text(ex, "newDate")?.to_string()),
                    new_time: Some(text(ex, "newTime")?.to_string()),
                    sets_override: ex.get("setsOverride").and_then(Value::as_i64),
                },
            )?,
        }
        store::append_plan_history(
            &tx,
            &undo_row(
                Some(row.plan_id),
                Some(row.after.to_string()),
                Some(row.before.to_string()),
                Outcome::Applied,
            ),
        )?;
    }
    match restore {
        Restore::Daily {
            date,
            current,
            back,
        } => {
            store::put_daily_mode(&tx, &date, back, at_ms)?;
            store::append_history(
                &tx,
                at_ms,
                "studyMode",
                Some(current.as_str()),
                back.as_str(),
                Reason::User,
                Outcome::Applied,
            )?;
        }
        Restore::Config { current, back } => {
            store::put_mode_config(&tx, &back)?;
            let ser = |c: &ModeConfig| {
                serde_json::to_string(c).map_err(|e| {
                    AppError::Internal(format!("mode config serialization failed: {e}"))
                })
            };
            let key = format!("modeConfig.{}", back.level.as_str());
            store::append_history(
                &tx,
                at_ms,
                &key,
                Some(&ser(&current)?),
                &ser(&back)?,
                Reason::User,
                Outcome::Applied,
            )?;
        }
    }
    tx.commit()?;
    tracing::info!(event = "plans_mode_shift_undone", batch_id = %batch_id, plans = parsed.len());
    Ok(PlanChange::applied(lead_plan))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::plan::{CreatePlanInput, create_plan, list_occurrences, plan_history};
    use crate::study::{set_daily_mode, update_mode_config};
    use crate::timer::model::{NewRun, RunStatus};
    use chrono::Utc;

    const TODAY: &str = "2026-11-05";
    const MIDNIGHT: i64 = 24 * 60;

    fn at(h: u32, m: u32) -> i64 {
        Utc.with_ymd_and_hms(2026, 11, 5, h, m, 0)
            .single()
            .expect("valid time")
            .timestamp_millis()
    }

    // ---- pure rule (minutes since local midnight) ----

    fn item(start: i64, old: i64, new: i64, pending: bool) -> ShiftItem {
        ShiftItem {
            old_start_ms: start * MINUTE_MS,
            old_duration_min: old,
            new_duration_min: new,
            pending,
        }
    }

    fn moves(items: &[ShiftItem], now_min: i64) -> ShiftPlan {
        compute_shift(items, now_min * MINUTE_MS, MIDNIGHT * MINUTE_MS)
    }

    fn moved(pairs: &[(usize, i64)]) -> ShiftPlan {
        ShiftPlan::Moves(pairs.iter().map(|&(i, m)| (i, m * MINUTE_MS)).collect())
    }

    #[test]
    fn shift_table() {
        let cases: Vec<(&str, Vec<ShiftItem>, i64, ShiftPlan)> = vec![
            (
                "longer first plan pushes later ones by the difference, keeping gaps",
                vec![item(540, 180, 280, true), item(840, 60, 60, true)],
                480,
                moved(&[(1, 940)]),
            ),
            (
                "shorter plan pulls later ones earlier by the difference",
                vec![item(540, 180, 100, true), item(840, 60, 60, true)],
                480,
                moved(&[(1, 760)]),
            ),
            (
                "differences accumulate over the plans before",
                vec![
                    item(540, 100, 130, true),
                    item(700, 100, 120, true),
                    item(900, 60, 60, true),
                ],
                480,
                moved(&[(1, 730), (2, 950)]),
            ),
            (
                "a plan whose own length changes keeps its start; only its end moves",
                vec![item(540, 100, 160, true)],
                480,
                moved(&[]),
            ),
            (
                "same lengths move nothing",
                vec![item(540, 100, 100, true), item(700, 60, 60, true)],
                480,
                moved(&[]),
            ),
            (
                "started or finished plans neither move nor feed the accumulated difference",
                vec![item(540, 100, 160, false), item(700, 60, 60, true)],
                480,
                moved(&[]),
            ),
            (
                "never pulled before now (defensive clamp), later plans follow the clamp",
                vec![
                    item(600, 300, 100, true),
                    item(900, 60, 60, true),
                    item(1000, 60, 60, true),
                ],
                780,
                moved(&[(0, 780), (1, 880), (2, 980)]),
            ),
            (
                "ending exactly at midnight is allowed",
                vec![item(1200, 100, 240, true)],
                480,
                moved(&[]),
            ),
            (
                "ending after midnight moves nothing at all",
                vec![item(1200, 100, 241, true)],
                480,
                ShiftPlan::OverflowsMidnight,
            ),
            (
                "a later plan pushed past midnight cancels the whole shift",
                vec![item(540, 100, 160, true), item(1380, 60, 60, true)],
                480,
                ShiftPlan::OverflowsMidnight,
            ),
        ];
        for (label, items, now, expected) in cases {
            assert_eq!(moves(&items, now), expected, "{label}");
        }
    }

    // ---- database ----

    fn plan_at(conn: &mut Connection, subject: &str, time: &str, sets: i64) -> i64 {
        let input = CreatePlanInput {
            name: format!("{subject} 공부"),
            subject: subject.into(),
            start_date: TODAY.into(),
            start_time: time.into(),
            sets,
            repeat: None,
        };
        create_plan(conn, &Utc, input, at(0, 1))
            .expect("create")
            .plan_id
            .expect("id")
    }

    fn starts(conn: &Connection) -> Vec<(i64, String, i64)> {
        list_occurrences(conn, &Utc, TODAY, TODAY)
            .expect("list")
            .into_iter()
            .map(|o| (o.plan_id, o.start_time, o.duration_min))
            .collect()
    }

    /// History rows the shift machinery wrote, newest first (plan creation rows are setup).
    fn shift_history(conn: &Connection) -> Vec<crate::plan::PlanHistoryEntry> {
        plan_history(conn, None, 50)
            .expect("history")
            .into_iter()
            .filter(|r| r.action != "create")
            .collect()
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).expect("count")
    }

    fn change(conn: &mut Connection, level: StudyLevel, now: i64) -> ModeChange {
        set_daily_mode(conn, &Utc, TODAY, level, now).expect("mode change")
    }

    fn run_for(plan_id: i64, status: RunStatus) -> NewRun {
        NewRun {
            plan_id,
            date: TODAY.into(),
            planned_start_ms: at(9, 0),
            planned_end_ms: at(10, 55),
            actual_start_ms: Some(at(9, 0)),
            mode_level: "medium".into(),
            study_min: 50,
            break_min: 15,
            long_break_min: None,
            sets_before_long_break: None,
            sets_total: 2,
            status,
            ended_at_ms: None,
            detail_code: None,
        }
    }

    fn medium_config(study: i64) -> ModeConfig {
        ModeConfig {
            level: StudyLevel::Medium,
            study_min: study,
            break_min: 15,
            long_break_min: None,
            sets_before_long_break: None,
        }
    }

    /// medium: P1 09:00 x2 = 115 min, P2 14:00 x1 = 50 min. hard: 190 and 90.
    fn two_plans() -> (Connection, i64, i64) {
        let mut conn = open_in_memory().expect("db");
        let p1 = plan_at(&mut conn, "수학", "09:00", 2);
        let p2 = plan_at(&mut conn, "영어", "14:00", 1);
        (conn, p1, p2)
    }

    #[test]
    fn medium_to_hard_pushes_the_later_plan_by_the_difference_in_one_batch() {
        let (mut conn, p1, p2) = two_plans();
        let result = change(&mut conn, StudyLevel::Hard, at(8, 0));
        let summary = result.shift.expect("summary");
        assert_eq!(
            (summary.moved_count, summary.overflows_midnight),
            (1, false)
        );
        let batch = summary.batch_id.expect("batch id");

        // P1 keeps its start (its own length change moves only its end); P2: 14:00 + (190 - 115).
        assert_eq!(
            starts(&conn),
            [(p1, "09:00".into(), 190), (p2, "15:15".into(), 90)]
        );
        let rows = shift_history(&conn);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(
            (
                row.action.as_str(),
                row.reason_code.as_str(),
                row.outcome.as_str()
            ),
            ("mode_shift", "mode_shift", "applied")
        );
        assert_eq!(
            (row.plan_id, row.batch_id.as_deref()),
            (Some(p2), Some(batch.as_str()))
        );
        assert!(
            row.before_json
                .as_deref()
                .is_some_and(|j| j.contains("\"time\":\"14:00\""))
        );
        assert!(
            row.after_json
                .as_deref()
                .is_some_and(|j| j.contains("\"time\":\"15:15\""))
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM plan_exceptions"), 1);
    }

    #[test]
    fn hard_to_medium_pulls_later_plans_earlier_and_keeps_the_gap() {
        let mut conn = open_in_memory().expect("db");
        change(&mut conn, StudyLevel::Hard, at(7, 0));
        let p1 = plan_at(&mut conn, "수학", "09:00", 2); // hard: 09:00-12:10
        let p2 = plan_at(&mut conn, "영어", "13:00", 1); // 50 min gap after P1
        let p3 = plan_at(&mut conn, "국어", "16:00", 1);

        change(&mut conn, StudyLevel::Medium, at(8, 0));
        // P1 shrinks 190 -> 115 and P2 90 -> 50, so P2 moves 75 earlier and P3 115 earlier; every gap is kept.
        assert_eq!(
            starts(&conn),
            [
                (p1, "09:00".into(), 115),
                (p2, "11:45".into(), 50),
                (p3, "14:05".into(), 50)
            ]
        );
    }

    #[test]
    fn started_plans_are_frozen_and_do_not_feed_the_difference() {
        let (mut conn, p1, p2) = two_plans();
        // P1 already has a run; only its length view changes.
        crate::store::insert_run(&conn, &run_for(p1, RunStatus::Completed)).expect("run");
        let result = change(&mut conn, StudyLevel::Hard, at(8, 0));
        assert_eq!(result.shift.expect("summary").moved_count, 0);
        assert_eq!(starts(&conn)[1], (p2, "14:00".into(), 90));
        assert!(shift_history(&conn).is_empty());
    }

    #[test]
    fn a_plan_whose_start_has_passed_without_a_run_is_not_moved() {
        let (mut conn, p1, p2) = two_plans();
        // 09:30: P1 should have started, so it is not "not yet started".
        let result = change(&mut conn, StudyLevel::Hard, at(9, 30));
        assert_eq!(result.shift.expect("summary").moved_count, 0);
        assert_eq!(
            starts(&conn),
            [(p1, "09:00".into(), 190), (p2, "14:00".into(), 90)]
        );
    }

    #[test]
    fn overflow_past_midnight_moves_nothing_and_warns_but_the_mode_still_changes() {
        let mut conn = open_in_memory().expect("db");
        let p1 = plan_at(&mut conn, "수학", "09:00", 2);
        let p2 = plan_at(&mut conn, "영어", "22:00", 1); // medium 50 min -> ends 22:50
        let result = change(&mut conn, StudyLevel::Hard, at(8, 0)); // P2 would run 23:15-00:45
        let summary = result.shift.expect("summary");
        assert!(summary.overflows_midnight);
        assert_eq!((summary.moved_count, summary.batch_id), (0, None));
        assert_eq!(
            starts(&conn)[1],
            (p2, "22:00".into(), 90),
            "start untouched"
        );
        assert_eq!(
            crate::store::read_daily_mode(&conn, TODAY).expect("mode"),
            Some(StudyLevel::Hard)
        );
        assert!(shift_history(&conn).is_empty());
        let _ = p1;
    }

    #[test]
    fn undo_restores_plans_and_mode_and_appends_history() {
        let (mut conn, p1, p2) = two_plans();
        let before = starts(&conn);
        let batch = change(&mut conn, StudyLevel::Hard, at(8, 0))
            .shift
            .expect("s")
            .batch_id
            .expect("batch");

        let undone = undo_plan_batch(&mut conn, &Utc, &batch, at(8, 5)).expect("undo");
        assert!(undone.is_applied());
        assert_eq!(starts(&conn), before);
        assert_eq!(
            crate::store::read_daily_mode(&conn, TODAY).expect("mode"),
            Some(StudyLevel::Medium)
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM plan_exceptions"), 0);

        let rows = shift_history(&conn);
        let kinds: Vec<_> = rows
            .iter()
            .map(|r| {
                (
                    r.action.as_str(),
                    r.reason_code.as_str(),
                    r.batch_id.clone(),
                )
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("undo", "undo", Some(batch.clone())),
                ("mode_shift", "mode_shift", Some(batch.clone())),
            ],
            "the original row stays; undo is an added row"
        );
        let settings = crate::settings::read_history(&conn, 1).expect("settings history");
        assert_eq!(
            (settings[0].key.as_str(), settings[0].after.as_str()),
            ("studyMode", "medium")
        );

        let again = undo_plan_batch(&mut conn, &Utc, &batch, at(8, 6));
        assert!(
            matches!(again, Err(AppError::Invalid(_))),
            "undoing twice is refused"
        );
        let _ = (p1, p2);
    }

    #[test]
    fn undo_is_rejected_and_recorded_while_locked() {
        let (mut conn, _p1, p2) = two_plans();
        let batch = change(&mut conn, StudyLevel::Hard, at(8, 0))
            .shift
            .expect("s")
            .batch_id
            .expect("batch");
        crate::testutil::set_lock_for_test(&conn, true);
        let result = undo_plan_batch(&mut conn, &Utc, &batch, at(8, 5)).expect("attempt");
        assert!(!result.is_applied());
        assert_eq!(
            starts(&conn)[1],
            (p2, "15:15".into(), 90),
            "plans unchanged"
        );
        assert_eq!(
            crate::store::read_daily_mode(&conn, TODAY).expect("mode"),
            Some(StudyLevel::Hard)
        );
        let latest = &shift_history(&conn)[0];
        assert_eq!(
            (latest.action.as_str(), latest.outcome.as_str()),
            ("undo", "rejected_locked")
        );

        // A rejected attempt does not use up the undo.
        crate::testutil::set_lock_for_test(&conn, false);
        assert!(
            undo_plan_batch(&mut conn, &Utc, &batch, at(8, 6))
                .expect("undo")
                .is_applied()
        );
    }

    #[test]
    fn undo_refuses_once_a_moved_plan_started_or_was_edited() {
        let (mut conn, _p1, p2) = two_plans();
        let batch = change(&mut conn, StudyLevel::Hard, at(8, 0))
            .shift
            .expect("s")
            .batch_id
            .expect("batch");
        assert!(
            undo_plan_batch(&mut conn, &Utc, &batch, at(15, 20)).is_err(),
            "moved plan's time passed"
        );

        crate::store::insert_run(
            &conn,
            &NewRun {
                date: TODAY.into(),
                ..run_for(p2, RunStatus::Active)
            },
        )
        .expect("run");
        assert!(
            undo_plan_batch(&mut conn, &Utc, &batch, at(8, 5)).is_err(),
            "moved plan has a run"
        );
        assert_eq!(starts(&conn)[1].1, "15:15");

        let (mut conn, _p1, p2) = two_plans();
        let batch = change(&mut conn, StudyLevel::Hard, at(8, 0))
            .shift
            .expect("s")
            .batch_id
            .expect("batch");
        conn.execute(
            "UPDATE plan_exceptions SET new_time = '16:00' WHERE plan_id = ?1",
            [p2],
        )
        .expect("edit");
        assert!(
            undo_plan_batch(&mut conn, &Utc, &batch, at(8, 5)).is_err(),
            "edited since the shift"
        );
        assert_eq!(
            crate::store::read_daily_mode(&conn, TODAY).expect("mode"),
            Some(StudyLevel::Hard)
        );
    }

    #[test]
    fn mode_change_while_locked_is_rejected_recorded_and_moves_nothing() {
        let (mut conn, _p1, p2) = two_plans();
        crate::testutil::set_lock_for_test(&conn, true); // study or a break between sets
        let result = change(&mut conn, StudyLevel::Hard, at(8, 0));
        assert_eq!(result, ModeChange::rejected("studyMode".into()));
        let edit = update_mode_config(&mut conn, &Utc, TODAY, medium_config(60), at(8, 0))
            .expect("attempt");
        assert_eq!(edit, ModeChange::rejected("modeConfig.medium".into()));
        assert_eq!(starts(&conn)[1], (p2, "14:00".into(), 50));
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM plan_exceptions"), 0);
        assert!(shift_history(&conn).is_empty());
        let outcomes: Vec<_> = crate::settings::read_history(&conn, 2)
            .expect("h")
            .into_iter()
            .map(|e| e.outcome)
            .collect();
        assert_eq!(outcomes, ["rejected_locked", "rejected_locked"]);
    }

    #[test]
    fn a_failure_in_the_shift_leaves_nothing_applied() {
        let (mut conn, p1, p2) = two_plans();
        conn.execute_batch(
            "CREATE TRIGGER fail_shift BEFORE INSERT ON plan_history WHEN NEW.action = 'mode_shift'
             BEGIN SELECT RAISE(ABORT, 'forced'); END;",
        )
        .expect("trigger");
        assert!(set_daily_mode(&mut conn, &Utc, TODAY, StudyLevel::Hard, at(8, 0)).is_err());
        assert_eq!(
            crate::store::read_daily_mode(&conn, TODAY).expect("mode"),
            None
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM plan_exceptions"), 0);
        assert!(
            crate::settings::read_history(&conn, 10)
                .expect("h")
                .is_empty()
        );
        assert_eq!(
            starts(&conn),
            [(p1, "09:00".into(), 115), (p2, "14:00".into(), 50)]
        );
    }

    #[test]
    fn mode_length_edit_uses_the_same_shift_and_never_rejects_for_overlap() {
        let (mut conn, p1, p2) = two_plans();
        // Medium study 50 -> 160: P1 grows 115 -> 335 min and would end past P2's 14:00 start.
        let result =
            update_mode_config(&mut conn, &Utc, TODAY, medium_config(160), at(8, 0)).expect("edit");
        assert!(matches!(result.outcome, UpdateOutcome::Applied { .. }));
        let summary = result.shift.expect("summary");
        assert_eq!(summary.moved_count, 1);
        // P2: 14:00 + (335 - 115) = 17:40, after P1's new end 14:35 with the original gap.
        assert_eq!(
            starts(&conn),
            [(p1, "09:00".into(), 335), (p2, "17:40".into(), 160)]
        );

        // Undo puts the old lengths back too.
        let batch = summary.batch_id.expect("batch");
        undo_plan_batch(&mut conn, &Utc, &batch, at(8, 1)).expect("undo");
        assert_eq!(
            starts(&conn),
            [(p1, "09:00".into(), 115), (p2, "14:00".into(), 50)]
        );
        let configs = crate::store::list_mode_configs(&conn).expect("configs");
        assert_eq!(
            configs
                .iter()
                .find(|c| c.level == StudyLevel::Medium)
                .expect("row")
                .study_min,
            50
        );
    }

    #[test]
    fn editing_a_level_that_today_does_not_use_moves_nothing() {
        let (mut conn, _p1, _p2) = two_plans();
        let easy = ModeConfig {
            level: StudyLevel::Easy,
            study_min: 30,
            break_min: 5,
            long_break_min: Some(20),
            sets_before_long_break: Some(4),
        };
        let result = update_mode_config(&mut conn, &Utc, TODAY, easy, at(8, 0)).expect("edit");
        assert_eq!(result.shift.expect("summary").moved_count, 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM plan_exceptions"), 0);
    }

    #[test]
    fn a_shifted_moved_occurrence_keeps_its_exception_and_undo_restores_it() {
        let (mut conn, _p1, p2) = two_plans();
        // Postpone P2 to 15:00 first (a user move), then change the mode.
        let input = crate::plan::UpdatePlanInput {
            plan_id: p2,
            scope: crate::plan::Scope::Occurrence,
            date: TODAY.into(),
            new_date: None,
            new_time: Some("15:00".into()),
            new_sets: Some(2),
            repeat: None,
            reason_code: ReasonCode::Postponed,
            reason_text: None,
        };
        crate::plan::update_plan(&mut conn, &Utc, input, at(7, 0)).expect("update");
        let batch = change(&mut conn, StudyLevel::Hard, at(8, 0))
            .shift
            .expect("s")
            .batch_id
            .expect("batch");
        // P2 (2 sets) moves by P1's difference (190 - 115).
        assert_eq!(starts(&conn)[1], (p2, "16:15".into(), 190));
        undo_plan_batch(&mut conn, &Utc, &batch, at(8, 1)).expect("undo");
        assert_eq!(
            starts(&conn)[1],
            (p2, "15:00".into(), 115),
            "sets override and user move survive"
        );
    }

    #[test]
    fn mode_change_serialization_matches_shared_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/api/fixtures/mode-change.json"
        );
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let applied = |key: &str, shift: ShiftSummary| ModeChange {
            outcome: UpdateOutcome::Applied { key: key.into() },
            shift: Some(shift),
        };
        let actual = serde_json::to_value([
            applied(
                "studyMode",
                ShiftSummary {
                    batch_id: Some("modeshift-3".into()),
                    moved_count: 1,
                    overflows_midnight: false,
                    source: ShiftSource::Rule,
                    fallback_code: None,
                },
            ),
            applied(
                "modeConfig.medium",
                ShiftSummary {
                    batch_id: None,
                    moved_count: 0,
                    overflows_midnight: true,
                    source: ShiftSource::Rule,
                    fallback_code: None,
                },
            ),
            applied(
                "studyMode",
                ShiftSummary {
                    batch_id: Some("llmadjust-9".into()),
                    moved_count: 2,
                    overflows_midnight: false,
                    source: ShiftSource::Llm,
                    fallback_code: None,
                },
            ),
            applied(
                "studyMode",
                ShiftSummary {
                    batch_id: Some("modeshift-4".into()),
                    moved_count: 1,
                    overflows_midnight: false,
                    source: ShiftSource::Rule,
                    fallback_code: Some("llm_disabled".into()),
                },
            ),
            ModeChange::rejected("studyMode".into()),
        ])
        .expect("serialize");
        assert_eq!(actual, expected);
    }
}
