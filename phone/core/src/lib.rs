//! The host-testable core of the phone app (stage 5): everything the phone does that needs no
//! Android API. The Tauri shell (`phone/src-tauri`) and the Kotlin blocker plugin
//! (`phone/plugins/blocker/android`) are thin around it.
//!
//! * `http`, `session`, `rest`, `pairing`: the Supabase REST contract (`fixtures/phone/rpc-contract.json`,
//!   `fixtures/http/`): anonymous sign-in, `claim_pair`, lease fetch, heartbeat. The phone talks to
//!   nothing else.
//! * `poller`: fetches the lease, lets `link_protocol::LeaseHolder` decide with the MONOTONIC
//!   clock, drives the blocker, sends heartbeats (a switched-off service is a heartbeat with
//!   `service_enabled: false`, which the server turns into a `phone_gap`).
//! * `allow_list`, `blocker`, `blocker_port`, `android_blocker`, `platform`: the blocker port and
//!   its bridge to the Kotlin plugin; iOS is `Unsupported` (no Swift exists).
//! * `schedule`: what the schedule/timer/diagnostics screens show.
//!
//! This crate does NOT depend on `src-tauri` (macOS-only dependencies break the Android targets);
//! `allow_list`, `blocker*` and `android_blocker` are therefore a second copy of the files of the
//! same name in `src-tauri/src/phone/`, tied together only by `fixtures/phone/blocker-bridge.json`
//! which both copies' tests read. Change one, change the other.
//!
//! Nothing here has run on a device (V5-1, V5-4, V5-5).

pub mod allow_list;
pub mod android_blocker;
pub mod blocker;
pub mod blocker_port;
pub mod http;
pub mod model;
pub mod pairing;
pub mod platform;
pub mod poller;
pub mod rest;
pub mod schedule;
pub mod session;

#[cfg(test)]
mod testing;

/// Time sources. `mono_ms` must keep counting while the device sleeps and must not be settable
/// by the user (Android: `CLOCK_BOOTTIME`, the same clock as `SystemClock.elapsedRealtime()`);
/// lease expiry is measured on it. `wall_ms` is used only for access-token expiry.
pub trait Clock: Send + Sync {
    fn mono_ms(&self) -> i64;
    fn wall_ms(&self) -> i64;
}
