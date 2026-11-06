//! LLM adjustment of the rest of today's plans when the study mode changes (feature 17, PLAN 17).
//!
//! The flow around a mode change:
//! 1. `preview`: a rolled-back trial of the mode write yields today's plans with their new lengths.
//!    Nothing is changed and no lock is held afterwards.
//! 2. `propose`: the LLM is asked, with the database unlocked, to place the plans that have not
//!    started. Only minute offsets and lengths are sent (no subject, no plan name). The answer must
//!    be strict JSON and must pass `plan_shift::validate_layout`: no overlaps, inside the day,
//!    frozen plans untouched, the same plans with the same lengths (so the set count is unchanged).
//! 3. The real change runs in one Immediate transaction (`study::*_with`): mode write, moves, and
//!    `llm_adjust` history rows commit together. If the day changed since the preview, or the
//!    layout is not valid, the transaction uses the rule shift instead.
//!
//! The LLM being off, unconfigured, over budget, failing, or answering badly always ends in the
//! rule-based shift; the result says which one decided (`ShiftSource`) and why the LLM did not.
//! The batch is undone with `plan_shift::undo_plan_batch`, exactly once.

use std::sync::Mutex;

use chrono::{NaiveTime, TimeZone};
use rusqlite::Connection;
use serde::Deserialize;

use crate::commands::lock_db;
use crate::error::AppResult;
use crate::llm::egress::{AdjustItem, Prompt, MAX_ADJUST_ITEMS};
use crate::llm::service::LlmService;
use crate::llm::{LlmRequest, Purpose};
use crate::plan::{local_to_ms, parse_date};
use crate::plan_shift::{validate_layout, LlmProposal, ModeChange, ShiftItem, ShiftSource};
use crate::study::{self, ModeConfig, StudyLevel};

const MINUTE_MS: i64 = 60_000;
const ADJUST_MAX_TOKENS: u32 = 512;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLayout {
    starts: Vec<RawStart>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStart {
    index: usize,
    start_min: i64,
}

/// Start times (ms) for every item from the model's answer: items it does not mention keep their
/// start. `None` when the answer is not the documented shape, repeats an index, names an item that
/// does not exist, or moves one that has started.
fn parse_layout(answer: &str, items: &[ShiftItem], day_start_ms: i64) -> Option<Vec<i64>> {
    let raw: RawLayout = serde_json::from_str(crate::wiki::ingest::json_object(answer)?).ok()?;
    let mut starts: Vec<i64> = items.iter().map(|i| i.old_start_ms).collect();
    let mut seen = vec![false; items.len()];
    for entry in raw.starts {
        let item = items.get(entry.index)?;
        if !item.pending || std::mem::replace(seen.get_mut(entry.index)?, true) {
            return None;
        }
        starts[entry.index] = day_start_ms.checked_add(entry.start_min.checked_mul(MINUTE_MS)?)?;
    }
    Some(starts)
}

fn ceil_minutes(ms: i64) -> i64 {
    (ms + MINUTE_MS - 1).div_euclid(MINUTE_MS)
}

/// Asks the model for a layout. `Err(code)` is the stable reason the rule runs instead.
fn propose(llm: &LlmService, items: &[ShiftItem], day_start_ms: i64, day_end_ms: i64, now_ms: i64) -> Result<LlmProposal, String> {
    if !items.iter().any(|i| i.pending && i.new_duration_min != i.old_duration_min) {
        return Err("no_length_change".into());
    }
    if items.len() > MAX_ADJUST_ITEMS {
        return Err("too_many_plans".into());
    }
    let wire: Vec<AdjustItem> = items
        .iter()
        .enumerate()
        .map(|(index, i)| AdjustItem {
            index,
            start_min: (i.old_start_ms - day_start_ms).div_euclid(MINUTE_MS),
            old_min: i.old_duration_min,
            new_min: i.new_duration_min,
            pending: i.pending,
        })
        .collect();
    let prompt = Prompt::plan_adjust(wire, ceil_minutes(now_ms - day_start_ms), (day_end_ms - day_start_ms) / MINUTE_MS);
    let request = LlmRequest { purpose: Purpose::PlanAdjust, prompt, max_tokens: ADJUST_MAX_TOKENS };
    let reply = llm.complete(&request).map_err(|err| err.code().to_string())?;
    let starts = parse_layout(&reply.text, items, day_start_ms).ok_or_else(|| "llm_bad_output".to_string())?;
    if !validate_layout(items, &starts, now_ms, day_end_ms) {
        return Err("llm_layout_invalid".into());
    }
    Ok(LlmProposal { items: items.to_vec(), starts })
}

type Preview = Option<(Vec<ShiftItem>, i64)>;

/// Runs the preview -> propose -> transaction sequence. `apply` performs the real change with the
/// proposal, if any.
fn adjust<Tz: TimeZone>(
    db: &Mutex<Connection>,
    llm: &LlmService,
    tz: &Tz,
    today: &str,
    at_ms: i64,
    preview: impl FnOnce(&mut Connection) -> AppResult<Preview>,
    apply: impl FnOnce(&mut Connection, Option<&LlmProposal>) -> AppResult<ModeChange>,
) -> AppResult<ModeChange> {
    let previewed = preview(&mut *lock_db(db)?)?;
    let (proposal, why_not) = match previewed {
        // Locked: the change will be rejected, so nothing is asked.
        None => (None, None),
        Some((items, day_end_ms)) => {
            let day_start_ms = local_to_ms(tz, parse_date(today)?, NaiveTime::MIN)?;
            match propose(llm, &items, day_start_ms, day_end_ms, at_ms) {
                Ok(proposal) => (Some(proposal), None),
                Err(code) => (None, Some(code)),
            }
        }
    };
    let mut change = apply(&mut *lock_db(db)?, proposal.as_ref())?;
    if let Some(shift) = change.shift.as_mut() {
        if shift.source == ShiftSource::Rule && shift.fallback_code.is_none() {
            shift.fallback_code = why_not;
        }
    }
    Ok(change)
}

/// `study::set_daily_mode` with the LLM adjustment around it.
pub fn change_daily_mode<Tz: TimeZone>(
    db: &Mutex<Connection>,
    llm: &LlmService,
    tz: &Tz,
    today: &str,
    level: StudyLevel,
    at_ms: i64,
) -> AppResult<ModeChange> {
    adjust(
        db,
        llm,
        tz,
        today,
        at_ms,
        |conn| study::preview_daily_mode(conn, tz, today, level, at_ms),
        |conn, proposal| study::set_daily_mode_with(conn, tz, today, level, at_ms, proposal),
    )
}

/// `study::update_mode_config` with the LLM adjustment around it.
pub fn change_mode_config<Tz: TimeZone>(
    db: &Mutex<Connection>,
    llm: &LlmService,
    tz: &Tz,
    today: &str,
    config: ModeConfig,
    at_ms: i64,
) -> AppResult<ModeChange> {
    adjust(
        db,
        llm,
        tz,
        today,
        at_ms,
        |conn| study::preview_mode_config(conn, tz, today, &config, at_ms),
        |conn, proposal| study::update_mode_config_with(conn, tz, today, config.clone(), at_ms, proposal),
    )
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::llm::config::{update_setting, LlmSettingKey};
    use crate::llm::mock::MockStep;
    use crate::llm::service::tests::{mock_harness, Harness};
    use crate::llm::LlmError;
    use crate::plan::{create_plan, list_occurrences, plan_history, CreatePlanInput};
    use crate::plan_shift::undo_plan_batch;
    use crate::settings::{SettingValue, UpdateOutcome};
    use crate::testutil::set_lock_for_test;

    const TODAY: &str = "2026-11-05";

    fn at(h: u32, m: u32) -> i64 {
        Utc.with_ymd_and_hms(2026, 11, 5, h, m, 0).single().expect("valid time").timestamp_millis()
    }

    fn add_plan(conn: &mut Connection, subject: &str, time: &str, sets: i64) -> i64 {
        let input = CreatePlanInput {
            name: format!("{subject} 공부"),
            subject: subject.into(),
            start_date: TODAY.into(),
            start_time: time.into(),
            sets,
            repeat: None,
        };
        create_plan(conn, &Utc, input, at(0, 1)).expect("create").plan_id.expect("id")
    }

    /// medium: P1 09:00 x2 = 115 min, P2 14:00 x1 = 50 min. Hard: 190 and 90.
    fn rig() -> (Harness, i64, i64) {
        let h = mock_harness();
        let (p1, p2) = {
            let mut conn = h.db.lock().expect("db");
            (add_plan(&mut conn, "수학", "09:00", 2), add_plan(&mut conn, "영어", "14:00", 1))
        };
        (h, p1, p2)
    }

    fn starts(h: &Harness) -> Vec<(i64, String, i64)> {
        let conn = h.db.lock().expect("db");
        list_occurrences(&conn, &Utc, TODAY, TODAY).expect("list").into_iter().map(|o| (o.plan_id, o.start_time, o.duration_min)).collect()
    }

    fn history(h: &Harness) -> Vec<(String, String, Option<String>)> {
        let conn = h.db.lock().expect("db");
        plan_history(&conn, None, 50)
            .expect("history")
            .into_iter()
            .filter(|r| r.action != "create")
            .map(|r| (r.action, r.reason_code, r.batch_id))
            .collect()
    }

    fn to_hard(h: &Harness) -> ModeChange {
        change_daily_mode(&h.db, &h.service, &Utc, TODAY, StudyLevel::Hard, at(8, 0)).expect("change")
    }

    fn script(h: &Harness, answer: &str) {
        h.remote.mock_llm().push(MockStep::Text(answer.into()));
    }

    #[test]
    fn a_valid_model_layout_is_applied_as_llm_adjust_in_one_batch_and_undone_once() {
        let (h, p1, p2) = rig();
        let before = starts(&h);
        let change = to_hard(&h);
        let shift = change.shift.expect("summary");
        assert_eq!((shift.source, shift.moved_count, shift.fallback_code.clone()), (ShiftSource::Llm, 1, None));
        let batch = shift.batch_id.expect("batch");
        assert!(batch.starts_with("llmadjust-"));
        // The mock answers like the rule: P2 14:00 + (190 - 115) minutes.
        assert_eq!(starts(&h), [(p1, "09:00".into(), 190), (p2, "15:15".into(), 90)]);
        assert_eq!(history(&h), [("llm_adjust".to_string(), "llm_adjust".to_string(), Some(batch.clone()))]);
        assert_eq!(h.remote.mock_llm().calls().len(), 1);
        assert_eq!(h.remote.mock_llm().calls()[0].purpose, Purpose::PlanAdjust);

        let undone = undo_plan_batch(&mut h.db.lock().expect("db"), &Utc, &batch, at(8, 5)).expect("undo");
        assert!(undone.is_applied());
        assert_eq!(starts(&h), before);
        assert_eq!(crate::store::read_daily_mode(&h.db.lock().expect("db"), TODAY).expect("mode"), Some(StudyLevel::Medium));
        assert!(undo_plan_batch(&mut h.db.lock().expect("db"), &Utc, &batch, at(8, 6)).is_err(), "undo works once");
    }

    #[test]
    fn the_model_may_choose_a_different_valid_layout_than_the_rule() {
        let (h, p1, p2) = rig();
        // Rule would say 15:15 (915 min); the model puts P2 at 16:00 (960).
        script(&h, "{\"starts\":[{\"index\":1,\"start_min\":960}]}");
        let change = to_hard(&h);
        assert_eq!(change.shift.expect("summary").source, ShiftSource::Llm);
        assert_eq!(starts(&h), [(p1, "09:00".into(), 190), (p2, "16:00".into(), 90)]);
    }

    #[test]
    fn every_kind_of_bad_answer_falls_back_to_the_rule_and_says_why() {
        let bad: [(&str, &str, &str); 8] = [
            ("overlap with the first plan", "{\"starts\":[{\"index\":1,\"start_min\":600}]}", "llm_layout_invalid"),
            ("before now", "{\"starts\":[{\"index\":1,\"start_min\":300}]}", "llm_layout_invalid"),
            ("past the end of the day", "{\"starts\":[{\"index\":1,\"start_min\":1400}]}", "llm_layout_invalid"),
            ("moves a plan that is not pending", "{\"starts\":[{\"index\":9,\"start_min\":1000}]}", "llm_bad_output"),
            ("repeated index", "{\"starts\":[{\"index\":1,\"start_min\":960},{\"index\":1,\"start_min\":970}]}", "llm_bad_output"),
            ("unknown field", "{\"starts\":[],\"note\":\"x\"}", "llm_bad_output"),
            ("not json", "죄송합니다, 할 수 없어요", "llm_bad_output"),
            ("a plan left overlapping its neighbour", "{\"starts\":[{\"index\":0,\"start_min\":800}]}", "llm_layout_invalid"),
        ];
        for (label, answer, code) in bad {
            let (h, p1, p2) = rig();
            script(&h, answer);
            let change = to_hard(&h);
            let shift = change.shift.expect("summary");
            assert_eq!((shift.source, shift.fallback_code.as_deref()), (ShiftSource::Rule, Some(code)), "{label}");
            assert_eq!(starts(&h), [(p1, "09:00".into(), 190), (p2, "15:15".into(), 90)], "{label}: the rule result");
            assert_eq!(history(&h)[0].0, "mode_shift", "{label}");
        }
    }

    #[test]
    fn a_frozen_plan_cannot_be_moved_by_the_model() {
        let (h, p1, p2) = rig();
        crate::store::insert_run(
            &h.db.lock().expect("db"),
            &crate::timer::model::NewRun {
                plan_id: p1,
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
                status: crate::timer::model::RunStatus::Completed,
                ended_at_ms: None,
                detail_code: None,
            },
        )
        .expect("run");
        script(&h, "{\"starts\":[{\"index\":0,\"start_min\":700}]}");
        let change = to_hard(&h);
        let shift = change.shift.expect("summary");
        assert_eq!((shift.source, shift.fallback_code.as_deref()), (ShiftSource::Rule, Some("llm_bad_output")));
        assert_eq!(starts(&h)[0], (p1, "09:00".into(), 190));
        assert_eq!(starts(&h)[1], (p2, "14:00".into(), 90));
    }

    #[test]
    fn without_a_usable_llm_the_rule_shift_runs_and_the_reason_is_reported() {
        for (label, prepare, code) in [
            ("failure", Some(MockStep::Fail(LlmError::Overloaded)), "llm_overloaded"),
            ("switched off", None, "llm_disabled"),
        ] {
            let (h, p1, p2) = rig();
            match prepare {
                Some(step) => h.remote.mock_llm().push(step),
                None => {
                    update_setting(&mut h.db.lock().expect("db"), LlmSettingKey::Enabled, SettingValue::Bool(false), 3).expect("off");
                }
            }
            let shift = to_hard(&h).shift.expect("summary");
            assert_eq!((shift.source, shift.moved_count, shift.fallback_code.as_deref()), (ShiftSource::Rule, 1, Some(code)), "{label}");
            assert_eq!(starts(&h), [(p1, "09:00".into(), 190), (p2, "15:15".into(), 90)], "{label}");
            assert_eq!(history(&h)[0].1, "mode_shift", "{label}");
        }
    }

    #[test]
    fn a_locked_change_is_rejected_without_asking_the_model() {
        let (h, ..) = rig();
        set_lock_for_test(&h.db.lock().expect("db"), true);
        let change = to_hard(&h);
        assert!(matches!(change.outcome, UpdateOutcome::RejectedLocked { .. }));
        assert!(change.shift.is_none());
        assert_eq!(h.remote.mock_llm().call_count(), 0);
        assert_eq!(crate::store::read_daily_mode(&h.db.lock().expect("db"), TODAY).expect("mode"), None, "nothing was written");
    }

    #[test]
    fn nothing_is_asked_when_no_plan_changes_length() {
        let (h, ..) = rig();
        // Editing a level today does not use changes no length.
        let config = ModeConfig { level: StudyLevel::Hard, study_min: 80, break_min: 10, long_break_min: None, sets_before_long_break: None };
        let change = change_mode_config(&h.db, &h.service, &Utc, TODAY, config, at(8, 0)).expect("change");
        let shift = change.shift.expect("summary");
        assert_eq!((shift.moved_count, shift.source, shift.fallback_code.as_deref()), (0, ShiftSource::Rule, Some("no_length_change")));
        assert_eq!(h.remote.mock_llm().call_count(), 0);
    }

    #[test]
    fn a_mode_config_edit_is_adjusted_like_a_mode_switch() {
        let (h, p1, p2) = rig();
        let config = ModeConfig { level: StudyLevel::Medium, study_min: 70, break_min: 15, long_break_min: None, sets_before_long_break: None };
        let change = change_mode_config(&h.db, &h.service, &Utc, TODAY, config, at(8, 0)).expect("change");
        let shift = change.shift.expect("summary");
        assert_eq!((shift.source, shift.moved_count), (ShiftSource::Llm, 1));
        // P1: 2 x 70 + 15 = 155 (was 115); P2 moves 40 minutes later; its own length 70.
        assert_eq!(starts(&h), [(p1, "09:00".into(), 155), (p2, "14:40".into(), 70)]);
        assert_eq!(history(&h)[0].0, "llm_adjust");
    }

    #[test]
    fn a_proposal_for_a_different_day_is_ignored_inside_the_transaction() {
        let (h, p1, p2) = rig();
        let (items, day_end) = {
            let mut conn = h.db.lock().expect("db");
            study::preview_daily_mode(&mut conn, &Utc, TODAY, StudyLevel::Hard, at(8, 0)).expect("preview").expect("items")
        };
        assert_eq!(crate::store::read_daily_mode(&h.db.lock().expect("db"), TODAY).expect("mode"), None, "a preview writes nothing");
        let mut stale_items = items.clone();
        stale_items[1].old_start_ms += MINUTE_MS;
        let stale = LlmProposal { items: stale_items, starts: items.iter().map(|i| i.old_start_ms).collect() };
        let _ = day_end;
        let change = study::set_daily_mode_with(&mut h.db.lock().expect("db"), &Utc, TODAY, StudyLevel::Hard, at(8, 0), Some(&stale)).expect("change");
        let shift = change.shift.expect("summary");
        assert_eq!((shift.source, shift.fallback_code.as_deref()), (ShiftSource::Rule, Some("llm_day_changed")));
        assert_eq!(starts(&h), [(p1, "09:00".into(), 190), (p2, "15:15".into(), 90)]);
    }

    #[test]
    fn the_request_carries_minutes_and_lengths_only() {
        let (h, ..) = rig();
        to_hard(&h);
        let call = &h.remote.mock_llm().calls()[0];
        for forbidden in ["수학", "영어", "수학 공부", TODAY] {
            assert!(!call.user.contains(forbidden) && !call.system.contains(forbidden), "leaked `{forbidden}`: {}", call.user);
        }
        assert!(call.user.contains("index: 1, start_min: 840, old_min: 50, new_min: 90, pending: true"), "{}", call.user);
    }
}
