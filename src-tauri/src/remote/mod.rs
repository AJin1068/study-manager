//! Ports to the outside world (server, notify channel, LLM, push) and the shared machinery around
//! them: mode resolution, HTTP with a host policy, sealed outbound payloads, secrets, sign-in.
//!
//! Every network port has three states the user can see: not configured, mock (nothing leaves the
//! device), live. Callers hold a port and never learn which adapter is behind it. Failures of an
//! integration never weaken locking, timing, or blocking: local features run without any of this.

pub mod audit;
pub mod auth;
pub mod config;
pub mod fake;
pub mod http;
pub mod notify;
pub mod payload;
pub mod qr;
pub mod runtime;
pub mod secret_flow;
pub mod secrets;
pub mod supabase;

use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Time source for the remote layer, injectable so token expiry, cooldowns and retry spacing are
/// tested without sleeping.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> AppResult<i64>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> AppResult<i64> {
        crate::clock::now_ms()
    }
}

/// Settable clock for tests and the fake backend.
pub struct ManualClock(AtomicI64);

impl ManualClock {
    pub fn new(start_ms: i64) -> Self {
        Self(AtomicI64::new(start_ms))
    }

    pub fn set(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }

    pub fn advance(&self, delta_ms: i64) {
        self.0.fetch_add(delta_ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> AppResult<i64> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

/// Network ports that have the mode machinery. Secrets and the phone bridge choose an adapter
/// without a user-visible mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationPort {
    Supabase,
    Notify,
    Llm,
    Push,
}

impl IntegrationPort {
    pub const ALL: [IntegrationPort; 4] = [
        IntegrationPort::Supabase,
        IntegrationPort::Notify,
        IntegrationPort::Llm,
        IntegrationPort::Push,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            IntegrationPort::Supabase => "supabase",
            IntegrationPort::Notify => "notify",
            IntegrationPort::Llm => "llm",
            IntegrationPort::Push => "push",
        }
    }
}

/// What the user chose (`integration.<port>.mode`). `auto` is the default; mock must be picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModeSetting {
    Auto,
    Mock,
}

impl ModeSetting {
    pub fn as_str(self) -> &'static str {
        match self {
            ModeSetting::Auto => "auto",
            ModeSetting::Mock => "mock",
        }
    }

    pub fn parse(raw: &str) -> AppResult<Self> {
        match raw {
            "auto" => Ok(ModeSetting::Auto),
            "mock" => Ok(ModeSetting::Mock),
            other => Err(AppError::Invalid(format!(
                "unknown integration mode {other:?}"
            ))),
        }
    }
}

/// What the port actually runs as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationMode {
    NotConfigured,
    Mock,
    Live,
}

/// `auto` goes live only when the port is configured; otherwise it stays visibly unconfigured
/// (a missing key is never answered by silently using the mock). `mock` always runs the mock,
/// configured or not, so a configured port can be kept offline on purpose.
pub fn resolve_mode(setting: ModeSetting, configured: bool) -> IntegrationMode {
    match (setting, configured) {
        (ModeSetting::Mock, _) => IntegrationMode::Mock,
        (ModeSetting::Auto, true) => IntegrationMode::Live,
        (ModeSetting::Auto, false) => IntegrationMode::NotConfigured,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Health {
    Ok,
    Degraded { code: String },
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationState {
    pub port: IntegrationPort,
    /// The stored choice, so the UI can show the toggle without guessing.
    pub setting: ModeSetting,
    pub mode: IntegrationMode,
    pub configured: bool,
    pub health: Health,
    pub last_ok_ms: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_mode_table() {
        use IntegrationMode::{Live, Mock, NotConfigured};
        use ModeSetting::{Auto, Mock as MockSetting};
        let table = [
            (Auto, true, Live),
            (Auto, false, NotConfigured),
            (MockSetting, true, Mock),
            (MockSetting, false, Mock),
        ];
        for (setting, configured, expected) in table {
            assert_eq!(
                resolve_mode(setting, configured),
                expected,
                "{setting:?} configured={configured}"
            );
        }
    }

    #[test]
    fn mode_setting_text_round_trips_and_rejects_live() {
        for mode in [ModeSetting::Auto, ModeSetting::Mock] {
            assert_eq!(ModeSetting::parse(mode.as_str()).expect("parse"), mode);
        }
        assert!(
            ModeSetting::parse("live").is_err(),
            "live is derived from configuration, never chosen"
        );
    }

    #[test]
    fn state_serializes_with_camel_case_tags() {
        let state = IntegrationState {
            port: IntegrationPort::Supabase,
            setting: ModeSetting::Auto,
            mode: IntegrationMode::NotConfigured,
            configured: false,
            health: Health::Degraded {
                code: "auth_expired".into(),
            },
            last_ok_ms: None,
        };
        let json = serde_json::to_value(&state).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "port": "supabase", "setting": "auto", "mode": "notConfigured", "configured": false,
                "health": {"kind": "degraded", "code": "auth_expired"}, "lastOkMs": null
            })
        );
    }
}
