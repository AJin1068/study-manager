//! Planning chatbot (PLAN 14). The model talks to the app through a small tool protocol: it reads
//! the calendar, the focus summary, and the wiki's weak pages, and it *proposes* plan changes. A
//! proposal is only a draft. Nothing in the plan tables changes until the user applies the draft,
//! and applying goes through the same plan functions (and the same study lock) as a manual edit.
//!
//! * `protocol`: the strict shape of what the model may say; anything else is discarded.
//! * `tools`: the read-only tools and the free-slot rule.
//! * `service`: the turn loop, drafts, and applying them.
//!
//! The conversation lives in the UI's memory only. The database holds drafts (the plan operations,
//! no chat text). Without a usable LLM the chat answers from local rules.

pub mod protocol;
pub mod service;
pub mod tools;

#[cfg(test)]
mod tests;
