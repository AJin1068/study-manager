//! Study rooms (stage 4): a small group of friends sees each other's level and study status.
//!
//! Layout: `model` (wire + UI types), `code` (invite codes and the join deep link), `port`/`rest`
//! (the ports and their one Supabase adapter), `prefs`/`local` (what this user shares and the local
//! facts it is computed from), `status` (publisher), `feed` (polling), `view`/`ranking` (pure view
//! building), `together` (copying the owner's timetable). Only status, counts, level, plan names and
//! cosmetic ids ever leave the PC.

pub mod code;
pub mod feed;
pub mod local;
pub mod model;
pub mod port;
pub mod prefs;
pub mod ranking;
pub mod realtime;
pub mod rest;
pub mod service;
pub mod status;
pub mod together;
pub mod view;

#[cfg(test)]
mod scenarios;
#[cfg(test)]
pub mod testkit;
