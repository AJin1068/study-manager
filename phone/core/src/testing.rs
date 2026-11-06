//! Host-test doubles: a scripted HTTP transport, and a clock the test moves by hand.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

use crate::Clock;
use crate::http::{HttpRequest, HttpResponse, HttpTransport, TransportError};

#[derive(Default)]
pub struct ScriptedTransport {
    replies: Mutex<VecDeque<Result<HttpResponse, TransportError>>>,
    seen: Mutex<Vec<HttpRequest>>,
}

impl ScriptedTransport {
    pub fn new(replies: Vec<Result<HttpResponse, TransportError>>) -> Self {
        Self { replies: Mutex::new(replies.into()), seen: Mutex::new(Vec::new()) }
    }

    pub fn seen(&self) -> Vec<HttpRequest> {
        self.seen.lock().expect("seen").clone()
    }
}

impl HttpTransport for ScriptedTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.seen.lock().expect("seen").push(request);
        self.replies.lock().expect("replies").pop_front().unwrap_or(Err(TransportError::Network))
    }
}

pub fn json(status: u16, body: &str) -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse { status, headers: Vec::new(), body: body.as_bytes().to_vec() })
}

/// The wall clock and the monotonic clock move independently, so a test can shift one and show
/// the other did not matter.
pub struct TestClock {
    mono: AtomicI64,
    wall: AtomicI64,
}

impl TestClock {
    pub fn new(mono: i64, wall: i64) -> Self {
        Self { mono: AtomicI64::new(mono), wall: AtomicI64::new(wall) }
    }

    pub fn advance_mono(&self, ms: i64) {
        self.mono.fetch_add(ms, Ordering::SeqCst);
    }

    pub fn shift_wall(&self, ms: i64) {
        self.wall.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn mono_ms(&self) -> i64 {
        self.mono.load(Ordering::SeqCst)
    }

    fn wall_ms(&self) -> i64 {
        self.wall.load(Ordering::SeqCst)
    }
}
