//! The desktop side of pairing: make a one-time code, show it as a QR code, follow the pairing
//! state and end it. Network calls always run with the database lock released; an unreachable
//! server changes nothing locally except that the status says it is offline.
//!
//! Ending a pairing is a weakening of the lock (the phone stops being locked), so it is refused
//! while a study run holds the lock, like every other setting that lowers the guard. Starting one
//! is allowed at any time.

use std::sync::{Arc, Mutex};

use link_protocol::PairingOffer;
use rusqlite::Connection;

use super::model::{PairStart, PairStatusView, PhoneAck, PhoneCode, PhoneError};
use super::port::PairingApi;
use super::rest::RestPairing;
use crate::commands::lock_db;
use crate::error::{AppError, AppResult};
use crate::remote::config::RemoteConfig;
use crate::remote::fake::{FAKE_ANON_KEY, FAKE_ENDPOINT};
use crate::remote::runtime::RemoteRuntime;
use crate::remote::supabase::RemoteError;
use crate::remote::{Clock, IntegrationMode};
use crate::store::phone as repo;

/// Where the server port and the public connection details come from: `None` when signed out.
pub trait PairingBackend: Send + Sync {
    fn api(&self) -> Result<Arc<dyn PairingApi>, PhoneError>;
    /// The server address and PUBLIC anon key that go into the QR code.
    fn server(&self) -> Option<(String, String)>;
}

struct AppBackend {
    db: Arc<Mutex<Connection>>,
    runtime: Arc<RemoteRuntime>,
}

impl PairingBackend for AppBackend {
    fn api(&self) -> Result<Arc<dyn PairingApi>, PhoneError> {
        let handle = {
            let conn = lock_db(&self.db).map_err(|_| PhoneError::Remote(RemoteError::NotSignedIn))?;
            self.runtime.supabase(&conn).map_err(|_| PhoneError::Remote(RemoteError::NotSignedIn))?
        };
        if !handle.auth.status().signed_in {
            return Err(PhoneError::Remote(RemoteError::NotSignedIn));
        }
        Ok(Arc::new(RestPairing::new(handle.rest)))
    }

    fn server(&self) -> Option<(String, String)> {
        let conn = lock_db(&self.db).ok()?;
        let handle = self.runtime.supabase(&conn).ok()?;
        match handle.mode {
            IntegrationMode::Mock => Some((FAKE_ENDPOINT.to_string(), FAKE_ANON_KEY.to_string())),
            IntegrationMode::Live => {
                let config = RemoteConfig::load(&conn).ok()?;
                Some((config.supabase_endpoint, config.supabase_anon_key))
            }
            IntegrationMode::NotConfigured => None,
        }
    }
}

/// The app's backend: the Supabase session of the signed-in user (the fake server in mock mode).
pub fn app_backend(db: Arc<Mutex<Connection>>, runtime: Arc<RemoteRuntime>) -> Arc<dyn PairingBackend> {
    Arc::new(AppBackend { db, runtime })
}

pub struct PairingService {
    db: Arc<Mutex<Connection>>,
    clock: Arc<dyn Clock>,
    backend: Arc<dyn PairingBackend>,
}

fn rejected_or_err<T>(result: Result<T, PhoneError>) -> AppResult<Result<T, PhoneCode>> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(PhoneError::Rejected(code)) => Ok(Err(code)),
        Err(other) => Err(other.into()),
    }
}

fn display_code(code: &str) -> String {
    if code.len() == 10 && code.is_ascii() {
        format!("{}-{}", &code[..5], &code[5..])
    } else {
        code.to_string()
    }
}

impl PairingService {
    pub fn new(db: Arc<Mutex<Connection>>, runtime: Arc<RemoteRuntime>, clock: Arc<dyn Clock>) -> Self {
        let backend = Arc::new(AppBackend { db: Arc::clone(&db), runtime });
        Self::with_parts(db, clock, backend)
    }

    pub(crate) fn with_parts(db: Arc<Mutex<Connection>>, clock: Arc<dyn Clock>, backend: Arc<dyn PairingBackend>) -> Self {
        Self { db, clock, backend }
    }

    /// Makes a new one-time code and the QR code that carries it with the server address and the
    /// public anon key. The code is shown, never logged.
    pub fn start(&self) -> AppResult<PairStart> {
        let none = |code| PairStart { code, display_code: None, expires_at_ms: None, qr_svg: None };
        let api = self.backend.api().map_err(AppError::from)?;
        let (endpoint, anon_key) = self.backend.server().ok_or(AppError::NotConfigured { port: "supabase" })?;
        let made = match rejected_or_err(api.create_pair_code())? {
            Ok(made) => made,
            Err(code) => return Ok(none(code)),
        };
        let offer = PairingOffer::new(&made.pair_code, &endpoint, &anon_key).and_then(|offer| offer.to_qr_text());
        let qr_svg = offer.ok().and_then(|text| crate::remote::qr::render_svg(&text));
        if qr_svg.is_none() {
            return Ok(none(PhoneCode::BadOffer));
        }
        Ok(PairStart {
            code: PhoneCode::Ok,
            display_code: Some(display_code(&made.pair_code)),
            expires_at_ms: Some(made.expires_at_ms),
            qr_svg,
        })
    }

    /// Refreshes the pairing state from the server and remembers it; offline, answers with the
    /// last known state.
    pub fn status(&self) -> AppResult<PairStatusView> {
        let known = repo::pair(&*lock_db(&self.db)?)?;
        let offline = PairStatusView { paired: known.paired, paired_at_ms: known.paired_at_ms, last_beat_ms: None, service_enabled: None, online: false };
        let Ok(api) = self.backend.api() else { return Ok(offline) };
        match api.status() {
            Ok(wire) => {
                let now = self.clock.now_ms()?;
                repo::save_pair(&*lock_db(&self.db)?, wire.paired, wire.paired_at_ms, now)?;
                Ok(PairStatusView {
                    paired: wire.paired,
                    paired_at_ms: wire.paired_at_ms,
                    last_beat_ms: wire.last_beat_ms,
                    service_enabled: wire.service_enabled,
                    online: true,
                })
            }
            Err(_) => Ok(offline),
        }
    }

    /// Ends the pairing. Refused while a study run holds the lock.
    pub fn revoke(&self) -> AppResult<PhoneAck> {
        if crate::study::lock_held(&*lock_db(&self.db)?)? {
            return Ok(PhoneAck { code: PhoneCode::RunLocked });
        }
        let api = self.backend.api().map_err(AppError::from)?;
        let code = match rejected_or_err(api.revoke_pair())? {
            Ok(()) => PhoneCode::Ok,
            // Nothing to end: the server already has no pairing, so the local state follows it.
            Err(PhoneCode::NotFound) => PhoneCode::NotFound,
            Err(code) => return Ok(PhoneAck { code }),
        };
        repo::save_pair(&*lock_db(&self.db)?, false, None, self.clock.now_ms()?)?;
        Ok(PhoneAck { code })
    }
}
