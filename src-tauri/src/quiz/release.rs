//! The early-release quiz gate: which proof an early release needs, the attempt cap, and the
//! cooldown. Only `PassKind::EarlyRelease` ever reaches this module (`ReleaseDesk::begin` asks only
//! for it); emergency pass and skip-today keep the typing path and never read or change these
//! counters.
//!
//! Rules (O-8, O-24, O-26):
//! - With at least [`RELEASE_QUESTIONS`] drawable questions the proof is a quiz: all of them right.
//!   With fewer the proof is the typed sentence.
//! - At most `quiz.release_max_attempts_per_day` attempts per local day. An attempt is counted when
//!   the questions are issued, so closing the card or rerolling costs one too. Once the cap is
//!   spent, early release is refused for the rest of the day; typing does not reopen (a parallel
//!   exit would make the cap pointless). The counter is checked before the bank size on purpose:
//!   the bank cannot be made small enough to fall back to typing after attempts were used: with an
//!   attempt on record and fewer than [`RELEASE_QUESTIONS`] drawable questions, the day is closed.
//! - A failed or newly opened attempt starts a cooldown (`quiz.release_cooldown_min`).
//! - Questions asked in the last 24 hours are avoided while enough others exist, and top up the
//!   draw only when the bank is that small. The typing fallback is decided by the size of the
//!   bank, not by how many are fresh, so using up questions never opens it.
//! - At least one blank is included when the bank has one, which keeps blind guessing at about
//!   1/64 for three four-choice questions and far below that with a blank.

use chrono::TimeZone;
use rusqlite::Connection;
use serde::Serialize;

use super::config::QuizConfig;
use super::select::pick;
use super::{Context, QuizItem, RELEASE_QUESTIONS};
use crate::error::AppResult;
use crate::store::quiz as repo;
use crate::timer::study_pass::date_of;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleasePlan {
    /// Not enough questions in the wiki: the typed sentence is the proof.
    Typing,
    /// Answer all of these.
    Quiz(Vec<QuizItem>),
    CoolingDown {
        retry_after_ms: i64,
    },
    Exhausted {
        max_attempts: i64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gate {
    Typing,
    Quiz,
    CoolingDown { retry_after_ms: i64 },
    Exhausted,
}

fn gate<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    config: &QuizConfig,
    now_ms: i64,
) -> AppResult<(Gate, i64)> {
    let date = date_of(tz, now_ms)?;
    let (attempts, last_ms) = repo::release_state(conn, &date)?.unwrap_or((0, 0));
    if attempts >= config.release_max_attempts_per_day {
        return Ok((Gate::Exhausted, attempts));
    }
    if attempts > 0 {
        let retry_after_ms = last_ms.saturating_add(config.cooldown_ms()) - now_ms;
        if retry_after_ms > 0 {
            return Ok((Gate::CoolingDown { retry_after_ms }, attempts));
        }
    }
    let drawable = repo::drawable_count(conn)?;
    let enough = usize::try_from(drawable).is_ok_and(|n| n >= RELEASE_QUESTIONS);
    if !enough && attempts > 0 {
        // The bank shrank after an attempt was spent: typing must not open as a way around the cap.
        return Ok((Gate::Exhausted, attempts));
    }
    Ok((if enough { Gate::Quiz } else { Gate::Typing }, attempts))
}

/// Decides the proof for a new early-release challenge and, when it is a quiz, counts the attempt.
pub fn begin_attempt<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    now_ms: i64,
    seed: u64,
) -> AppResult<ReleasePlan> {
    let config = QuizConfig::load(conn)?;
    let (decided, _) = gate(conn, tz, &config, now_ms)?;
    match decided {
        Gate::Exhausted => Ok(ReleasePlan::Exhausted {
            max_attempts: config.release_max_attempts_per_day,
        }),
        Gate::CoolingDown { retry_after_ms } => Ok(ReleasePlan::CoolingDown { retry_after_ms }),
        Gate::Typing => Ok(ReleasePlan::Typing),
        Gate::Quiz => {
            let candidates = repo::candidates(conn, None)?;
            let picked = pick(&candidates, RELEASE_QUESTIONS, now_ms, seed, false, true);
            if picked.len() < RELEASE_QUESTIONS {
                return Ok(ReleasePlan::Typing);
            }
            repo::open_release_attempt(conn, &date_of(tz, now_ms)?, now_ms)?;
            Ok(ReleasePlan::Quiz(
                picked
                    .into_iter()
                    .map(|i| candidates[i].item.clone())
                    .collect(),
            ))
        }
    }
}

/// A wrong set of answers: the cooldown restarts from now.
pub fn fail_attempt<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<()> {
    repo::mark_release_failure(conn, &date_of(tz, now_ms)?, now_ms)
}

/// Writes one attempt row per question (right or wrong), tagged as the early-release proof.
pub fn record_answers(
    conn: &Connection,
    items: &[QuizItem],
    correct: &[bool],
    run_id: Option<i64>,
    response_ms: i64,
    at_ms: i64,
) -> AppResult<()> {
    for (item, ok) in items.iter().zip(correct) {
        repo::insert_attempt(
            conn,
            &repo::NewAttempt {
                at_ms,
                item_id: item.id,
                context: Context::Release.as_str(),
                release_kind: Some("early_release"),
                run_id,
                segment_id: None,
                correct: *ok,
                skipped: false,
                response_ms,
            },
        )?;
    }
    Ok(())
}

/// What the early-release card shows before the user starts.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseQuizStatus {
    /// `typing`, `quiz`, `cooldown`, or `exhausted`.
    pub mode: &'static str,
    pub required: usize,
    pub drawable: i64,
    pub attempts_used: i64,
    pub max_attempts: i64,
    pub cooldown_remaining_ms: i64,
    pub cooldown_min: i64,
}

pub fn status<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    now_ms: i64,
) -> AppResult<ReleaseQuizStatus> {
    let config = QuizConfig::load(conn)?;
    let (decided, attempts) = gate(conn, tz, &config, now_ms)?;
    let (mode, cooldown_remaining_ms) = match decided {
        Gate::Typing => ("typing", 0),
        Gate::Quiz => ("quiz", 0),
        Gate::CoolingDown { retry_after_ms } => ("cooldown", retry_after_ms),
        Gate::Exhausted => ("exhausted", 0),
    };
    Ok(ReleaseQuizStatus {
        mode,
        required: RELEASE_QUESTIONS,
        drawable: repo::drawable_count(conn)?,
        attempts_used: attempts,
        max_attempts: config.release_max_attempts_per_day,
        cooldown_remaining_ms,
        cooldown_min: config.release_cooldown_min,
    })
}
