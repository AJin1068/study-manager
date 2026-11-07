//! Review questions that never lift anything: one when a blocked app is opened, one with a random
//! focus check-in (off by default), three after a run ends (can be skipped). They are practice and
//! a small bonus. Nothing here reads or writes the lock, `block_events`, or the run state, so
//! whatever the user answers (or ignores) the block and the end-of-block decision are the same
//! (O-8: the plan says a question appears, not that the block opens, so no new exit is created).
//!
//! Pop-ups are rationed: a cooldown and a daily limit per context, and none outside a locked study
//! block (not during breaks, not between runs). The session-end quiz is offered once per run.
//! A right answer pays [`rewards::QUIZ_BONUS_POINTS`] through the point ledger under
//! `quiz:attempt:<id>`, so an attempt pays once, up to [`rewards::QUIZ_BONUS_DAILY_CAP`] a day.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::TimeZone;
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;

use super::config::QuizConfig;
use super::select::{self, Candidate, pick};
use super::{Context, QuizItem, QuizQuestion, SESSION_END_QUESTIONS, grade};
use crate::error::{AppError, AppResult};
use crate::llm::service::day_bounds_ms;
use crate::points::rewards;
use crate::store;
use crate::store::quiz as repo;
use crate::timer::model::{RunStatus, TimerState};
use crate::timer::study_pass::date_of;

/// Minimum time between two pop-ups of the same kind.
pub const COOLDOWN_MS: i64 = 5 * 60 * 1000;
/// Pop-ups of the same kind per local day.
pub const DAILY_LIMIT: usize = 6;
/// A finished run is offered its quiz for this long.
pub const SESSION_END_WINDOW_MS: i64 = 30 * 60 * 1000;
const KEEP_PENDING: usize = 8;

/// The Tauri event carrying a card to the window.
pub const CARD_EVENT: &str = "quiz://card";

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuizCard {
    pub card_id: u64,
    pub context: Context,
    pub questions: Vec<QuizQuestion>,
    /// Only the session-end quiz may be skipped (and the skip is recorded).
    pub skippable: bool,
}

/// After answering a review card the right answer is shown. (Not used for the release proof.)
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ItemResult {
    pub item_id: i64,
    pub correct: bool,
    pub answer: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CardResult {
    pub card_id: u64,
    pub context: Context,
    pub results: Vec<ItemResult>,
    pub skipped: bool,
    /// Points added to the ledger by this answer (0 once the daily cap is reached).
    pub bonus_points: i64,
}

struct PendingCard {
    id: u64,
    context: Context,
    items: Vec<QuizItem>,
    run_id: Option<i64>,
    segment_id: Option<i64>,
    issued_at_ms: i64,
}

#[derive(Default)]
struct GateState {
    next_id: u64,
    pending: Vec<PendingCard>,
    offers: HashMap<&'static str, Vec<i64>>,
}

#[derive(Default)]
pub struct QuizGate {
    state: Mutex<GateState>,
}

fn lock(gate: &QuizGate) -> std::sync::MutexGuard<'_, GateState> {
    // Plain data: a poisoned guard is still consistent.
    gate.state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The locked study block, if one is running: pop-ups exist only inside it.
fn study_block<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    now_ms: i64,
) -> AppResult<Option<TimerState>> {
    let state = crate::timer::current_state(conn, tz, now_ms)?;
    Ok(crate::enforce::study_window(&state).map(|_| state))
}

/// The wiki subject of a run's plan, so its questions come first.
fn run_subject(conn: &Connection, run_id: Option<i64>) -> AppResult<Option<i64>> {
    let Some(run) = run_id
        .map(|id| store::get_run(conn, id))
        .transpose()?
        .flatten()
    else {
        return Ok(None);
    };
    let Some(plan) = store::get_plan(conn, run.plan_id)? else {
        return Ok(None);
    };
    Ok(crate::wiki::service::subject_for_plan(conn, &plan.subject)?.map(|s| s.id))
}

/// Questions from the preferred subject first; if it has none to give, from any subject.
fn draw(
    conn: &Connection,
    subject: Option<i64>,
    n: usize,
    now_ms: i64,
    seed: u64,
    want_cloze: bool,
) -> AppResult<Vec<QuizItem>> {
    let from = |candidates: &[Candidate]| -> Vec<QuizItem> {
        pick(candidates, n, now_ms, seed, true, want_cloze)
            .into_iter()
            .map(|i| candidates[i].item.clone())
            .collect()
    };
    if subject.is_some() {
        let own = from(&repo::candidates(conn, subject)?);
        if !own.is_empty() {
            return Ok(own);
        }
    }
    Ok(from(&repo::candidates(conn, None)?))
}

impl QuizGate {
    /// Whether this kind of pop-up is allowed right now (cooldown, daily limit), without counting it.
    fn allowed<Tz: TimeZone>(state: &GateState, tz: &Tz, kind: &'static str, now_ms: i64) -> bool {
        let times = state.offers.get(kind).map(Vec::as_slice).unwrap_or(&[]);
        if times
            .iter()
            .any(|&t| now_ms.saturating_sub(t) < COOLDOWN_MS && t <= now_ms)
        {
            return false;
        }
        let (from, to) = day_bounds_ms(tz, now_ms);
        times.iter().filter(|&&t| t >= from && t < to).count() < DAILY_LIMIT
    }

    fn issue(
        &self,
        kind: &'static str,
        context: Context,
        items: Vec<QuizItem>,
        at: (Option<i64>, Option<i64>),
        now_ms: i64,
        skippable: bool,
    ) -> QuizCard {
        let mut state = lock(self);
        state.next_id += 1;
        let id = state.next_id;
        let seed = select::mix(u64::try_from(now_ms).unwrap_or(0) ^ id);
        let questions = items.iter().map(|item| item.view(seed)).collect();
        let (run_id, segment_id) = at;
        state.pending.push(PendingCard {
            id,
            context,
            items,
            run_id,
            segment_id,
            issued_at_ms: now_ms,
        });
        if state.pending.len() > KEEP_PENDING {
            state.pending.remove(0);
        }
        let log = state.offers.entry(kind).or_default();
        log.retain(|&t| now_ms.saturating_sub(t) < 24 * 60 * 60 * 1000);
        log.push(now_ms);
        tracing::info!(
            event = "quiz_gate",
            context = context.as_str(),
            result = "offered"
        );
        QuizCard {
            card_id: id,
            context,
            questions,
            skippable,
        }
    }

    fn seed(&self, now_ms: i64) -> u64 {
        select::mix(u64::try_from(now_ms).unwrap_or(0) ^ lock(self).next_id ^ 0xA5)
    }

    /// One question because a blocked app or site was opened. `None` when pop-ups are off, there is
    /// no locked study block, it is too soon after the last one, or the bank has nothing to ask.
    pub fn offer_distraction<Tz: TimeZone>(
        &self,
        conn: &Connection,
        tz: &Tz,
        now_ms: i64,
    ) -> AppResult<Option<QuizCard>> {
        if !QuizConfig::load(conn)?.distraction_enabled {
            return Ok(None);
        }
        self.offer_in_block(conn, tz, now_ms, "distraction", Context::Distraction)
    }

    /// One question attached to a random focus check-in (`quiz.checkin_enabled`, off by default).
    pub fn offer_checkin<Tz: TimeZone>(
        &self,
        conn: &Connection,
        tz: &Tz,
        now_ms: i64,
    ) -> AppResult<Option<QuizCard>> {
        if !QuizConfig::load(conn)?.checkin_enabled {
            return Ok(None);
        }
        self.offer_in_block(conn, tz, now_ms, "checkin", Context::Checkin)
    }

    fn offer_in_block<Tz: TimeZone>(
        &self,
        conn: &Connection,
        tz: &Tz,
        now_ms: i64,
        kind: &'static str,
        context: Context,
    ) -> AppResult<Option<QuizCard>> {
        let Some(block) = study_block(conn, tz, now_ms)? else {
            return Ok(None);
        };
        if !Self::allowed(&lock(self), tz, kind, now_ms) {
            return Ok(None);
        }
        let items = draw(
            conn,
            run_subject(conn, block.run_id)?,
            1,
            now_ms,
            self.seed(now_ms),
            false,
        )?;
        if items.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.issue(
            kind,
            context,
            items,
            (block.run_id, block.segment_id),
            now_ms,
            false,
        )))
    }

    /// The quiz of the newest run that finished by itself in the last half hour and has not been
    /// answered or skipped. Once per run.
    pub fn offer_session_end<Tz: TimeZone>(
        &self,
        conn: &Connection,
        tz: &Tz,
        now_ms: i64,
    ) -> AppResult<Option<QuizCard>> {
        if !QuizConfig::load(conn)?.session_end_enabled || study_block(conn, tz, now_ms)?.is_some()
        {
            return Ok(None);
        }
        let since = date_of(tz, now_ms.saturating_sub(24 * 60 * 60 * 1000))?;
        let mut finished: Vec<_> = store::list_runs_since(conn, &since)?
            .into_iter()
            .filter(|r| {
                r.status == RunStatus::Completed
                    && r.ended_at_ms
                        .is_some_and(|e| e <= now_ms && now_ms - e <= SESSION_END_WINDOW_MS)
            })
            .collect();
        finished.sort_by_key(|r| r.ended_at_ms);
        let Some(run) = finished.pop() else {
            return Ok(None);
        };
        if repo::session_end_done(conn, run.id)?
            || lock(self)
                .pending
                .iter()
                .any(|p| p.context == Context::SessionEnd && p.run_id == Some(run.id))
        {
            return Ok(None);
        }
        let subject = run_subject(conn, Some(run.id))?;
        let items = draw(
            conn,
            subject,
            SESSION_END_QUESTIONS,
            now_ms,
            self.seed(now_ms),
            true,
        )?;
        if items.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.issue(
            "session_end",
            Context::SessionEnd,
            items,
            (Some(run.id), None),
            now_ms,
            true,
        )))
    }

    /// Practice questions on request (the wiki screen). Not rationed, never in a gate.
    pub fn offer_review(
        &self,
        conn: &Connection,
        now_ms: i64,
        subject_id: Option<i64>,
        n: usize,
    ) -> AppResult<Option<QuizCard>> {
        let items = draw(
            conn,
            subject_id,
            n.clamp(1, SESSION_END_QUESTIONS),
            now_ms,
            self.seed(now_ms),
            true,
        )?;
        if items.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.issue(
            "review",
            Context::Review,
            items,
            (None, None),
            now_ms,
            false,
        )))
    }

    /// Grades a card (one answer per question, in order; a missing one is wrong), records the
    /// attempts and pays the bonus. `skip` records a session-end quiz as skipped and drops any
    /// other card unrecorded. `None` when the card is unknown or already used.
    pub fn answer<Tz: TimeZone>(
        &self,
        conn: &mut Connection,
        tz: &Tz,
        card_id: u64,
        answers: &[String],
        skip: bool,
        now_ms: i64,
    ) -> AppResult<Option<CardResult>> {
        let card = {
            let mut state = lock(self);
            let Some(pos) = state.pending.iter().position(|p| p.id == card_id) else {
                return Ok(None);
            };
            state.pending.remove(pos)
        };
        if skip && card.context != Context::SessionEnd {
            tracing::info!(
                event = "quiz_gate",
                context = card.context.as_str(),
                result = "dismissed"
            );
            return Ok(Some(CardResult {
                card_id,
                context: card.context,
                results: Vec::new(),
                skipped: true,
                bonus_points: 0,
            }));
        }
        let response_ms = (now_ms - card.issued_at_ms).max(0);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (from, to) = day_bounds_ms(tz, now_ms);
        let mut paid_today = repo::bonus_rows_between(&tx, rewards::REASON_QUIZ_BONUS, from, to)?;
        let (mut results, mut bonus_points) = (Vec::new(), 0);
        for (index, item) in card.items.iter().enumerate() {
            let correct = !skip
                && answers
                    .get(index)
                    .is_some_and(|a| grade::is_correct(item, a));
            let attempt = repo::insert_attempt(
                &tx,
                &repo::NewAttempt {
                    at_ms: now_ms,
                    item_id: item.id,
                    context: card.context.as_str(),
                    release_kind: None,
                    run_id: card.run_id,
                    segment_id: card.segment_id,
                    correct,
                    skipped: skip,
                    response_ms,
                },
            )?;
            if correct && paid_today < rewards::QUIZ_BONUS_DAILY_CAP {
                let key = format!("quiz:attempt:{attempt}");
                if store::insert_ledger_entry(
                    &tx,
                    now_ms,
                    rewards::QUIZ_BONUS_POINTS,
                    rewards::REASON_QUIZ_BONUS,
                    &key,
                )? {
                    paid_today += 1;
                    bonus_points += rewards::QUIZ_BONUS_POINTS;
                }
            }
            results.push(ItemResult {
                item_id: item.id,
                correct,
                answer: item.answer_key.clone(),
            });
        }
        tx.commit()?;
        tracing::info!(
            event = "quiz_gate",
            context = card.context.as_str(),
            result = if skip { "skipped" } else { "answered" }
        );
        Ok(Some(CardResult {
            card_id,
            context: card.context,
            results: if skip { Vec::new() } else { results },
            skipped: skip,
            bonus_points,
        }))
    }
}

/// Reports a question: it is excluded from every later draw. Refused while the lock is held, so
/// the bank cannot be shrunk from inside a locked block to reach the typing fallback of the
/// early-release gate.
pub fn flag_item(conn: &Connection, item_id: i64, reason: &str, now_ms: i64) -> AppResult<()> {
    if crate::study::lock_held(conn)? {
        return Err(AppError::Invalid(
            "questions cannot be reported while the lock is held".into(),
        ));
    }
    repo::flag_item(conn, item_id, reason, now_ms)
}

/// Accuracy of the session-end quizzes of one run as `(answered, correct)`; skipped ones are not
/// counted. The focus score's `quiz_accuracy` input reads this.
pub fn session_end_accuracy(conn: &Connection, run_id: i64) -> AppResult<(i64, i64)> {
    let (answered, correct, _skipped) = repo::session_end_stats(conn, run_id)?;
    Ok((answered, correct))
}
