//! Points and levels (stage 1-5). Points are never stored as a number that could drift: the
//! append-only `point_ledger` is the truth, and [`settle`] derives every payable event from the
//! recorded study (`focus_minutes`, `block_events`, `plan_runs`, `study_passes`) and posts the
//! ones the ledger does not have yet. Each event has a stable `ref_key`, so running `settle` any
//! number of times, in any order, after a restart, or over the same data again pays it once.
//!
//! The level counts points *earned* (positive deltas), not the balance, so spending points in a
//! later stage cannot lower it.

pub mod rewards;
#[cfg(test)]
mod tests;

use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;

use crate::error::AppResult;
use crate::focus::model::MinuteDelta;
use crate::store;
use crate::timer::model::RunStatus;
use rewards::Stage;

const MINUTE_MS: i64 = 60_000;

/// One ledger row as the UI sees it.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntry {
    pub id: i64,
    pub at_ms: i64,
    pub delta: i64,
    pub reason_code: String,
    pub ref_key: String,
}

/// Balance, level, and growth stage.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PointsStatus {
    /// Sum of the ledger.
    pub balance: i64,
    /// Sum of the positive rows: what the level is computed from.
    pub earned: i64,
    pub level: i64,
    pub stage: Stage,
    pub stage_label: String,
    /// Total earned points at which the next level starts; `None` at the top level.
    pub next_level_at: Option<i64>,
}

pub fn status_for(balance: i64, earned: i64) -> PointsStatus {
    let level = rewards::level_for(earned);
    let stage = rewards::stage_for_level(level);
    PointsStatus {
        balance,
        earned,
        level,
        stage,
        stage_label: stage.label().into(),
        next_level_at: rewards::next_level_at(level),
    }
}

pub fn status(conn: &Connection) -> AppResult<PointsStatus> {
    let (balance, earned) = store::ledger_sums(conn)?;
    Ok(status_for(balance, earned))
}

/// A row to post.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Payable {
    at_ms: i64,
    delta: i64,
    reason: &'static str,
    ref_key: String,
}

/// A finished reward window: 30 minutes of recognized study.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub index: i64,
    pub end_ms: i64,
    /// No away time, no zoning out, and no distraction attempt inside it.
    pub clean: bool,
}

/// Splits a run's minute buckets into reward windows. Recognized time accumulates in time order
/// (a break between sets does not reset it); a window closes when it holds 30 recognized minutes.
/// It is clean only if none of its minutes has away or idle time or a distraction attempt, so
/// an interrupted stretch is lost, not postponed. Only minutes that ended before `now` count: a
/// minute still being written could change after the window was paid. Overshoot past 30 minutes
/// inside the closing bucket is dropped.
pub fn windows(minutes: &[(i64, MinuteDelta)], attempts: &[i64], now_ms: i64) -> Vec<Window> {
    let mut out = Vec::new();
    let mut credited = 0;
    let mut dirty = false;
    for (start, minute) in minutes {
        let end = start + MINUTE_MS;
        if end > now_ms {
            break;
        }
        credited += minute.credited_ms;
        dirty |= minute.away_ms > 0
            || minute.idle_ms > 0
            || attempts.iter().any(|at| (*start..end).contains(at));
        if credited >= rewards::WINDOW_MS {
            out.push(Window {
                index: i64::try_from(out.len()).unwrap_or(i64::MAX),
                end_ms: end,
                clean: !dirty,
            });
            credited = 0;
            dirty = false;
        }
    }
    out
}

fn payables(conn: &Connection, now_ms: i64) -> AppResult<Vec<Payable>> {
    let mut out = Vec::new();
    for run in store::list_runs_since(conn, "")? {
        if let Some(actual) = run.actual_start_ms {
            if actual - run.planned_start_ms <= rewards::START_TOLERANCE_MS {
                out.push(Payable {
                    at_ms: actual,
                    delta: rewards::START_BONUS_POINTS,
                    reason: rewards::REASON_START_BONUS,
                    ref_key: format!("start:run:{}", run.id),
                });
            }
        }
        let minutes = store::list_run_minutes(conn, run.id)?;
        let attempts = store::list_attempt_times(conn, run.id)?;
        for w in windows(&minutes, &attempts, now_ms)
            .into_iter()
            .filter(|w| w.clean)
        {
            out.push(Payable {
                at_ms: w.end_ms,
                delta: rewards::WINDOW_POINTS,
                reason: rewards::REASON_FOCUS_WINDOW,
                ref_key: format!("focus30:run:{}:win:{}", run.id, w.index),
            });
        }
        if run.status == RunStatus::Aborted {
            out.push(Payable {
                at_ms: run.ended_at_ms.unwrap_or(run.planned_end_ms),
                delta: -rewards::ABORT_PENALTY,
                reason: rewards::REASON_RUN_ABORTED,
                ref_key: format!("abort:run:{}", run.id),
            });
        }
    }
    for (id, at_ms, points) in store::list_pass_penalties(conn)? {
        out.push(Payable {
            at_ms,
            delta: -points,
            reason: rewards::REASON_EMERGENCY_PASS,
            ref_key: format!("pass:{id}"),
        });
    }
    Ok(out)
}

/// Posts every payable event the ledger lacks, in one transaction. Returns how many rows were added.
pub fn settle(conn: &mut Connection, now_ms: i64) -> AppResult<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut added = 0;
    for p in payables(&tx, now_ms)? {
        if store::insert_ledger_entry(&tx, p.at_ms, p.delta, p.reason, &p.ref_key)? {
            added += 1;
        }
    }
    tx.commit()?;
    if added > 0 {
        tracing::info!(event = "points_settled", added);
    }
    Ok(added)
}

/// Settles, then reports.
pub fn settled_status(conn: &mut Connection, now_ms: i64) -> AppResult<PointsStatus> {
    settle(conn, now_ms)?;
    status(conn)
}
