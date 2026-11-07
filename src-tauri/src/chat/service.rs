//! One chat turn, drafts, and applying a draft.
//!
//! A turn asks the model (database unlocked) for one protocol step at a time. Read tools run
//! locally and their result goes back to the model as a data block. A `propose_*` tool adds one
//! operation to the turn's draft after a dry run: the whole draft so far is run through the normal
//! plan functions inside a transaction that is rolled back, so overlaps and bad input come back to
//! the model as an error instead of reaching the user. Nothing in the plan tables changes before
//! the user applies the stored draft.
//!
//! Study lock (same rule as the settings lock): while a run holds the lock, `propose_move` and
//! `propose_delete` are refused and recorded as `rejected_locked`, and applying a draft is
//! refused and recorded; additions may still be drafted and applied after the run.
//!
//! The model being off, unconfigured, over budget, or failing ends in a local answer that says why.

use std::sync::{Arc, Mutex};

use chrono::{Duration, Local, NaiveDate, TimeZone};
use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::protocol::{CreateOp, DeleteOp, DraftOp, MoveOp, Step, Tool, parse_step};
use super::tools::{self, hhmm};
use crate::commands::lock_db;
use crate::error::{AppError, AppResult};
use crate::llm::egress::{ChatLine, ChatRole, Prompt};
use crate::llm::service::LlmService;
use crate::llm::{LlmRequest, Purpose};
use crate::plan::{
    self, Action, CreatePlanInput, DeletePlanInput, Origin, ReasonCode, Repeat, Scope,
    UpdatePlanInput, fmt_date, local_to_ms, parse_date, parse_time, plan_duration_min,
};
use crate::plan_shift::{ShiftItem, validate_layout};
use crate::remote::Clock;
use crate::store::chat as repo;

/// Model calls in one turn (each tool call costs one).
pub const MAX_STEPS: usize = 6;
/// Answers outside the protocol tolerated in one turn before the turn gives up.
pub const MAX_DISCARDS: usize = 2;
/// Questions the model may ask back before it must draft.
pub const MAX_CLARIFICATIONS: usize = 3;
pub const MAX_OPS: usize = 12;
pub const MAX_MESSAGE_CHARS: usize = 1_000;
/// Conversation lines the UI may send along.
const MAX_HISTORY: usize = 12;
const CHAT_MAX_TOKENS: u32 = 700;
/// Unapplied drafts are dropped after this long.
const DRAFT_TTL_MS: i64 = 2 * 86_400_000;
const STEP_MIN: i64 = 5;
const FALLBACK_DAYS: i64 = 7;
const FALLBACK_SLOTS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: Speaker,
    pub text: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DraftItem {
    /// `create`, `move`, or `delete`.
    pub kind: &'static str,
    pub label: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DraftView {
    pub id: i64,
    pub items: Vec<DraftItem>,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplySource {
    Llm,
    Local,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    pub text: String,
    pub source: ReplySource,
    /// Why the model did not answer (a stable code), when it did not.
    pub fallback_reason: Option<String>,
    pub draft: Option<DraftView>,
    /// A move or cancel was refused because a study run holds the lock.
    pub locked: bool,
    /// The model asked back more than allowed without drafting.
    pub over_asked: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub applied: bool,
    /// The study lock refused it; the attempt is in the plan history.
    pub locked: bool,
    pub changed: usize,
    pub batch_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDraft {
    ops: Vec<DraftOp>,
}

// ---- operations -> plan inputs ----

fn weekday_mask(days: &[u8]) -> AppResult<i64> {
    if days.is_empty() || days.iter().any(|d| !(1..=7).contains(d)) {
        return Err(AppError::Invalid(
            "repeat_weekdays must be 1 (Monday) to 7 (Sunday)".into(),
        ));
    }
    Ok(days.iter().fold(0_i64, |mask, d| mask | (1 << (d - 1))))
}

fn create_input(op: &CreateOp) -> AppResult<CreatePlanInput> {
    let repeat = match (&op.repeat_weekdays, &op.repeat_until) {
        (Some(days), until) => Some(Repeat {
            weekdays: weekday_mask(days)?,
            until: until.clone(),
        }),
        (None, Some(_)) => {
            return Err(AppError::Invalid(
                "repeat_until needs repeat_weekdays".into(),
            ));
        }
        (None, None) => None,
    };
    Ok(CreatePlanInput {
        name: op.name.clone(),
        subject: op.subject.clone(),
        start_date: op.date.clone(),
        start_time: op.time.clone(),
        sets: op.sets,
        repeat,
    })
}

fn move_input(op: &MoveOp) -> UpdatePlanInput {
    UpdatePlanInput {
        plan_id: op.plan_id,
        scope: Scope::Occurrence,
        date: op.date.clone(),
        new_date: op.new_date.clone(),
        new_time: op.new_time.clone(),
        new_sets: None,
        repeat: None,
        reason_code: ReasonCode::Chatbot,
        reason_text: None,
    }
}

fn delete_input(op: &DeleteOp) -> DeletePlanInput {
    DeletePlanInput {
        plan_id: op.plan_id,
        scope: Scope::Occurrence,
        date: op.date.clone(),
        reason_code: ReasonCode::Chatbot,
        reason_text: None,
    }
}

fn action_of(op: &DraftOp) -> Action {
    match op {
        DraftOp::Create(_) => Action::ChatbotCreate,
        DraftOp::Move(_) => Action::Move,
        DraftOp::Delete(_) => Action::Delete,
    }
}

/// Runs the operations in order inside the caller's transaction. A change that is not applied
/// (the lock) is an error here: callers check the lock first.
fn run_ops<Tz: TimeZone>(
    tx: &Connection,
    tz: &Tz,
    ops: &[DraftOp],
    at_ms: i64,
    batch: &str,
) -> AppResult<()> {
    let origin = Origin::Chatbot { batch_id: batch };
    for op in ops {
        let change = match op {
            DraftOp::Create(c) => plan::create_plan_in(tx, tz, create_input(c)?, at_ms, origin)?,
            DraftOp::Move(m) => plan::update_plan_in(tx, tz, move_input(m), at_ms, origin)?,
            DraftOp::Delete(d) => plan::delete_plan_in(tx, delete_input(d), at_ms, origin)?,
        };
        if !change.is_applied() {
            return Err(AppError::Invalid(
                "the study lock refused the change".into(),
            ));
        }
    }
    Ok(())
}

/// Whether the operations would apply right now. The transaction is rolled back: no plan data and
/// no history row is left behind. `Err(message)` is for the model; internal errors propagate.
fn dry_run<Tz: TimeZone>(
    conn: &mut Connection,
    tz: &Tz,
    ops: &[DraftOp],
    at_ms: i64,
) -> AppResult<Result<(), String>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = run_ops(&tx, tz, ops, at_ms, "chat-preview");
    drop(tx);
    match result {
        Ok(()) => Ok(Ok(())),
        Err(AppError::Invalid(message)) => Ok(Err(message)),
        Err(other) => Err(other),
    }
}

// ---- overlap adjustment ----

/// Moves a new single plan to the earliest start at or after the requested one where the day's
/// layout stays valid (`plan_shift::validate_layout`: no overlap with anything on the day, not
/// before now, finished before midnight). `Ok(None)` when the day has no room.
fn fit_create<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    op: &CreateOp,
    earlier: &[DraftOp],
    now_ms: i64,
) -> AppResult<Option<String>> {
    let date = parse_date(&op.date)?;
    let requested = local_to_ms(tz, date, parse_time(&op.time)?)?;
    let day_end = local_to_ms(tz, date + Duration::days(1), chrono::NaiveTime::MIN)?;
    let (occurrences, modes) = plan::occurrences_in(conn, tz, date - Duration::days(1), date)?;
    let minutes = plan_duration_min(modes.config(modes.level_for(&op.date))?, op.sets)?;

    let mut frozen: Vec<ShiftItem> = occurrences
        .iter()
        .map(|o| ShiftItem {
            old_start_ms: o.planned_start_ms,
            old_duration_min: o.duration_min,
            new_duration_min: o.duration_min,
            pending: false,
        })
        .collect();
    for other in earlier {
        if let DraftOp::Create(c) = other {
            if c.date == op.date && c.repeat_weekdays.is_none() {
                let start = local_to_ms(tz, date, parse_time(&c.time)?)?;
                let m = plan_duration_min(modes.config(modes.level_for(&c.date))?, c.sets)?;
                frozen.push(ShiftItem {
                    old_start_ms: start,
                    old_duration_min: m,
                    new_duration_min: m,
                    pending: false,
                });
            }
        }
    }
    let mut starts: Vec<i64> = frozen.iter().map(|i| i.old_start_ms).collect();
    frozen.push(ShiftItem {
        old_start_ms: requested,
        old_duration_min: minutes,
        new_duration_min: minutes,
        pending: true,
    });
    starts.push(requested);

    let mut candidate = requested;
    while candidate + minutes * 60_000 <= day_end {
        if let Some(last) = starts.last_mut() {
            *last = candidate;
        }
        if validate_layout(&frozen, &starts, now_ms, day_end) {
            let minute_of_day =
                (candidate - local_to_ms(tz, date, chrono::NaiveTime::MIN)?) / 60_000;
            return Ok(Some(hhmm(minute_of_day)));
        }
        candidate += STEP_MIN * 60_000;
    }
    Ok(None)
}

// ---- descriptions ----

fn describe(conn: &Connection, ops: &[DraftOp]) -> AppResult<Vec<DraftItem>> {
    let day_names = ["월", "화", "수", "목", "금", "토", "일"];
    ops.iter()
        .map(|op| {
            Ok(match op {
                DraftOp::Create(c) => {
                    let repeat = c.repeat_weekdays.as_ref().map_or(String::new(), |days| {
                        let names: Vec<&str> = days
                            .iter()
                            .filter_map(|d| day_names.get(usize::from(*d).saturating_sub(1)))
                            .copied()
                            .collect();
                        format!(" · 매주 {}", names.join(","))
                    });
                    DraftItem {
                        kind: "create",
                        label: format!(
                            "{} ({}) · {} {} · {}세트{repeat}",
                            c.name, c.subject, c.date, c.time, c.sets
                        ),
                    }
                }
                DraftOp::Move(m) => {
                    let label = tools::plan_label(conn, m.plan_id)?
                        .unwrap_or_else(|| format!("계획 {}", m.plan_id));
                    let to = format!(
                        "{} {}",
                        m.new_date.as_deref().unwrap_or("같은 날"),
                        m.new_time.as_deref().unwrap_or("같은 시각")
                    );
                    DraftItem {
                        kind: "move",
                        label: format!("{label} · {} → {to}", m.date),
                    }
                }
                DraftOp::Delete(d) => {
                    let label = tools::plan_label(conn, d.plan_id)?
                        .unwrap_or_else(|| format!("계획 {}", d.plan_id));
                    DraftItem {
                        kind: "delete",
                        label: format!("{label} · {} 취소", d.date),
                    }
                }
            })
        })
        .collect()
}

fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
}

fn error_result(message: &str) -> Value {
    json!({ "ok": false, "error": message })
}

// ---- the service ----

pub struct ChatService {
    db: Arc<Mutex<Connection>>,
    llm: Arc<LlmService>,
    clock: Arc<dyn Clock>,
}

/// Mutable state of one turn.
struct Turn {
    lines: Vec<ChatLine>,
    ops: Vec<DraftOp>,
    locked: bool,
}

impl ChatService {
    pub fn new(db: Arc<Mutex<Connection>>, llm: Arc<LlmService>, clock: Arc<dyn Clock>) -> Self {
        Self { db, llm, clock }
    }

    pub fn send(&self, history: &[ChatMessage], message: &str) -> AppResult<ChatReply> {
        self.send_in(&Local, history, message)
    }

    pub fn apply_draft(&self, draft_id: i64) -> AppResult<ApplyResult> {
        self.apply_draft_in(&Local, draft_id)
    }

    pub fn discard_draft(&self, draft_id: i64) -> AppResult<bool> {
        repo::discard_draft(&*lock_db(&self.db)?, draft_id)
    }

    /// One user message, answered. `history` is the conversation so far (kept by the UI only).
    pub fn send_in<Tz: TimeZone>(
        &self,
        tz: &Tz,
        history: &[ChatMessage],
        message: &str,
    ) -> AppResult<ChatReply> {
        let message = message.trim();
        if message.is_empty() || message.chars().count() > MAX_MESSAGE_CHARS {
            return Err(AppError::Invalid(format!(
                "a message is 1-{MAX_MESSAGE_CHARS} characters"
            )));
        }
        let now = self.clock.now_ms()?;
        let today = tz
            .timestamp_millis_opt(now)
            .single()
            .map(|t| fmt_date(t.date_naive()))
            .ok_or_else(|| AppError::Internal("now has no unique local time".into()))?;

        let clarifications = history
            .iter()
            .filter(|m| m.role == Speaker::Assistant)
            .count();
        let must_draft = clarifications >= MAX_CLARIFICATIONS;
        let skip = history.len().saturating_sub(MAX_HISTORY);
        let mut lines: Vec<ChatLine> = history
            .iter()
            .skip(skip)
            .map(|m| ChatLine {
                role: if m.role == Speaker::User {
                    ChatRole::User
                } else {
                    ChatRole::Assistant
                },
                text: m.text.clone(),
            })
            .collect();
        lines.push(ChatLine {
            role: ChatRole::User,
            text: message.to_string(),
        });
        let mut turn = Turn {
            lines,
            ops: Vec::new(),
            locked: false,
        };

        let mut discards = 0;
        for _ in 0..MAX_STEPS {
            let request = LlmRequest {
                purpose: Purpose::PlanChat,
                prompt: Prompt::plan_chat(&today, &turn.lines, must_draft),
                max_tokens: CHAT_MAX_TOKENS,
            };
            let response = match self.llm.complete(&request) {
                Ok(response) => response,
                Err(err) if turn.ops.is_empty() => {
                    return self.local_reply(tz, message, err.code(), now);
                }
                Err(err) => {
                    let text = "모델 응답이 끊겨서 여기까지의 초안만 만들었어요.".to_string();
                    return self.finish(&turn, text, Some(err.code().to_string()), must_draft, now);
                }
            };
            let Some(step) = parse_step(&response.text) else {
                discards += 1;
                if discards > MAX_DISCARDS {
                    let text =
                        "답변 형식을 이해하지 못해 폐기했어요. 다시 말씀해 주세요.".to_string();
                    return self.finish(&turn, text, None, must_draft, now);
                }
                continue;
            };
            match step {
                Step::Reply(text) => return self.finish(&turn, text, None, must_draft, now),
                Step::Tool(tool) => {
                    let result = self.run_tool(tz, &mut turn, &tool, now)?;
                    turn.lines.push(ChatLine {
                        role: ChatRole::Assistant,
                        text: response.text.trim().to_string(),
                    });
                    turn.lines.push(ChatLine {
                        role: ChatRole::Tool,
                        text: compact(&result),
                    });
                }
            }
        }
        let text = if turn.ops.is_empty() {
            "여기까지 알아봤어요. 원하는 날짜와 과목을 알려 주세요.".to_string()
        } else {
            "여기까지 초안으로 정리했어요.".to_string()
        };
        self.finish(&turn, text, None, must_draft, now)
    }

    fn run_tool<Tz: TimeZone>(
        &self,
        tz: &Tz,
        turn: &mut Turn,
        tool: &Tool,
        now: i64,
    ) -> AppResult<Value> {
        let mut conn = lock_db(&self.db)?;
        match tool {
            Tool::Propose(op) => self.propose(&mut conn, tz, turn, op, now),
            read => match tools::run_read(&conn, tz, read, now) {
                Ok(value) => Ok(value),
                Err(AppError::Invalid(message)) => Ok(error_result(&message)),
                Err(other) => Err(other),
            },
        }
    }

    fn propose<Tz: TimeZone>(
        &self,
        conn: &mut Connection,
        tz: &Tz,
        turn: &mut Turn,
        op: &DraftOp,
        now: i64,
    ) -> AppResult<Value> {
        if turn.ops.len() >= MAX_OPS {
            return Ok(error_result("too_many_operations"));
        }
        let mut op = op.clone();
        let mut adjusted = None;
        if crate::study::lock_held(conn)? {
            match &op {
                DraftOp::Move(m) => {
                    return self.refuse_locked(
                        conn,
                        turn,
                        Action::Move,
                        Some(m.plan_id),
                        json!({ "date": m.date, "newDate": m.new_date, "newTime": m.new_time }),
                        now,
                    );
                }
                DraftOp::Delete(d) => {
                    return self.refuse_locked(
                        conn,
                        turn,
                        Action::Delete,
                        Some(d.plan_id),
                        json!({ "date": d.date }),
                        now,
                    );
                }
                // Additions are drafted without the dry run (the plan functions refuse while the
                // lock is held); the shape is checked now and everything is checked again on apply.
                DraftOp::Create(c) => {
                    let shape = create_input(c)
                        .and_then(|_| parse_date(&c.date))
                        .and_then(|_| parse_time(&c.time));
                    if let Err(AppError::Invalid(message)) = shape {
                        return Ok(error_result(&message));
                    }
                    turn.ops.push(op);
                    return Ok(
                        json!({ "ok": true, "draft_operations": turn.ops.len(), "note": "locked_until_run_ends" }),
                    );
                }
            }
        }
        if let DraftOp::Create(c) = &mut op {
            if c.repeat_weekdays.is_none() {
                match fit_create(conn, tz, c, &turn.ops, now) {
                    Ok(Some(time)) => {
                        if time != c.time {
                            adjusted = Some(time.clone());
                            c.time = time;
                        }
                    }
                    Ok(None) => return self.no_room(conn, tz, c, now),
                    Err(AppError::Invalid(message)) => return Ok(error_result(&message)),
                    Err(other) => return Err(other),
                }
            }
        }
        let mut candidate = turn.ops.clone();
        candidate.push(op.clone());
        match dry_run(conn, tz, &candidate, now)? {
            Ok(()) => {
                turn.ops.push(op);
                let mut result = json!({ "ok": true, "draft_operations": turn.ops.len() });
                if let Some(time) = adjusted {
                    result["adjusted_time"] = Value::String(time);
                }
                Ok(result)
            }
            Err(message) => Ok(error_result(&message)),
        }
    }

    fn no_room<Tz: TimeZone>(
        &self,
        conn: &Connection,
        tz: &Tz,
        op: &CreateOp,
        now: i64,
    ) -> AppResult<Value> {
        let from = parse_date(&op.date)?;
        let suggestions = free_around(conn, tz, from, now, 60)?;
        Ok(json!({ "ok": false, "error": "no_room_that_day", "free_slots": suggestions }))
    }

    fn refuse_locked(
        &self,
        conn: &Connection,
        turn: &mut Turn,
        action: Action,
        plan_id: Option<i64>,
        attempt: Value,
        now: i64,
    ) -> AppResult<Value> {
        plan::record_chat_rejection(conn, now, action, plan_id, attempt)?;
        turn.locked = true;
        Ok(error_result("locked_during_study_run"))
    }

    fn finish(
        &self,
        turn: &Turn,
        text: String,
        fallback_reason: Option<String>,
        must_draft: bool,
        now: i64,
    ) -> AppResult<ChatReply> {
        let draft = if turn.ops.is_empty() {
            None
        } else {
            let stored = serde_json::to_string(&StoredDraft {
                ops: turn.ops.clone(),
            })
            .map_err(|e| AppError::Internal(format!("draft could not be stored: {e}")))?;
            let conn = lock_db(&self.db)?;
            repo::prune_drafts(&conn, now - DRAFT_TTL_MS)?;
            let id = repo::insert_draft(&conn, &stored, now)?;
            Some(DraftView {
                id,
                items: describe(&conn, &turn.ops)?,
            })
        };
        Ok(ChatReply {
            over_asked: must_draft && draft.is_none() && fallback_reason.is_none(),
            text,
            source: ReplySource::Llm,
            fallback_reason,
            draft,
            locked: turn.locked,
        })
    }

    /// The answer without a model: free time for "빈 시간" questions, otherwise a pointer to the
    /// calendar and the settings.
    fn local_reply<Tz: TimeZone>(
        &self,
        tz: &Tz,
        message: &str,
        code: &str,
        now: i64,
    ) -> AppResult<ChatReply> {
        let asks_free = ["빈 시간", "빈시간", "비는 시간", "비는시간"]
            .iter()
            .any(|k| message.contains(k));
        let text = if asks_free {
            let conn = lock_db(&self.db)?;
            let today = tz
                .timestamp_millis_opt(now)
                .single()
                .map(|t| t.date_naive())
                .ok_or_else(|| AppError::Internal("now has no unique local time".into()))?;
            let slots = free_around(&conn, tz, today, now, 60)?;
            let listed: Vec<String> = slots
                .iter()
                .take(FALLBACK_SLOTS)
                .filter_map(|s| {
                    Some(format!(
                        "{} {}–{}",
                        s.get("date")?.as_str()?,
                        s.get("start")?.as_str()?,
                        s.get("end")?.as_str()?
                    ))
                })
                .collect();
            if listed.is_empty() {
                "앞으로 일주일은 한 시간 이상 비는 시간이 없어요.".to_string()
            } else {
                format!("앞으로 비는 시간: {}", listed.join(", "))
            }
        } else {
            "지금은 모델을 쓸 수 없어서 계획 초안은 만들 수 없어요. 캘린더에서 직접 추가하거나, 설정에서 LLM을 켜 주세요. \"빈 시간\"은 알려 드릴 수 있어요.".to_string()
        };
        Ok(ChatReply {
            text,
            source: ReplySource::Local,
            fallback_reason: Some(code.to_string()),
            draft: None,
            locked: false,
            over_asked: false,
        })
    }

    /// Applies a stored draft in one Immediate transaction: every operation or none. While a run
    /// holds the lock nothing is applied and the attempt is recorded.
    pub fn apply_draft_in<Tz: TimeZone>(&self, tz: &Tz, draft_id: i64) -> AppResult<ApplyResult> {
        let now = self.clock.now_ms()?;
        let mut conn = lock_db(&self.db)?;
        let row = repo::get_draft(&conn, draft_id)?
            .ok_or_else(|| AppError::Invalid("that draft does not exist anymore".into()))?;
        if row.applied {
            return Err(AppError::Invalid("that draft was already applied".into()));
        }
        let stored: StoredDraft = serde_json::from_str(&row.draft_json)
            .map_err(|_| AppError::Internal("stored draft is unreadable".into()))?;
        let batch = format!("chat-{draft_id}");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if crate::study::lock_held(&tx)? {
            let action = stored.ops.first().map_or(Action::ChatbotCreate, action_of);
            plan::record_chat_rejection(
                &tx,
                now,
                action,
                None,
                json!({ "draft": draft_id, "operations": stored.ops.len() }),
            )?;
            tx.commit()?;
            return Ok(ApplyResult {
                applied: false,
                locked: true,
                changed: 0,
                batch_id: None,
            });
        }
        run_ops(&tx, tz, &stored.ops, now, &batch)?;
        repo::mark_applied(&tx, draft_id, now, &batch)?;
        tx.commit()?;
        tracing::info!(
            event = "chat_draft_applied",
            draft_id,
            operations = stored.ops.len()
        );
        Ok(ApplyResult {
            applied: true,
            locked: false,
            changed: stored.ops.len(),
            batch_id: Some(batch),
        })
    }

    /// The preview of a stored draft (after a restart of the UI, for example).
    pub fn draft_view(&self, draft_id: i64) -> AppResult<Option<DraftView>> {
        let conn = lock_db(&self.db)?;
        let Some(row) = repo::get_draft(&conn, draft_id)?.filter(|r| !r.applied) else {
            return Ok(None);
        };
        let stored: StoredDraft = serde_json::from_str(&row.draft_json)
            .map_err(|_| AppError::Internal("stored draft is unreadable".into()))?;
        Ok(Some(DraftView {
            id: row.id,
            items: describe(&conn, &stored.ops)?,
        }))
    }
}

/// Free windows over the week starting at `from`, as the tool result's JSON objects.
fn free_around<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    from: NaiveDate,
    now_ms: i64,
    min_minutes: i64,
) -> AppResult<Vec<Value>> {
    let to = from + Duration::days(FALLBACK_DAYS - 1);
    let (occurrences, _) = plan::occurrences_in(conn, tz, from - Duration::days(1), to)?;
    let slots = tools::free_slots(tz, &occurrences, from, to, now_ms, min_minutes)?;
    Ok(slots
        .into_iter()
        .take(FALLBACK_SLOTS)
        .map(|s| json!({ "date": s.date, "start": s.start, "end": s.end }))
        .collect())
}
