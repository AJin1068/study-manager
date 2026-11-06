//! Shared test helpers.

use std::io;
use std::sync::{Arc, Mutex};

/// Sets the lock row directly. Writes no history, unlike the removed manual toggle; production
/// code changes the lock only through `timer`.
pub fn set_lock_for_test(conn: &rusqlite::Connection, held: bool) {
    crate::store::put_study_state(conn, held, 0).expect("lock row must exist in tests");
}

/// Inserts a plan, one run of it, and one study segment of that run; returns `(run_id, segment_id)`.
/// The rows are the smallest valid ones: tests that need real timing build their own.
pub fn insert_run_for_test(conn: &rusqlite::Connection, subject: &str) -> (i64, i64) {
    conn.execute(
        "INSERT INTO plans (series_id, plan_name, subject, start_date, start_time, sets, effective_from_date, created_at_ms)
         VALUES (1, 'p', ?1, '2026-01-01', '09:00', 1, '2026-01-01', 0)",
        [subject],
    )
    .expect("plan");
    let plan_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO plan_runs (plan_id, date, planned_start_ms, planned_end_ms, mode_level, study_min, break_min, sets_total, status)
         VALUES (?1, '2026-01-01', 0, 10, 'easy', 25, 5, 1, 'completed')",
        [plan_id],
    )
    .expect("run");
    let run_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO timer_segments (run_id, seq, kind, planned_start_ms, planned_end_ms, started_at_ms) VALUES (?1, 1, 'study', 0, 10, 0)",
        [run_id],
    )
    .expect("segment");
    (run_id, conn.last_insert_rowid())
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

/// A scratch directory removed on drop.
pub struct TempDir(std::path::PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("study-manager-test-{tag}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
