//! Sleep/wake handling and the heartbeat worker. One thread owns the timing: it applies
//! power events as they arrive and ticks every 30s to refresh the heartbeat and detect gaps.
//!
//! While the system sleeps the process is suspended, so ticks do not run then. A gap found
//! by a tick is therefore a fallback for when WillSleep/DidWake never reached us.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::clock::now_ms;
use crate::downtime::{self, Interval, HEARTBEAT_INTERVAL_MS};
use crate::error::AppResult;
use crate::platform::PowerEvent;
use crate::store;

/// Event sent to the UI after a sleep interval is recorded. The UI re-checks the plan here
/// (display only in 1-1; querying the plan comes in 1-2).
pub const RESUMED_EVENT: &str = "resumed";

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResumedPayload {
    pub start_at_ms: i64,
    pub end_at_ms: i64,
    pub estimated: bool,
}

/// State of one sleep, from WillSleep until DidWake or a lost-DidWake estimate.
///
/// `open_since_ms` is the real WillSleep time and is never moved by awake-time ticks. Measuring
/// every span from it keeps the recorded sleep anchored to the sleep itself, not to the last tick.
/// `estimated_lost` marks a sleep already closed by a lost-DidWake estimate, so a DidWake that
/// arrives after it is recognized as late rather than as a stray event.
#[derive(Debug, Default)]
pub struct SleepTracker {
    open_since_ms: Option<i64>,
    estimated_lost: bool,
}

/// Applies one power event. Returns the measured interval when a sleep was closed.
pub fn on_power_event(
    conn: &mut Connection,
    tracker: &mut SleepTracker,
    session_id: i64,
    event: PowerEvent,
    at_ms: i64,
) -> AppResult<Option<Interval>> {
    match event {
        PowerEvent::WillSleep => {
            tracing::info!(kind = "will_sleep", at_ms, "power event");
            tracker.open_since_ms = Some(at_ms);
            tracker.estimated_lost = false;
            // Stamp the heartbeat now so the awake time before sleep is not counted as gap.
            store::touch_heartbeat(conn, session_id, at_ms)?;
            Ok(None)
        }
        PowerEvent::DidWake => {
            tracing::info!(kind = "did_wake", at_ms, "power event");
            let estimated_lost = std::mem::take(&mut tracker.estimated_lost);
            let closed = match tracker.open_since_ms.take() {
                Some(start) if start < at_ms => {
                    let interval = Interval { start, end: at_ms };
                    downtime::record_measured_sleep(conn, interval)?;
                    Some(interval)
                }
                Some(start) => {
                    // Clock went backwards or the events collapsed. downtime_records is append-only,
                    // so writing a non-positive interval could not be corrected later; skip it, loudly.
                    tracing::warn!(kind = "sleep_not_increasing", start, at_ms, "sleep interval skipped");
                    None
                }
                // A DidWake whose sleep was already closed by a lost-DidWake estimate: the estimate
                // covers WillSleep up to the tick, so the late wake adds nothing that is safe to record.
                // Logged, not silent, so a bad estimate can be diagnosed.
                None if estimated_lost => {
                    tracing::warn!(kind = "did_wake_after_estimate", at_ms, "late DidWake; estimate already covers the sleep");
                    None
                }
                // DidWake with no preceding WillSleep: nothing was measured, nothing to record.
                None => None,
            };
            store::touch_heartbeat(conn, session_id, at_ms)?;
            Ok(closed)
        }
    }
}

/// Heartbeat tick.
///
/// A tick only runs while the machine is awake, so a tick that finds an open sleep proves the sleep
/// ended. If DidWake has not arrived within the gap threshold of WillSleep, its DidWake is lost:
/// the sleep is estimated from WillSleep to this tick and the open sleep is closed.
///
/// Error bound: the estimate can overstate a sleep by up to `GAP_THRESHOLD_MS + HEARTBEAT_INTERVAL_MS`
/// (about 120 s) when the machine wakes within the first threshold window, because the first tick
/// that can detect the loss is only after the threshold. It is exact to within one heartbeat interval
/// for sleeps longer than the threshold. The loss is recorded as `estimated=1`, never as measured.
/// Within the threshold the tick writes nothing and leaves the sleep anchored at WillSleep.
pub fn on_tick(
    conn: &mut Connection,
    tracker: &mut SleepTracker,
    session_id: i64,
    at_ms: i64,
) -> AppResult<Vec<Interval>> {
    if let Some(start) = tracker.open_since_ms {
        if at_ms - start <= downtime::GAP_THRESHOLD_MS {
            return Ok(Vec::new());
        }
        // The first tick past the threshold proves the machine is awake, so the sleep ends here.
        // The estimate runs from WillSleep to this tick and the sleep is then closed. Awake ticks
        // after this point are ordinary heartbeat gaps, not sleep.
        tracing::warn!(kind = "did_wake_lost", start, at_ms, "open sleep has no DidWake; estimating");
        tracker.open_since_ms = None;
        tracker.estimated_lost = true;
        let written = downtime::record_estimated_interval(conn, Interval { start, end: at_ms })?;
        store::touch_heartbeat(conn, session_id, at_ms)?;
        return Ok(written);
    }
    let last = store::session_last_heartbeat(conn, session_id)?;
    let written = downtime::record_estimated_gap(conn, last, at_ms)?;
    if !written.is_empty() {
        tracing::info!(kind = "heartbeat_gap", pieces = written.len(), at_ms, "estimated sleep recorded");
    }
    store::touch_heartbeat(conn, session_id, at_ms)?;
    Ok(written)
}

/// Starts the worker. `db` and `session_id` are shared with the command handlers.
pub fn spawn(app: AppHandle, db: Arc<Mutex<Connection>>, session_id: i64, events: Receiver<PowerEvent>) {
    thread::spawn(move || {
        let mut tracker = SleepTracker::default();
        let interval = Duration::from_millis(HEARTBEAT_INTERVAL_MS.unsigned_abs());
        let mut next_tick = Instant::now() + interval;
        let mut events_open = true;

        loop {
            let wait = next_tick.saturating_duration_since(Instant::now());
            let received = if events_open {
                events.recv_timeout(wait)
            } else {
                // Power source unavailable: keep heartbeating on the timer alone.
                thread::sleep(wait);
                Err(RecvTimeoutError::Timeout)
            };

            match received {
                Ok(event) => {
                    let result = with_db(&db, |conn| {
                        on_power_event(conn, &mut tracker, session_id, event, now_ms()?)
                    });
                    match result {
                        Ok(Some(measured)) => emit_resumed(&app, measured, false),
                        Ok(None) => {}
                        Err(err) => tracing::error!(error = %err, "power event handling failed"),
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    let result = with_db(&db, |conn| on_tick(conn, &mut tracker, session_id, now_ms()?));
                    match result {
                        Ok(written) => {
                            for piece in written {
                                emit_resumed(&app, piece, true);
                            }
                        }
                        Err(err) => tracing::error!(error = %err, "heartbeat tick failed"),
                    }
                    next_tick = Instant::now() + interval;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    tracing::warn!("power event source disconnected; heartbeat-only mode");
                    events_open = false;
                }
            }
        }
    });
}

fn with_db<T>(
    db: &Mutex<Connection>,
    f: impl FnOnce(&mut Connection) -> AppResult<T>,
) -> AppResult<T> {
    let mut guard = db
        .lock()
        .map_err(|_| crate::error::AppError::Internal("database lock poisoned".into()))?;
    f(&mut guard)
}

fn emit_resumed(app: &AppHandle, interval: Interval, estimated: bool) {
    let payload = ResumedPayload {
        start_at_ms: interval.start,
        end_at_ms: interval.end,
        estimated,
    };
    if let Err(err) = app.emit(RESUMED_EVENT, payload) {
        tracing::error!(error = %err, "failed to emit resumed event");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn setup() -> (Connection, i64) {
        let conn = open_in_memory().expect("db");
        let session = store::insert_session(&conn, 0).expect("session");
        (conn, session)
    }

    /// Order A: WillSleep, an in-window tick before DidWake, then DidWake. The tick must not
    /// duplicate the measured sleep. (A tick past the threshold is a lost DidWake; see
    /// `lost_did_wake_is_estimated_after_threshold`.)
    #[test]
    fn tick_before_did_wake_does_not_duplicate_measured_sleep() {
        let (mut conn, session) = setup();
        let mut tracker = SleepTracker::default();
        on_power_event(&mut conn, &mut tracker, session, PowerEvent::WillSleep, 100_000).expect("will");
        let ticked = on_tick(&mut conn, &mut tracker, session, 160_000).expect("tick");
        assert!(ticked.is_empty(), "an in-window tick must not write an estimate");
        let closed = on_power_event(&mut conn, &mut tracker, session, PowerEvent::DidWake, 220_000).expect("wake");
        assert_eq!(closed, Some(Interval { start: 100_000, end: 220_000 }));
        let rows = store::list_downtime(&conn, 10).expect("rows");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].estimated);
    }

    /// Order B: WillSleep, DidWake, then a tick. Only time not covered by measured sleep is estimated.
    #[test]
    fn did_wake_before_tick_keeps_measured_priority() {
        let (mut conn, session) = setup();
        let mut tracker = SleepTracker::default();
        on_power_event(&mut conn, &mut tracker, session, PowerEvent::WillSleep, 100_000).expect("will");
        on_power_event(&mut conn, &mut tracker, session, PowerEvent::DidWake, 220_000).expect("wake");
        // The heartbeat was stamped at DidWake, so a tick right after finds no gap.
        assert!(on_tick(&mut conn, &mut tracker, session, 250_000).expect("tick").is_empty());
        let rows = store::list_downtime(&conn, 10).expect("rows");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].estimated);
    }

    /// Fallback when events are missing: a stale heartbeat becomes an estimated sleep.
    #[test]
    fn missing_events_fall_back_to_estimated_gap() {
        let (mut conn, session) = setup();
        let mut tracker = SleepTracker::default();
        store::touch_heartbeat(&conn, session, 10_000).expect("hb");
        let written = on_tick(&mut conn, &mut tracker, session, 10_000 + 200_000).expect("tick");
        assert_eq!(written, vec![Interval { start: 10_000, end: 210_000 }]);
        let rows = store::list_downtime(&conn, 10).expect("rows");
        assert!(rows[0].estimated);
    }

    /// A tick within the threshold writes nothing and leaves the sleep open and anchored at
    /// WillSleep. It must not move the anchor: moving it would record awake time as sleep.
    #[test]
    fn tick_within_threshold_writes_nothing_and_keeps_anchor() {
        let (mut conn, session) = setup();
        let mut tracker = SleepTracker::default();
        on_power_event(&mut conn, &mut tracker, session, PowerEvent::WillSleep, 100_000).expect("will");
        assert!(on_tick(&mut conn, &mut tracker, session, 160_000).expect("tick").is_empty());
        assert!(on_tick(&mut conn, &mut tracker, session, 190_000).expect("tick").is_empty());
        assert_eq!(tracker.open_since_ms, Some(100_000), "the anchor must stay at WillSleep");
        assert!(store::list_downtime(&conn, 10).expect("rows").is_empty());
    }

    /// Lost DidWake (AC5 fallback): the first tick past the threshold proves the machine is awake,
    /// so it estimates the whole sleep from WillSleep and closes the sleep. Awake ticks afterward
    /// write no sleep rows.
    #[test]
    fn lost_did_wake_estimates_from_will_sleep_then_closes() {
        let (mut conn, session) = setup();
        let mut tracker = SleepTracker::default();
        on_power_event(&mut conn, &mut tracker, session, PowerEvent::WillSleep, 100_000).expect("will");
        assert!(on_tick(&mut conn, &mut tracker, session, 160_000).expect("tick").is_empty());
        assert!(on_tick(&mut conn, &mut tracker, session, 190_000).expect("tick").is_empty());
        let at = 100_000 + downtime::GAP_THRESHOLD_MS + 30_000;
        let written = on_tick(&mut conn, &mut tracker, session, at).expect("tick");
        assert_eq!(written, vec![Interval { start: 100_000, end: at }], "estimate must start at WillSleep");
        assert!(tracker.open_since_ms.is_none(), "the estimate ends the sleep");
        let rows = store::list_downtime(&conn, 10).expect("rows");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].estimated);
    }

    /// Pins the documented error bound. A 5 s sleep whose DidWake is lost is estimated up to the
    /// first past-threshold tick, so it overstates by at most threshold + one heartbeat interval.
    #[test]
    fn short_lost_sleep_overstates_within_documented_bound() {
        let (mut conn, session) = setup();
        let mut tracker = SleepTracker::default();
        on_power_event(&mut conn, &mut tracker, session, PowerEvent::WillSleep, 100_000).expect("will");
        // Real sleep was 5 s; the machine woke at 105_000 but the DidWake never arrived. The next
        // ticks come every 30 s, and the first one past the threshold is at 220_000.
        let at = 100_000 + downtime::GAP_THRESHOLD_MS + 30_000;
        let written = on_tick(&mut conn, &mut tracker, session, at).expect("tick");
        let estimated: i64 = written.iter().map(|i| i.end - i.start).sum();
        let real = 5_000;
        let overstate = estimated - real;
        assert!(overstate > 0, "a short lost sleep is overstated by design");
        assert!(
            overstate <= downtime::GAP_THRESHOLD_MS + HEARTBEAT_INTERVAL_MS,
            "overstatement {overstate} exceeds the documented bound",
        );
    }

    /// The reviewer's failure: after a lost-DidWake estimate, awake ticks must not keep writing
    /// sleep rows. Exactly one estimated row exists however many awake ticks follow.
    #[test]
    fn awake_ticks_after_estimate_write_no_further_sleep() {
        let (mut conn, session) = setup();
        let mut tracker = SleepTracker::default();
        on_power_event(&mut conn, &mut tracker, session, PowerEvent::WillSleep, 100_000).expect("will");
        let at = 100_000 + downtime::GAP_THRESHOLD_MS + 30_000;
        on_tick(&mut conn, &mut tracker, session, at).expect("tick");
        for step in 1..=5 {
            let awake = on_tick(&mut conn, &mut tracker, session, at + step * 30_000).expect("tick");
            assert!(awake.is_empty(), "awake tick {step} must not write sleep");
        }
        let rows = store::list_downtime(&conn, 10).expect("rows");
        assert_eq!(rows.len(), 1, "exactly one estimated sleep row");
    }

    /// A DidWake that arrives after the estimate closed the sleep records nothing new (the estimate
    /// already covers WillSleep up to the tick) and is logged, not silently dropped.
    #[test]
    fn late_did_wake_after_estimate_adds_nothing() {
        let (mut conn, session) = setup();
        let mut tracker = SleepTracker::default();
        on_power_event(&mut conn, &mut tracker, session, PowerEvent::WillSleep, 100_000).expect("will");
        let estimated_at = 100_000 + downtime::GAP_THRESHOLD_MS + 30_000;
        on_tick(&mut conn, &mut tracker, session, estimated_at).expect("tick");
        let closed = on_power_event(&mut conn, &mut tracker, session, PowerEvent::DidWake, estimated_at + 600_000).expect("wake");
        assert!(closed.is_none());
        let total: i64 = store::list_downtime(&conn, 10)
            .expect("rows")
            .iter()
            .map(|r| r.end_at_ms - r.start_at_ms)
            .sum();
        assert_eq!(total, estimated_at - 100_000, "only the estimated span is recorded");
    }

    /// A DidWake that does not come after its WillSleep must write nothing to the append-only
    /// table and must clear the open sleep, so the next real sleep starts clean.
    #[test]
    fn non_increasing_sleep_is_skipped_and_clears_the_open_sleep() {
        let (mut conn, session) = setup();
        let mut tracker = SleepTracker::default();
        on_power_event(&mut conn, &mut tracker, session, PowerEvent::WillSleep, 9_000).expect("will");
        let closed = on_power_event(&mut conn, &mut tracker, session, PowerEvent::DidWake, 9_000).expect("wake");
        assert!(closed.is_none(), "equal start and end is not a sleep");
        assert!(tracker.open_since_ms.is_none(), "the open sleep must be cleared");
        assert!(store::list_downtime(&conn, 10).expect("rows").is_empty());
    }

    #[test]
    fn did_wake_without_will_sleep_records_nothing_measured() {
        let (mut conn, session) = setup();
        let mut tracker = SleepTracker::default();
        let closed = on_power_event(&mut conn, &mut tracker, session, PowerEvent::DidWake, 5_000).expect("wake");
        assert_eq!(closed, None);
        assert!(store::list_downtime(&conn, 10).expect("rows").is_empty());
    }
}
