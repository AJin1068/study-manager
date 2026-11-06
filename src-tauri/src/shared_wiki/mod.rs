//! Shared wiki of a study room (S4.7).
//!
//! A wiki page leaves the PC only when the user explicitly marked it as shared with a room
//! (`store::shared_wiki` holds the marks; no mark, no payload). Others' contributions to the same
//! page title are kept side by side as append-only revisions on the server; `merge` asks an LLM to
//! combine them, but only the contributions whose author separately agreed to LLM merging, and the
//! contributor list is kept. `concepts` tells the user about friends' pages whose concept they have
//! not written themselves, by comparing page names locally. `quiz` makes mutual questions from a
//! shared page without any LLM, and `summary` writes the day's recap from what the room shared.
//!
//! Layout: `model` (codes, wire types), `payload` (reviewed RPC argument structs), `port`/`rest`
//! (the port and its one Supabase adapter; mock mode runs it on the in-memory backend), `merge`,
//! `concepts`, `quiz`, `summary` (pure), `copy` (the local `shared/` copy), `service`.

pub mod concepts;
pub mod copy;
pub mod merge;
pub mod model;
pub mod payload;
pub mod port;
pub mod quiz;
pub mod rest;
pub mod service;
pub mod summary;

#[cfg(test)]
mod scenarios;
