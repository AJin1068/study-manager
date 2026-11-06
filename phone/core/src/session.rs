//! The phone's anonymous Supabase session (GoTrue; samples in `fixtures/http/auth-*.json`). The
//! phone has no email: it signs in anonymously once, keeps the refresh token on the device and
//! renews the access token from it. A rejected renewal never creates a NEW anonymous identity by
//! itself, because a new identity would silently lose the pairing; the caller decides.

use std::sync::{Arc, Mutex, MutexGuard};

use serde::Deserialize;

use crate::Clock;
use crate::http::{HttpRequest, HttpTransport, Method, TransportError};

/// Renew this long before the token really expires.
pub const EXPIRY_SKEW_MS: i64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("not_signed_in")]
    NotSignedIn,
    /// The refresh token (or the request) was refused: the identity cannot be renewed.
    #[error("auth_expired")]
    Expired,
    #[error("rate_limited")]
    RateLimited { retry_after_ms: u64 },
    #[error("auth_server_{status}")]
    Server { status: u16 },
    #[error("auth_decode")]
    Decode,
    #[error("{0}")]
    Network(TransportError),
}

impl AuthError {
    pub fn code(self) -> &'static str {
        match self {
            AuthError::NotSignedIn => "not_signed_in",
            AuthError::Expired => "auth_expired",
            AuthError::RateLimited { .. } => "rate_limited",
            AuthError::Server { .. } => "auth_server",
            AuthError::Decode => "auth_decode",
            AuthError::Network(TransportError::Timeout) => "timeout",
            AuthError::Network(TransportError::Network) => "network",
        }
    }
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

struct Live {
    access: String,
    refresh: String,
    expires_at_ms: i64,
    user_id: String,
}

pub struct PhoneSession {
    endpoint: String,
    anon_key: String,
    transport: Arc<dyn HttpTransport>,
    clock: Arc<dyn Clock>,
    live: Mutex<Option<Live>>,
}

impl PhoneSession {
    pub fn new(endpoint: &str, anon_key: &str, transport: Arc<dyn HttpTransport>, clock: Arc<dyn Clock>) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            anon_key: anon_key.to_string(),
            transport,
            clock,
            live: Mutex::new(None),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn anon_key(&self) -> &str {
        &self.anon_key
    }

    fn slot(&self) -> MutexGuard<'_, Option<Live>> {
        match self.live.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn post(&self, path: &str, body: &[u8]) -> Result<(), AuthError> {
        let request = HttpRequest::new(Method::Post, format!("{}{}", self.endpoint, path)).header("apikey", &self.anon_key).json_body(body.to_vec());
        let response = self.transport.send(request).map_err(AuthError::Network)?;
        match response.status {
            200..=299 => {
                let parsed: TokenResponse = serde_json::from_slice(&response.body).map_err(|_| AuthError::Decode)?;
                *self.slot() = Some(Live {
                    access: parsed.access_token,
                    refresh: parsed.refresh_token,
                    expires_at_ms: self.clock.wall_ms().saturating_add(parsed.expires_in.saturating_mul(1000)),
                    user_id: parsed.user.id,
                });
                Ok(())
            }
            400 | 401 | 403 => Err(AuthError::Expired),
            429 => Err(AuthError::RateLimited { retry_after_ms: response.retry_after_ms().unwrap_or(30_000) }),
            status => Err(AuthError::Server { status }),
        }
    }

    /// First launch: `POST /auth/v1/signup` with an empty object makes a new anonymous user.
    pub fn sign_in(&self) -> Result<String, AuthError> {
        self.post("/auth/v1/signup", b"{}")?;
        self.user_id().ok_or(AuthError::Decode)
    }

    /// Later launches: renews from the stored refresh token.
    pub fn restore(&self, refresh_token: &str) -> Result<String, AuthError> {
        self.renew(refresh_token)?;
        self.user_id().ok_or(AuthError::Decode)
    }

    fn renew(&self, refresh_token: &str) -> Result<(), AuthError> {
        let body = serde_json::to_vec(&serde_json::json!({ "refresh_token": refresh_token })).map_err(|_| AuthError::Decode)?;
        self.post("/auth/v1/token?grant_type=refresh_token", &body)
    }

    /// The token to keep on the device. Every renewal rotates it: store the new one.
    pub fn refresh_token(&self) -> Option<String> {
        self.slot().as_ref().map(|live| live.refresh.clone())
    }

    pub fn user_id(&self) -> Option<String> {
        self.slot().as_ref().map(|live| live.user_id.clone())
    }

    pub fn access_token(&self) -> Result<String, AuthError> {
        let (access, refresh, expires_at) = match self.slot().as_ref() {
            Some(live) => (live.access.clone(), live.refresh.clone(), live.expires_at_ms),
            None => return Err(AuthError::NotSignedIn),
        };
        if self.clock.wall_ms().saturating_add(EXPIRY_SKEW_MS) < expires_at {
            return Ok(access);
        }
        self.renew(&refresh)?;
        self.current_access()
    }

    /// After a 401: renews whatever the expiry time says.
    pub fn force_refresh(&self) -> Result<String, AuthError> {
        let refresh = self.refresh_token().ok_or(AuthError::NotSignedIn)?;
        self.renew(&refresh)?;
        self.current_access()
    }

    fn current_access(&self) -> Result<String, AuthError> {
        self.slot().as_ref().map(|live| live.access.clone()).ok_or(AuthError::NotSignedIn)
    }
}
