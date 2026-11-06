//! Question bank and quiz engine (stage 3-5..3-7). Questions are generated ahead of time from wiki
//! pages (`generate`), so showing one never needs the LLM and works offline. Answers are held and
//! graded in Rust (`grade`); the UI only ever receives the prompt and the choices. What the user
//! answered is never stored or logged: only whether it was right, whether it was skipped, and how
//! long it took.
//!
//! - `select`: which questions to ask (deterministic for a seed and a clock).
//! - `release`: the early-release gate (attempt cap, cooldown, proof choice) used by
//!   `timer::release::ReleaseDesk`. Quiz proofs exist for early release only.
//! - `gate`: review questions that never lift a block (blocked app, random check-in, session end)
//!   and their bonus points.
//! - `service`: bank generation from pages (local template generator, optionally the LLM).
//! - `watch`: offers the distraction card by reading the block-event log (read-only).

pub mod config;
pub mod gate;
pub mod generate;
pub mod grade;
pub mod release;
pub mod select;
pub mod service;
pub mod watch;

#[cfg(test)]
pub mod testkit;
#[cfg(test)]
mod tests;

use serde::Serialize;

/// Questions a card asks, and how many an early release needs right (all of them).
pub const RELEASE_QUESTIONS: usize = 3;
pub const SESSION_END_QUESTIONS: usize = 3;
/// A question that was asked is not asked again for this long (when enough others exist).
pub const REASK_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Mcq,
    Cloze,
}

impl ItemKind {
    #[cfg(test)]
    pub const ALL: [ItemKind; 2] = [ItemKind::Mcq, ItemKind::Cloze];

    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Mcq => "mcq",
            ItemKind::Cloze => "cloze",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "mcq" => Some(ItemKind::Mcq),
            "cloze" => Some(ItemKind::Cloze),
            _ => None,
        }
    }
}

/// Where a question was asked. Matches the `quiz_attempts.context` CHECK (drift-tested).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Context {
    Distraction,
    Release,
    SessionEnd,
    Checkin,
    Review,
}

impl Context {
    #[cfg(test)]
    pub const ALL: [Context; 5] = [Context::Distraction, Context::Release, Context::SessionEnd, Context::Checkin, Context::Review];

    pub fn as_str(self) -> &'static str {
        match self {
            Context::Distraction => "distraction",
            Context::Release => "release",
            Context::SessionEnd => "session_end",
            Context::Checkin => "checkin",
            Context::Review => "review",
        }
    }
}

/// A question as stored. Holds the answer, so it is never serialized to the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuizItem {
    pub id: i64,
    pub page_id: i64,
    pub kind: ItemKind,
    pub prompt: String,
    pub choices: Vec<String>,
    pub answer_key: String,
}

/// What the UI sees of a question: no answer, no evidence.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuizQuestion {
    pub item_id: i64,
    pub kind: ItemKind,
    pub prompt: String,
    /// Empty for a fill-in-the-blank question.
    pub choices: Vec<String>,
}

impl QuizItem {
    /// Choices are shown in a stable order that differs per question and per draw `seed`, so the
    /// right one is not always in the same place.
    pub fn view(&self, seed: u64) -> QuizQuestion {
        let mut choices = self.choices.clone();
        choices.sort_by_key(|c| select::mix(seed ^ select::hash_str(c)));
        QuizQuestion { item_id: self.id, kind: self.kind, prompt: self.prompt.clone(), choices }
    }
}
