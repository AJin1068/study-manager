//! Downtime (sleep and forced quit) recording.
//!
//! Overlap rule (single rule, verified by the AC5 order tests):
//! - Measured sleep (`estimated = 0`, from WillSleep..DidWake) always wins.
//! - An estimated gap (`estimated = 1`, heartbeat gap > 90s) is recorded only for the parts
//!   that no measured interval already covers. Overlap is subtracted before insert because
//!   downtime rows are append-only and cannot be corrected afterwards.

use rusqlite::Connection;
use serde::Serialize;

use crate::error::AppResult;
use crate::store;

pub const HEARTBEAT_INTERVAL_MS: i64 = 30_000;
/// A heartbeat older than this means the process did not run in between (proposed value).
pub const GAP_THRESHOLD_MS: i64 = 90_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Interval {
    pub start: i64,
    pub end: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DowntimeKind {
    Sleep,
    ForcedQuit,
}

impl DowntimeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DowntimeKind::Sleep => "sleep",
            DowntimeKind::ForcedQuit => "forced_quit",
        }
    }

    pub fn parse(raw: &str) -> Option<DowntimeKind> {
        match raw {
            "sleep" => Some(DowntimeKind::Sleep),
            "forced_quit" => Some(DowntimeKind::ForcedQuit),
            _ => None,
        }
    }
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DowntimeRecord {
    pub id: i64,
    pub kind: DowntimeKind,
    pub start_at_ms: i64,
    pub end_at_ms: i64,
    pub estimated: bool,
}

impl Serialize for DowntimeKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Returns `base` minus the union of `covers`, as sorted, non-overlapping, non-empty pieces.
pub fn subtract(base: Interval, covers: &[Interval]) -> Vec<Interval> {
    let mut sorted: Vec<Interval> = covers.to_vec();
    sorted.sort();
    let mut pieces = Vec::new();
    let mut cursor = base.start;
    for cover in sorted {
        if cursor >= base.end {
            break;
        }
        if cover.end <= cursor {
            continue;
        }
        if cover.start > cursor {
            pieces.push(Interval {
                start: cursor,
                end: cover.start.min(base.end),
            });
        }
        cursor = cursor.max(cover.end);
    }
    if cursor < base.end {
        pieces.push(Interval {
            start: cursor,
            end: base.end,
        });
    }
    pieces.retain(|p| p.start < p.end);
    pieces
}

/// The heartbeat gap as a candidate interval, or `None` when the gap is within threshold.
pub fn heartbeat_gap(last_heartbeat_ms: i64, now_ms: i64) -> Option<Interval> {
    if now_ms - last_heartbeat_ms > GAP_THRESHOLD_MS {
        Some(Interval {
            start: last_heartbeat_ms,
            end: now_ms,
        })
    } else {
        None
    }
}

/// Records an estimated sleep for a heartbeat gap, minus measured sleep. Returns what was written.
pub fn record_estimated_gap(
    conn: &Connection,
    last_heartbeat_ms: i64,
    now_ms: i64,
) -> AppResult<Vec<Interval>> {
    let Some(gap) = heartbeat_gap(last_heartbeat_ms, now_ms) else {
        return Ok(Vec::new());
    };
    record_estimated_interval(conn, gap)
}

/// Records an estimated sleep over `interval`, minus measured sleep already on record, so an
/// estimate never overlaps a measured sleep. Returns what was written.
pub fn record_estimated_interval(
    conn: &Connection,
    interval: Interval,
) -> AppResult<Vec<Interval>> {
    let measured = store::measured_sleep_intervals(conn)?;
    let pieces = subtract(interval, &measured);
    for piece in &pieces {
        store::insert_downtime(conn, DowntimeKind::Sleep, *piece, true)?;
    }
    Ok(pieces)
}

/// Records a measured sleep from WillSleep to DidWake (estimated = 0).
pub fn record_measured_sleep(conn: &Connection, interval: Interval) -> AppResult<()> {
    store::insert_downtime(conn, DowntimeKind::Sleep, interval, false)
}

/// Time the app was off or the machine asleep inside a window, per kind. `forced_quit` is the
/// "app killed" time the relaunch watcher bridges; it is shown, never hidden, in statistics.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DowntimeSummary {
    pub sleep_ms: i64,
    pub forced_quit_ms: i64,
    pub forced_quit_count: i64,
}

/// Sums each kind's overlap with `[from_ms, to_ms)`. Rows of one kind never overlap each other
/// (overlap is removed before insert); rows of different kinds are reported separately.
pub fn summarize(records: &[DowntimeRecord], from_ms: i64, to_ms: i64) -> DowntimeSummary {
    let mut out = DowntimeSummary {
        sleep_ms: 0,
        forced_quit_ms: 0,
        forced_quit_count: 0,
    };
    for record in records {
        let overlap = record.end_at_ms.min(to_ms) - record.start_at_ms.max(from_ms);
        if overlap <= 0 {
            continue;
        }
        match record.kind {
            DowntimeKind::Sleep => out.sleep_ms += overlap,
            DowntimeKind::ForcedQuit => {
                out.forced_quit_ms += overlap;
                out.forced_quit_count += 1;
            }
        }
    }
    out
}

/// Closes every session that never recorded a clean exit, writing a forced_quit row from its
/// last heartbeat to `now_ms`. Each session is marked `clean_exit = 0` so it is recorded once.
pub fn recover_unclean_sessions(conn: &mut Connection, now_ms: i64) -> AppResult<usize> {
    let tx = conn.transaction()?;
    let open = store::open_sessions(&tx)?;
    for (session_id, last_heartbeat_ms) in &open {
        if now_ms > *last_heartbeat_ms {
            store::insert_downtime(
                &tx,
                DowntimeKind::ForcedQuit,
                Interval {
                    start: *last_heartbeat_ms,
                    end: now_ms,
                },
                false,
            )?;
        }
        store::set_clean_exit(&tx, *session_id, false, now_ms)?;
    }
    tx.commit()?;
    Ok(open.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn iv(start: i64, end: i64) -> Interval {
        Interval { start, end }
    }

    #[test]
    fn subtract_without_covers_returns_base() {
        assert_eq!(subtract(iv(0, 100), &[]), vec![iv(0, 100)]);
    }

    #[test]
    fn subtract_removes_measured_middle_and_keeps_edges() {
        assert_eq!(
            subtract(iv(0, 100), &[iv(30, 60)]),
            vec![iv(0, 30), iv(60, 100)]
        );
    }

    #[test]
    fn subtract_full_cover_leaves_nothing() {
        assert!(subtract(iv(10, 20), &[iv(0, 100)]).is_empty());
    }

    #[test]
    fn subtract_merges_overlapping_covers() {
        assert_eq!(
            subtract(iv(0, 100), &[iv(10, 40), iv(30, 50), iv(90, 200)]),
            vec![iv(0, 10), iv(50, 90)]
        );
    }

    #[test]
    fn heartbeat_gap_threshold_is_strict() {
        assert_eq!(heartbeat_gap(0, GAP_THRESHOLD_MS), None);
        assert_eq!(
            heartbeat_gap(0, GAP_THRESHOLD_MS + 1),
            Some(iv(0, GAP_THRESHOLD_MS + 1))
        );
    }

    /// Tick first: the gap is recorded only where no measured sleep exists.
    #[test]
    fn estimated_gap_is_trimmed_by_measured_sleep_in_either_order() {
        let conn = open_in_memory().expect("db");
        // Measured sleep 1_000..61_000 already recorded (DidWake arrived first).
        record_measured_sleep(&conn, iv(1_000, 61_000)).expect("measured");
        // Heartbeat stale from 0 to 200_000: only 0..1_000 and 61_000..200_000 are estimated.
        let written = record_estimated_gap(&conn, 0, 200_000).expect("gap");
        assert_eq!(written, vec![iv(0, 1_000), iv(61_000, 200_000)]);
        let rows = store::list_downtime(&conn, 10).expect("rows");
        assert_eq!(rows.iter().filter(|r| r.estimated).count(), 2);
        assert_eq!(rows.iter().filter(|r| !r.estimated).count(), 1);
    }

    #[test]
    fn gap_fully_covered_by_measured_sleep_writes_no_estimate() {
        let conn = open_in_memory().expect("db");
        record_measured_sleep(&conn, iv(0, 200_000)).expect("measured");
        assert!(
            record_estimated_gap(&conn, 0, 200_000)
                .expect("gap")
                .is_empty()
        );
    }

    #[test]
    fn short_gap_within_threshold_writes_nothing() {
        let conn = open_in_memory().expect("db");
        assert!(
            record_estimated_gap(&conn, 0, 30_000)
                .expect("gap")
                .is_empty()
        );
    }

    #[test]
    fn summary_clips_to_the_window_and_counts_forced_quits() {
        let rec = |kind, start_at_ms, end_at_ms| DowntimeRecord {
            id: 0,
            kind,
            start_at_ms,
            end_at_ms,
            estimated: false,
        };
        let rows = [
            rec(DowntimeKind::Sleep, 0, 100),
            rec(DowntimeKind::ForcedQuit, 150, 250),
            rec(DowntimeKind::ForcedQuit, 400, 500),
            rec(DowntimeKind::ForcedQuit, 600, 700),
        ];
        let s = summarize(&rows, 50, 450);
        assert_eq!(
            (s.sleep_ms, s.forced_quit_ms, s.forced_quit_count),
            (50, 150, 2)
        );
        let empty = summarize(&rows, 250, 400);
        assert_eq!(
            (
                empty.sleep_ms,
                empty.forced_quit_ms,
                empty.forced_quit_count
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn unclean_session_becomes_forced_quit_once() {
        let mut conn = open_in_memory().expect("db");
        let session = store::insert_session(&conn, 1_000).expect("session");
        store::touch_heartbeat(&conn, session, 5_000).expect("hb");
        // Process killed here: clean_exit stays NULL. Next launch at 9_000.
        assert_eq!(
            recover_unclean_sessions(&mut conn, 9_000).expect("recover"),
            1
        );
        let rows = store::list_downtime(&conn, 10).expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, DowntimeKind::ForcedQuit);
        assert_eq!((rows[0].start_at_ms, rows[0].end_at_ms), (5_000, 9_000));
        assert!(!rows[0].estimated);
        // Second recovery pass must not duplicate the row.
        assert_eq!(
            recover_unclean_sessions(&mut conn, 10_000).expect("recover"),
            0
        );
        assert_eq!(store::list_downtime(&conn, 10).expect("rows").len(), 1);
    }
}
