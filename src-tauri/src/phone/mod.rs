//! The phone side of the app (stage 5): pairing, lock leases, the blocker port and the install
//! card.
//!
//! Layout: `model` (codes, wire answers, UI views), `payload` (the sealed request bodies),
//! `port`/`rest` (the two ports - desktop `PairingApi`, phone `PhoneLink` - and their Supabase
//! adapters; mock mode runs the same adapters on the in-memory backend), `session` (the phone's
//! anonymous login), `pairing` (what the commands call), `blocker` and `apk` (the blocker port and
//! the install card), `publisher` (publishes the lock lease at every segment change, pulls the
//! pick-up counts, writes gaps) with its pure parts `gap` (heartbeat gaps) and `schedule` (what the
//! phone's common screen shows). The pure lease rules (`lock_decision`, sequence checks) come from the
//! `link-protocol` crate, shared with the phone project.
//!
//! Privacy: a phone reports counts and ids only. No payload type has a field for an app name,
//! package name, usage history, window title or URL.

pub mod allow_list;
pub mod android_blocker;
pub mod apk;
pub mod blocker;
pub mod blocker_port;
pub mod gap;
pub mod model;
pub mod pairing;
pub mod payload;
pub mod port;
pub mod publisher;
pub mod push;
pub mod rest;
pub mod schedule;
pub mod session;

#[cfg(test)]
mod publisher_tests;
#[cfg(test)]
mod tests;
