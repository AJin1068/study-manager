//! Profile home and gifts (stage 4, S4.9-S4.11): a public face the user chooses field by field, a
//! guestbook, visitor counts, and clothes or furniture given between room mates.
//!
//! Layout: `model` (codes, sections, scopes, wire and UI types), `payload` (the sealed request
//! bodies), `port`/`rest` (the one port and its one Supabase adapter; mock mode runs the same
//! adapter on the in-memory backend), `publish` (builds what leaves the PC from the local facts and
//! the scopes), `gift` (ledger-backed sending with refunds only after a certain failure), `link`
//! (the web profile link) and `service` (what the commands call).
//!
//! Privacy: every section defaults to private; a private section is replaced by an empty value
//! before the payload is built, so it never leaves the PC and the server deletes what it held.
//! Deviation statistics have no field in any type here.

pub mod gift;
pub mod link;
pub mod model;
pub mod payload;
pub mod port;
pub mod publish;
pub mod rest;
pub mod service;

#[cfg(test)]
mod tests;
