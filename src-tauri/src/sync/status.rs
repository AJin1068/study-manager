//! What the settings screen shows about the mirror and the signal: last success times and the
//! last failure code of each stream. Codes only, never a message from the network stack.

use std::sync::{Mutex, MutexGuard};

use serde::Serialize;

#[derive(Default)]
struct Inner {
    active: bool,
    last_beat_ok_ms: Option<i64>,
    last_sync_ok_ms: Option<i64>,
    beat_error: Option<&'static str>,
    sync_error: Option<&'static str>,
    presence_degraded: bool,
}

#[derive(Default)]
pub struct SyncStatus(Mutex<Inner>);

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusView {
    /// Signed in and configured: the workers are sending. False means nothing leaves the device.
    pub active: bool,
    pub last_beat_ok_ms: Option<i64>,
    pub last_sync_ok_ms: Option<i64>,
    pub beat_error: Option<&'static str>,
    pub sync_error: Option<&'static str>,
    /// `presence_unreachable` after repeated failed beats.
    pub degraded_code: Option<&'static str>,
}

pub const PRESENCE_UNREACHABLE: &str = "presence_unreachable";

impl SyncStatus {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        match self.0.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn set_active(&self, active: bool) {
        let mut inner = self.lock();
        inner.active = active;
        if !active {
            inner.beat_error = None;
            inner.sync_error = None;
            inner.presence_degraded = false;
        }
    }

    pub fn beat(&self, at_ms: i64, result: Result<(), &'static str>) {
        let mut inner = self.lock();
        match result {
            Ok(()) => {
                inner.last_beat_ok_ms = Some(at_ms);
                inner.beat_error = None;
            }
            Err(code) => inner.beat_error = Some(code),
        }
    }

    pub fn set_presence_degraded(&self, degraded: bool) {
        self.lock().presence_degraded = degraded;
    }

    pub fn sync(&self, at_ms: i64, result: Result<(), &'static str>) {
        let mut inner = self.lock();
        match result {
            Ok(()) => {
                inner.last_sync_ok_ms = Some(at_ms);
                inner.sync_error = None;
            }
            Err(code) => inner.sync_error = Some(code),
        }
    }

    pub fn view(&self) -> SyncStatusView {
        let inner = self.lock();
        SyncStatusView {
            active: inner.active,
            last_beat_ok_ms: inner.last_beat_ok_ms,
            last_sync_ok_ms: inner.last_sync_ok_ms,
            beat_error: inner.beat_error,
            sync_error: inner.sync_error,
            degraded_code: inner.presence_degraded.then_some(PRESENCE_UNREACHABLE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_clears_the_error_and_going_inactive_clears_the_failure_state() {
        let status = SyncStatus::default();
        status.set_active(true);
        status.beat(10, Err("connect_failed"));
        status.set_presence_degraded(true);
        let view = status.view();
        assert_eq!((view.beat_error, view.degraded_code), (Some("connect_failed"), Some(PRESENCE_UNREACHABLE)));
        status.beat(20, Ok(()));
        status.sync(25, Ok(()));
        assert_eq!(status.view().beat_error, None);
        assert_eq!((status.view().last_beat_ok_ms, status.view().last_sync_ok_ms), (Some(20), Some(25)));
        status.sync(30, Err("http_error"));
        status.set_active(false);
        let view = status.view();
        assert!(!view.active);
        assert_eq!((view.sync_error, view.degraded_code), (None, None));
        assert_eq!(view.last_beat_ok_ms, Some(20), "the last success time is kept for display");
    }
}
