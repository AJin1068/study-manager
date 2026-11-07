//! The only way application code calls the LLM. A call passes, in order: the switch and consent
//! (`llm.enabled`, plus `llm.judge_enabled` for tab/window purposes), configuration (a missing key
//! is `NotConfigured`, never silently the mock), and the daily token budget. Then it runs with the
//! database lock released and leaves one content-free `llm_usage` row.
//!
//! A refusal is a value (`LlmError::Disabled` / `NotConfigured` / `BudgetExceeded`) that callers
//! answer with their local fallback. Nothing here can touch locking, timing, or blocking.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{Local, TimeZone};
use rusqlite::Connection;
use serde::Serialize;

use super::config::LlmConfig;
use super::{LlmError, LlmRequest, LlmResponse, Purpose};
use crate::commands::lock_db;
use crate::error::{AppError, AppResult};
use crate::llm::egress::Prompt;
use crate::remote::IntegrationPort;
use crate::remote::runtime::{ConnectionTest, RemoteRuntime};
use crate::remote::{Clock, IntegrationMode};
use crate::store::llm::{self as usage, NewUsage, PurposeUsage};

pub struct LlmService {
    db: Arc<Mutex<Connection>>,
    remote: Arc<RemoteRuntime>,
    clock: Arc<dyn Clock>,
}

/// What the LLM card shows.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LlmStatus {
    pub enabled: bool,
    pub judge_enabled: bool,
    pub model: String,
    pub mode: IntegrationMode,
    pub daily_budget_tokens: i64,
    pub used_today_tokens: i64,
    pub by_purpose: Vec<PurposeUsage>,
}

/// The local day containing `at_ms` as `[start, end)` in epoch milliseconds. Falls back to the UTC
/// day when the local midnight does not exist or is ambiguous.
pub fn day_bounds_ms<Tz: TimeZone>(tz: &Tz, at_ms: i64) -> (i64, i64) {
    const DAY_MS: i64 = 86_400_000;
    let utc_floor = at_ms.div_euclid(DAY_MS) * DAY_MS;
    let Some(now) = tz.timestamp_millis_opt(at_ms).single() else {
        return (utc_floor, utc_floor + DAY_MS);
    };
    let date = now.date_naive();
    let midnight = |d: chrono::NaiveDate| {
        d.and_hms_opt(0, 0, 0)
            .and_then(|t| tz.from_local_datetime(&t).earliest())
            .map(|t| t.timestamp_millis())
    };
    match (midnight(date), date.succ_opt().and_then(midnight)) {
        (Some(start), Some(end)) if start <= at_ms && at_ms < end => (start, end),
        _ => (utc_floor, utc_floor + DAY_MS),
    }
}

/// The pure part of the gate. `configured` is whether an adapter (mock or live) exists.
pub fn gate(
    config: &LlmConfig,
    purpose: Purpose,
    configured: bool,
    used_tokens: i64,
    estimate: u64,
) -> Result<(), LlmError> {
    if !config.enabled || (purpose.needs_judge_consent() && !config.judge_enabled) {
        return Err(LlmError::Disabled);
    }
    if !configured {
        return Err(LlmError::NotConfigured);
    }
    let estimate = i64::try_from(estimate).unwrap_or(i64::MAX);
    if used_tokens.saturating_add(estimate) > config.daily_budget_tokens {
        return Err(LlmError::BudgetExceeded);
    }
    Ok(())
}

impl LlmService {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        remote: Arc<RemoteRuntime>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { db, remote, clock }
    }

    fn now(&self) -> Result<i64, LlmError> {
        self.clock.now_ms().map_err(|_| LlmError::Internal)
    }

    /// Runs one call through the gate. Blocks for the length of the model call, so callers run it
    /// off the UI thread; the database lock is not held meanwhile.
    pub fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let now = self.now()?;
        let (config, client, used) = {
            let conn = lock_db(&self.db).map_err(|_| LlmError::Internal)?;
            let config = LlmConfig::load(&conn).map_err(|_| LlmError::Internal)?;
            let client = match self.remote.llm_client(&conn, &config.model) {
                Ok(pair) => Some(pair),
                Err(AppError::NotConfigured { .. }) => None,
                Err(_) => return Err(LlmError::Internal),
            };
            let (from, to) = day_bounds_ms(&Local, now);
            let used = usage::tokens_between(&conn, from, to).map_err(|_| LlmError::Internal)?;
            (config, client, used)
        };
        // Only the input is estimated: the output is unknown until it arrives, and it is counted
        // in the day's total afterwards.
        let estimate = request.prompt.estimated_input_tokens();
        if let Err(refusal) = gate(&config, request.purpose, client.is_some(), used, estimate) {
            tracing::info!(
                event = "llm_call",
                purpose = request.purpose.as_str(),
                outcome = refusal.code()
            );
            return Err(refusal);
        }
        let Some((mode, client)) = client else {
            return Err(LlmError::NotConfigured);
        };

        let started = Instant::now();
        let result = client.complete(request);
        let latency_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);

        let (in_tokens, out_tokens, code) = match &result {
            Ok(response) => (
                i64::from(response.in_tokens),
                i64::from(response.out_tokens),
                "ok",
            ),
            Err(err) => (0, 0, err.code()),
        };
        tracing::info!(
            event = "llm_call",
            purpose = request.purpose.as_str(),
            outcome = code,
            latency_ms,
            in_tokens,
            out_tokens,
            cached = false
        );
        let mode_name = if mode == IntegrationMode::Mock {
            "mock"
        } else {
            "live"
        };
        match lock_db(&self.db) {
            Ok(conn) => {
                let row = NewUsage {
                    at_ms: self.now().unwrap_or(now),
                    purpose: request.purpose.as_str(),
                    model: &config.model,
                    mode: mode_name,
                    in_tokens,
                    out_tokens,
                    latency_ms,
                    cached: false,
                    result_code: code,
                };
                if let Err(err) = usage::insert_usage(&conn, &row) {
                    tracing::error!(event = "llm_usage_dropped", error = %err);
                }
            }
            Err(err) => tracing::error!(event = "llm_usage_dropped", error = %err),
        }
        if mode == IntegrationMode::Live {
            self.remote.record_llm_health(result.is_ok(), code);
        }
        result
    }

    /// "Test connection": one tiny gated call, counted against the budget like any other.
    pub fn test_connection(&self) -> AppResult<ConnectionTest> {
        let mode = self.status()?.mode;
        let request = LlmRequest {
            purpose: Purpose::Probe,
            prompt: Prompt::probe(),
            max_tokens: 16,
        };
        let outcome = self.complete(&request);
        let code = match (&outcome, mode) {
            (Ok(_), IntegrationMode::Mock) => "mock".to_string(),
            (Ok(_), _) => "ok".to_string(),
            (Err(err), _) => err.code().to_string(),
        };
        Ok(ConnectionTest {
            port: IntegrationPort::Llm,
            ok: outcome.is_ok(),
            code,
        })
    }

    pub fn status(&self) -> AppResult<LlmStatus> {
        let now = self.clock.now_ms()?;
        let conn = lock_db(&self.db)?;
        let config = LlmConfig::load(&conn)?;
        let mode = match self.remote.llm_client(&conn, &config.model) {
            Ok((mode, _)) => mode,
            Err(AppError::NotConfigured { .. }) => IntegrationMode::NotConfigured,
            Err(err) => return Err(err),
        };
        let (from, to) = day_bounds_ms(&Local, now);
        Ok(LlmStatus {
            enabled: config.enabled,
            judge_enabled: config.judge_enabled,
            model: config.model,
            mode,
            daily_budget_tokens: config.daily_budget_tokens,
            used_today_tokens: usage::tokens_between(&conn, from, to)?,
            by_purpose: usage::usage_by_purpose(&conn, from, to)?,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::llm::config::{LlmSettingKey, update_setting};
    use crate::llm::egress::Prompt;
    use crate::llm::mock::MockStep;
    use crate::remote::audit::tests::VecSink;
    use crate::remote::config::set_mode;
    use crate::remote::http::{
        HostPolicy, HttpRequest, HttpResponse, HttpTransport, ScriptedTransport, TransportError,
    };
    use crate::remote::runtime::TransportFactory;
    use crate::remote::secrets::{MemorySecretStore, SecretName, select_secret_store};
    use crate::remote::{ManualClock, ModeSetting};
    use crate::settings::SettingValue;
    use crate::testutil::capture_logs;

    pub(crate) const NOW: i64 = 1_800_000_000_000;

    pub(crate) struct Harness {
        pub db: Arc<Mutex<Connection>>,
        pub remote: Arc<RemoteRuntime>,
        pub service: Arc<LlmService>,
        pub clock: Arc<ManualClock>,
    }

    fn build(factory: TransportFactory, setup: impl FnOnce(&mut Connection)) -> Harness {
        let mut conn = open_in_memory().expect("db");
        setup(&mut conn);
        let db = Arc::new(Mutex::new(conn));
        let clock = Arc::new(ManualClock::new(NOW));
        let sink = Arc::new(VecSink::default());
        let selected = select_secret_store(Arc::new(MemorySecretStore::in_memory()));
        let remote = Arc::new(RemoteRuntime::new(
            selected,
            clock.clone(),
            sink.clone(),
            factory,
        ));
        let service = Arc::new(LlmService::new(
            Arc::clone(&db),
            Arc::clone(&remote),
            clock.clone(),
        ));
        Harness {
            db,
            remote,
            service,
            clock,
        }
    }

    pub(crate) fn never_called() -> TransportFactory {
        Arc::new(|_| panic!("no transport may be built in this scenario"))
    }

    /// A service in mock mode with the LLM switched on.
    pub(crate) fn mock_harness() -> Harness {
        build(never_called(), |conn| {
            set_mode(
                conn,
                crate::remote::IntegrationPort::Llm,
                ModeSetting::Mock,
                1,
            )
            .expect("mock mode");
            update_setting(conn, LlmSettingKey::Enabled, SettingValue::Bool(true), 1)
                .expect("enable");
        })
    }

    fn request(text: &str) -> LlmRequest {
        LlmRequest {
            purpose: Purpose::WikiIngest,
            prompt: Prompt::ingest("과목", text),
            max_tokens: 100,
        }
    }

    fn usage_rows(h: &Harness) -> Vec<(String, String, String, i64)> {
        let conn = h.db.lock().expect("db");
        let mut stmt = conn.prepare("SELECT purpose, mode, result_code, in_tokens + out_tokens FROM llm_usage ORDER BY id").expect("prepare");
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .expect("rows")
            .collect::<Result<_, _>>()
            .expect("collect")
    }

    fn config_with(enabled: bool, judge: bool, budget: i64) -> LlmConfig {
        LlmConfig {
            enabled,
            judge_enabled: judge,
            model: "m".into(),
            wiki_dir: String::new(),
            daily_budget_tokens: budget,
        }
    }

    #[test]
    fn gate_order_is_switch_then_configuration_then_budget() {
        use Purpose::{TabJudge, WikiAsk};
        let on = config_with(true, false, 1_000);
        assert_eq!(
            gate(&config_with(false, true, 1_000), WikiAsk, true, 0, 1),
            Err(LlmError::Disabled)
        );
        assert_eq!(
            gate(&on, WikiAsk, false, 0, 1),
            Err(LlmError::NotConfigured)
        );
        assert_eq!(
            gate(&config_with(false, false, 1_000), WikiAsk, false, 0, 1),
            Err(LlmError::Disabled),
            "off beats unconfigured"
        );
        assert_eq!(
            gate(&on, TabJudge, true, 0, 1),
            Err(LlmError::Disabled),
            "tab judging needs its own consent"
        );
        assert_eq!(
            gate(&config_with(true, true, 1_000), TabJudge, true, 0, 1),
            Ok(())
        );
        assert_eq!(gate(&on, WikiAsk, true, 999, 1), Ok(()));
        assert_eq!(
            gate(&on, WikiAsk, true, 999, 2),
            Err(LlmError::BudgetExceeded)
        );
        assert_eq!(
            gate(&on, WikiAsk, true, 1_000, 0),
            Ok(()),
            "exactly at the budget is still allowed"
        );
        assert_eq!(
            gate(&on, WikiAsk, true, i64::MAX, u64::MAX),
            Err(LlmError::BudgetExceeded)
        );
    }

    #[test]
    fn local_day_bounds_follow_the_zone_and_contain_the_instant() {
        use chrono_tz::Asia::Seoul;
        // 2026-11-06 11:44 KST
        let at = 1_793_933_040_000;
        let (start, end) = day_bounds_ms(&Seoul, at);
        assert_eq!(end - start, 86_400_000);
        assert!(start <= at && at < end);
        assert_eq!(
            Seoul
                .timestamp_millis_opt(start)
                .single()
                .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                .as_deref(),
            Some("2026-11-06 00:00")
        );
        // One millisecond before midnight stays on the same day; midnight starts the next.
        assert_eq!(day_bounds_ms(&Seoul, end - 1), (start, end));
        assert_eq!(day_bounds_ms(&Seoul, end), (end, end + 86_400_000));
    }

    #[test]
    fn llm_off_refuses_before_any_adapter_runs_and_records_nothing() {
        let h = build(never_called(), |conn| {
            set_mode(
                conn,
                crate::remote::IntegrationPort::Llm,
                ModeSetting::Mock,
                1,
            )
            .expect("mock");
        });
        assert_eq!(
            h.service.complete(&request("노트")),
            Err(LlmError::Disabled)
        );
        assert_eq!(h.remote.mock_llm().call_count(), 0);
        assert!(usage_rows(&h).is_empty());
    }

    #[test]
    fn enabled_without_a_key_is_not_configured_never_the_mock() {
        let h = build(never_called(), |conn| {
            update_setting(conn, LlmSettingKey::Enabled, SettingValue::Bool(true), 1)
                .expect("enable");
        });
        assert_eq!(
            h.service.complete(&request("노트")),
            Err(LlmError::NotConfigured)
        );
        assert_eq!(h.remote.mock_llm().call_count(), 0);
        assert_eq!(
            h.service.status().expect("status").mode,
            IntegrationMode::NotConfigured
        );
    }

    #[test]
    fn mock_mode_answers_records_usage_without_content_and_logs_no_prompt_text() {
        let h = mock_harness();
        let mut logs_result = None;
        let logs = capture_logs(|| {
            logs_result = Some(h.service.complete(&request("SECRET-NOTE-123 를 정리해줘")))
        });
        let response = logs_result.expect("ran").expect("reply");
        assert!(response.text.contains("pages"));
        assert!(
            logs.contains("llm_call") && logs.contains("wiki_ingest"),
            "{logs}"
        );
        assert!(!logs.contains("SECRET-NOTE-123"), "{logs}");

        let rows = usage_rows(&h);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].0.as_str(), rows[0].1.as_str(), rows[0].2.as_str()),
            ("wiki_ingest", "mock", "ok")
        );
        assert_eq!(
            i64::from(response.in_tokens + response.out_tokens),
            rows[0].3
        );
        let conn = h.db.lock().expect("db");
        let dump: String = conn
            .query_row(
                "SELECT purpose || model || mode || result_code FROM llm_usage",
                [],
                |r| r.get(0),
            )
            .expect("dump");
        assert!(!dump.contains("SECRET"));
        let status_used = usage::tokens_between(&conn, 0, i64::MAX).expect("sum");
        assert_eq!(status_used, rows[0].3);
    }

    #[test]
    fn the_daily_budget_stops_calls_and_resets_with_the_local_day() {
        let h = mock_harness();
        {
            let mut conn = h.db.lock().expect("db");
            update_setting(
                &mut conn,
                LlmSettingKey::DailyBudgetTokens,
                SettingValue::Int(1_000),
                2,
            )
            .expect("budget");
        }
        // The first call fits; its recorded tokens then use the budget up.
        let first = h.service.complete(&request("짧은 노트"));
        assert!(first.is_ok());
        let used = h.service.status().expect("status").used_today_tokens;
        assert!(used > 0);
        {
            let conn = h.db.lock().expect("db");
            usage::insert_usage(
                &conn,
                &NewUsage {
                    at_ms: NOW,
                    purpose: "wiki_ingest",
                    model: "m",
                    mode: "live",
                    in_tokens: 990,
                    out_tokens: 0,
                    latency_ms: 1,
                    cached: false,
                    result_code: "ok",
                },
            )
            .expect("usage");
        }
        let calls_before = h.remote.mock_llm().call_count();
        assert_eq!(
            h.service.complete(&request("또 하나")),
            Err(LlmError::BudgetExceeded)
        );
        assert_eq!(
            h.remote.mock_llm().call_count(),
            calls_before,
            "a refused call never reaches the adapter"
        );
        // A day later the budget is fresh again.
        h.clock.advance(2 * 86_400_000);
        assert!(h.service.complete(&request("내일")).is_ok());
    }

    #[test]
    fn an_oversized_prompt_is_refused_by_its_own_estimate() {
        let h = mock_harness();
        {
            let mut conn = h.db.lock().expect("db");
            update_setting(
                &mut conn,
                LlmSettingKey::DailyBudgetTokens,
                SettingValue::Int(1_000),
                2,
            )
            .expect("budget");
        }
        let big = LlmRequest {
            purpose: Purpose::WikiIngest,
            prompt: Prompt::ingest("s", &"가".repeat(5_000)),
            max_tokens: 100,
        };
        assert_eq!(h.service.complete(&big), Err(LlmError::BudgetExceeded));
    }

    #[test]
    fn failures_leave_a_usage_row_with_the_code_and_no_tokens() {
        let h = mock_harness();
        h.remote
            .mock_llm()
            .push(MockStep::Fail(LlmError::Overloaded));
        assert_eq!(
            h.service.complete(&request("노트")),
            Err(LlmError::Overloaded)
        );
        let rows = usage_rows(&h);
        assert_eq!((rows[0].2.as_str(), rows[0].3), ("llm_overloaded", 0));
    }

    #[test]
    fn live_mode_uses_the_anthropic_host_audits_the_request_and_never_leaks_the_key_into_the_database()
     {
        let body = crate::llm::anthropic::tests::ok_body("{\"pages\":[]}", 20, 5);
        let scripted = Arc::new(ScriptedTransport::new(
            HostPolicy::configured(["api.anthropic.com"]),
            vec![Ok(HttpResponse::ok_json(200, &body))],
        ));
        let seen_policy: Arc<Mutex<Option<HostPolicy>>> = Arc::new(Mutex::new(None));
        let factory: TransportFactory = {
            let (scripted, seen_policy) = (scripted.clone(), seen_policy.clone());
            Arc::new(move |policy| {
                *seen_policy.lock().expect("policy") = Some(policy);
                scripted.clone()
            })
        };
        let h = build(factory, |conn| {
            update_setting(conn, LlmSettingKey::Enabled, SettingValue::Bool(true), 1)
                .expect("enable");
        });
        h.remote
            .secrets()
            .set(SecretName::AnthropicApiKey, "sk-ant-LIVE-KEY")
            .expect("key");
        assert_eq!(
            h.service.status().expect("status").mode,
            IntegrationMode::Live
        );

        let response = h.service.complete(&request("노트")).expect("live reply");
        assert_eq!((response.in_tokens, response.out_tokens), (20, 5));
        assert_eq!(
            *seen_policy.lock().expect("policy"),
            Some(HostPolicy::configured(["api.anthropic.com"]))
        );
        let rows = usage_rows(&h);
        assert_eq!((rows[0].1.as_str(), rows[0].3), ("live", 25));
        let sent = scripted.requests();
        assert_eq!((sent[0].tag.port, sent[0].tag.kind), ("llm", "messages"));

        let conn = h.db.lock().expect("db");
        for table in ["settings", "setting_history", "llm_usage", "remote_events"] {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM {table}"))
                .expect("prepare");
            let columns = stmt.column_count();
            let rows = stmt
                .query_map([], |row| {
                    (0..columns)
                        .map(|i| row.get_ref(i).map(|v| format!("{v:?}")))
                        .collect::<Result<Vec<_>, _>>()
                })
                .expect("query");
            for row in rows {
                assert!(
                    !row.expect("row").join(" ").contains("sk-ant-LIVE-KEY"),
                    "key leaked into {table}"
                );
            }
        }
    }

    /// Reports whether the database lock was free while the request was in flight.
    struct LockProbe {
        db: Arc<Mutex<Connection>>,
        free: Arc<Mutex<Vec<bool>>>,
    }

    impl HttpTransport for LockProbe {
        fn send(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.free
                .lock()
                .expect("log")
                .push(self.db.try_lock().is_ok());
            Ok(HttpResponse::ok_json(
                200,
                &crate::llm::anthropic::tests::ok_body("{}", 1, 1),
            ))
        }
    }

    #[test]
    fn the_model_call_runs_with_the_database_lock_released() {
        let free = Arc::new(Mutex::new(Vec::new()));
        let db_slot: Arc<Mutex<Option<Arc<Mutex<Connection>>>>> = Arc::new(Mutex::new(None));
        let factory: TransportFactory = {
            let (free, db_slot) = (free.clone(), db_slot.clone());
            Arc::new(move |_| {
                let db = db_slot
                    .lock()
                    .expect("slot")
                    .clone()
                    .expect("db set before the first call");
                Arc::new(LockProbe {
                    db,
                    free: free.clone(),
                })
            })
        };
        let h = build(factory, |conn| {
            update_setting(conn, LlmSettingKey::Enabled, SettingValue::Bool(true), 1)
                .expect("enable");
        });
        *db_slot.lock().expect("slot") = Some(Arc::clone(&h.db));
        h.remote
            .secrets()
            .set(SecretName::AnthropicApiKey, "k")
            .expect("key");
        h.service.complete(&request("노트")).expect("reply");
        assert_eq!(*free.lock().expect("log"), vec![true]);
    }
}
