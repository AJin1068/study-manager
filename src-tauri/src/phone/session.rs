//! The phone's anonymous Supabase session. The phone has no email: it signs in anonymously once,
//! keeps its refresh token on the device, and renews the access token from it. A rejected renewal
//! never creates a NEW anonymous identity by itself, because a new identity would silently lose
//! the pairing; the caller decides (the UI asks the user to pair again).

use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::remote::Clock;
use crate::remote::auth::{AccessTokenSource, AuthError, AuthTokens, EXPIRY_SKEW_MS};
use crate::remote::http::{HttpRequest, HttpTransport, Method, RequestTag};
use crate::remote::payload::{Body, NoArgs, RefreshRequest};

/// Anonymous sign-in and renewal.
pub trait AnonymousAuth: Send + Sync {
    fn sign_in_anonymously(&self) -> Result<AuthTokens, AuthError>;
    fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, AuthError>;
}

/// GoTrue over HTTP: `POST /auth/v1/signup` with an empty body signs a new anonymous user in.
pub struct SupabaseAnonymousAuth {
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

impl SupabaseAnonymousAuth {
    pub fn new(endpoint: &str, anon_key: &str, transport: Arc<dyn HttpTransport>) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            anon_key: anon_key.to_string(),
            transport,
        }
    }

    fn post(&self, kind: &'static str, path: &str, body: Body) -> Result<AuthTokens, AuthError> {
        let tag = RequestTag {
            port: "supabase",
            kind,
        };
        let request = HttpRequest::new(tag, Method::Post, format!("{}{}", self.endpoint, path))
            .header("apikey", &self.anon_key)
            .with_body(body);
        let response = self.transport.send(request).map_err(AuthError::Network)?;
        match response.status {
            200..=299 => {
                let parsed: TokenResponse =
                    serde_json::from_slice(&response.body).map_err(|_| AuthError::Decode)?;
                Ok(AuthTokens {
                    access_token: parsed.access_token,
                    refresh_token: parsed.refresh_token,
                    expires_in_sec: parsed.expires_in,
                    user_id: parsed.user.id,
                })
            }
            400 | 401 | 403 => Err(AuthError::Expired),
            429 => Err(AuthError::RateLimited {
                retry_after_ms: response.retry_after_ms().unwrap_or(30_000),
            }),
            status => Err(AuthError::Server { status }),
        }
    }
}

impl AnonymousAuth for SupabaseAnonymousAuth {
    fn sign_in_anonymously(&self) -> Result<AuthTokens, AuthError> {
        let body = Body::from_payload(&NoArgs {}).map_err(|_| AuthError::Decode)?;
        self.post("auth_anonymous", "/auth/v1/signup", body)
    }

    fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, AuthError> {
        let body = Body::from_payload(&RefreshRequest {
            refresh_token: refresh_token.to_string(),
        })
        .map_err(|_| AuthError::Decode)?;
        self.post(
            "auth_refresh",
            "/auth/v1/token?grant_type=refresh_token",
            body,
        )
    }
}

struct Live {
    access: String,
    refresh: String,
    expires_at_ms: i64,
    user_id: String,
}

/// The phone's `AccessTokenSource`. Persisting the refresh token between launches is the phone
/// app's job (`refresh_token` / `restore`).
pub struct PhoneSession {
    auth: Arc<dyn AnonymousAuth>,
    clock: Arc<dyn Clock>,
    live: Mutex<Option<Live>>,
}

impl PhoneSession {
    pub fn new(auth: Arc<dyn AnonymousAuth>, clock: Arc<dyn Clock>) -> Self {
        Self {
            auth,
            clock,
            live: Mutex::new(None),
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<Live>> {
        match self.live.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn store(&self, tokens: AuthTokens) -> Result<String, AuthError> {
        let now = self.clock.now_ms().map_err(|_| AuthError::Decode)?;
        let access = tokens.access_token.clone();
        *self.slot() = Some(Live {
            access: tokens.access_token,
            refresh: tokens.refresh_token,
            expires_at_ms: now + tokens.expires_in_sec * 1000,
            user_id: tokens.user_id,
        });
        Ok(access)
    }

    /// First launch: a new anonymous identity. Returns its user id.
    pub fn sign_in(&self) -> Result<String, AuthError> {
        let tokens = self.auth.sign_in_anonymously()?;
        let user_id = tokens.user_id.clone();
        self.store(tokens)?;
        Ok(user_id)
    }

    /// Later launches: renews from the stored refresh token.
    pub fn restore(&self, refresh_token: &str) -> Result<String, AuthError> {
        let tokens = self.auth.refresh(refresh_token)?;
        let user_id = tokens.user_id.clone();
        self.store(tokens)?;
        Ok(user_id)
    }

    /// The token to keep on the device. It is rotated by every renewal: store the new one.
    pub fn refresh_token(&self) -> Option<String> {
        self.slot().as_ref().map(|live| live.refresh.clone())
    }

    pub fn user_id(&self) -> Option<String> {
        self.slot().as_ref().map(|live| live.user_id.clone())
    }
}

impl AccessTokenSource for PhoneSession {
    fn access_token(&self) -> Result<String, AuthError> {
        let now = self.clock.now_ms().map_err(|_| AuthError::Decode)?;
        let (access, refresh, expires_at) = match self.slot().as_ref() {
            Some(live) => (
                live.access.clone(),
                live.refresh.clone(),
                live.expires_at_ms,
            ),
            None => return Err(AuthError::NotSignedIn),
        };
        if now + EXPIRY_SKEW_MS < expires_at {
            return Ok(access);
        }
        self.store(self.auth.refresh(&refresh)?)
    }

    fn force_refresh(&self) -> Result<String, AuthError> {
        let refresh = self
            .slot()
            .as_ref()
            .map(|live| live.refresh.clone())
            .ok_or(AuthError::NotSignedIn)?;
        self.store(self.auth.refresh(&refresh)?)
    }
}
