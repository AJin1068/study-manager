//! Cheer and scold messages shown while a study block runs. Phrases are code constants
//! (`templates`), picked by tone, kind, and a seed; when the wiki has a page worth reviewing, a
//! wiki phrase naming its concept replaces the general one. Nothing here calls the LLM.
//!
//! - `config`: the `messages.*` settings (tone, frequency, events, system notification).
//! - `engine`: whether a message is due now (study block only, event or random interval, hourly cap).
//! - `service`: builds the message, logs it, and delivers it; `service::spawn` runs it.
//!
//! The in-app toast may name a wiki concept. The system notification never does: it can appear
//! on a lock screen, so it always carries the general phrase (no subject, no page name).

pub mod config;
pub mod engine;
pub mod service;
pub mod templates;

use serde::{Deserialize, Serialize};

/// Five steps from gentle to harsh. Matches the `message_log.tone` CHECK (drift-tested).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Gentle,
    Warm,
    Normal,
    Firm,
    Harsh,
}

impl Tone {
    pub const ALL: [Tone; 5] = [
        Tone::Gentle,
        Tone::Warm,
        Tone::Normal,
        Tone::Firm,
        Tone::Harsh,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Tone::Gentle => "gentle",
            Tone::Warm => "warm",
            Tone::Normal => "normal",
            Tone::Firm => "firm",
            Tone::Harsh => "harsh",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == raw)
    }
}

/// What the message is about. Matches the `message_log.kind` CHECK (drift-tested).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A blocked app or site was tried.
    Distraction,
    /// A set was finished and the days in a row reached the streak.
    Streak,
    /// A set was finished.
    SetEnd,
    /// The random interval ran out.
    Random,
}

impl Kind {
    #[cfg(test)]
    pub const ALL: [Kind; 4] = [Kind::Distraction, Kind::Streak, Kind::SetEnd, Kind::Random];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Distraction => "distraction",
            Kind::Streak => "streak",
            Kind::SetEnd => "set_end",
            Kind::Random => "random",
        }
    }
}

/// Where the text came from. Matches the `message_log.source` CHECK (drift-tested). `Llm` is
/// reserved for a polished phrase and is not produced yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Template,
    Wiki,
    #[allow(dead_code)]
    Llm,
}

impl Source {
    #[cfg(test)]
    pub const ALL: [Source; 3] = [Source::Template, Source::Wiki, Source::Llm];

    pub fn as_str(self) -> &'static str {
        match self {
            Source::Template => "template",
            Source::Wiki => "wiki",
            Source::Llm => "llm",
        }
    }
}

/// What the window shows (`message://cheer`). `notice` is the lock-screen-safe phrase for the
/// system notification and is not sent to the window.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub kind: Kind,
    pub tone: Tone,
    pub source: Source,
    pub text: String,
    #[serde(skip)]
    pub notice: String,
}

/// The Tauri event carrying a message to the window.
pub const MESSAGE_EVENT: &str = "message://cheer";

/// Builds the message for `kind`. `concept` is a wiki page worth reviewing; it is used for the
/// kinds that can ask a review question (not for a distraction, which stays short).
pub fn compose(kind: Kind, tone: Tone, concept: Option<&str>, seed: u64) -> Message {
    let notice = templates::general(kind, tone, seed).to_string();
    let wiki = concept
        .filter(|_| kind != Kind::Distraction)
        .map(|c| templates::wiki(tone, c, seed));
    match wiki {
        Some(text) => Message {
            kind,
            tone,
            source: Source::Wiki,
            text,
            notice,
        },
        None => Message {
            kind,
            tone,
            source: Source::Template,
            text: notice.clone(),
            notice,
        },
    }
}
