//! One push of local state to the mirror. Each step reads under the database lock, releases it,
//! then talks to the network; the position is saved under the lock afterwards. The lock is never
//! held across a request.

use std::sync::Mutex;

use chrono::TimeZone;
use rusqlite::Connection;

use super::ports::{PlanMirror, SyncError};
use super::snapshot::{build_snapshot, SkipMark, Snapshot, HORIZON_DAYS};
use crate::commands::lock_db;
use crate::error::{AppError, AppResult};
use crate::plan::{self, ModeLookup};
use crate::store;
use crate::store::remote::{self as cursor, HISTORY_CURSOR};

/// History rows per request.
pub const HISTORY_BATCH: i64 = 500;
/// Requests per cycle: a very long backlog is worked off over several cycles instead of
/// occupying the worker.
const MAX_BATCHES_PER_CYCLE: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PushError {
    #[error("local_{0}")]
    Local(&'static str),
    #[error("{0}")]
    Remote(SyncError),
}

impl PushError {
    pub fn code(self) -> &'static str {
        match self {
            PushError::Local(code) => code,
            PushError::Remote(err) => err.code(),
        }
    }

    pub fn is_device_missing(self) -> bool {
        matches!(self, PushError::Remote(err) if err.is_device_missing())
    }
}

impl From<SyncError> for PushError {
    fn from(err: SyncError) -> Self {
        PushError::Remote(err)
    }
}

impl From<AppError> for PushError {
    fn from(err: AppError) -> Self {
        tracing::error!(event = "sync_local_error", error = %err);
        PushError::Local("database")
    }
}

/// Everything `build_snapshot` needs, read in one short lock.
struct Inputs {
    plans: Vec<plan::Plan>,
    exceptions: Vec<plan::PlanException>,
    modes: ModeLookup,
    skips: Vec<SkipMark>,
}

fn load_inputs(conn: &Connection, from_date: &str) -> AppResult<Inputs> {
    Ok(Inputs {
        plans: store::list_live_plans(conn)?,
        exceptions: store::list_plan_exceptions(conn)?,
        modes: ModeLookup::load(conn)?,
        skips: store::remote::list_skip_passes(conn, from_date)?
            .into_iter()
            .map(|(date, at_ms)| SkipMark { date, at_ms })
            .collect(),
    })
}

/// The snapshot for `now_ms`. The lock is held for the reads only.
pub fn snapshot_now<Tz: TimeZone>(db: &Mutex<Connection>, tz: &Tz, now_ms: i64) -> Result<Snapshot, PushError> {
    let today = tz
        .timestamp_millis_opt(now_ms)
        .single()
        .map(|dt| dt.date_naive())
        .ok_or(PushError::Local("clock"))?;
    // One day of margin matches the snapshot's own lookback.
    let from_date = (today - chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
    let inputs = {
        let conn = lock_db(db)?;
        load_inputs(&conn, &from_date)?
    };
    Ok(build_snapshot(tz, &inputs.plans, &inputs.exceptions, &inputs.modes, &inputs.skips, now_ms, HORIZON_DAYS)?)
}

/// Sends history rows after the saved position, in batches, saving the position after each
/// accepted batch. Returns how many rows were sent. A failure leaves the position at the last
/// accepted batch, so the next cycle continues from there.
pub fn push_history(db: &Mutex<Connection>, mirror: &dyn PlanMirror, device_id: &str, now_ms: i64) -> Result<u64, PushError> {
    let mut sent = 0u64;
    for _ in 0..MAX_BATCHES_PER_CYCLE {
        let entries = {
            let conn = lock_db(db)?;
            let position = cursor::get_cursor(&conn, HISTORY_CURSOR)?;
            cursor::list_plan_history_after(&conn, position, HISTORY_BATCH)?
        };
        let Some(last) = entries.last().map(|e| e.id) else { break };
        mirror.push_history(device_id, &entries)?;
        {
            let conn = lock_db(db)?;
            cursor::set_cursor(&conn, HISTORY_CURSOR, last, now_ms)?;
        }
        sent += entries.len() as u64;
        if (entries.len() as i64) < HISTORY_BATCH {
            break;
        }
    }
    Ok(sent)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::db::open_in_memory;
    use crate::plan::PlanHistoryEntry;
    use crate::remote::payload::NotifyChannelRow;
    use crate::sync::ports::{DeviceIdentity, SnapshotOutcome};

    /// A server stand-in that only knows history: unique per local id, counts requests, can fail.
    #[derive(Default)]
    pub(crate) struct HistorySink {
        pub seen: Mutex<HashSet<i64>>,
        pub requests: Mutex<Vec<usize>>,
        pub fail_next: Mutex<u32>,
        /// Was the database lock free when a request went out?
        pub db_free_during_request: Mutex<Vec<bool>>,
        pub db: Mutex<Option<Arc<Mutex<Connection>>>>,
    }

    impl PlanMirror for HistorySink {
        fn register_device(&self, _device: &DeviceIdentity) -> Result<(), SyncError> {
            Ok(())
        }
        fn push_snapshot(&self, _device_id: &str, _snapshot: &Snapshot) -> Result<SnapshotOutcome, SyncError> {
            Ok(SnapshotOutcome::Replaced)
        }
        fn push_history(&self, _device_id: &str, rows: &[PlanHistoryEntry]) -> Result<(), SyncError> {
            if let Some(db) = self.db.lock().expect("db").as_ref() {
                self.db_free_during_request.lock().expect("log").push(db.try_lock().is_ok());
            }
            {
                let mut fail = self.fail_next.lock().expect("fail");
                if *fail > 0 {
                    *fail -= 1;
                    return Err(SyncError::Remote(crate::remote::supabase::RemoteError::Http { status: 503 }));
                }
            }
            self.requests.lock().expect("requests").push(rows.len());
            let mut seen = self.seen.lock().expect("seen");
            for row in rows {
                seen.insert(row.id);
            }
            Ok(())
        }
        fn push_notify_channel(&self, _row: &NotifyChannelRow) -> Result<(), SyncError> {
            Ok(())
        }
    }

    pub(crate) fn insert_history_rows(conn: &Connection, count: i64) {
        let tx = conn.unchecked_transaction().expect("tx");
        for i in 0..count {
            tx.execute(
                "INSERT INTO plan_history (plan_id, at_ms, action, reason_code, outcome) VALUES (NULL, ?1, 'create', 'created', 'applied')",
                [i],
            )
            .expect("insert history");
        }
        tx.commit().expect("commit");
    }

    fn db_with_history(count: i64) -> Arc<Mutex<Connection>> {
        let conn = open_in_memory().expect("db");
        insert_history_rows(&conn, count);
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn ten_thousand_rows_go_in_batches_of_500_without_duplicates_and_the_cursor_resumes() {
        let db = db_with_history(10_000);
        let sink = HistorySink::default();
        let mut total = 0;
        let mut cycles = 0;
        loop {
            let sent = push_history(&db, &sink, "d", 5).expect("push");
            cycles += 1;
            total += sent;
            if sent == 0 {
                break;
            }
        }
        assert_eq!(total, 10_000);
        assert_eq!(cycles, 2, "40 batches per cycle: 20 batches fit in the first cycle, the second finds nothing");
        assert!(sink.requests.lock().expect("requests").iter().all(|&n| n == 500));
        assert_eq!(sink.requests.lock().expect("requests").len(), 20);
        assert_eq!(sink.seen.lock().expect("seen").len(), 10_000);
        let conn = db.lock().expect("lock");
        assert_eq!(cursor::get_cursor(&conn, HISTORY_CURSOR).expect("cursor"), 10_000);
    }

    #[test]
    fn resending_after_a_reset_adds_no_duplicates_on_the_server() {
        let db = db_with_history(1_200);
        let sink = HistorySink::default();
        assert_eq!(push_history(&db, &sink, "d", 1).expect("first"), 1_200);
        cursor::reset_cursor(&db.lock().expect("lock"), HISTORY_CURSOR).expect("reset");
        assert_eq!(push_history(&db, &sink, "d", 2).expect("again"), 1_200);
        assert_eq!(sink.seen.lock().expect("seen").len(), 1_200, "ids are the idempotency key");
    }

    #[test]
    fn a_failed_batch_keeps_the_cursor_at_the_last_accepted_batch() {
        let db = db_with_history(1_200);
        // The first request is accepted (500 rows), the second fails.
        let flaky = FailSecond { inner: HistorySink::default(), calls: Mutex::new(0) };
        let result = push_history(&db, &flaky, "d", 3);
        assert!(matches!(result, Err(PushError::Remote(_))));
        let position = cursor::get_cursor(&db.lock().expect("lock"), HISTORY_CURSOR).expect("cursor");
        assert_eq!(position, 500, "only the accepted batch advanced the cursor");
        // Next cycle continues from there and delivers the rest without resending the first batch.
        assert_eq!(push_history(&db, &flaky, "d", 4), Ok(700));
        assert_eq!(flaky.inner.seen.lock().expect("seen").len(), 1_200);
    }

    struct FailSecond {
        inner: HistorySink,
        calls: Mutex<u32>,
    }

    impl PlanMirror for FailSecond {
        fn register_device(&self, device: &DeviceIdentity) -> Result<(), SyncError> {
            self.inner.register_device(device)
        }
        fn push_snapshot(&self, device_id: &str, snapshot: &Snapshot) -> Result<SnapshotOutcome, SyncError> {
            self.inner.push_snapshot(device_id, snapshot)
        }
        fn push_history(&self, device_id: &str, rows: &[PlanHistoryEntry]) -> Result<(), SyncError> {
            let mut calls = self.calls.lock().expect("calls");
            *calls += 1;
            if *calls == 2 {
                return Err(SyncError::Remote(crate::remote::supabase::RemoteError::Transport(crate::remote::http::TransportError::Connect)));
            }
            drop(calls);
            self.inner.push_history(device_id, rows)
        }
        fn push_notify_channel(&self, row: &NotifyChannelRow) -> Result<(), SyncError> {
            self.inner.push_notify_channel(row)
        }
    }

    #[test]
    fn no_request_goes_out_while_the_database_lock_is_held() {
        let db = db_with_history(1_100);
        let sink = HistorySink::default();
        *sink.db.lock().expect("db") = Some(db.clone());
        push_history(&db, &sink, "d", 1).expect("push");
        let log = sink.db_free_during_request.lock().expect("log");
        assert_eq!(log.len(), 3);
        assert!(log.iter().all(|free| *free), "the lock was held across a request: {log:?}");
    }

    #[test]
    fn rejected_attempts_are_mirrored_too() {
        let conn = open_in_memory().expect("db");
        conn.execute(
            "INSERT INTO plan_history (plan_id, at_ms, action, reason_code, outcome) VALUES (NULL, 1, 'create', 'custom', 'rejected_locked')",
            [],
        )
        .expect("insert");
        let entries = cursor::list_plan_history_after(&conn, 0, 10).expect("list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].outcome, "rejected_locked");
    }
}
