//! Supabase REST adapter (PostgREST tables and RPCs). This is the shell the later stories use:
//! it attaches the public `apikey`, the user's bearer token, renews the token once on a 401, and
//! turns failures into stable codes. Request bodies are `EgressPayload`s only.

use std::sync::Arc;

use serde_json::Value;

use super::auth::{AccessTokenSource, AuthError, DeviceDirectory};
use super::http::{HttpRequest, HttpResponse, HttpTransport, Method, RequestTag, TransportError};
use super::payload::{Body, EgressPayload};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    #[error("not_signed_in")]
    NotSignedIn,
    /// Renewal was rejected: the user must sign in again. Local features are unaffected.
    #[error("auth_expired")]
    AuthExpired,
    #[error("http_{status}")]
    Http { status: u16 },
    #[error("transport")]
    Transport(TransportError),
    #[error("bad_response")]
    Decode,
    #[error("encode")]
    Encode,
}

impl RemoteError {
    pub fn code(self) -> &'static str {
        match self {
            RemoteError::NotSignedIn => "not_signed_in",
            RemoteError::AuthExpired => "auth_expired",
            RemoteError::Http { .. } => "http_error",
            RemoteError::Transport(err) => err.code(),
            RemoteError::Decode => "bad_response",
            RemoteError::Encode => "encode",
        }
    }
}

impl From<AuthError> for RemoteError {
    fn from(err: AuthError) -> Self {
        match err {
            AuthError::NotSignedIn => RemoteError::NotSignedIn,
            AuthError::Expired => RemoteError::AuthExpired,
            AuthError::Network(t) => RemoteError::Transport(t),
            _ => RemoteError::Decode,
        }
    }
}

pub struct SupabaseRest {
    endpoint: String,
    anon_key: String,
    transport: Arc<dyn HttpTransport>,
    tokens: Arc<dyn AccessTokenSource>,
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

impl SupabaseRest {
    pub fn new(endpoint: &str, anon_key: &str, transport: Arc<dyn HttpTransport>, tokens: Arc<dyn AccessTokenSource>) -> Self {
        Self { endpoint: endpoint.trim_end_matches('/').to_string(), anon_key: anon_key.to_string(), transport, tokens }
    }

    fn attempt(
        &self,
        tag: RequestTag,
        method: Method,
        path: &str,
        bearer: &str,
        body: Option<Body>,
        prefer: Option<&str>,
    ) -> Result<HttpResponse, RemoteError> {
        let mut request = HttpRequest::new(tag, method, format!("{}{}", self.endpoint, path))
            .header("apikey", &self.anon_key)
            .header("authorization", &format!("Bearer {bearer}"));
        if let Some(prefer) = prefer {
            request = request.header("prefer", prefer);
        }
        if let Some(body) = body {
            request = request.with_body(body);
        }
        self.transport.send(request).map_err(RemoteError::Transport)
    }

    /// Sends with the current token; a 401 renews the token once and repeats the request.
    /// `make_body` is called per attempt because a body is consumed by sending.
    fn call(
        &self,
        kind: &'static str,
        method: Method,
        path: &str,
        make_body: &dyn Fn() -> Result<Option<Body>, RemoteError>,
        prefer: Option<&str>,
    ) -> Result<HttpResponse, RemoteError> {
        let tag = RequestTag { port: "supabase", kind };
        let token = self.tokens.access_token()?;
        let mut response = self.attempt(tag, method, path, &token, make_body()?, prefer)?;
        if response.status == 401 {
            let renewed = self.tokens.force_refresh()?;
            response = self.attempt(tag, method, path, &renewed, make_body()?, prefer)?;
        }
        if response.is_success() {
            Ok(response)
        } else if response.status == 401 {
            Err(RemoteError::AuthExpired)
        } else {
            Err(RemoteError::Http { status: response.status })
        }
    }

    fn json_body<P: EgressPayload>(payload: &P) -> Result<Option<Body>, RemoteError> {
        Body::from_payload(payload).map(Some).map_err(|_| RemoteError::Encode)
    }

    fn parse(response: &HttpResponse) -> Result<Value, RemoteError> {
        if response.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&response.body).map_err(|_| RemoteError::Decode)
    }

    /// Calls `POST /rest/v1/rpc/<name>`. `kind` is the audit label (`rpc_heartbeat`).
    pub fn rpc<P: EgressPayload>(&self, kind: &'static str, name: &str, args: &P) -> Result<Value, RemoteError> {
        let response = self.call(kind, Method::Post, &format!("/rest/v1/rpc/{name}"), &|| Self::json_body(args), None)?;
        Self::parse(&response)
    }

    /// Inserts one row. `ignore_duplicates` makes a key clash a no-op (idempotent batches).
    pub fn insert<P: EgressPayload>(&self, kind: &'static str, table: &str, row: &P, ignore_duplicates: bool) -> Result<(), RemoteError> {
        let prefer = if ignore_duplicates { "resolution=ignore-duplicates,return=minimal" } else { "return=minimal" };
        self.call(kind, Method::Post, &format!("/rest/v1/{table}"), &|| Self::json_body(row), Some(prefer))?;
        Ok(())
    }

    /// Inserts a row, or overwrites the row with the same primary key (`merge-duplicates`).
    pub fn upsert<P: EgressPayload>(&self, kind: &'static str, table: &str, row: &P) -> Result<(), RemoteError> {
        self.call(kind, Method::Post, &format!("/rest/v1/{table}"), &|| Self::json_body(row), Some("resolution=merge-duplicates,return=minimal"))?;
        Ok(())
    }

    /// POSTs to an Edge Function (`/functions/v1/<name>`) and returns its JSON answer.
    pub fn function<P: EgressPayload>(&self, kind: &'static str, name: &str, args: &P) -> Result<Value, RemoteError> {
        let response = self.call(kind, Method::Post, &format!("/functions/v1/{name}"), &|| Self::json_body(args), None)?;
        Self::parse(&response)
    }

    /// `GET /rest/v1/<table>?col=eq.value...`.
    pub fn select(&self, kind: &'static str, table: &str, filters: &[(&str, &str)]) -> Result<Value, RemoteError> {
        let response = self.call(kind, Method::Get, &Self::table_path(table, filters), &|| Ok(None), None)?;
        Self::parse(&response)
    }

    pub fn delete(&self, kind: &'static str, table: &str, filters: &[(&str, &str)]) -> Result<(), RemoteError> {
        self.call(kind, Method::Delete, &Self::table_path(table, filters), &|| Ok(None), None)?;
        Ok(())
    }

    fn table_path(table: &str, filters: &[(&str, &str)]) -> String {
        let query: Vec<String> = filters.iter().map(|(col, value)| format!("{col}=eq.{}", encode(value))).collect();
        if query.is_empty() { format!("/rest/v1/{table}") } else { format!("/rest/v1/{table}?{}", query.join("&")) }
    }

    /// Reachability check for the "test connection" button: GoTrue's health route needs only the
    /// public key, so it works before any sign-in.
    pub fn probe(&self) -> Result<(), RemoteError> {
        let tag = RequestTag { port: "supabase", kind: "probe" };
        let request = HttpRequest::new(tag, Method::Get, format!("{}/auth/v1/health", self.endpoint)).header("apikey", &self.anon_key);
        let response = self.transport.send(request).map_err(RemoteError::Transport)?;
        if response.is_success() { Ok(()) } else { Err(RemoteError::Http { status: response.status }) }
    }
}

impl DeviceDirectory for SupabaseRest {
    fn remove_device(&self, access_token: &str, device_id: &str) -> Result<(), AuthError> {
        // The caller already holds a token; use it directly rather than going through the source.
        let tag = RequestTag { port: "supabase", kind: "device_remove" };
        let response = self
            .attempt(tag, Method::Delete, &Self::table_path("devices", &[("id", device_id)]), access_token, None, None)
            .map_err(|err| match err {
                RemoteError::Transport(t) => AuthError::Network(t),
                _ => AuthError::Decode,
            })?;
        if response.is_success() { Ok(()) } else { Err(AuthError::Server { status: response.status }) }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::remote::auth::AuthService;
    use crate::remote::fake::{FakeBackend, FAKE_ANON_KEY, FAKE_ENDPOINT};
    use crate::remote::http::{HostPolicy, ScriptedTransport};
    use crate::remote::payload::PresenceBeat;
    use crate::remote::secrets::MemorySecretStore;
    use crate::remote::ManualClock;

    struct StubTokens {
        refreshes: AtomicU32,
        refresh_result: Result<String, AuthError>,
    }

    impl AccessTokenSource for StubTokens {
        fn access_token(&self) -> Result<String, AuthError> {
            Ok("stale-token".into())
        }
        fn force_refresh(&self) -> Result<String, AuthError> {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            self.refresh_result.clone()
        }
    }

    fn beat() -> PresenceBeat {
        PresenceBeat { device_id: "d".into(), app_version: "0.1.0".into(), studying: false }
    }

    fn scripted_rest(fixture: &str, tokens: StubTokens) -> (Arc<ScriptedTransport>, Arc<StubTokens>, SupabaseRest) {
        let transport = Arc::new(ScriptedTransport::from_fixture(HostPolicy::configured(["proj.supabase.co"]), fixture).expect("fixture"));
        let tokens = Arc::new(tokens);
        let rest = SupabaseRest::new("https://proj.supabase.co", "anon-public-key", transport.clone(), tokens.clone());
        (transport, tokens, rest)
    }

    #[test]
    fn a_401_renews_the_token_once_and_repeats_the_request() {
        let tokens = StubTokens { refreshes: AtomicU32::new(0), refresh_result: Ok("fresh-token".into()) };
        let (transport, tokens, rest) = scripted_rest(include_str!("../../../fixtures/http/rest-401-then-ok.json"), tokens);
        assert_eq!(rest.rpc("rpc_heartbeat", "heartbeat", &beat()), Ok(serde_json::json!("ok")));
        assert_eq!(tokens.refreshes.load(Ordering::SeqCst), 1);
        let seen = transport.requests();
        assert_eq!(seen.len(), 2);
        let bearer = |i: usize| seen[i].headers.iter().find(|(k, _)| k == "authorization").map(|(_, v)| v.clone());
        assert_eq!(bearer(0).as_deref(), Some("Bearer stale-token"));
        assert_eq!(bearer(1).as_deref(), Some("Bearer fresh-token"));
        assert_eq!(seen[0].body, seen[1].body, "the retry sends the same payload");
        assert_eq!(seen[0].url, "https://proj.supabase.co/rest/v1/rpc/heartbeat");
    }

    #[test]
    fn a_second_401_is_reported_as_auth_expired_without_a_third_try() {
        let tokens = StubTokens { refreshes: AtomicU32::new(0), refresh_result: Ok("fresh".into()) };
        let (transport, _, rest) = scripted_rest(include_str!("../../../fixtures/http/rest-401-twice.json"), tokens);
        assert_eq!(rest.rpc("rpc_heartbeat", "heartbeat", &beat()), Err(RemoteError::AuthExpired));
        assert_eq!(transport.requests().len(), 2);
    }

    #[test]
    fn a_rejected_renewal_surfaces_as_auth_expired() {
        let tokens = StubTokens { refreshes: AtomicU32::new(0), refresh_result: Err(AuthError::Expired) };
        let (_, _, rest) = scripted_rest(include_str!("../../../fixtures/http/rest-401-twice.json"), tokens);
        assert_eq!(rest.rpc("rpc_heartbeat", "heartbeat", &beat()), Err(RemoteError::AuthExpired));
    }

    #[test]
    fn server_errors_keep_their_status_and_offline_is_a_transport_error() {
        let tokens = StubTokens { refreshes: AtomicU32::new(0), refresh_result: Ok("x".into()) };
        let (_, _, rest) = scripted_rest(include_str!("../../../fixtures/http/rest-failures.json"), tokens);
        assert_eq!(rest.rpc("rpc_heartbeat", "heartbeat", &beat()), Err(RemoteError::Http { status: 503 }));
        assert_eq!(rest.rpc("rpc_heartbeat", "heartbeat", &beat()), Err(RemoteError::Transport(TransportError::Connect)));
        assert_eq!(rest.select("select_devices", "devices", &[]), Err(RemoteError::Decode));
    }

    #[test]
    fn filters_are_url_encoded() {
        assert_eq!(SupabaseRest::table_path("devices", &[("id", "a b&c=d")]), "/rest/v1/devices?id=eq.a+b%26c%3Dd");
        assert_eq!(SupabaseRest::table_path("devices", &[]), "/rest/v1/devices");
    }

    // ---- real adapter against the fake backend (same code path, no sockets) ----

    #[test]
    fn rest_adapter_round_trips_against_the_fake_backend() {
        let clock = Arc::new(ManualClock::new(1_000));
        let backend = Arc::new(FakeBackend::new(clock.clone()));
        let service = Arc::new(AuthService::new(backend.auth(), Arc::new(MemorySecretStore::in_memory()), backend.clone(), clock.clone()));
        service.request_otp("a@example.test").expect("otp");
        service.verify_otp("a@example.test", "000000").expect("verify");
        let rest = SupabaseRest::new(FAKE_ENDPOINT, FAKE_ANON_KEY, backend.clone(), service.clone());
        let device = "10000000-0000-4000-8000-00000000000a";

        let registration = crate::remote::payload::DeviceRegistration { id: device.into(), app_version: "0.1.0".into(), platform: "macos" };
        rest.insert("device_register", "devices", &registration, false).expect("register");
        assert_eq!(rest.insert("device_register", "devices", &registration, false), Err(RemoteError::Http { status: 409 }));
        rest.insert("device_register", "devices", &registration, true).expect("ignore duplicates");

        let beat = PresenceBeat { device_id: device.into(), app_version: "0.1.0".into(), studying: true };
        assert_eq!(rest.rpc("rpc_heartbeat", "heartbeat", &beat), Ok(serde_json::json!("ok")));
        assert_eq!(rest.select("select_presence", "presence", &[]).expect("rows").as_array().map(Vec::len), Some(1));
        rest.probe().expect("probe");

        // Token expiry is handled before the request (no 401 round trip), then the device goes away.
        clock.advance(3_600_000);
        assert_eq!(rest.rpc("rpc_heartbeat", "heartbeat", &beat), Ok(serde_json::json!("ok")));
        assert_eq!(backend.refresh_calls(), 1);
        service.sign_out(device).expect("sign out");
        assert_eq!(backend.device_count(), 0);
        assert_eq!(rest.rpc("rpc_heartbeat", "heartbeat", &beat), Err(RemoteError::NotSignedIn));
    }
}
