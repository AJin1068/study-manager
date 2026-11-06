//! Link to the browser extension. The extension talks to a small native-messaging host process
//! (`host`) that relays frames to this app over local IPC (`platform::browser_ipc`). Layers:
//! `protocol` (frames and messages, no I/O policy), `service` (authentication, per-connection
//! state machine, judgement, recording), and the transport behind `platform::LinkStream`, so the
//! transport can change without touching the first two.
//!
//! Failure policy (D-22): the link fails open. A lost or refused link never blocks a site; the
//! gap is recorded as `extension_disconnected` and shown as status. Tab URLs and titles are used
//! in memory only: logs and `block_events` carry the domain and a fixed reason code.

pub mod coverage;
pub mod host;
mod llm_judge;
pub mod protocol;
pub mod register;
mod service;
#[cfg(test)]
mod e2e_tests;
#[cfg(test)]
mod tests;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::clock;
use crate::commands::lock_db;
use crate::enforce;
use crate::platform;
use crate::timer::HighWater;

use service::{Service, SystemEnv};

pub const BROWSER_LINK_CHANGED_EVENT: &str = "browser_link_changed";
/// Must equal `identifier` in `tauri.conf.json` (drift-tested): the host finds the app data
/// directory from it without a Tauri runtime.
pub const BUNDLE_IDENTIFIER: &str = "com.studymanager.app";
/// Extension IDs allowed to link: the ID that `extension/public/manifest.json`'s `key` pins
/// (drift-tested). Any other ID is refused.
pub const ALLOWED_EXTENSION_IDS: &[&str] = &["manoifclikefdhhomeklmdjbeknmloem"];
const MAX_CONNECTIONS: usize = 4;

/// What `get_browser_link_status` and `browser_link_changed` carry.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinkStatus {
    /// The local listener is up. False means no extension can ever connect (see `transport_error`).
    pub transport_active: bool,
    pub transport_error: Option<String>,
    pub connected: bool,
    pub connections: u32,
    pub last_connected_at_ms: Option<i64>,
    pub last_disconnected_at_ms: Option<i64>,
    /// Fixed reason code of the latest disconnect, never free text.
    pub last_disconnect_reason: Option<String>,
}

type OnChange = Box<dyn Fn(LinkStatus) + Send + Sync>;

/// Shared connection state. The change callback runs after the lock is released.
pub struct LinkHub {
    status: Mutex<LinkStatus>,
    on_change: OnChange,
}

impl LinkHub {
    pub fn new(on_change: OnChange) -> LinkHub {
        LinkHub { status: Mutex::new(LinkStatus::default()), on_change }
    }

    /// Emits `browser_link_changed` to the UI on every change.
    pub fn for_app(app: AppHandle) -> LinkHub {
        LinkHub::new(Box::new(move |status| {
            if let Err(err) = app.emit(BROWSER_LINK_CHANGED_EVENT, status) {
                tracing::error!(error = %err, "failed to emit browser link status");
            }
        }))
    }

    pub fn status(&self) -> LinkStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    fn update(&self, change: impl FnOnce(&mut LinkStatus)) {
        let snapshot = {
            let mut guard = self.status.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            change(&mut guard);
            guard.clone()
        };
        (self.on_change)(snapshot);
    }

    pub(crate) fn set_transport(&self, error: Option<String>) {
        self.update(|s| {
            s.transport_active = error.is_none();
            s.transport_error = error;
        });
    }

    pub(crate) fn connected(&self, at_ms: i64) {
        self.update(|s| {
            s.connections += 1;
            s.connected = true;
            s.last_connected_at_ms = Some(at_ms);
        });
    }

    pub(crate) fn disconnected(&self, at_ms: i64, reason: &'static str) {
        self.update(|s| {
            s.connections = s.connections.saturating_sub(1);
            s.connected = s.connections > 0;
            s.last_disconnected_at_ms = Some(at_ms);
            s.last_disconnect_reason = Some(reason.to_owned());
        });
    }
}

/// Starts the listener thread. A transport that cannot start is reported in the status and the
/// app carries on: the link is fail-open, so this never affects study enforcement.
pub fn spawn(
    db: Arc<Mutex<Connection>>,
    high_water: Arc<HighWater>,
    hub: Arc<LinkHub>,
    llm: Arc<crate::llm::service::LlmService>,
    data_dir: PathBuf,
) {
    // Links the previous run never closed (kill, crash) would otherwise read as watched forever.
    let closed = lock_db(&db).and_then(|conn| {
        let mode = enforce::read_config(&conn)?.mode;
        coverage::close_stale_links(&conn, clock::now_ms()?, mode)
    });
    if let Err(err) = closed {
        tracing::error!(error = %err, "failed to close stale browser links");
    }
    let started = platform::ensure_token(&data_dir)
        .and_then(|token| platform::bind_listener(&data_dir).map(|listener| (token, listener)));
    let (token, listener) = match started {
        Ok(pair) => pair,
        Err(err) => {
            tracing::warn!(error = %err, "browser link transport unavailable; browser blocking is off");
            hub.set_transport(Some(err.to_string()));
            return;
        }
    };
    hub.set_transport(None);
    let service = Arc::new(Service::new(
        Arc::clone(&db),
        Arc::clone(&hub),
        Arc::new(SystemEnv::new(high_water)),
        // Rules first; the LLM judge only ever acts on tabs the rules leave open, and only when the
        // user switched it on (llm.judge_enabled, default off).
        Arc::new(llm_judge::LlmTabJudge::new(
            Arc::new(service::RulesJudge::new(Arc::clone(&db))),
            db,
            llm,
            Arc::new(crate::remote::SystemClock),
            Arc::new(llm_judge::ThreadRunner),
        )),
        token,
        ALLOWED_EXTENSION_IDS.iter().map(|id| (*id).to_owned()).collect(),
    ));
    let active = Arc::new(AtomicUsize::new(0));
    thread::spawn(move || loop {
        let mut stream = match listener.accept_stream() {
            Ok(stream) => stream,
            Err(err) => {
                tracing::error!(error = %err, "browser link accept failed");
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            tracing::warn!(reason = "too_many_connections", "browser link connection refused");
            continue;
        }
        let service = Arc::clone(&service);
        let active = Arc::clone(&active);
        thread::spawn(move || {
            if catch_unwind(AssertUnwindSafe(|| service.serve(stream.as_mut()))).is_err() {
                tracing::error!("browser link connection thread panicked");
            }
            active.fetch_sub(1, Ordering::SeqCst);
        });
    });
}
