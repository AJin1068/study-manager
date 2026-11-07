//! Decides whether a message is due. Reads state, never writes it (except its own marks).
//!
//! Rules, in order:
//! 1. The marks (last block attempt seen, last set seen, when the study block began) are always
//!    updated, so an event that happened during a break or before the app looked is never
//!    delivered late.
//! 2. Nothing is due unless messages are on and a locked study block is running: none in a
//!    break, between runs, or with the lock free.
//! 3. A blocked-app attempt, then a finished set, are events (if events are on); a random message
//!    is due after the configured quiet time with no message (if random is on).
//! 4. At most `max_per_hour` messages in any rolling hour, counted from `message_log`.

use std::sync::Mutex;

use rusqlite::Connection;

use super::config::MessageConfig;
use crate::error::AppResult;
use crate::store::messages as repo;
use crate::store::quiz as quiz_repo;
use crate::timer::model::TimerState;

const HOUR_MS: i64 = 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Distraction,
    SetDone,
    Random,
}

#[derive(Default)]
struct Marks {
    last_attempt: Option<i64>,
    /// `(run_id, set_index)` seen last.
    last_set: Option<(i64, i64)>,
    /// When the current study block was first seen.
    window_since: Option<i64>,
}

#[derive(Default)]
pub struct Engine {
    marks: Mutex<Marks>,
}

impl Engine {
    pub fn decide(
        &self,
        conn: &Connection,
        state: &TimerState,
        cfg: &MessageConfig,
        now_ms: i64,
    ) -> AppResult<Option<Trigger>> {
        let latest_attempt = quiz_repo::latest_attempt_event_id(conn)?;
        let in_block = crate::enforce::study_window(state).is_some();

        let (new_attempt, set_done, since) = {
            let mut marks = self
                .marks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let new_attempt = marks.last_attempt.is_some_and(|seen| latest_attempt > seen);
            marks.last_attempt = Some(latest_attempt);

            let set_done = match (state.run_id, marks.last_set) {
                (Some(run), Some((prev_run, prev_set))) => {
                    prev_run == run && state.set_index > prev_set
                }
                _ => false,
            };
            marks.last_set = state.run_id.map(|run| (run, state.set_index));

            if in_block {
                marks.window_since.get_or_insert(now_ms);
            } else {
                marks.window_since = None;
            }
            (new_attempt, set_done, marks.window_since)
        };

        if !cfg.enabled || !in_block {
            return Ok(None);
        }
        let candidate = if cfg.events_enabled && new_attempt {
            Some(Trigger::Distraction)
        } else if cfg.events_enabled && set_done {
            Some(Trigger::SetDone)
        } else if cfg.random_enabled && self.random_due(conn, cfg, since, now_ms)? {
            Some(Trigger::Random)
        } else {
            None
        };
        if candidate.is_none()
            || repo::count_since(conn, now_ms.saturating_sub(HOUR_MS))? >= cfg.max_per_hour
        {
            return Ok(None);
        }
        Ok(candidate)
    }

    fn random_due(
        &self,
        conn: &Connection,
        cfg: &MessageConfig,
        since: Option<i64>,
        now_ms: i64,
    ) -> AppResult<bool> {
        let quiet_since = repo::last_at(conn)?
            .unwrap_or(i64::MIN)
            .max(since.unwrap_or(now_ms));
        Ok(now_ms.saturating_sub(quiet_since) >= cfg.random_interval_min.saturating_mul(60_000))
    }
}
