//! Public (non-secret) settings of the remote layer, stored in the settings table through the
//! same locked, history-recorded path as every other setting. Secrets are not here: they are in
//! `secrets`, and history only notes that one was set or cleared.
//!
//! The Supabase anon key is a public client key (row-level security protects the data), so it is
//! an ordinary setting; the UI says so.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::{IntegrationPort, ModeSetting};
use crate::error::AppResult;
use crate::settings::{self, FreeShape, Reason, SettingSpec, SettingValue, UpdateOutcome, ValueKind};
use crate::store;

const CATEGORY: &str = "remote";

pub const SUPABASE_ENDPOINT: SettingSpec = SettingSpec {
    name: "supabase.endpoint",
    category: CATEGORY,
    kind: ValueKind::Free { max_len: 200, shape: FreeShape::Endpoint },
    default: "",
    tighten_to: None,
};
pub const SUPABASE_ANON_KEY: SettingSpec = SettingSpec {
    name: "supabase.anon_key",
    category: CATEGORY,
    kind: ValueKind::Free { max_len: 1024, shape: FreeShape::Token },
    default: "",
    tighten_to: None,
};
/// Written by sign-in/sign-out only; the user does not edit it.
pub const SUPABASE_USER_ID: SettingSpec = SettingSpec {
    name: "supabase.user_id",
    category: CATEGORY,
    kind: ValueKind::Free { max_len: 64, shape: FreeShape::Token },
    default: "",
    tighten_to: None,
};
pub const NOTIFY_CHANNEL: SettingSpec = SettingSpec {
    name: "notify.channel",
    category: CATEGORY,
    kind: ValueKind::Text { allowed: &["none", "ntfy", "telegram", "phone_app"] },
    default: "none",
    // Turning the channel off narrows what leaves the device, so it works during a run.
    tighten_to: Some("none"),
};
pub const NOTIFY_NTFY_SERVER: SettingSpec = SettingSpec {
    name: "notify.ntfy_server",
    category: CATEGORY,
    kind: ValueKind::Free { max_len: 200, shape: FreeShape::Endpoint },
    default: "https://ntfy.sh",
    tighten_to: None,
};
pub const NOTIFY_TELEGRAM_CHAT_ID: SettingSpec = SettingSpec {
    name: "notify.telegram_chat_id",
    category: CATEGORY,
    kind: ValueKind::Free { max_len: 40, shape: FreeShape::Token },
    default: "",
    tighten_to: None,
};
/// Grace before a missed plan is reported. Capped at 3 so detection plus delivery stays under the
/// 5-minute promise (stage 2-6 states the arithmetic).
pub const NOTIFY_MISSED_GRACE_MIN: SettingSpec = SettingSpec {
    name: "notify.missed_grace_min",
    category: CATEGORY,
    kind: ValueKind::Int { min: 1, max: 3 },
    default: "3",
    tighten_to: None,
};
const DEVICE_ID_KEY: &str = "device.id";

/// The settings the integrations screen edits. API name = camelCase of the variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteSettingKey {
    SupabaseEndpoint,
    SupabaseAnonKey,
    NotifyChannel,
    NotifyNtfyServer,
    NotifyTelegramChatId,
    NotifyMissedGraceMin,
}

impl RemoteSettingKey {
    pub fn spec(self) -> SettingSpec {
        match self {
            RemoteSettingKey::SupabaseEndpoint => SUPABASE_ENDPOINT,
            RemoteSettingKey::SupabaseAnonKey => SUPABASE_ANON_KEY,
            RemoteSettingKey::NotifyChannel => NOTIFY_CHANNEL,
            RemoteSettingKey::NotifyNtfyServer => NOTIFY_NTFY_SERVER,
            RemoteSettingKey::NotifyTelegramChatId => NOTIFY_TELEGRAM_CHAT_ID,
            RemoteSettingKey::NotifyMissedGraceMin => NOTIFY_MISSED_GRACE_MIN,
        }
    }
}

/// `integration.<port>.mode`: `auto` (default) or `mock`. Choosing mock keeps everything on the
/// device, so it is a privacy tightening and stays allowed during a run.
pub fn mode_spec(port: IntegrationPort) -> SettingSpec {
    let name = match port {
        IntegrationPort::Supabase => "integration.supabase.mode",
        IntegrationPort::Notify => "integration.notify.mode",
        IntegrationPort::Llm => "integration.llm.mode",
        IntegrationPort::Push => "integration.push.mode",
    };
    SettingSpec {
        name,
        category: CATEGORY,
        kind: ValueKind::Text { allowed: &["auto", "mock"] },
        default: "auto",
        tighten_to: Some("mock"),
    }
}

pub fn update_setting(conn: &mut Connection, key: RemoteSettingKey, value: SettingValue, at_ms: i64) -> AppResult<UpdateOutcome> {
    settings::apply_setting(conn, &key.spec(), &value, at_ms, || Ok(()))
}

pub fn set_mode(conn: &mut Connection, port: IntegrationPort, mode: ModeSetting, at_ms: i64) -> AppResult<UpdateOutcome> {
    settings::apply_setting(conn, &mode_spec(port), &SettingValue::Text(mode.as_str().to_string()), at_ms, || Ok(()))
}

pub fn read_mode(conn: &Connection, port: IntegrationPort) -> AppResult<ModeSetting> {
    match settings::read_value(conn, &mode_spec(port))? {
        SettingValue::Text(raw) => ModeSetting::parse(&raw),
        other => Err(crate::error::AppError::Internal(format!("mode setting has the wrong type: {other:?}"))),
    }
}

fn read_text(conn: &Connection, spec: &SettingSpec) -> AppResult<String> {
    match settings::read_value(conn, spec)? {
        SettingValue::Text(text) => Ok(text),
        other => Err(crate::error::AppError::Internal(format!("{} is not text: {other:?}", spec.name))),
    }
}

/// Snapshot of the public remote settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfig {
    pub supabase_endpoint: String,
    pub supabase_anon_key: String,
    pub supabase_user_id: String,
    pub notify_channel: String,
    pub notify_ntfy_server: String,
    pub notify_telegram_chat_id: String,
    pub notify_missed_grace_min: i64,
}

impl RemoteConfig {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        let grace = match settings::read_value(conn, &NOTIFY_MISSED_GRACE_MIN)? {
            SettingValue::Int(v) => v,
            other => {
                return Err(crate::error::AppError::Internal(format!("grace is not an integer: {other:?}")));
            }
        };
        Ok(Self {
            supabase_endpoint: read_text(conn, &SUPABASE_ENDPOINT)?,
            supabase_anon_key: read_text(conn, &SUPABASE_ANON_KEY)?,
            supabase_user_id: read_text(conn, &SUPABASE_USER_ID)?,
            notify_channel: read_text(conn, &NOTIFY_CHANNEL)?,
            notify_ntfy_server: read_text(conn, &NOTIFY_NTFY_SERVER)?,
            notify_telegram_chat_id: read_text(conn, &NOTIFY_TELEGRAM_CHAT_ID)?,
            notify_missed_grace_min: grace,
        })
    }

    pub fn supabase_configured(&self) -> bool {
        !self.supabase_endpoint.is_empty() && !self.supabase_anon_key.is_empty()
    }
}

/// Which secrets exist. Only booleans cross the UI boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretFlags {
    pub anthropic_api_key: bool,
    pub supabase_refresh_token: bool,
    pub ntfy_topic: bool,
}

/// Is the port usable as configured? Notify needs its channel's inputs; push has no adapter yet.
pub fn is_configured(port: IntegrationPort, config: &RemoteConfig, secrets: SecretFlags) -> bool {
    match port {
        IntegrationPort::Supabase => config.supabase_configured(),
        IntegrationPort::Notify => match config.notify_channel.as_str() {
            "ntfy" => secrets.ntfy_topic,
            "telegram" => !config.notify_telegram_chat_id.is_empty(),
            _ => false,
        },
        IntegrationPort::Llm => secrets.anthropic_api_key,
        IntegrationPort::Push => false,
    }
}

/// First-run device id. Generated once and kept; there is no way to change it from the UI.
pub fn ensure_device_id(conn: &Connection, at_ms: i64) -> AppResult<String> {
    if let Some(existing) = store::get_setting(conn, DEVICE_ID_KEY)? {
        return Ok(existing);
    }
    let id = uuid::Uuid::new_v4().to_string();
    store::put_setting(conn, DEVICE_ID_KEY, CATEGORY, &id, at_ms)?;
    store::append_history(conn, at_ms, DEVICE_ID_KEY, None, &id, Reason::Default, settings::Outcome::Applied)?;
    Ok(id)
}

/// Records which account is signed in (an opaque id, not an email). Sign-in and sign-out write it
/// directly: the lock rule for these actions is enforced by the caller (`secret_flow`).
pub fn write_user_id(conn: &Connection, user_id: &str, at_ms: i64) -> AppResult<()> {
    let before = store::get_setting(conn, SUPABASE_USER_ID.name)?;
    store::put_setting(conn, SUPABASE_USER_ID.name, CATEGORY, user_id, at_ms)?;
    store::append_history(
        conn,
        at_ms,
        SUPABASE_USER_ID.name,
        before.as_deref(),
        user_id,
        Reason::User,
        settings::Outcome::Applied,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::testutil::set_lock_for_test;

    #[test]
    fn defaults_are_unconfigured_and_ntfy_server_is_the_public_default() {
        let conn = open_in_memory().expect("db");
        let config = RemoteConfig::load(&conn).expect("load");
        assert!(!config.supabase_configured());
        assert_eq!(config.notify_channel, "none");
        assert_eq!(config.notify_ntfy_server, "https://ntfy.sh");
        assert_eq!(config.notify_missed_grace_min, 3);
        for port in IntegrationPort::ALL {
            assert_eq!(read_mode(&conn, port).expect("mode"), ModeSetting::Auto);
            assert!(!is_configured(port, &config, SecretFlags::default()));
        }
    }

    #[test]
    fn grace_above_three_minutes_is_rejected() {
        let mut conn = open_in_memory().expect("db");
        assert!(update_setting(&mut conn, RemoteSettingKey::NotifyMissedGraceMin, SettingValue::Int(4), 1).is_err());
        assert!(update_setting(&mut conn, RemoteSettingKey::NotifyMissedGraceMin, SettingValue::Int(0), 1).is_err());
        assert!(matches!(
            update_setting(&mut conn, RemoteSettingKey::NotifyMissedGraceMin, SettingValue::Int(2), 1),
            Ok(UpdateOutcome::Applied { .. })
        ));
    }

    #[test]
    fn endpoint_must_be_https_without_credentials_and_errors_do_not_echo_the_text() {
        let mut conn = open_in_memory().expect("db");
        for bad in ["http://abc.supabase.co", "https://u:p@abc.supabase.co", "abc.supabase.co", "https://x.co/?apikey=SECRETKEY"] {
            let err = update_setting(&mut conn, RemoteSettingKey::SupabaseEndpoint, SettingValue::Text(bad.into()), 1)
                .expect_err("rejected");
            assert!(!err.to_string().contains("SECRETKEY"), "{err}");
        }
        assert!(update_setting(&mut conn, RemoteSettingKey::SupabaseEndpoint, SettingValue::Text("https://abc.supabase.co".into()), 1).is_ok());
    }

    #[test]
    fn locked_run_rejects_entry_but_allows_tightening_with_its_own_outcome() {
        let mut conn = open_in_memory().expect("db");
        update_setting(&mut conn, RemoteSettingKey::NotifyChannel, SettingValue::Text("ntfy".into()), 1).expect("set");
        set_lock_for_test(&conn, true);

        // Widening or entering a value during a run: refused, recorded, unchanged.
        let locked = update_setting(&mut conn, RemoteSettingKey::SupabaseEndpoint, SettingValue::Text("https://abc.supabase.co".into()), 2)
            .expect("outcome");
        assert_eq!(locked, UpdateOutcome::RejectedLocked { key: "supabase.endpoint".into() });
        assert_eq!(RemoteConfig::load(&conn).expect("load").supabase_endpoint, "");
        let widen = set_mode(&mut conn, IntegrationPort::Supabase, ModeSetting::Auto, 3).expect("outcome");
        assert_eq!(widen, UpdateOutcome::RejectedLocked { key: "integration.supabase.mode".into() });

        // Tightening: allowed during the run.
        let tighten = update_setting(&mut conn, RemoteSettingKey::NotifyChannel, SettingValue::Text("none".into()), 4).expect("outcome");
        assert!(matches!(tighten, UpdateOutcome::Applied { .. }));
        let mock = set_mode(&mut conn, IntegrationPort::Notify, ModeSetting::Mock, 5).expect("outcome");
        assert!(matches!(mock, UpdateOutcome::Applied { .. }));
        assert_eq!(read_mode(&conn, IntegrationPort::Notify).expect("mode"), ModeSetting::Mock);

        let history = settings::read_history(&conn, 10).expect("history");
        let outcomes: Vec<(&str, &str)> = history.iter().map(|h| (h.key.as_str(), h.outcome.as_str())).collect();
        assert!(outcomes.contains(&("supabase.endpoint", "rejected_locked")));
        assert!(outcomes.contains(&("notify.channel", "applied_privacy_tighten")));
        assert!(outcomes.contains(&("integration.notify.mode", "applied_privacy_tighten")));
    }

    #[test]
    fn leaving_mock_during_a_run_is_locked() {
        let mut conn = open_in_memory().expect("db");
        set_mode(&mut conn, IntegrationPort::Supabase, ModeSetting::Mock, 1).expect("mock");
        set_lock_for_test(&conn, true);
        let outcome = set_mode(&mut conn, IntegrationPort::Supabase, ModeSetting::Auto, 2).expect("outcome");
        assert_eq!(outcome, UpdateOutcome::RejectedLocked { key: "integration.supabase.mode".into() });
        assert_eq!(read_mode(&conn, IntegrationPort::Supabase).expect("mode"), ModeSetting::Mock);
    }

    #[test]
    fn device_id_is_created_once() {
        let conn = open_in_memory().expect("db");
        let first = ensure_device_id(&conn, 1).expect("id");
        assert_eq!(uuid::Uuid::parse_str(&first).map(|_| ()), Ok(()));
        assert_eq!(ensure_device_id(&conn, 2).expect("id"), first);
        let history = settings::read_history(&conn, 10).expect("history");
        assert_eq!(history.iter().filter(|h| h.key == "device.id").count(), 1);
    }

    #[test]
    fn configured_rules_per_port() {
        let mut config = RemoteConfig::load(&open_in_memory().expect("db")).expect("load");
        let none = SecretFlags::default();
        config.supabase_endpoint = "https://abc.supabase.co".into();
        assert!(!is_configured(IntegrationPort::Supabase, &config, none), "anon key missing");
        config.supabase_anon_key = "anon".into();
        assert!(is_configured(IntegrationPort::Supabase, &config, none));
        config.notify_channel = "ntfy".into();
        assert!(!is_configured(IntegrationPort::Notify, &config, none));
        assert!(is_configured(IntegrationPort::Notify, &config, SecretFlags { ntfy_topic: true, ..none }));
        config.notify_channel = "telegram".into();
        assert!(!is_configured(IntegrationPort::Notify, &config, none));
        config.notify_telegram_chat_id = "123".into();
        assert!(is_configured(IntegrationPort::Notify, &config, none));
        assert!(is_configured(IntegrationPort::Llm, &config, SecretFlags { anthropic_api_key: true, ..none }));
        assert!(!is_configured(IntegrationPort::Push, &config, SecretFlags { anthropic_api_key: true, ntfy_topic: true, supabase_refresh_token: true }));
    }
}
