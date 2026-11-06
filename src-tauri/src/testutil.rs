//! Shared test helpers.

use std::io;
use std::sync::{Arc, Mutex};

/// Sets the lock row directly. Writes no history, unlike the removed manual toggle; production
/// code changes the lock only through `timer`.
pub fn set_lock_for_test(conn: &rusqlite::Connection, held: bool) {
    crate::store::put_study_state(conn, held, 0).expect("lock row must exist in tests");
}

/// Records every notification instead of showing it.
#[derive(Default)]
pub struct FakeNotifier {
    sent: Mutex<Vec<(String, String)>>,
}

impl FakeNotifier {
    pub fn sent(&self) -> Vec<(String, String)> {
        self.sent.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl crate::platform::Notifier for FakeNotifier {
    fn notify(&self, title: &str, body: &str) -> Result<(), crate::platform::PlatformError> {
        self.sent
            .lock()
            .map_err(|_| crate::platform::PlatformError::Notification("fake notifier poisoned".into()))?
            .push((title.to_owned(), body.to_owned()));
        Ok(())
    }
}

#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl io::Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("log buffer poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Runs `f` with a thread-local tracing subscriber and returns everything it logged, so tests
/// can assert that logs carry no window titles, URLs, or typed text.
pub fn capture_logs(f: impl FnOnce()) -> String {
    let buf = SharedBuf::default();
    let writer = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = buf.0.lock().map(|b| b.clone()).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_logs_returns_emitted_events() {
        let out = capture_logs(|| tracing::info!(kind = "probe", "hello"));
        assert!(out.contains("probe"), "captured: {out}");
    }
}
