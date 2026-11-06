//! Test scaffolding: several signed-in people on one in-memory server, every request body they
//! send recorded so tests can search what left the PC.

use std::sync::{Arc, Mutex};

use super::rest::RestRooms;
use crate::remote::auth::AuthService;
use crate::remote::fake::{FakeBackend, FAKE_ANON_KEY, FAKE_ENDPOINT};
use crate::remote::http::{HttpRequest, HttpResponse, HttpTransport, TransportError};
use crate::remote::secrets::MemorySecretStore;
use crate::remote::supabase::SupabaseRest;
use crate::remote::ManualClock;

pub const T0: i64 = 1_793_975_400_000; // 2026-11-06T14:30:00Z

/// Wraps the fake server and keeps `(rpc or table name, body text)` of every request.
pub struct Recorder {
    inner: Arc<FakeBackend>,
    sent: Mutex<Vec<(String, String)>>,
}

impl Recorder {
    pub fn bodies(&self) -> Vec<(String, String)> {
        self.sent.lock().map(|s| s.clone()).unwrap_or_default()
    }

    #[allow(dead_code)]
    pub fn clear(&self) {
        if let Ok(mut sent) = self.sent.lock() {
            sent.clear();
        }
    }

    /// Bodies of calls to one RPC.
    pub fn bodies_of(&self, rpc: &str) -> Vec<String> {
        self.bodies().into_iter().filter(|(name, _)| name == rpc).map(|(_, body)| body).collect()
    }
}

impl HttpTransport for Recorder {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let name = request.url.rsplit('/').next().unwrap_or_default().to_string();
        let body = request.body.as_ref().map(|b| String::from_utf8_lossy(b.as_bytes()).into_owned()).unwrap_or_default();
        if let Ok(mut sent) = self.sent.lock() {
            sent.push((name, body));
        }
        self.inner.send(request)
    }
}

pub struct World {
    pub clock: Arc<ManualClock>,
    pub backend: Arc<FakeBackend>,
    pub recorder: Arc<Recorder>,
}

pub struct Person {
    pub rooms: Arc<RestRooms>,
    pub rest: Arc<SupabaseRest>,
    pub user_id: String,
}

impl World {
    pub fn new() -> Self {
        let clock = Arc::new(ManualClock::new(T0));
        let backend = Arc::new(FakeBackend::new(clock.clone()));
        let recorder = Arc::new(Recorder { inner: backend.clone(), sent: Mutex::new(Vec::new()) });
        Self { clock, backend, recorder }
    }

    /// Signs a new person in (each has their own session and secret store).
    pub fn person(&self, email: &str) -> Person {
        let auth = Arc::new(AuthService::new(
            self.backend.auth(),
            Arc::new(MemorySecretStore::in_memory()),
            self.backend.clone(),
            self.clock.clone(),
        ));
        auth.request_otp(email).expect("otp");
        let user_id = auth.verify_otp(email, "000000").expect("verify");
        let rest = Arc::new(SupabaseRest::new(FAKE_ENDPOINT, FAKE_ANON_KEY, self.recorder.clone(), auth));
        Person { rooms: Arc::new(RestRooms::new(Arc::clone(&rest))), rest, user_id }
    }
}
