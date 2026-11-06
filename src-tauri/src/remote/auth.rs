//! Email one-time-code sign-in and session upkeep.
//!
//! Only the refresh token is persisted (in `SecretStore`); the access token lives in memory and is
//! renewed when it is about to expire. A failed renewal marks the account `auth_expired` and asks
//! for a new sign-in; local features never depend on it. Logs name the mail domain at most: no full
//! address, code, or token.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::http::{HttpRequest, HttpResponse, HttpTransport, Method, RequestTag, TransportError};
use super::payload::{Body, OtpRequest, OtpVerify, RefreshRequest};
use super::secrets::{SecretError, SecretName, SecretStore};
use super::Clock;
use crate::error::AppError;

/// Wrong codes tolerated before the client stops asking the server for a while.
pub const MAX_WRONG_CODES: u32 = 3;
pub const CODE_COOLDOWN_MS: i64 = 30_000;
/// An access token is renewed this long before it expires, so a request never starts with a token
/// that dies in flight.
pub const EXPIRY_SKEW_MS: i64 = 30_000;
pub const DEGRADED_AUTH_EXPIRED: &str = "auth_expired";

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("invalid_email")]
    InvalidEmail,
    #[error("invalid_code")]
    InvalidCode,
    #[error("rate_limited")]
    RateLimited { retry_after_ms: u64 },
    #[error("code_cooldown")]
    Cooldown { until_ms: i64 },
    /// The refresh token was rejected or is gone: sign in again.
    #[error("auth_expired")]
    Expired,
    #[error("not_signed_in")]
    NotSignedIn,
    #[error("network")]
    Network(TransportError),
    #[error("server_error")]
    Server { status: u16 },
    #[error("bad_response")]
    Decode,
    #[error("secret_store")]
    Secret(SecretError),
}

impl AuthError {
    /// Stable code for the UI and logs.
    pub fn code(self) -> &'static str {
        match self {
            AuthError::InvalidEmail => "invalid_email",
            AuthError::InvalidCode => "invalid_code",
            AuthError::RateLimited { .. } => "rate_limited",
            AuthError::Cooldown { .. } => "code_cooldown",
            AuthError::Expired => DEGRADED_AUTH_EXPIRED,
            AuthError::NotSignedIn => "not_signed_in",
            AuthError::Network(_) => "network",
            AuthError::Server { .. } => "server_error",
            AuthError::Decode => "bad_response",
            AuthError::Secret(_) => "secret_store",
        }
    }
}

impl From<AuthError> for AppError {
    fn from(err: AuthError) -> Self {
        AppError::Remote(err.code().to_string())
    }
}

/// Tokens from the server. `Debug` prints no token text.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in_sec: i64,
    pub user_id: String,
}

impl std::fmt::Debug for AuthTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthTokens { .. }")
    }
}

pub trait AuthProvider: Send + Sync {
    fn request_otp(&self, email: &str) -> Result<(), AuthError>;
    fn verify_otp(&self, email: &str, code: &str) -> Result<AuthTokens, AuthError>;
    fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, AuthError>;
    fn sign_out(&self, access_token: &str) -> Result<(), AuthError>;
}

/// Removes this device's server-side row on sign-out (its presence row goes with it).
pub trait DeviceDirectory: Send + Sync {
    fn remove_device(&self, access_token: &str, device_id: &str) -> Result<(), AuthError>;
}

/// Gives the sync side a current access token without letting it see the refresh token.
pub trait AccessTokenSource: Send + Sync {
    fn access_token(&self) -> Result<String, AuthError>;
    /// Called after a 401: renews once regardless of the cached expiry.
    fn force_refresh(&self) -> Result<String, AuthError>;
}

/// Mail domain for logs (`example.com`), or `?` when the text has no `@`.
pub fn email_domain(email: &str) -> &str {
    email.rsplit_once('@').map_or("?", |(_, domain)| domain)
}

/// Light shape check; the server decides whether the address is deliverable.
pub fn validate_email(email: &str) -> Result<(), AuthError> {
    let ok = email.len() <= 254
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
        })
        && !email.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok { Ok(()) } else { Err(AuthError::InvalidEmail) }
}

// ---- real adapter: Supabase GoTrue ----

pub struct SupabaseAuth {
    endpoint: String,
    anon_key: String,
    transport: Arc<dyn HttpTransport>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    user: TokenUser,
}

#[derive(Deserialize)]
struct TokenUser {
    id: String,
}

impl SupabaseAuth {
    pub fn new(endpoint: &str, anon_key: &str, transport: Arc<dyn HttpTransport>) -> Self {
        Self { endpoint: endpoint.trim_end_matches('/').to_string(), anon_key: anon_key.to_string(), transport }
    }

    fn call(&self, kind: &'static str, path: &str, body: Option<Body>, bearer: Option<&str>) -> Result<HttpResponse, AuthError> {
        let tag = RequestTag { port: "supabase", kind };
        let mut request = HttpRequest::new(tag, Method::Post, format!("{}{}", self.endpoint, path)).header("apikey", &self.anon_key);
        if let Some(token) = bearer {
            request = request.header("authorization", &format!("Bearer {token}"));
        }
        if let Some(body) = body {
            request = request.with_body(body);
        }
        self.transport.send(request).map_err(AuthError::Network)
    }

    fn tokens(response: &HttpResponse) -> Result<AuthTokens, AuthError> {
        let parsed: TokenResponse = serde_json::from_slice(&response.body).map_err(|_| AuthError::Decode)?;
        Ok(AuthTokens {
            access_token: parsed.access_token,
            refresh_token: parsed.refresh_token,
            expires_in_sec: parsed.expires_in,
            user_id: parsed.user.id,
        })
    }

    fn common_failure(response: &HttpResponse) -> AuthError {
        if response.status == 429 {
            AuthError::RateLimited { retry_after_ms: response.retry_after_ms().unwrap_or(30_000) }
        } else {
            AuthError::Server { status: response.status }
        }
    }
}

impl AuthProvider for SupabaseAuth {
    fn request_otp(&self, email: &str) -> Result<(), AuthError> {
        validate_email(email)?;
        let body = Body::from_payload(&OtpRequest { email: email.to_string(), create_user: true }).map_err(|_| AuthError::Decode)?;
        let response = self.call("auth_otp", "/auth/v1/otp", Some(body), None)?;
        match response.status {
            200..=299 => Ok(()),
            400 | 422 => Err(AuthError::InvalidEmail),
            _ => Err(Self::common_failure(&response)),
        }
    }

    fn verify_otp(&self, email: &str, code: &str) -> Result<AuthTokens, AuthError> {
        let payload = OtpVerify { email: email.to_string(), token: code.to_string(), kind: "email" };
        let body = Body::from_payload(&payload).map_err(|_| AuthError::Decode)?;
        let response = self.call("auth_verify", "/auth/v1/verify", Some(body), None)?;
        match response.status {
            200..=299 => Self::tokens(&response),
            400 | 401 | 403 | 422 => Err(AuthError::InvalidCode),
            _ => Err(Self::common_failure(&response)),
        }
    }

    fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, AuthError> {
        let body = Body::from_payload(&RefreshRequest { refresh_token: refresh_token.to_string() }).map_err(|_| AuthError::Decode)?;
        let response = self.call("auth_refresh", "/auth/v1/token?grant_type=refresh_token", Some(body), None)?;
        match response.status {
            200..=299 => Self::tokens(&response),
            400 | 401 | 403 => Err(AuthError::Expired),
            _ => Err(Self::common_failure(&response)),
        }
    }

    fn sign_out(&self, access_token: &str) -> Result<(), AuthError> {
        let response = self.call("auth_logout", "/auth/v1/logout", None, Some(access_token))?;
        match response.status {
            // 401: the session is already gone, which is the state we wanted.
            200..=299 | 401 => Ok(()),
            _ => Err(Self::common_failure(&response)),
        }
    }
}

// ---- session service ----

struct CachedAccess {
    token: String,
    expires_at_ms: i64,
}

#[derive(Default)]
struct AuthState {
    access: Option<CachedAccess>,
    user_id: Option<String>,
    wrong_codes: u32,
    cooldown_until_ms: i64,
    expired: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub signed_in: bool,
    pub user_id: Option<String>,
    /// `auth_expired` when the stored login no longer works and a new sign-in is needed.
    pub degraded_code: Option<&'static str>,
    pub cooldown_until_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignOutReport {
    pub device_removed: bool,
    pub server_session_ended: bool,
}

pub struct AuthService {
    provider: Arc<dyn AuthProvider>,
    secrets: Arc<dyn SecretStore>,
    devices: Arc<dyn DeviceDirectory>,
    clock: Arc<dyn Clock>,
    state: Mutex<AuthState>,
    signed_in: Arc<AtomicBool>,
}

impl AuthService {
    /// Resumes a stored login: `signed_in` starts true when a refresh token exists.
    pub fn new(
        provider: Arc<dyn AuthProvider>,
        secrets: Arc<dyn SecretStore>,
        devices: Arc<dyn DeviceDirectory>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let has_token = matches!(secrets.get(SecretName::SupabaseRefreshToken), Ok(Some(_)));
        Self {
            provider,
            secrets,
            devices,
            clock,
            state: Mutex::new(AuthState::default()),
            signed_in: Arc::new(AtomicBool::new(has_token)),
        }
    }

    /// Workers poll this and stop when it turns false (sign-out).
    pub fn signed_in_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.signed_in)
    }

    pub fn is_signed_in(&self) -> bool {
        self.signed_in.load(Ordering::SeqCst)
    }

    fn now(&self) -> Result<i64, AuthError> {
        self.clock.now_ms().map_err(|_| AuthError::Decode)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AuthState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn request_otp(&self, email: &str) -> Result<(), AuthError> {
        validate_email(email)?;
        let result = self.provider.request_otp(email);
        tracing::info!(event = "auth_request_otp", domain = email_domain(email), ok = result.is_ok());
        result
    }

    /// Exchanges the mailed code for a session. Three wrong codes start a 30-second client-side
    /// cooldown during which the server is not asked at all.
    pub fn verify_otp(&self, email: &str, code: &str) -> Result<String, AuthError> {
        validate_email(email)?;
        let now = self.now()?;
        {
            let state = self.lock();
            if now < state.cooldown_until_ms {
                return Err(AuthError::Cooldown { until_ms: state.cooldown_until_ms });
            }
        }
        let wellformed = code.len() == 6 && code.chars().all(|c| c.is_ascii_digit());
        let outcome = if wellformed { self.provider.verify_otp(email, code) } else { Err(AuthError::InvalidCode) };
        tracing::info!(event = "auth_verify", domain = email_domain(email), ok = outcome.is_ok());
        match outcome {
            Ok(tokens) => {
                self.secrets
                    .set(SecretName::SupabaseRefreshToken, &tokens.refresh_token)
                    .map_err(AuthError::Secret)?;
                let mut state = self.lock();
                state.wrong_codes = 0;
                state.cooldown_until_ms = 0;
                state.expired = false;
                state.user_id = Some(tokens.user_id.clone());
                state.access = Some(CachedAccess {
                    token: tokens.access_token,
                    expires_at_ms: now.saturating_add(tokens.expires_in_sec.saturating_mul(1000)),
                });
                self.signed_in.store(true, Ordering::SeqCst);
                Ok(tokens.user_id)
            }
            Err(AuthError::InvalidCode) => {
                let mut state = self.lock();
                state.wrong_codes += 1;
                if state.wrong_codes >= MAX_WRONG_CODES {
                    state.wrong_codes = 0;
                    state.cooldown_until_ms = now.saturating_add(CODE_COOLDOWN_MS);
                }
                Err(AuthError::InvalidCode)
            }
            Err(other) => Err(other),
        }
    }

    fn renew(&self) -> Result<String, AuthError> {
        let refresh = match self.secrets.get(SecretName::SupabaseRefreshToken) {
            Ok(Some(value)) => value,
            Ok(None) => {
                self.signed_in.store(false, Ordering::SeqCst);
                return Err(AuthError::NotSignedIn);
            }
            Err(err) => return Err(AuthError::Secret(err)),
        };
        let now = self.now()?;
        match self.provider.refresh(refresh.expose()) {
            Ok(tokens) => {
                // Refresh tokens rotate: the new one replaces the old before the old is forgotten.
                self.secrets.set(SecretName::SupabaseRefreshToken, &tokens.refresh_token).map_err(AuthError::Secret)?;
                let mut state = self.lock();
                state.expired = false;
                state.user_id = Some(tokens.user_id);
                let access = tokens.access_token.clone();
                state.access = Some(CachedAccess {
                    token: tokens.access_token,
                    expires_at_ms: now.saturating_add(tokens.expires_in_sec.saturating_mul(1000)),
                });
                Ok(access)
            }
            Err(AuthError::Expired) => {
                let mut state = self.lock();
                state.expired = true;
                state.access = None;
                tracing::warn!(event = "auth_expired", "stored login was rejected; sign in again");
                Err(AuthError::Expired)
            }
            Err(other) => Err(other),
        }
    }

    pub fn status(&self) -> AuthStatus {
        let state = self.lock();
        let now = self.clock.now_ms().unwrap_or(0);
        AuthStatus {
            signed_in: self.is_signed_in() && !state.expired,
            user_id: state.user_id.clone(),
            degraded_code: state.expired.then_some(DEGRADED_AUTH_EXPIRED),
            cooldown_until_ms: (state.cooldown_until_ms > now).then_some(state.cooldown_until_ms),
        }
    }

    /// Ends the login. Local sign-out always completes: the server calls are best effort and the
    /// report says which succeeded. Order matters: the device row and the server session need the
    /// access token, so they go first; then the stored secret and in-memory state are dropped and
    /// the `signed_in` flag stops the workers.
    pub fn sign_out(&self, device_id: &str) -> Result<SignOutReport, AuthError> {
        let access = self.access_token().ok();
        let mut report = SignOutReport { device_removed: false, server_session_ended: false };
        if let Some(token) = access.as_deref() {
            report.device_removed = self.devices.remove_device(token, device_id).is_ok();
            report.server_session_ended = self.provider.sign_out(token).is_ok();
        }
        self.secrets.delete(SecretName::SupabaseRefreshToken).map_err(AuthError::Secret)?;
        let mut state = self.lock();
        *state = AuthState::default();
        self.signed_in.store(false, Ordering::SeqCst);
        tracing::info!(event = "auth_sign_out", device_removed = report.device_removed, session_ended = report.server_session_ended);
        Ok(report)
    }
}

impl AccessTokenSource for AuthService {
    fn access_token(&self) -> Result<String, AuthError> {
        if !self.is_signed_in() {
            return Err(AuthError::NotSignedIn);
        }
        let now = self.now()?;
        {
            let state = self.lock();
            if let Some(access) = &state.access {
                if access.expires_at_ms - EXPIRY_SKEW_MS > now {
                    return Ok(access.token.clone());
                }
            }
        }
        self.renew()
    }

    fn force_refresh(&self) -> Result<String, AuthError> {
        if !self.is_signed_in() {
            return Err(AuthError::NotSignedIn);
        }
        self.renew()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::fake::FakeBackend;
    use crate::remote::http::{HostPolicy, ScriptedTransport};
    use crate::remote::secrets::MemorySecretStore;
    use crate::remote::ManualClock;
    use crate::testutil::capture_logs;

    struct Rig {
        backend: Arc<FakeBackend>,
        clock: Arc<ManualClock>,
        secrets: Arc<MemorySecretStore>,
        service: AuthService,
    }

    fn rig() -> Rig {
        let clock = Arc::new(ManualClock::new(1_000_000));
        let backend = Arc::new(FakeBackend::new(clock.clone()));
        let secrets = Arc::new(MemorySecretStore::in_memory());
        let service = AuthService::new(backend.auth(), secrets.clone(), backend.clone(), clock.clone());
        Rig { backend, clock, secrets, service }
    }

    fn sign_in(rig: &Rig) -> String {
        rig.service.request_otp("person@example.test").expect("otp");
        rig.service.verify_otp("person@example.test", "000000").expect("verify")
    }

    #[test]
    fn sign_in_stores_only_the_refresh_token_and_survives_a_restart() {
        let rig = rig();
        let user = sign_in(&rig);
        assert!(rig.service.is_signed_in());
        let stored = rig.secrets.get(SecretName::SupabaseRefreshToken).expect("get").expect("stored");
        assert!(stored.expose().starts_with("fake-refresh-"));
        // A new service over the same secret store resumes without a code.
        let resumed = AuthService::new(rig.backend.auth(), rig.secrets.clone(), rig.backend.clone(), rig.clock.clone());
        assert!(resumed.is_signed_in());
        assert!(!resumed.access_token().expect("renewed").is_empty());
        assert_eq!(resumed.status().user_id.as_deref(), Some(user.as_str()));
    }

    #[test]
    fn expired_access_token_is_renewed_before_the_request() {
        let rig = rig();
        sign_in(&rig);
        let first = rig.service.access_token().expect("token");
        assert_eq!(rig.service.access_token().expect("cached"), first, "valid token is reused");
        rig.clock.advance(3_600_000 - EXPIRY_SKEW_MS + 1);
        let second = rig.service.access_token().expect("renewed");
        assert_ne!(second, first);
        assert_eq!(rig.backend.refresh_calls(), 1);
    }

    #[test]
    fn rejected_refresh_degrades_without_touching_anything_else() {
        let rig = rig();
        sign_in(&rig);
        rig.backend.revoke_all_sessions();
        rig.clock.advance(3_600_000);
        assert_eq!(rig.service.access_token(), Err(AuthError::Expired));
        let status = rig.service.status();
        assert!(!status.signed_in);
        assert_eq!(status.degraded_code, Some("auth_expired"));
        // Signing in again clears the degraded state.
        rig.service.verify_otp("person@example.test", "000000").expect("sign in again");
        assert_eq!(rig.service.status().degraded_code, None);
    }

    #[test]
    fn network_failure_during_refresh_is_not_an_expiry() {
        let rig = rig();
        sign_in(&rig);
        rig.clock.advance(3_600_000);
        rig.backend.set_outage(true);
        assert!(matches!(rig.service.access_token(), Err(AuthError::Network(_))));
        assert_eq!(rig.service.status().degraded_code, None, "offline is not a reason to ask for a new login");
        rig.backend.set_outage(false);
        assert!(rig.service.access_token().is_ok());
    }

    #[test]
    fn three_wrong_codes_start_a_thirty_second_cooldown() {
        let rig = rig();
        rig.service.request_otp("person@example.test").expect("otp");
        for _ in 0..3 {
            assert_eq!(rig.service.verify_otp("person@example.test", "111111"), Err(AuthError::InvalidCode));
        }
        let calls = rig.backend.verify_calls();
        let until = 1_000_000 + CODE_COOLDOWN_MS;
        assert_eq!(rig.service.verify_otp("person@example.test", "000000"), Err(AuthError::Cooldown { until_ms: until }));
        assert_eq!(rig.backend.verify_calls(), calls, "the server is not asked during the cooldown");
        assert_eq!(rig.service.status().cooldown_until_ms, Some(until));
        rig.clock.advance(CODE_COOLDOWN_MS);
        assert!(rig.service.verify_otp("person@example.test", "000000").is_ok());
    }

    #[test]
    fn malformed_code_counts_as_wrong_without_a_server_call() {
        let rig = rig();
        assert_eq!(rig.service.verify_otp("person@example.test", "12"), Err(AuthError::InvalidCode));
        assert_eq!(rig.backend.verify_calls(), 0);
    }

    #[test]
    fn sign_out_removes_secret_device_session_and_stops_workers() {
        let rig = rig();
        sign_in(&rig);
        let flag = rig.service.signed_in_flag();
        let token = rig.service.access_token().expect("token");
        rig.backend.register_device_for(&token, "device-1");
        assert!(flag.load(Ordering::SeqCst));

        let report = rig.service.sign_out("device-1").expect("sign out");
        assert_eq!(report, SignOutReport { device_removed: true, server_session_ended: true });
        assert!(!flag.load(Ordering::SeqCst), "workers see the stop signal");
        assert_eq!(rig.secrets.get(SecretName::SupabaseRefreshToken), Ok(None));
        assert_eq!(rig.backend.device_count(), 0);
        assert!(rig.backend.is_revoked(&token));
        assert_eq!(rig.service.access_token(), Err(AuthError::NotSignedIn));
    }

    #[test]
    fn sign_out_completes_locally_even_when_the_server_is_unreachable() {
        let rig = rig();
        sign_in(&rig);
        rig.backend.set_outage(true);
        let report = rig.service.sign_out("device-1").expect("local sign-out still works");
        assert_eq!(report, SignOutReport { device_removed: false, server_session_ended: false });
        assert_eq!(rig.secrets.get(SecretName::SupabaseRefreshToken), Ok(None));
        assert!(!rig.service.is_signed_in());
    }

    #[test]
    fn logs_carry_the_mail_domain_but_no_address_code_or_token() {
        let rig = rig();
        let logs = capture_logs(|| {
            let _ = rig.service.request_otp("person@example.test");
            let _ = rig.service.verify_otp("person@example.test", "000000");
            let _ = rig.service.access_token();
            let _ = rig.service.sign_out("device-1");
        });
        assert!(logs.contains("example.test"), "{logs}");
        assert!(!logs.contains("person@"), "{logs}");
        assert!(!logs.contains("000000"), "{logs}");
        assert!(!logs.contains("fake-access") && !logs.contains("fake-refresh"), "{logs}");
    }

    #[test]
    fn email_helpers() {
        assert_eq!(email_domain("a@b.test"), "b.test");
        assert_eq!(email_domain("nope"), "?");
        assert!(validate_email("a@b.test").is_ok());
        for bad in ["", "a", "a@b", "@b.test", "a b@c.test", "a@.test", "a@b."] {
            assert_eq!(validate_email(bad), Err(AuthError::InvalidEmail), "{bad}");
        }
    }

    // ---- real adapter against recorded responses ----

    fn scripted(fixture: &str) -> (Arc<ScriptedTransport>, SupabaseAuth) {
        let transport = Arc::new(ScriptedTransport::from_fixture(HostPolicy::configured(["proj.supabase.co"]), fixture).expect("fixture"));
        let auth = SupabaseAuth::new("https://proj.supabase.co/", "anon-public-key", transport.clone());
        (transport, auth)
    }

    #[test]
    fn real_adapter_builds_gotrue_requests_and_parses_tokens() {
        let (transport, auth) = scripted(include_str!("../../../fixtures/http/auth-success.json"));
        auth.request_otp("person@example.test").expect("otp");
        let tokens = auth.verify_otp("person@example.test", "123456").expect("verify");
        assert_eq!((tokens.user_id.as_str(), tokens.expires_in_sec), ("8d2e5c1a-0000-4000-8000-000000000001", 3600));
        auth.refresh("old-refresh").expect("refresh");
        auth.sign_out(&tokens.access_token).expect("logout");

        let seen = transport.requests();
        let urls: Vec<&str> = seen.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://proj.supabase.co/auth/v1/otp",
                "https://proj.supabase.co/auth/v1/verify",
                "https://proj.supabase.co/auth/v1/token?grant_type=refresh_token",
                "https://proj.supabase.co/auth/v1/logout",
            ]
        );
        assert!(seen.iter().all(|r| r.headers.iter().any(|(k, v)| k == "apikey" && v == "anon-public-key")));
        let verify_body: serde_json::Value = serde_json::from_slice(&seen[1].body).expect("json");
        assert_eq!(verify_body, serde_json::json!({"email": "person@example.test", "token": "123456", "type": "email"}));
        assert!(seen[3].headers.iter().any(|(k, v)| k == "authorization" && v.starts_with("Bearer ")));
    }

    #[test]
    fn real_adapter_maps_failures() {
        let (_, auth) = scripted(include_str!("../../../fixtures/http/auth-failures.json"));
        // Order follows the fixture file.
        assert_eq!(auth.verify_otp("person@example.test", "123456"), Err(AuthError::InvalidCode));
        assert_eq!(auth.refresh("r"), Err(AuthError::Expired));
        assert_eq!(auth.request_otp("person@example.test"), Err(AuthError::RateLimited { retry_after_ms: 42_000 }));
        assert_eq!(auth.verify_otp("person@example.test", "123456"), Err(AuthError::Server { status: 503 }));
        assert_eq!(auth.refresh("r"), Err(AuthError::Network(TransportError::Timeout)));
        assert_eq!(auth.verify_otp("person@example.test", "123456"), Err(AuthError::Decode));
    }

    #[test]
    fn real_adapter_refuses_a_foreign_host_before_sending() {
        let transport = Arc::new(ScriptedTransport::new(HostPolicy::configured(["proj.supabase.co"]), vec![]));
        let auth = SupabaseAuth::new("https://evil.example.com", "anon", transport.clone());
        assert_eq!(auth.request_otp("person@example.test"), Err(AuthError::Network(TransportError::HostDenied)));
        assert!(transport.requests().is_empty());
    }
}
