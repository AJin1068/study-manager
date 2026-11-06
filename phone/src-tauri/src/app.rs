//! The phone app's state: the persisted pairing, the poller built from it and the commands the UI
//! calls. Everything that needs Tauri is in `lib.rs`; this file takes its transport, clock and
//! plugin bridge as arguments so the host tests can script all three.
//!
//! The pairing (endpoint, public key, refresh token) lives in `phone-state.json` in the app's
//! private data directory. The refresh token is the only secret; it is not in the QR code and is
//! rotated by every renewal, so it is written back after each poll. Android's app sandbox is the
//! only protection of that file (no keystore use): a rooted phone can read it.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use study_phone_core::allow_list::AllowList;
use study_phone_core::android_blocker::{BridgeEvent, PluginBridge};
use study_phone_core::blocker_port::PhoneBlocker;
use study_phone_core::http::HttpTransport;
use study_phone_core::model::PhoneError;
use study_phone_core::pairing::{StoredPairing, pair_with_offer, restore};
use study_phone_core::platform::platform_blocker;
use study_phone_core::poller::{PhonePoller, PollReport};
use study_phone_core::rest::{PhoneLink, RestPhoneLink};
use study_phone_core::schedule::PhoneSchedule;
use study_phone_core::session::{AuthError, PhoneSession};
use study_phone_core::Clock;

const STATE_FILE: &str = "phone-state.json";

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    pairing: Option<StoredPairing>,
    /// The allow list in its wire form. It stays on this device.
    allow: Option<Value>,
}

struct Runtime {
    poller: PhonePoller,
    session: Arc<PhoneSession>,
    link: Arc<RestPhoneLink>,
}

#[derive(Default)]
struct Inner {
    persisted: Persisted,
    runtime: Option<Runtime>,
    pairing_lost: bool,
    startup_error: Option<&'static str>,
}

/// What the UI renders. The nested `report` keeps the core's snake_case field names.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PhoneView {
    pub paired: bool,
    /// The stored identity was refused: the user must pair again (a new identity is never made silently).
    pub pairing_lost: bool,
    pub report: Option<PollReport>,
    pub platform: &'static str,
    pub last_error: Option<&'static str>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Ack {
    pub code: &'static str,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct ScheduleView {
    pub items: PhoneSchedule,
    /// Why the list may be empty. There is no server path that gives a paired phone the plan rows
    /// yet (the phone's grants cover `phone_*` tables only), so this is always `no_feed`.
    pub source: &'static str,
}

pub struct PhoneApp {
    dir: PathBuf,
    transport: Arc<dyn HttpTransport>,
    clock: Arc<dyn Clock>,
    blocker: Arc<dyn PhoneBlocker>,
    inner: Mutex<Inner>,
}

fn platform() -> &'static str {
    if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "ios") {
        "ios"
    } else {
        "other"
    }
}

impl PhoneApp {
    /// Reads the persisted state (no network). A missing or unreadable file is "not paired".
    pub fn new(dir: PathBuf, transport: Arc<dyn HttpTransport>, clock: Arc<dyn Clock>, bridge: Arc<dyn PluginBridge>) -> Self {
        let persisted = fs::read(dir.join(STATE_FILE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { dir, transport, clock, blocker: platform_blocker(bridge), inner: Mutex::new(Inner { persisted, ..Inner::default() }) }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn save(&self, persisted: &Persisted) -> Result<(), &'static str> {
        fs::create_dir_all(&self.dir).map_err(|_| "storage")?;
        let bytes = serde_json::to_vec(persisted).map_err(|_| "storage")?;
        let tmp = self.dir.join(format!("{STATE_FILE}.tmp"));
        fs::write(&tmp, bytes).map_err(|_| "storage")?;
        fs::rename(&tmp, self.dir.join(STATE_FILE)).map_err(|_| "storage")
    }

    fn list_of(persisted: &Persisted) -> AllowList {
        persisted.allow.as_ref().and_then(|wire| AllowList::from_wire(wire).ok()).unwrap_or_default()
    }

    fn make_runtime(&self, persisted: &Persisted, link: Arc<RestPhoneLink>, session: Arc<PhoneSession>) -> Runtime {
        let poller = PhonePoller::new(link.clone(), self.blocker.clone(), self.clock.clone(), Self::list_of(persisted));
        Runtime { poller, session, link }
    }

    fn view_of(inner: &Inner) -> PhoneView {
        PhoneView {
            paired: inner.persisted.pairing.is_some() && !inner.pairing_lost,
            pairing_lost: inner.pairing_lost,
            report: inner.runtime.as_ref().map(|r| r.poller.snapshot()),
            platform: platform(),
            last_error: inner.startup_error,
        }
    }

    /// The state as last known; no network call.
    pub fn view(&self) -> PhoneView {
        Self::view_of(&self.inner())
    }

    /// One poll round. After a restart the session is restored from the stored refresh token
    /// first; while that fails (offline) the plugin's own deadline keeps governing the lock.
    pub fn tick(&self) -> PhoneView {
        let mut guard = self.inner();
        let inner = &mut *guard;
        if inner.runtime.is_none() && !inner.pairing_lost {
            if let Some(stored) = inner.persisted.pairing.clone() {
                match restore(&stored, self.transport.clone(), self.clock.clone()) {
                    Ok((link, session)) => {
                        inner.runtime = Some(self.make_runtime(&inner.persisted, link, session));
                        inner.startup_error = None;
                    }
                    Err(PhoneError::Auth(AuthError::Expired)) => inner.pairing_lost = true,
                    Err(err) => inner.startup_error = Some(err.code()),
                }
            }
        }
        if let Some(runtime) = inner.runtime.as_mut() {
            runtime.poller.tick();
            // The refresh token rotates with every renewal: keep the newest one.
            let current = runtime.session.refresh_token();
            let stored = inner.persisted.pairing.as_ref().map(|p| p.refresh_token.clone());
            if let (Some(current), Some(pairing)) = (current, inner.persisted.pairing.as_mut()) {
                if stored.as_deref() != Some(current.as_str()) {
                    pairing.refresh_token = current;
                    if self.save(&inner.persisted).is_err() {
                        inner.startup_error = Some("storage");
                    }
                }
            }
        }
        Self::view_of(inner)
    }

    /// Pair-by-QR: `text` is what the scanner read.
    pub fn pair(&self, text: &str) -> Ack {
        let paired = match pair_with_offer(text, self.transport.clone(), self.clock.clone()) {
            Ok(paired) => paired,
            Err(err) => return Ack { code: err.code() },
        };
        let mut guard = self.inner();
        let inner = &mut *guard;
        inner.persisted.pairing = Some(paired.stored);
        if self.save(&inner.persisted).is_err() {
            inner.persisted.pairing = None;
            return Ack { code: "storage" };
        }
        inner.pairing_lost = false;
        inner.startup_error = None;
        inner.runtime = Some(self.make_runtime(&inner.persisted, paired.link, paired.session));
        Ack { code: "ok" }
    }

    /// Ends the pairing from the phone. Refused while the lock holds: unpairing frees the phone, so
    /// it would be a way around the lock (clearing the app's data still is one; see the docs).
    pub fn unpair(&self) -> Ack {
        let mut guard = self.inner();
        let inner = &mut *guard;
        if let Some(runtime) = inner.runtime.as_ref() {
            if runtime.poller.snapshot().diagnostics.lock == link_protocol::PhoneLock::Locked {
                return Ack { code: "locked" };
            }
            match runtime.link.leave() {
                Ok(()) | Err(PhoneError::Rejected(study_phone_core::model::PhoneCode::NotPaired | study_phone_core::model::PhoneCode::NotFound)) => {}
                Err(err) => return Ack { code: err.code() },
            }
        }
        inner.persisted.pairing = None;
        inner.runtime = None;
        inner.pairing_lost = false;
        match self.save(&inner.persisted) {
            Ok(()) => Ack { code: "ok" },
            Err(code) => Ack { code },
        }
    }

    /// A plugin event (`fixtures/phone/blocker-bridge.json`, section `events`).
    pub fn blocker_event(&self, raw: &Value) -> Ack {
        let Ok(event) = BridgeEvent::parse(raw) else { return Ack { code: "bridge_malformed" } };
        if let Some(runtime) = self.inner().runtime.as_mut() {
            runtime.poller.on_event(event);
        }
        Ack { code: "ok" }
    }

    pub fn allow_list(&self) -> Value {
        Self::list_of(&self.inner().persisted).to_wire()
    }

    pub fn set_allow_list(&self, wire: &Value) -> Ack {
        let list = match AllowList::from_wire(wire) {
            Ok(list) => list,
            Err(err) => return Ack { code: err.code() },
        };
        let mut guard = self.inner();
        let inner = &mut *guard;
        inner.persisted.allow = Some(list.to_wire());
        if let Some(runtime) = inner.runtime.as_mut() {
            runtime.poller.set_allow_list(list);
        }
        match self.save(&inner.persisted) {
            Ok(()) => Ack { code: "ok" },
            Err(code) => Ack { code },
        }
    }

    pub fn schedule(&self) -> ScheduleView {
        ScheduleView { items: PhoneSchedule::default(), source: "no_feed" }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicI64, Ordering};

    use serde_json::json;
    use study_phone_core::android_blocker::{BridgeFailure, ScriptedBridge};
    use study_phone_core::http::{HttpRequest, HttpResponse, TransportError};

    use super::*;

    struct Script(Mutex<VecDeque<Result<HttpResponse, TransportError>>>, Mutex<Vec<HttpRequest>>);

    impl HttpTransport for Script {
        fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.1.lock().expect("seen").push(request);
            self.0.lock().expect("script").pop_front().unwrap_or(Err(TransportError::Network))
        }
    }

    struct Clk(AtomicI64);

    impl Clock for Clk {
        fn mono_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }

        fn wall_ms(&self) -> i64 {
            1_000_000
        }
    }

    fn reply(body: &str) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse { status: 200, headers: Vec::new(), body: body.as_bytes().to_vec() })
    }

    fn tokens(refresh: &str) -> Result<HttpResponse, TransportError> {
        reply(&format!(r#"{{"access_token":"a-{refresh}","expires_in":3600,"refresh_token":"{refresh}","user":{{"id":"phone-user"}}}}"#))
    }

    fn app(dir: &PathBuf, script: Vec<Result<HttpResponse, TransportError>>, bridge: Vec<Result<Value, BridgeFailure>>) -> (PhoneApp, Arc<Script>) {
        let (app, transport, _) = app_with_clock(dir, script, bridge);
        (app, transport)
    }

    fn app_with_clock(dir: &PathBuf, script: Vec<Result<HttpResponse, TransportError>>, bridge: Vec<Result<Value, BridgeFailure>>) -> (PhoneApp, Arc<Script>, Arc<Clk>) {
        let transport = Arc::new(Script(Mutex::new(script.into()), Mutex::new(Vec::new())));
        let clock = Arc::new(Clk(AtomicI64::new(100)));
        let app = PhoneApp::new(dir.clone(), transport.clone(), clock.clone(), Arc::new(ScriptedBridge::new(bridge)));
        (app, transport, clock)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("study-phone-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn qr() -> String {
        let wire: Value = serde_json::from_str(include_str!("../../../fixtures/phone/link-protocol.json")).expect("fixture");
        wire["pairingOfferQrText"].as_str().expect("qr").to_string()
    }

    const CLAIMED: &str = r#"{"code":"ok","pair_id":"p","owner_user_id":"o","server_now_ms":1}"#;

    #[test]
    fn pairing_stores_the_identity_and_a_new_process_restores_it_without_signing_up_again() {
        let dir = temp_dir("pair");
        let (first, _) = app(&dir, vec![tokens("r1"), reply(CLAIMED)], vec![]);
        assert_eq!(first.view().paired, false);
        assert_eq!(first.pair(&qr()), Ack { code: "ok" });
        assert!(first.view().paired);

        let (second, transport) = app(&dir, vec![tokens("r2"), reply(r#"{"code":"ok","lease":null,"server_now_ms":5}"#), reply(r#"{"code":"ok"}"#)], vec![Ok(json!({ "kind": "inactive" })), Ok(json!({ "kind": "inactive" }))]);
        assert!(second.view().paired, "the file survives a restart");
        let view = second.tick();
        let seen = transport.1.lock().expect("seen");
        assert_eq!(seen[0].url, "https://proj.supabase.co/auth/v1/token?grant_type=refresh_token");
        assert!(!seen.iter().any(|r| r.url.ends_with("/auth/v1/signup")));
        drop(seen);
        assert!(view.report.is_some_and(|r| r.online));
        let saved = fs::read_to_string(dir.join(STATE_FILE)).expect("state");
        assert!(saved.contains("\"r2\""), "the rotated refresh token is the stored one");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_identity_asks_for_pairing_again_and_a_dead_network_keeps_the_pairing() {
        let dir = temp_dir("lost");
        let (first, _) = app(&dir, vec![tokens("r1"), reply(CLAIMED)], vec![]);
        first.pair(&qr());

        let (offline, _) = app(&dir, vec![Err(TransportError::Timeout)], vec![]);
        let view = offline.tick();
        assert!(view.paired && !view.pairing_lost);
        assert_eq!(view.last_error, Some("timeout"));

        let refused = Ok(HttpResponse { status: 400, headers: Vec::new(), body: br#"{"error_code":"refresh_token_not_found"}"#.to_vec() });
        let (lost, transport) = app(&dir, vec![refused], vec![]);
        let view = lost.tick();
        assert!(view.pairing_lost && !view.paired);
        assert_eq!(transport.1.lock().expect("seen").len(), 1, "no signup after a refusal");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_scan_or_a_used_code_leaves_the_phone_unpaired() {
        let dir = temp_dir("bad");
        let (app, _) = app(&dir, vec![tokens("r1"), reply(r#"{"code":"used"}"#)], vec![]);
        assert_eq!(app.pair("not a qr"), Ack { code: "bad_offer" });
        assert_eq!(app.pair(&qr()), Ack { code: "used" });
        assert!(!app.view().paired);
        assert!(!dir.join(STATE_FILE).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unpairing_is_refused_while_locked_and_ends_the_pairing_once_the_lock_has_run_out() {
        let dir = temp_dir("unpair");
        let lease = r#"{"code":"ok","lease":{"run_id":"run-1","seq":1,"state":"locked","release_cause":null,"issued_at_ms":1000,"valid_until_ms":601000},"server_now_ms":1000}"#;
        let (app, transport, clock) = app_with_clock(
            &dir,
            vec![tokens("r1"), reply(CLAIMED), reply(lease), reply(r#"{"code":"ok"}"#), reply(r#"{"code":"ok"}"#)],
            vec![Ok(json!({ "kind": "inactive" })), Ok(json!({ "ok": true })), Ok(json!({ "kind": "active" }))],
        );
        app.pair(&qr());
        let view = app.tick();
        assert_eq!(view.report.as_ref().map(|r| r.diagnostics.lock), Some(link_protocol::PhoneLock::Locked));
        assert_eq!(app.unpair(), Ack { code: "locked" });
        assert!(app.view().paired);
        assert!(!transport.1.lock().expect("seen").iter().any(|r| r.url.ends_with("/revoke_pair")), "no request while locked");

        clock.0.fetch_add(600_001, Ordering::SeqCst);
        assert_eq!(app.unpair(), Ack { code: "ok" });
        assert!(!app.view().paired);
        assert!(transport.1.lock().expect("seen").last().is_some_and(|r| r.url.ends_with("/revoke_pair")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugin_events_are_validated_and_the_allow_list_keeps_the_emergency_call() {
        let dir = temp_dir("events");
        let (app, _) = app(&dir, vec![], vec![]);
        assert_eq!(app.blocker_event(&json!({ "event": "locked" })), Ack { code: "bridge_malformed" });
        assert_eq!(app.blocker_event(&json!({ "event": "service_disabled", "package": "x" })), Ack { code: "bridge_malformed" });
        assert_eq!(app.blocker_event(&json!({ "event": "service_disabled" })), Ack { code: "ok" });

        assert_eq!(app.set_allow_list(&json!({ "roles": ["messages"], "packages": ["com.b.app", "com.a.app"] })), Ack { code: "ok" });
        assert_eq!(app.allow_list(), json!({ "roles": ["emergency_call", "messages"], "packages": ["com.a.app", "com.b.app"] }));
        assert_eq!(app.set_allow_list(&json!({ "roles": [], "packages": ["com.android.settings"] })), Ack { code: "settings_package" });
        assert_eq!(app.allow_list()["roles"][0], "emergency_call", "a refused list changes nothing");

        let (again, _, _) = app_with_clock(&dir, vec![], vec![]);
        assert_eq!(again.allow_list(), json!({ "roles": ["emergency_call", "messages"], "packages": ["com.a.app", "com.b.app"] }), "persisted on the device");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_schedule_says_there_is_no_feed_instead_of_inventing_one() {
        let dir = temp_dir("schedule");
        let (app, _) = app(&dir, vec![], vec![]);
        assert_eq!(app.schedule(), ScheduleView { items: PhoneSchedule::default(), source: "no_feed" });
    }
}
