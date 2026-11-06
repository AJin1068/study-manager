//! Integration commands: mode states, public settings, secrets, connection test, sign-in, and the
//! "what was sent" log. Network calls run with the database lock released; secret values are
//! never returned, logged, or written to the settings tables.

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use super::{lock_db, AppState, DEFAULT_LIMIT};
use crate::clock::now_ms;
use crate::error::{AppError, AppResult};
use crate::remote::auth::{AuthError, SignOutReport};
use crate::remote::config::{self, RemoteConfig, RemoteSettingKey};
use crate::remote::payload::DeviceRegistration;
use crate::remote::qr::{self, NtfySubscription};
use crate::sync::status::SyncStatusView;
use crate::remote::runtime::{ConnectionTest, IntegrationStatesView};
use crate::remote::secret_flow;
use crate::remote::secrets::SecretName;
use crate::remote::{IntegrationPort, ModeSetting};
use crate::settings::{SettingValue, UpdateOutcome};
use crate::store::remote::{self as events, EgressSummaryRow, RemoteEventRow};
use crate::study;

pub const REMOTE_STATE_EVENT: &str = "remote://state";

/// Pushes the current states to the UI. A failed emit is logged, never an error for the command.
fn announce(app: &AppHandle, state: &AppState) {
    let view = match lock_db(&state.db).and_then(|conn| state.remote.view(&conn)) {
        Ok(view) => view,
        Err(err) => {
            tracing::error!(error = %err, "could not build remote state for the UI");
            return;
        }
    };
    if let Err(err) = app.emit(REMOTE_STATE_EVENT, view) {
        tracing::error!(error = %err, "failed to emit remote state");
    }
}

#[tauri::command]
pub fn get_integration_states(state: State<'_, AppState>) -> AppResult<IntegrationStatesView> {
    let conn = lock_db(&state.db)?;
    state.remote.view(&conn)
}

#[tauri::command]
pub fn set_integration_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    port: IntegrationPort,
    mode: ModeSetting,
) -> AppResult<UpdateOutcome> {
    let outcome = {
        let mut conn = lock_db(&state.db)?;
        config::set_mode(&mut conn, port, mode, now_ms()?)?
    };
    announce(&app, &state);
    Ok(outcome)
}

#[tauri::command]
pub fn get_remote_settings(state: State<'_, AppState>) -> AppResult<RemoteConfig> {
    let conn = lock_db(&state.db)?;
    RemoteConfig::load(&conn)
}

#[tauri::command]
pub fn update_remote_setting(
    app: AppHandle,
    state: State<'_, AppState>,
    key: RemoteSettingKey,
    value: SettingValue,
) -> AppResult<UpdateOutcome> {
    let outcome = {
        let mut conn = lock_db(&state.db)?;
        config::update_setting(&mut conn, key, value, now_ms()?)?
    };
    announce(&app, &state);
    Ok(outcome)
}

/// Stores a secret. The value is consumed here: the result carries only the outcome.
#[tauri::command]
pub fn save_secret(app: AppHandle, state: State<'_, AppState>, name: SecretName, value: String) -> AppResult<UpdateOutcome> {
    let outcome = {
        let mut conn = lock_db(&state.db)?;
        secret_flow::save_secret(&mut conn, state.remote.secrets().as_ref(), name, &value, now_ms()?)?
    };
    announce(&app, &state);
    Ok(outcome)
}

#[tauri::command]
pub fn clear_secret(app: AppHandle, state: State<'_, AppState>, name: SecretName) -> AppResult<UpdateOutcome> {
    let outcome = {
        let mut conn = lock_db(&state.db)?;
        secret_flow::clear_secret(&mut conn, state.remote.secrets().as_ref(), name, now_ms()?)?
    };
    announce(&app, &state);
    Ok(outcome)
}

#[tauri::command]
pub async fn test_connection(app: AppHandle, state: State<'_, AppState>, port: IntegrationPort) -> AppResult<ConnectionTest> {
    if port == IntegrationPort::Llm {
        // The model call blocks; it runs off the UI thread with the database lock released.
        let llm = std::sync::Arc::clone(&state.llm);
        let result = tauri::async_runtime::spawn_blocking(move || llm.test_connection())
            .await
            .map_err(|_| AppError::Internal("the connection test did not finish".into()))?;
        announce(&app, &state);
        return result;
    }
    // The runtime locks the database only to read configuration; the probe itself runs unlocked.
    let result = state.remote.test_connection(&state.db, port);
    announce(&app, &state);
    result
}

#[tauri::command]
pub fn list_remote_events(state: State<'_, AppState>, limit: Option<i64>) -> AppResult<Vec<RemoteEventRow>> {
    let conn = lock_db(&state.db)?;
    events::list_events(&conn, limit.unwrap_or(DEFAULT_LIMIT))
}

#[tauri::command]
pub fn get_egress_summary(state: State<'_, AppState>) -> AppResult<Vec<EgressSummaryRow>> {
    let conn = lock_db(&state.db)?;
    events::egress_summary(&conn)
}

#[tauri::command]
pub fn auth_request_otp(state: State<'_, AppState>, email: String) -> AppResult<()> {
    let handle = {
        let conn = lock_db(&state.db)?;
        state.remote.supabase(&conn)?
    };
    handle.auth.request_otp(email.trim()).map_err(AppError::from)
}

/// Result of entering the mailed code. Expected user-facing outcomes are values, not errors.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum VerifyOutcome {
    SignedIn { user_id: String },
    /// A planned run holds the settings lock: signing in is refused and recorded.
    RejectedLocked,
    InvalidCode,
    Cooldown { until_ms: i64 },
    RateLimited { retry_after_ms: u64 },
}

pub(crate) fn platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "other"
    }
}

#[tauri::command]
pub fn auth_verify_otp(app: AppHandle, state: State<'_, AppState>, email: String, code: String) -> AppResult<VerifyOutcome> {
    let now = now_ms()?;
    let (handle, device_id) = {
        let mut conn = lock_db(&state.db)?;
        if secret_flow::refuse_entry_if_locked(&mut conn, SecretName::SupabaseRefreshToken, now)? {
            return Ok(VerifyOutcome::RejectedLocked);
        }
        (state.remote.supabase(&conn)?, config::ensure_device_id(&conn, now)?)
    };
    let user_id = match handle.auth.verify_otp(email.trim(), code.trim()) {
        Ok(user_id) => user_id,
        Err(AuthError::InvalidCode) => return Ok(VerifyOutcome::InvalidCode),
        Err(AuthError::Cooldown { until_ms }) => return Ok(VerifyOutcome::Cooldown { until_ms }),
        Err(AuthError::RateLimited { retry_after_ms }) => return Ok(VerifyOutcome::RateLimited { retry_after_ms }),
        Err(other) => return Err(other.into()),
    };
    {
        let conn = lock_db(&state.db)?;
        config::write_user_id(&conn, &user_id, now)?;
        secret_flow::record_set(&conn, SecretName::SupabaseRefreshToken, now)?;
    }
    // Register this device so later pushes and heartbeats have a row to point at. Offline or a
    // server hiccup only delays that; sign-in itself stands.
    let registration = DeviceRegistration { id: device_id, app_version: env!("CARGO_PKG_VERSION").into(), platform: platform_name() };
    if let Err(err) = handle.rest.insert("device_register", "devices", &registration, true) {
        tracing::warn!(event = "device_register_failed", code = err.code());
    }
    announce(&app, &state);
    Ok(VerifyOutcome::SignedIn { user_id })
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SignOutResult {
    pub device_removed: bool,
    pub server_session_ended: bool,
}

/// Signs out locally no matter what the server says. Allowed during a run: it only narrows what
/// leaves the device.
#[tauri::command]
pub fn auth_sign_out(app: AppHandle, state: State<'_, AppState>) -> AppResult<SignOutResult> {
    let now = now_ms()?;
    let (handle, device_id) = {
        let conn = lock_db(&state.db)?;
        let device_id = config::ensure_device_id(&conn, now)?;
        (state.remote.supabase(&conn).ok(), device_id)
    };
    let report = match handle {
        Some(handle) => handle.auth.sign_out(&device_id)?,
        None => SignOutReport { device_removed: false, server_session_ended: false },
    };
    {
        let conn = lock_db(&state.db)?;
        // Idempotent: covers the unconfigured and mock paths, where the service did not touch the
        // real store.
        state
            .remote
            .secrets()
            .delete(SecretName::SupabaseRefreshToken)
            .map_err(|_| AppError::Remote("secret_store_unavailable".into()))?;
        let tighten = study::lock_held(&conn)?;
        config::write_user_id(&conn, "", now)?;
        // A different account later must receive the whole history, not the tail after this cursor.
        crate::store::remote::reset_cursor(&conn, crate::store::remote::HISTORY_CURSOR)?;
        secret_flow::record_cleared(&conn, SecretName::SupabaseRefreshToken, tighten, now)?;
    }
    announce(&app, &state);
    Ok(SignOutResult { device_removed: report.device_removed, server_session_ended: report.server_session_ended })
}

/// Last success times and failure codes of the mirror and the "PC is on" signal.
#[tauri::command]
pub fn get_sync_status(state: State<'_, AppState>) -> SyncStatusView {
    state.sync.status.view()
}

/// Makes a fresh, hard-to-guess ntfy topic and stores it as a secret (never in the settings
/// tables). 122 random bits; the topic is the only thing protecting an ntfy channel.
#[tauri::command]
pub fn generate_ntfy_topic(app: AppHandle, state: State<'_, AppState>) -> AppResult<UpdateOutcome> {
    let topic = uuid::Uuid::new_v4().simple().to_string();
    save_secret(app, state, SecretName::NtfyTopic, topic)
}

/// The deep link and QR code that subscribe a phone to the topic. Computed on the PC; `None` until
/// a topic exists.
#[tauri::command]
pub fn get_ntfy_subscription(state: State<'_, AppState>) -> AppResult<Option<NtfySubscription>> {
    let server = {
        let conn = lock_db(&state.db)?;
        RemoteConfig::load(&conn)?.notify_ntfy_server
    };
    let topic = state.remote.secrets().get(SecretName::NtfyTopic).map_err(|_| AppError::Remote("secret_store_unavailable".into()))?;
    Ok(topic.and_then(|t| qr::subscription(&server, t.expose())))
}

/// "Send a test notification". The send runs with the database lock released.
#[tauri::command]
pub fn test_notification(app: AppHandle, state: State<'_, AppState>) -> AppResult<ConnectionTest> {
    let client = {
        let conn = lock_db(&state.db)?;
        state.remote.notify_client(&conn)?
    };
    let result = client.send_test();
    let (ok, code) = match &result {
        Ok(()) => (true, "ok".to_string()),
        Err(err) => (false, err.code().to_string()),
    };
    let health = if ok { crate::remote::Health::Ok } else { crate::remote::Health::Degraded { code: code.clone() } };
    state.remote.record_health(IntegrationPort::Notify, health);
    announce(&app, &state);
    Ok(ConnectionTest { port: IntegrationPort::Notify, ok, code })
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::remote::auth::AuthStatus;
    use crate::remote::config::SecretFlags;
    use crate::remote::runtime::SecretStoreStatus;
    use crate::remote::secrets::SecretStoreKind;
    use crate::remote::{Health, IntegrationMode, IntegrationState};

    fn port(port: IntegrationPort, setting: ModeSetting, mode: IntegrationMode, configured: bool, health: Health, last_ok_ms: Option<i64>) -> IntegrationState {
        IntegrationState { port, setting, mode, configured, health, last_ok_ms }
    }

    #[test]
    fn remote_wire_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/remote-contract.json");
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let view = IntegrationStatesView {
            ports: vec![
                port(IntegrationPort::Supabase, ModeSetting::Auto, IntegrationMode::Live, true, Health::Degraded { code: "auth_expired".into() }, Some(1_773_000_000_000)),
                port(IntegrationPort::Notify, ModeSetting::Mock, IntegrationMode::Mock, false, Health::Unknown, None),
                port(IntegrationPort::Llm, ModeSetting::Auto, IntegrationMode::NotConfigured, false, Health::Ok, None),
                port(IntegrationPort::Push, ModeSetting::Auto, IntegrationMode::NotConfigured, false, Health::Unknown, None),
            ],
            secret_store: SecretStoreStatus {
                kind: SecretStoreKind::SessionOnly,
                degraded_code: Some("secret_store_session_only"),
                configured: SecretFlags { anthropic_api_key: false, supabase_refresh_token: false, ntfy_topic: true },
            },
            auth: AuthStatus { signed_in: false, user_id: None, degraded_code: Some("auth_expired"), cooldown_until_ms: None },
        };
        let settings = RemoteConfig {
            supabase_endpoint: "https://abc.supabase.co".into(),
            supabase_anon_key: "anon-public-key".into(),
            supabase_user_id: "u-1".into(),
            notify_channel: "ntfy".into(),
            notify_ntfy_server: "https://ntfy.sh".into(),
            notify_telegram_chat_id: String::new(),
            notify_missed_grace_min: 3,
        };
        let actual = json!({
            "view": view,
            "settings": settings,
            "connectionTest": ConnectionTest { port: IntegrationPort::Supabase, ok: false, code: "http_error".into() },
            "verifyOutcomes": [
                VerifyOutcome::SignedIn { user_id: "u-1".into() },
                VerifyOutcome::RejectedLocked,
                VerifyOutcome::InvalidCode,
                VerifyOutcome::Cooldown { until_ms: 1_773_000_030_000 },
                VerifyOutcome::RateLimited { retry_after_ms: 42_000 },
            ],
            "syncStatus": SyncStatusView {
                active: true,
                last_beat_ok_ms: Some(1_773_000_000_000),
                last_sync_ok_ms: Some(1_773_000_005_000),
                beat_error: None,
                sync_error: Some("http_error"),
                degraded_code: Some("presence_unreachable"),
            },
            "ntfySubscription": NtfySubscription { deep_link: "ntfy://ntfy.sh/abc".into(), qr_svg: "<svg></svg>".into() },
            "signOut": SignOutResult { device_removed: true, server_session_ended: false },
            "event": RemoteEventRow {
                id: 7, at_ms: 1_773_000_000_000, port: "supabase".into(), kind: "rpc_heartbeat".into(),
                outcome: "ok".into(), http_status: Some(200), bytes: 96, detail_code: None,
            },
            "egressSummary": EgressSummaryRow { port: "supabase".into(), kind: "rpc_heartbeat".into(), count: 12, last_at_ms: 1_773_000_000_000 },
        });
        assert_eq!(actual, expected);
    }
}
