//! Wires settings, secrets, mode resolution, and adapters together. Commands call this and never
//! learn which adapter (mock or live) answers.
//!
//! Network calls happen outside the database lock: configuration is read under the lock, the lock
//! is released, then the adapter runs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::Serialize;

use super::audit::{AuditedTransport, EventSink};
use super::auth::{AccessTokenSource, AuthProvider, AuthService, AuthStatus, SupabaseAuth};
use super::config::{self, RemoteConfig, SecretFlags};
use super::fake::{FAKE_ANON_KEY, FAKE_ENDPOINT, FakeBackend};
use super::http::{HostPolicy, HttpTransport, UreqTransport, host_of};
use super::notify::{NotifyClient, NtfyClient, RecordingNotify, ServerTest};
use super::secrets::{MemorySecretStore, SecretName, SecretStore, SecretStoreKind, SelectedStore};
use super::supabase::SupabaseRest;
use super::{Clock, Health, IntegrationMode, IntegrationPort, IntegrationState, resolve_mode};
use crate::error::{AppError, AppResult};
use crate::llm::LlmClient;
use crate::llm::anthropic::{self, AnthropicHttp, ThreadSleeper};
use crate::llm::mock::MockLlm;

/// Builds the transport for a policy. The app uses the real one; tests substitute a scripted or
/// loopback-bound one without changing any caller.
pub type TransportFactory = Arc<dyn Fn(HostPolicy) -> Arc<dyn HttpTransport> + Send + Sync>;

pub fn ureq_factory() -> TransportFactory {
    Arc::new(|policy| Arc::new(UreqTransport::new(policy)))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStoreStatus {
    pub kind: SecretStoreKind,
    /// `secret_store_session_only` when the keychain was unusable: keys vanish on restart.
    pub degraded_code: Option<&'static str>,
    pub configured: SecretFlags,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationStatesView {
    pub ports: Vec<IntegrationState>,
    pub secret_store: SecretStoreStatus,
    pub auth: AuthStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTest {
    pub port: IntegrationPort,
    pub ok: bool,
    /// Stable code: `ok`, `mock`, `http_error`, `timeout`, `host_denied`, ...
    pub code: String,
}

#[derive(Clone, PartialEq, Eq)]
struct AdapterKey {
    mode: IntegrationMode,
    endpoint: String,
    anon_key: String,
}

/// The Supabase adapters for the current mode and settings.
#[derive(Clone)]
pub struct SupabaseHandle {
    pub mode: IntegrationMode,
    pub auth: Arc<AuthService>,
    pub rest: Arc<SupabaseRest>,
}

struct AuthSlot {
    key: AdapterKey,
    handle: SupabaseHandle,
}

pub struct RemoteRuntime {
    secrets: Arc<dyn SecretStore>,
    secret_degraded: Option<&'static str>,
    /// Mock mode keeps fake tokens here so they never reach the real keychain.
    mock_secrets: Arc<MemorySecretStore>,
    clock: Arc<dyn Clock>,
    sink: Arc<dyn EventSink>,
    transports: TransportFactory,
    /// The LLM has its own limits (30 second timeout), so it gets its own factory.
    llm_transports: TransportFactory,
    /// Mock mode answers from this one instance, so scripted steps and recorded calls persist.
    mock_llm: Arc<MockLlm>,
    fake: Arc<FakeBackend>,
    health: Mutex<HashMap<IntegrationPort, (Health, Option<i64>)>>,
    supabase: Mutex<Option<AuthSlot>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl RemoteRuntime {
    pub fn new(
        selected: SelectedStore,
        clock: Arc<dyn Clock>,
        sink: Arc<dyn EventSink>,
        transports: TransportFactory,
    ) -> Self {
        Self {
            secrets: selected.store,
            secret_degraded: selected.degraded,
            mock_secrets: Arc::new(MemorySecretStore::in_memory()),
            fake: Arc::new(FakeBackend::new(Arc::clone(&clock))),
            clock,
            sink,
            llm_transports: Arc::clone(&transports),
            transports,
            mock_llm: Arc::new(MockLlm::new()),
            health: Mutex::new(HashMap::new()),
            supabase: Mutex::new(None),
        }
    }

    /// Replaces the transport used for model calls (the app installs the 30 second one; tests
    /// keep the shared factory).
    pub fn with_llm_transports(mut self, factory: TransportFactory) -> Self {
        self.llm_transports = factory;
        self
    }

    pub fn secrets(&self) -> &Arc<dyn SecretStore> {
        &self.secrets
    }

    pub fn mock_llm(&self) -> &Arc<MockLlm> {
        &self.mock_llm
    }

    /// The LLM adapter for the current settings: the mock when mock mode is chosen, the
    /// Anthropic adapter (host policy: exactly `api.anthropic.com`, audited) when a key exists,
    /// otherwise `NotConfigured`. A missing key is never answered with the mock.
    pub fn llm_client(
        &self,
        conn: &Connection,
        model: &str,
    ) -> AppResult<(IntegrationMode, Arc<dyn LlmClient>)> {
        let setting = config::read_mode(conn, IntegrationPort::Llm)?;
        let key = self
            .secrets
            .get(SecretName::AnthropicApiKey)
            .map_err(|_| AppError::Remote("secret_store_unavailable".into()))?;
        match resolve_mode(setting, key.is_some()) {
            IntegrationMode::NotConfigured => Err(AppError::NotConfigured {
                port: IntegrationPort::Llm.as_str(),
            }),
            IntegrationMode::Mock => Ok((IntegrationMode::Mock, self.mock_llm.clone())),
            IntegrationMode::Live => {
                let key = key.ok_or(AppError::NotConfigured {
                    port: IntegrationPort::Llm.as_str(),
                })?;
                let inner = (self.llm_transports)(HostPolicy::configured([anthropic::HOST]));
                let transport: Arc<dyn HttpTransport> = Arc::new(AuditedTransport::new(
                    inner,
                    Arc::clone(&self.sink),
                    Arc::clone(&self.clock),
                ));
                let client = AnthropicHttp::new(
                    anthropic::DEFAULT_BASE,
                    key,
                    model,
                    transport,
                    Arc::new(ThreadSleeper),
                );
                Ok((IntegrationMode::Live, Arc::new(client)))
            }
        }
    }

    /// Records the outcome of an LLM call for the integrations screen.
    pub fn record_llm_health(&self, ok: bool, code: &str) {
        let health = if ok {
            Health::Ok
        } else {
            Health::Degraded {
                code: code.to_string(),
            }
        };
        self.record_health(IntegrationPort::Llm, health);
    }

    fn secret_flags(&self) -> SecretFlags {
        let has = |name| matches!(self.secrets.get(name), Ok(Some(_)));
        SecretFlags {
            anthropic_api_key: has(SecretName::AnthropicApiKey),
            supabase_refresh_token: has(SecretName::SupabaseRefreshToken),
            ntfy_topic: has(SecretName::NtfyTopic),
        }
    }

    pub fn secret_store_status(&self) -> SecretStoreStatus {
        SecretStoreStatus {
            kind: self.secrets.kind(),
            degraded_code: self.secret_degraded,
            configured: self.secret_flags(),
        }
    }

    pub fn record_health(&self, port: IntegrationPort, health: Health) {
        let now = self.clock.now_ms().ok();
        let mut map = lock(&self.health);
        let last_ok = if health == Health::Ok {
            now
        } else {
            map.get(&port).and_then(|(_, ok)| *ok)
        };
        map.insert(port, (health, last_ok));
    }

    /// One state per mode-capable port. Health is what the adapters last reported; an expired
    /// login shows as Degraded(auth_expired) even if no request has failed yet.
    pub fn states(&self, conn: &Connection) -> AppResult<Vec<IntegrationState>> {
        let config = RemoteConfig::load(conn)?;
        let flags = self.secret_flags();
        let auth_degraded = lock(&self.supabase)
            .as_ref()
            .and_then(|slot| slot.handle.auth.status().degraded_code);
        let health = lock(&self.health);
        IntegrationPort::ALL
            .iter()
            .map(|&port| {
                let setting = config::read_mode(conn, port)?;
                let configured = config::is_configured(port, &config, flags);
                let (mut state_health, last_ok_ms) = health
                    .get(&port)
                    .cloned()
                    .unwrap_or((Health::Unknown, None));
                if port == IntegrationPort::Supabase {
                    if let Some(code) = auth_degraded {
                        state_health = Health::Degraded {
                            code: code.to_string(),
                        };
                    }
                }
                Ok(IntegrationState {
                    port,
                    setting,
                    mode: resolve_mode(setting, configured),
                    configured,
                    health: state_health,
                    last_ok_ms,
                })
            })
            .collect()
    }

    pub fn view(&self, conn: &Connection) -> AppResult<IntegrationStatesView> {
        let ports = self.states(conn)?;
        let auth = lock(&self.supabase)
            .as_ref()
            .map(|slot| slot.handle.auth.status())
            .unwrap_or(AuthStatus {
                signed_in: matches!(
                    self.secrets.get(SecretName::SupabaseRefreshToken),
                    Ok(Some(_))
                ),
                user_id: None,
                degraded_code: None,
                cooldown_until_ms: None,
            });
        Ok(IntegrationStatesView {
            ports,
            secret_store: self.secret_store_status(),
            auth,
        })
    }

    /// The Supabase adapters for the current settings. `NotConfigured` is returned as an error the
    /// UI turns into an input card; nothing is attempted.
    pub fn supabase(&self, conn: &Connection) -> AppResult<SupabaseHandle> {
        let config = RemoteConfig::load(conn)?;
        let setting = config::read_mode(conn, IntegrationPort::Supabase)?;
        let mode = resolve_mode(setting, config.supabase_configured());
        let key = match mode {
            IntegrationMode::NotConfigured => {
                return Err(AppError::NotConfigured {
                    port: IntegrationPort::Supabase.as_str(),
                });
            }
            IntegrationMode::Mock => AdapterKey {
                mode,
                endpoint: FAKE_ENDPOINT.into(),
                anon_key: FAKE_ANON_KEY.into(),
            },
            IntegrationMode::Live => AdapterKey {
                mode,
                endpoint: config.supabase_endpoint.clone(),
                anon_key: config.supabase_anon_key.clone(),
            },
        };
        let mut slot = lock(&self.supabase);
        if let Some(existing) = slot.as_ref() {
            if existing.key == key {
                return Ok(existing.handle.clone());
            }
        }
        let handle = self.build_supabase(&key)?;
        *slot = Some(AuthSlot {
            key,
            handle: handle.clone(),
        });
        Ok(handle)
    }

    fn build_supabase(&self, key: &AdapterKey) -> AppResult<SupabaseHandle> {
        match key.mode {
            IntegrationMode::Mock => {
                let fake = Arc::clone(&self.fake);
                let secrets: Arc<dyn SecretStore> = self.mock_secrets.clone();
                let provider: Arc<dyn AuthProvider> = fake.auth();
                let auth = Arc::new(AuthService::new(
                    provider,
                    secrets,
                    fake.clone(),
                    Arc::clone(&self.clock),
                ));
                let tokens: Arc<dyn AccessTokenSource> = auth.clone();
                let rest = Arc::new(SupabaseRest::new(
                    &key.endpoint,
                    &key.anon_key,
                    fake,
                    tokens,
                ));
                Ok(SupabaseHandle {
                    mode: IntegrationMode::Mock,
                    auth,
                    rest,
                })
            }
            IntegrationMode::Live => {
                let host = host_of(&key.endpoint)
                    .ok_or_else(|| AppError::Invalid("the Supabase endpoint has no host".into()))?;
                let inner = (self.transports)(HostPolicy::configured([host]));
                let transport: Arc<dyn HttpTransport> = Arc::new(AuditedTransport::new(
                    inner,
                    Arc::clone(&self.sink),
                    Arc::clone(&self.clock),
                ));
                let provider = Arc::new(SupabaseAuth::new(
                    &key.endpoint,
                    &key.anon_key,
                    Arc::clone(&transport),
                ));
                // Sign-out removes the device row through a REST adapter that borrows the caller's
                // token, so it cannot call back into the (not yet built) auth service.
                let directory = Arc::new(SupabaseRest::new(
                    &key.endpoint,
                    &key.anon_key,
                    Arc::clone(&transport),
                    Arc::new(NoTokens),
                ));
                let auth = Arc::new(AuthService::new(
                    provider,
                    Arc::clone(&self.secrets),
                    directory,
                    Arc::clone(&self.clock),
                ));
                let tokens: Arc<dyn AccessTokenSource> = auth.clone();
                let rest = Arc::new(SupabaseRest::new(
                    &key.endpoint,
                    &key.anon_key,
                    transport,
                    tokens,
                ));
                Ok(SupabaseHandle {
                    mode: IntegrationMode::Live,
                    auth,
                    rest,
                })
            }
            IntegrationMode::NotConfigured => Err(AppError::NotConfigured {
                port: IntegrationPort::Supabase.as_str(),
            }),
        }
    }

    /// The phone-notification adapter for the current settings: mock records in memory, ntfy posts
    /// to the configured server, telegram asks the Edge Function (it alone holds the bot token).
    pub fn notify_client(&self, conn: &Connection) -> AppResult<Arc<dyn NotifyClient>> {
        let config = RemoteConfig::load(conn)?;
        let setting = config::read_mode(conn, IntegrationPort::Notify)?;
        let mode = resolve_mode(
            setting,
            config::is_configured(IntegrationPort::Notify, &config, self.secret_flags()),
        );
        let not_configured = || AppError::NotConfigured {
            port: IntegrationPort::Notify.as_str(),
        };
        match mode {
            IntegrationMode::NotConfigured => Err(not_configured()),
            IntegrationMode::Mock => Ok(Arc::new(RecordingNotify::default())),
            IntegrationMode::Live => match config.notify_channel.as_str() {
                // `phone_app` posts to the same ntfy topic; the phone app subscribes to it.
                "ntfy" | "phone_app" => {
                    let topic = self
                        .secrets
                        .get(SecretName::NtfyTopic)
                        .ok()
                        .flatten()
                        .ok_or_else(not_configured)?;
                    let host = host_of(&config.notify_ntfy_server).ok_or_else(not_configured)?;
                    let inner = (self.transports)(HostPolicy::configured([host]));
                    let transport: Arc<dyn HttpTransport> = Arc::new(AuditedTransport::new(
                        inner,
                        Arc::clone(&self.sink),
                        Arc::clone(&self.clock),
                    ));
                    Ok(Arc::new(NtfyClient::new(
                        &config.notify_ntfy_server,
                        topic.expose(),
                        transport,
                    )))
                }
                "telegram" => Ok(Arc::new(ServerTest::new(self.supabase(conn)?.rest))),
                _ => Err(not_configured()),
            },
        }
    }

    /// "Test connection" button. Mock answers locally; other ports have no adapter yet and say so.
    /// The database lock is held only while reading configuration, never during the probe.
    pub fn test_connection(
        &self,
        db: &Mutex<Connection>,
        port: IntegrationPort,
    ) -> AppResult<ConnectionTest> {
        let (mode, handle) = {
            let conn = crate::commands::lock_db(db)?;
            let setting = config::read_mode(&conn, port)?;
            let config = RemoteConfig::load(&conn)?;
            let mode = resolve_mode(
                setting,
                config::is_configured(port, &config, self.secret_flags()),
            );
            if mode == IntegrationMode::NotConfigured {
                return Err(AppError::NotConfigured {
                    port: port.as_str(),
                });
            }
            let handle = if port == IntegrationPort::Supabase {
                Some(self.supabase(&conn)?)
            } else {
                None
            };
            (mode, handle)
        };
        let Some(handle) = handle else {
            let (ok, code) = if mode == IntegrationMode::Mock {
                (true, "mock")
            } else {
                (false, "adapter_unavailable")
            };
            return Ok(ConnectionTest {
                port,
                ok,
                code: code.into(),
            });
        };
        let outcome = handle.rest.probe();
        match &outcome {
            Ok(()) => self.record_health(port, Health::Ok),
            Err(err) => self.record_health(
                port,
                Health::Degraded {
                    code: err.code().to_string(),
                },
            ),
        }
        let code = match (&outcome, mode) {
            (Ok(()), IntegrationMode::Mock) => "mock".to_string(),
            (Ok(()), _) => "ok".to_string(),
            (Err(err), _) => err.code().to_string(),
        };
        Ok(ConnectionTest {
            port,
            ok: outcome.is_ok(),
            code,
        })
    }
}

/// Token source for the device-directory adapter, which always receives a token explicitly.
struct NoTokens;

impl AccessTokenSource for NoTokens {
    fn access_token(&self) -> Result<String, super::auth::AuthError> {
        Err(super::auth::AuthError::NotSignedIn)
    }
    fn force_refresh(&self) -> Result<String, super::auth::AuthError> {
        Err(super::auth::AuthError::NotSignedIn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::remote::audit::tests::VecSink;
    use crate::remote::http::{HttpResponse, ScriptedTransport};
    use crate::remote::secrets::select_secret_store;
    use crate::remote::{ManualClock, ModeSetting};
    use crate::settings::SettingValue;

    fn runtime_with(factory: TransportFactory) -> (RemoteRuntime, Arc<VecSink>) {
        let sink = Arc::new(VecSink::default());
        let selected = select_secret_store(Arc::new(MemorySecretStore::in_memory()));
        (
            RemoteRuntime::new(
                selected,
                Arc::new(ManualClock::new(1_000)),
                sink.clone(),
                factory,
            ),
            sink,
        )
    }

    fn never_called() -> TransportFactory {
        Arc::new(|_| panic!("no transport may be built in this scenario"))
    }

    fn set(conn: &mut Connection, key: config::RemoteSettingKey, text: &str) {
        config::update_setting(conn, key, SettingValue::Text(text.into()), 1).expect("setting");
    }

    #[test]
    fn unconfigured_supabase_is_not_configured_and_builds_nothing() {
        let conn = open_in_memory().expect("db");
        let (runtime, _) = runtime_with(never_called());
        let states = runtime.states(&conn).expect("states");
        assert!(
            states
                .iter()
                .all(|s| s.mode == IntegrationMode::NotConfigured && s.health == Health::Unknown)
        );
        assert!(matches!(
            runtime.supabase(&conn),
            Err(AppError::NotConfigured { port: "supabase" })
        ));
        let db = Mutex::new(conn);
        assert!(matches!(
            runtime.test_connection(&db, IntegrationPort::Supabase),
            Err(AppError::NotConfigured { .. })
        ));
    }

    #[test]
    fn mock_mode_runs_end_to_end_without_any_transport_or_audit_row() {
        let mut conn = open_in_memory().expect("db");
        config::set_mode(&mut conn, IntegrationPort::Supabase, ModeSetting::Mock, 1).expect("mock");
        let (runtime, sink) = runtime_with(never_called());
        let handle = runtime.supabase(&conn).expect("mock handle");
        assert_eq!(handle.mode, IntegrationMode::Mock);
        handle.auth.request_otp("a@example.test").expect("otp");
        handle
            .auth
            .verify_otp("a@example.test", "000000")
            .expect("fixed mock code");
        let db = Mutex::new(conn);
        let test = runtime
            .test_connection(&db, IntegrationPort::Supabase)
            .expect("test");
        assert_eq!((test.ok, test.code.as_str()), (true, "mock"));
        let conn = db.lock().expect("lock");
        assert!(
            sink.0.lock().expect("sink").is_empty(),
            "nothing is sent, so nothing is audited"
        );
        // The mock login never touches the real secret store.
        assert!(
            !runtime
                .secret_store_status()
                .configured
                .supabase_refresh_token
        );
        let view = runtime.view(&conn).expect("view");
        assert!(view.auth.signed_in);
        assert_eq!(view.ports[0].mode, IntegrationMode::Mock);
        // The handle is reused while settings are unchanged.
        assert!(Arc::ptr_eq(
            &handle.auth,
            &runtime.supabase(&conn).expect("again").auth
        ));
    }

    #[test]
    fn live_mode_restricts_the_transport_to_the_configured_host_and_audits_requests() {
        let mut conn = open_in_memory().expect("db");
        set(
            &mut conn,
            config::RemoteSettingKey::SupabaseEndpoint,
            "https://proj.supabase.co",
        );
        set(
            &mut conn,
            config::RemoteSettingKey::SupabaseAnonKey,
            "anon-public-key",
        );
        let scripted = Arc::new(ScriptedTransport::new(
            HostPolicy::configured(["proj.supabase.co"]),
            vec![
                Ok(HttpResponse::ok_json(200, "{}")),
                Ok(HttpResponse::ok_json(503, "")),
            ],
        ));
        let seen_policy: Arc<Mutex<Option<HostPolicy>>> = Arc::new(Mutex::new(None));
        let factory: TransportFactory = {
            let scripted = scripted.clone();
            let seen_policy = seen_policy.clone();
            Arc::new(move |policy| {
                *seen_policy.lock().expect("policy") = Some(policy);
                scripted.clone()
            })
        };
        let (runtime, sink) = runtime_with(factory);
        assert_eq!(
            runtime.states(&conn).expect("states")[0].mode,
            IntegrationMode::Live
        );

        let db = Mutex::new(conn);
        let ok = runtime
            .test_connection(&db, IntegrationPort::Supabase)
            .expect("test");
        assert_eq!((ok.ok, ok.code.as_str()), (true, "ok"));
        let bad = runtime
            .test_connection(&db, IntegrationPort::Supabase)
            .expect("test");
        assert_eq!((bad.ok, bad.code.as_str()), (false, "http_error"));
        let conn = db.lock().expect("lock");

        assert_eq!(
            *seen_policy.lock().expect("policy"),
            Some(HostPolicy::configured(["proj.supabase.co"]))
        );
        let records = sink.0.lock().expect("sink").clone();
        assert_eq!(
            records
                .iter()
                .map(|r| (r.kind, r.outcome))
                .collect::<Vec<_>>(),
            [("probe", "ok"), ("probe", "http_error")]
        );
        let state = &runtime.states(&conn).expect("states")[0];
        assert_eq!(
            state.health,
            Health::Degraded {
                code: "http_error".into()
            }
        );
        assert_eq!(state.last_ok_ms, Some(1_000));
    }

    #[test]
    fn the_ntfy_notify_transport_is_audited_without_the_topic() {
        let mut conn = open_in_memory().expect("db");
        set(&mut conn, config::RemoteSettingKey::NotifyChannel, "ntfy");
        let scripted = Arc::new(ScriptedTransport::new(
            HostPolicy::configured(["ntfy.sh"]),
            vec![Ok(HttpResponse::ok_json(200, ""))],
        ));
        let factory: TransportFactory = {
            let scripted = scripted.clone();
            Arc::new(move |_| scripted.clone())
        };
        let (runtime, sink) = runtime_with(factory);
        runtime
            .secrets()
            .set(SecretName::NtfyTopic, "abcdefghijklmnopqrstuvwx")
            .expect("topic");
        runtime
            .notify_client(&conn)
            .expect("client")
            .send_test()
            .expect("sent");
        let records = sink.0.lock().expect("sink").clone();
        assert_eq!(
            records
                .iter()
                .map(|r| (r.port, r.kind, r.outcome))
                .collect::<Vec<_>>(),
            [("notify", "ntfy_test", "ok")]
        );
        assert!(!format!("{records:?}").contains("abcdefghijklmnopqrstuvwx"));
    }

    /// Reports whether the database lock was free at the moment a request was sent.
    struct LockProbe {
        db: Arc<Mutex<Connection>>,
        free_when_sent: Arc<Mutex<Vec<bool>>>,
    }

    impl HttpTransport for LockProbe {
        fn send(
            &self,
            _request: crate::remote::http::HttpRequest,
        ) -> Result<HttpResponse, crate::remote::http::TransportError> {
            self.free_when_sent
                .lock()
                .expect("log")
                .push(self.db.try_lock().is_ok());
            Ok(HttpResponse::ok_json(200, "{}"))
        }
    }

    #[test]
    fn connection_test_sends_nothing_while_the_database_lock_is_held() {
        let mut conn = open_in_memory().expect("db");
        set(
            &mut conn,
            config::RemoteSettingKey::SupabaseEndpoint,
            "https://proj.supabase.co",
        );
        set(&mut conn, config::RemoteSettingKey::SupabaseAnonKey, "k");
        let db = Arc::new(Mutex::new(conn));
        let log = Arc::new(Mutex::new(Vec::new()));
        let factory: TransportFactory = {
            let (db, log) = (db.clone(), log.clone());
            Arc::new(move |_| {
                Arc::new(LockProbe {
                    db: db.clone(),
                    free_when_sent: log.clone(),
                })
            })
        };
        let (runtime, _) = runtime_with(factory);
        assert!(
            runtime
                .test_connection(&db, IntegrationPort::Supabase)
                .expect("test")
                .ok
        );
        assert_eq!(
            *log.lock().expect("log"),
            vec![true],
            "the request went out with the lock released"
        );
    }

    #[test]
    fn changing_the_endpoint_replaces_the_adapters() {
        let mut conn = open_in_memory().expect("db");
        set(
            &mut conn,
            config::RemoteSettingKey::SupabaseEndpoint,
            "https://one.supabase.co",
        );
        set(&mut conn, config::RemoteSettingKey::SupabaseAnonKey, "k");
        let (runtime, _) = runtime_with(Arc::new(|_| {
            Arc::new(ScriptedTransport::new(HostPolicy::LoopbackOnly, vec![]))
        }));
        let first = runtime.supabase(&conn).expect("first");
        set(
            &mut conn,
            config::RemoteSettingKey::SupabaseEndpoint,
            "https://two.supabase.co",
        );
        let second = runtime.supabase(&conn).expect("second");
        assert!(!Arc::ptr_eq(&first.auth, &second.auth));
    }

    #[test]
    fn non_supabase_ports_report_honestly_until_their_adapters_exist() {
        let mut conn = open_in_memory().expect("db");
        config::set_mode(&mut conn, IntegrationPort::Notify, ModeSetting::Mock, 1).expect("mock");
        let (runtime, _) = runtime_with(never_called());
        let db = Mutex::new(conn);
        let mock = runtime
            .test_connection(&db, IntegrationPort::Notify)
            .expect("mock test");
        assert_eq!((mock.ok, mock.code.as_str()), (true, "mock"));
        assert!(matches!(
            runtime.test_connection(&db, IntegrationPort::Llm),
            Err(AppError::NotConfigured { port: "llm" })
        ));
    }

    #[test]
    fn degraded_secret_store_is_visible_in_the_view() {
        let conn = open_in_memory().expect("db");
        let faulty = crate::remote::secrets::tests::FaultyStore::new(true, false);
        let selected = select_secret_store(Arc::new(faulty));
        let runtime = RemoteRuntime::new(
            selected,
            Arc::new(ManualClock::new(1)),
            Arc::new(VecSink::default()),
            never_called(),
        );
        let view = runtime.view(&conn).expect("view");
        assert_eq!(view.secret_store.kind, SecretStoreKind::SessionOnly);
        assert_eq!(
            view.secret_store.degraded_code,
            Some("secret_store_session_only")
        );
    }
}
