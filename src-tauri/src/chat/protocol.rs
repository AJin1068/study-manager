//! What the model may say, and what a draft is made of.
//!
//! One answer is one JSON object: `{"reply": "..."}` or `{"tool": "...", "args": {...}}`. Unknown
//! keys, unknown tools, unknown arguments, wrong types, and over-long text all discard the answer.
//! Plan operations carry no path, command, or URL field, so there is nothing for an injected
//! document to aim at.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Longest reply the app shows from the model.
pub const MAX_REPLY_CHARS: usize = 1_000;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeArgs {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreeSlotArgs {
    pub from: String,
    pub to: String,
    pub min_minutes: Option<i64>,
}

/// A new plan (or a repeating one).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOp {
    pub name: String,
    pub subject: String,
    pub date: String,
    pub time: String,
    pub sets: i64,
    /// 1 = Monday .. 7 = Sunday.
    pub repeat_weekdays: Option<Vec<u8>>,
    pub repeat_until: Option<String>,
}

/// Move one occurrence to another day and/or time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveOp {
    pub plan_id: i64,
    /// The occurrence's current date.
    pub date: String,
    pub new_date: Option<String>,
    pub new_time: Option<String>,
}

/// Cancel one occurrence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteOp {
    pub plan_id: i64,
    pub date: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DraftOp {
    Create(CreateOp),
    Move(MoveOp),
    Delete(DeleteOp),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tool {
    ListSubjects,
    ListPlans(RangeArgs),
    ListFreeSlots(FreeSlotArgs),
    GetFocusSummary,
    GetWikiWeakPages,
    Propose(DraftOp),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Reply(String),
    Tool(Tool),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

fn args<T: for<'de> Deserialize<'de>>(value: Value) -> Option<T> {
    serde_json::from_value(value).ok()
}

/// The step in a model answer, or `None` when the answer does not follow the protocol.
pub fn parse_step(answer: &str) -> Option<Step> {
    let Value::Object(mut map) = serde_json::from_str::<Value>(answer.trim()).ok()? else { return None };
    if let Some(reply) = map.remove("reply") {
        let text = reply.as_str()?.trim().to_string();
        let ok = map.is_empty() && !text.is_empty() && text.chars().count() <= MAX_REPLY_CHARS;
        return ok.then_some(Step::Reply(text));
    }
    let name = map.remove("tool")?.as_str()?.to_string();
    let value = map.remove("args").unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    if !map.is_empty() {
        return None;
    }
    let tool = match name.as_str() {
        "list_subjects" => args::<NoArgs>(value).map(|_| Tool::ListSubjects)?,
        "get_focus_summary" => args::<NoArgs>(value).map(|_| Tool::GetFocusSummary)?,
        "get_wiki_weak_pages" => args::<NoArgs>(value).map(|_| Tool::GetWikiWeakPages)?,
        "list_plans" => Tool::ListPlans(args(value)?),
        "list_free_slots" => Tool::ListFreeSlots(args(value)?),
        "propose_plan" => Tool::Propose(DraftOp::Create(args(value)?)),
        "propose_move" => Tool::Propose(DraftOp::Move(args(value)?)),
        "propose_delete" => Tool::Propose(DraftOp::Delete(args(value)?)),
        _ => return None,
    };
    Some(Step::Tool(tool))
}
