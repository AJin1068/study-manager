//! Release procedures that cost the user something: emergency pass, skip today, early release.
//! `release` decides *whether* a typed sentence is accepted; this module applies the effect and
//! writes the append-only `study_passes` row in the same Immediate transaction, so a granted
//! release is always recorded and a recorded one always took effect.
//!
//! - `emergency_pass`: enforcement pauses for [`EMERGENCY_PASS_MS`]; the lock stays. Limited per
//!   local day (setting `emergencyPassDailyLimit`, default 1). Costs [`EMERGENCY_PASS_PENALTY_POINTS`],
//!   stored on the row so the point ledger (stage 1-5) can read it without recomputing.
//! - `skip_today`: the active run and every remaining occurrence of today become `skipped`, and
//!   the lock is freed. Tomorrow's plans are untouched.
//! - `early_release`: ends the active run (status `aborted`, detail `early_release`, segments
//!   `early_release`) and frees the lock. The run is never resumed.
//!
//! Only the database is touched, never the engine thread, so all three work when it is down.
//! Nothing the user typed reaches this module: only its length and how long typing took.

use chrono::TimeZone;
use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::settings::{self, SettingKey, SettingValue};
use crate::store;

use super::model::{EndReason, NewRun, RunStatus};
use super::{end_active_runs, load, local_date, DATE_FMT};

pub const EMERGENCY_PASS_MS: i64 = 10 * 60 * 1000;
/// Points deducted for one emergency pass. Recorded here; `points::settle` posts it to the ledger.
pub const EMERGENCY_PASS_PENALTY_POINTS: i64 = crate::points::rewards::EMERGENCY_PASS_PENALTY;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PassKind {
    EmergencyPass,
    SkipToday,
    EarlyRelease,
}

impl PassKind {
    #[cfg(test)]
    pub const ALL: [PassKind; 3] = [PassKind::EmergencyPass, PassKind::SkipToday, PassKind::EarlyRelease];

    /// The value stored in `study_passes.kind` (matches the CHECK list).
    pub fn as_str(self) -> &'static str {
        match self {
            PassKind::EmergencyPass => "emergency_pass",
            PassKind::SkipToday => "skip_today",
            PassKind::EarlyRelease => "early_release",
        }
    }
}

/// What the typing step measured. Never the text itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Typing {
    pub challenge_len: i64,
    pub typing_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grant {
    Granted { expires_at_ms: Option<i64> },
    /// Nothing to release: no lock for a pass or early release, nothing left today to skip.
    NothingToRelease,
    LimitReached { limit: i64 },
    AlreadyActive { expires_at_ms: i64 },
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PassStatus {
    pub daily_limit: i64,
    pub used_today: i64,
    pub remaining_today: i64,
    /// End of the running emergency pass, if any.
    pub active_until_ms: Option<i64>,
    pub duration_min: i64,
    pub penalty_points: i64,
}

/// One recorded release, for history and statistics.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PassRecord {
    pub id: i64,
    pub at_ms: i64,
    pub date: String,
    pub kind: String,
    pub run_id: Option<i64>,
    pub expires_at_ms: Option<i64>,
    pub challenge_len: i64,
    pub typing_ms: i64,
    pub penalty_points: Option<i64>,
}

fn date_of<Tz: TimeZone>(tz: &Tz, at_ms: i64) -> AppResult<String> {
    Ok(local_date(tz, at_ms)?.format(DATE_FMT).to_string())
}

pub fn daily_limit(conn: &Connection) -> AppResult<i64> {
    match settings::read_value(conn, &SettingKey::EmergencyPassDailyLimit.spec())? {
        SettingValue::Int(limit) => Ok(limit),
        other => Err(AppError::Internal(format!("emergencyPassDailyLimit is not an integer: {other:?}"))),
    }
}

fn used_on(conn: &Connection, date: &str) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM study_passes WHERE kind = 'emergency_pass' AND date = ?1",
        params![date],
        |r| r.get(0),
    )?)
}

/// End of the emergency pass that is running at `now_ms`, if any. Enforcement reads this.
pub fn active_until(conn: &Connection, now_ms: i64) -> AppResult<Option<i64>> {
    Ok(conn.query_row(
        "SELECT MAX(expires_at_ms) FROM study_passes WHERE kind = 'emergency_pass' AND expires_at_ms > ?1",
        params![now_ms],
        |r| r.get(0),
    )?)
}

pub fn status<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<PassStatus> {
    let daily_limit = daily_limit(conn)?;
    let used_today = used_on(conn, &date_of(tz, now_ms)?)?;
    Ok(PassStatus {
        daily_limit,
        used_today,
        remaining_today: (daily_limit - used_today).max(0),
        active_until_ms: active_until(conn, now_ms)?,
        duration_min: EMERGENCY_PASS_MS / 60_000,
        penalty_points: EMERGENCY_PASS_PENALTY_POINTS,
    })
}

/// The refusal that would stop `kind` right now, or `None` when it could be granted. Used before
/// a challenge is issued so the user is not made to type for nothing; `grant` re-checks inside
/// its transaction.
pub fn refusal<Tz: TimeZone>(conn: &Connection, tz: &Tz, kind: PassKind, at_ms: i64) -> AppResult<Option<Grant>> {
    match kind {
        PassKind::SkipToday => Ok(None),
        PassKind::EarlyRelease => Ok((!store::read_study_state(conn)?.0).then_some(Grant::NothingToRelease)),
        PassKind::EmergencyPass => {
            if !store::read_study_state(conn)?.0 {
                return Ok(Some(Grant::NothingToRelease));
            }
            if let Some(expires_at_ms) = active_until(conn, at_ms)? {
                return Ok(Some(Grant::AlreadyActive { expires_at_ms }));
            }
            let limit = daily_limit(conn)?;
            if used_on(conn, &date_of(tz, at_ms)?)? >= limit {
                return Ok(Some(Grant::LimitReached { limit }));
            }
            Ok(None)
        }
    }
}

/// Marks every still-unrun occurrence of today as skipped so the engine does not start it.
/// Returns how many rows were added.
fn skip_remaining_today<Tz: TimeZone>(tx: &Connection, tz: &Tz, at_ms: i64) -> AppResult<usize> {
    let today = date_of(tz, at_ms)?;
    let loaded = load(tx, tz, at_ms)?;
    let mut added = 0;
    for o in &loaded.occurrences {
        let has_run = loaded.runs.iter().any(|r| r.run.plan_id == o.plan_id && r.run.date == o.date);
        // An occurrence that began before its plan existed was never the user's commitment.
        if o.date != today || has_run || !o.eligible_for_missed || o.planned_end_ms <= at_ms {
            continue;
        }
        let cfg = loaded.modes.config(o.level)?;
        store::insert_run(
            tx,
            &NewRun {
                plan_id: o.plan_id,
                date: o.date.clone(),
                planned_start_ms: o.planned_start_ms,
                planned_end_ms: o.planned_end_ms,
                actual_start_ms: None,
                mode_level: cfg.level.as_str().to_string(),
                study_min: cfg.study_min,
                break_min: cfg.break_min,
                long_break_min: cfg.long_break_min,
                sets_before_long_break: cfg.sets_before_long_break,
                sets_total: o.sets,
                status: RunStatus::Skipped,
                ended_at_ms: Some(at_ms),
                detail_code: Some("skip_today"),
            },
        )?;
        added += 1;
    }
    Ok(added)
}

/// Applies `kind` and records it, or says why not. All or nothing.
pub fn grant<Tz: TimeZone>(
    conn: &mut Connection,
    tz: &Tz,
    kind: PassKind,
    at_ms: i64,
    typing: Typing,
) -> AppResult<Grant> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(refused) = refusal(&tx, tz, kind, at_ms)? {
        return Ok(refused);
    }
    let run_id = store::list_active_runs(&tx)?.first().map(|r| r.id);
    let mut expires_at_ms = None;
    let mut penalty_points = None;
    match kind {
        PassKind::EmergencyPass => {
            expires_at_ms = Some(at_ms + EMERGENCY_PASS_MS);
            penalty_points = Some(EMERGENCY_PASS_PENALTY_POINTS);
        }
        PassKind::EarlyRelease => {
            if !end_active_runs(&tx, at_ms, RunStatus::Aborted, EndReason::EarlyRelease, "early_release")? {
                return Ok(Grant::NothingToRelease);
            }
        }
        PassKind::SkipToday => {
            let ended = end_active_runs(&tx, at_ms, RunStatus::Skipped, EndReason::Skipped, "skip_today")?;
            let skipped = skip_remaining_today(&tx, tz, at_ms)?;
            if !ended && skipped == 0 {
                return Ok(Grant::NothingToRelease);
            }
        }
    }
    tx.execute(
        "INSERT INTO study_passes (at_ms, date, kind, run_id, expires_at_ms, challenge_len, typing_ms, penalty_points)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            at_ms,
            date_of(tz, at_ms)?,
            kind.as_str(),
            run_id,
            expires_at_ms,
            typing.challenge_len,
            typing.typing_ms,
            penalty_points
        ],
    )?;
    tx.commit()?;
    tracing::info!(
        event = "pass_granted",
        kind = kind.as_str(),
        challenge_len = typing.challenge_len,
        typing_ms = typing.typing_ms,
        at_ms
    );
    Ok(Grant::Granted { expires_at_ms })
}

/// Newest first.
pub fn list_passes(conn: &Connection, limit: i64) -> AppResult<Vec<PassRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, at_ms, date, kind, run_id, expires_at_ms, challenge_len, typing_ms, penalty_points
         FROM study_passes ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        Ok(PassRecord {
            id: r.get(0)?,
            at_ms: r.get(1)?,
            date: r.get(2)?,
            kind: r.get(3)?,
            run_id: r.get(4)?,
            expires_at_ms: r.get(5)?,
            challenge_len: r.get(6)?,
            typing_ms: r.get(7)?,
            penalty_points: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
