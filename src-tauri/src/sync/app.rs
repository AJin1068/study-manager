//! Wiring of the sync workers to the running app: where the gate (signed in? configured?) comes
//! from, how outcomes reach the status screen, and the thread start-up.

use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use super::ports::{DeviceIdentity, RestSync};
use super::presence::{self, PresenceHandle, PresenceReport};
use super::status::{SyncStatus, PRESENCE_UNREACHABLE};
use super::worker::{self, MirrorWorker, SyncSource, SyncTarget};
use super::{RandomJitter, SystemMono};
use crate::commands::lock_db;
use crate::remote::config::{self, RemoteConfig};
use crate::remote::payload::NotifyChannelRow;
use crate::remote::runtime::RemoteRuntime;
use crate::remote::secrets::SecretName;
use crate::remote::{Clock, Health, IntegrationPort};
use crate::study;

/// What the rest of the app holds on to: the status for the UI and the presence handle that
/// `power.rs` forwards sleep and wake events to.
pub struct SyncHub {
    pub status: Arc<SyncStatus>,
    pub presence: PresenceHandle,
}

/// The notification channel as the server needs it. `topic` is the ntfy topic from the secret
/// store. A channel whose inputs are missing is mirrored as `none`, so the server never tries to
/// send to half a configuration.
pub fn channel_row(config: &RemoteConfig, topic: Option<String>, utc_offset_min: i64) -> NotifyChannelRow {
    let none = || NotifyChannelRow {
        channel: "none".into(),
        ntfy_topic: None,
        ntfy_server: None,
        telegram_chat_id: None,
        missed_grace_min: config.notify_missed_grace_min,
        utc_offset_min,
    };
    match config.notify_channel.as_str() {
        // `phone_app` is delivered through the same ntfy topic: the server posts to it and the
        // phone app subscribes. Without a topic it is mirrored as `none`, like ntfy.
        channel @ ("ntfy" | "phone_app") => match topic {
            Some(topic) => NotifyChannelRow {
                channel: channel.into(),
                ntfy_topic: Some(topic),
                ntfy_server: Some(config.notify_ntfy_server.clone()),
                ..none()
            },
            None => none(),
        },
        "telegram" if !config.notify_telegram_chat_id.is_empty() => NotifyChannelRow {
            channel: "telegram".into(),
            telegram_chat_id: Some(config.notify_telegram_chat_id.clone()),
            ..none()
        },
        _ => none(),
    }
}

fn local_utc_offset_min() -> i64 {
    i64::from(chrono::Local::now().offset().local_minus_utc()) / 60
}

struct AppSyncSource {
    db: Arc<Mutex<Connection>>,
    runtime: Arc<RemoteRuntime>,
    clock: Arc<dyn Clock>,
}

impl SyncSource for AppSyncSource {
    fn target(&self) -> Option<SyncTarget> {
        let (handle, device_id) = {
            let conn = lock_db(&self.db).ok()?;
            // NotConfigured is the common case until the user sets a server up: nothing is built.
            let handle = self.runtime.supabase(&conn).ok()?;
            let now = self.clock.now_ms().ok()?;
            (handle, config::ensure_device_id(&conn, now).ok()?)
        };
        // Signed out, or the stored login stopped working: send nothing until a new sign-in.
        if !handle.auth.status().signed_in {
            return None;
        }
        let port = Arc::new(RestSync::new(handle.rest));
        Some(SyncTarget {
            mirror: port.clone(),
            presence: port,
            device: DeviceIdentity {
                id: device_id,
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                platform: crate::commands::remote::platform_name(),
            },
        })
    }

    fn studying(&self) -> bool {
        lock_db(&self.db).and_then(|conn| study::lock_held(&conn)).unwrap_or(false)
    }

    fn notify_channel(&self) -> Option<NotifyChannelRow> {
        let config = {
            let conn = lock_db(&self.db).ok()?;
            RemoteConfig::load(&conn).ok()?
        };
        let topic = match self.runtime.secrets().get(SecretName::NtfyTopic) {
            Ok(Some(value)) if matches!(config.notify_channel.as_str(), "ntfy" | "phone_app") => Some(value.expose().to_string()),
            _ => None,
        };
        Some(channel_row(&config, topic, local_utc_offset_min()))
    }
}

struct HubReport {
    status: Arc<SyncStatus>,
    runtime: Arc<RemoteRuntime>,
    clock: Arc<dyn Clock>,
}

impl PresenceReport for HubReport {
    fn beat(&self, result: Result<(), &'static str>) {
        self.status.beat(self.clock.now_ms().unwrap_or(0), result);
    }

    fn degraded(&self, degraded: bool) {
        self.status.set_presence_degraded(degraded);
        let health = if degraded { Health::Degraded { code: PRESENCE_UNREACHABLE.to_string() } } else { Health::Ok };
        self.runtime.record_health(IntegrationPort::Supabase, health);
    }

    fn inactive(&self) {
        self.status.set_active(false);
    }
}

/// Starts the presence and mirror threads. Both stay idle (and send nothing) until the server is
/// configured and the user is signed in.
pub fn start(db: Arc<Mutex<Connection>>, runtime: Arc<RemoteRuntime>, clock: Arc<dyn Clock>) -> Arc<SyncHub> {
    let status = Arc::new(SyncStatus::default());
    let source: Arc<AppSyncSource> = Arc::new(AppSyncSource { db: Arc::clone(&db), runtime: Arc::clone(&runtime), clock: Arc::clone(&clock) });
    let mono = Arc::new(SystemMono::new());
    let jitter = Arc::new(RandomJitter);
    let sink = Arc::new(HubReport { status: Arc::clone(&status), runtime, clock: Arc::clone(&clock) });
    let presence = presence::spawn(source.clone(), mono.clone(), jitter.clone(), sink);
    worker::spawn(MirrorWorker::new(db, clock, chrono::Local, source, Arc::clone(&status), jitter), mono);
    Arc::new(SyncHub { status, presence })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn config(channel: &str) -> RemoteConfig {
        let mut config = RemoteConfig::load(&open_in_memory().expect("db")).expect("load");
        config.notify_channel = channel.into();
        config
    }

    #[test]
    fn channel_row_mirrors_only_what_the_chosen_channel_needs() {
        let ntfy = channel_row(&config("ntfy"), Some("t".repeat(24)), 540);
        assert_eq!((ntfy.channel.as_str(), ntfy.ntfy_topic.as_deref(), ntfy.ntfy_server.as_deref()), ("ntfy", Some("tttttttttttttttttttttttt"), Some("https://ntfy.sh")));
        assert!(ntfy.telegram_chat_id.is_none());

        let mut tg = config("telegram");
        tg.notify_telegram_chat_id = "123".into();
        let row = channel_row(&tg, Some("leftover-topic".into()), -300);
        assert_eq!((row.channel.as_str(), row.telegram_chat_id.as_deref(), row.utc_offset_min), ("telegram", Some("123"), -300));
        assert!(row.ntfy_topic.is_none(), "a leftover topic is not sent for another channel");
    }

    #[test]
    fn an_incomplete_channel_is_mirrored_as_none() {
        assert_eq!(channel_row(&config("ntfy"), None, 540).channel, "none");
        assert_eq!(channel_row(&config("telegram"), None, 540).channel, "none");
        assert_eq!(channel_row(&config("none"), Some("x".repeat(24)), 540).channel, "none");
        assert_eq!(channel_row(&config("none"), None, 540).ntfy_topic, None);
        assert_eq!(channel_row(&config("phone_app"), None, 540).channel, "none", "a phone_app row needs the topic the phone subscribes to");
    }

    #[test]
    fn the_phone_app_channel_row_matches_the_shared_contract() {
        let contract: serde_json::Value = serde_json::from_str(include_str!("../../../fixtures/notify/contract.json")).expect("contract");
        let expected = &contract["phoneApp"]["channelRow"];
        let topic = expected["ntfy_topic"].as_str().expect("topic").to_string();
        let row = channel_row(&config("phone_app"), Some(topic), 540);
        assert_eq!(serde_json::to_value(&row).expect("json"), *expected);
        assert_eq!(contract["phoneApp"]["rowWithoutTopic"], "none");
    }
}
