//! How much study time the browser was not watched. The link fails open (D-22), so while no
//! extension is connected a blocked site loads. The gap is derived at read time from the
//! `extension_connected` / `extension_disconnected` rows already in `block_events`; nothing else
//! is stored. A browser that never connected counts as unwatched, and so does one whose
//! extension was disabled or removed: the app cannot tell those apart, only that the link is gone.

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::enforce::model::{BlockAction, EnforceMode, NewBlockEvent, TargetKind};
use crate::error::AppResult;
use crate::plan::BlockKind;
use crate::store;

/// Detail code of the disconnect rows written for links the previous app run never closed.
pub const DETAIL_APP_RESTART: &str = "app_restart";

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    /// Study time of the run (closed segments, plus the open one up to `now`).
    pub study_ms: i64,
    /// Part of it with no extension connected.
    pub unmonitored_ms: i64,
}

/// Link events as (at_ms, +1 connected / -1 disconnected), oldest first.
fn link_deltas(conn: &Connection) -> AppResult<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT at_ms, action FROM block_events WHERE target_kind = ?1 AND action IN (?2, ?3) ORDER BY at_ms, id",
    )?;
    let rows = stmt.query_map(
        params![
            TargetKind::Extension.as_str(),
            BlockAction::ExtensionConnected.as_str(),
            BlockAction::ExtensionDisconnected.as_str()
        ],
        |row| {
            let at: i64 = row.get(0)?;
            let action: String = row.get(1)?;
            Ok((
                at,
                if action == BlockAction::ExtensionConnected.as_str() {
                    1
                } else {
                    -1
                },
            ))
        },
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Milliseconds inside `[from, to)` with no connection open. A disconnect with no open link
/// (for example its connect row was lost) never drives the count below zero.
pub fn unmonitored_ms(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<i64> {
    if to_ms <= from_ms {
        return Ok(0);
    }
    let deltas = link_deltas(conn)?;
    let mut open: i64 = 0;
    let mut cursor = from_ms;
    let mut gap = 0;
    let mut events = deltas.into_iter().peekable();
    while let Some(&(at, delta)) = events.peek() {
        if at >= to_ms {
            break;
        }
        events.next();
        if at > cursor {
            if open == 0 {
                gap += at - cursor;
            }
            cursor = at;
        }
        open = (open + delta).max(0);
    }
    if open == 0 {
        gap += to_ms - cursor;
    }
    Ok(gap)
}

/// Study time and unwatched study time of a run, from its study segments.
pub fn run_coverage(conn: &Connection, run_id: i64, now_ms: i64) -> AppResult<Coverage> {
    let mut coverage = Coverage::default();
    for segment in store::list_segments(conn, run_id)?
        .into_iter()
        .filter(|s| s.kind == BlockKind::Study)
    {
        let end = segment.ended_at_ms.unwrap_or(now_ms);
        if end <= segment.started_at_ms {
            continue;
        }
        coverage.study_ms += end - segment.started_at_ms;
        coverage.unmonitored_ms += unmonitored_ms(conn, segment.started_at_ms, end)?;
    }
    Ok(coverage)
}

/// Startup: a previous run that ended with a link open (kill, crash, power loss) never wrote its
/// disconnects. Closes each outstanding link now so the gap does not run on into this session.
pub fn close_stale_links(conn: &Connection, now_ms: i64, mode: EnforceMode) -> AppResult<()> {
    let open = link_deltas(conn)?
        .iter()
        .fold(0_i64, |open, (_, delta)| (open + delta).max(0));
    for _ in 0..open {
        store::insert_block_event(
            conn,
            &NewBlockEvent {
                at_ms: now_ms,
                run_id: None,
                segment_id: None,
                target_kind: TargetKind::Extension,
                target_id: "app",
                action: BlockAction::ExtensionDisconnected,
                mode,
                detail_code: Some(DETAIL_APP_RESTART),
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn link(conn: &Connection, at: i64, action: BlockAction) {
        store::insert_block_event(
            conn,
            &NewBlockEvent {
                at_ms: at,
                run_id: None,
                segment_id: None,
                target_kind: TargetKind::Extension,
                target_id: "ext",
                action,
                mode: EnforceMode::DryRun,
                detail_code: None,
            },
        )
        .expect("event");
    }

    #[test]
    fn no_link_events_means_the_whole_window_is_unwatched() {
        let conn = open_in_memory().expect("db");
        assert_eq!(unmonitored_ms(&conn, 100, 400).expect("gap"), 300);
        assert_eq!(unmonitored_ms(&conn, 400, 100).expect("empty"), 0);
    }

    #[test]
    fn gaps_are_the_time_between_a_disconnect_and_the_next_connect() {
        let conn = open_in_memory().expect("db");
        link(&conn, 50, BlockAction::ExtensionConnected);
        link(&conn, 200, BlockAction::ExtensionDisconnected);
        link(&conn, 260, BlockAction::ExtensionConnected);
        // Window 100..400: watched 100..200, unwatched 200..260, watched 260..400.
        assert_eq!(unmonitored_ms(&conn, 100, 400).expect("gap"), 60);
        // A window entirely inside the gap, and one that starts inside it.
        assert_eq!(unmonitored_ms(&conn, 210, 250).expect("gap"), 40);
        assert_eq!(unmonitored_ms(&conn, 230, 300).expect("gap"), 30);
    }

    #[test]
    fn a_link_still_open_at_the_end_watches_to_the_end_and_two_links_overlap() {
        let conn = open_in_memory().expect("db");
        link(&conn, 10, BlockAction::ExtensionConnected);
        link(&conn, 20, BlockAction::ExtensionConnected);
        link(&conn, 30, BlockAction::ExtensionDisconnected);
        assert_eq!(
            unmonitored_ms(&conn, 0, 100).expect("gap"),
            10,
            "only 0..10 is unwatched"
        );
        link(&conn, 60, BlockAction::ExtensionDisconnected);
        assert_eq!(unmonitored_ms(&conn, 0, 100).expect("gap"), 10 + 40);
    }

    #[test]
    fn a_stray_disconnect_never_makes_the_count_negative() {
        let conn = open_in_memory().expect("db");
        link(&conn, 10, BlockAction::ExtensionDisconnected);
        link(&conn, 20, BlockAction::ExtensionConnected);
        assert_eq!(unmonitored_ms(&conn, 0, 100).expect("gap"), 20);
    }

    #[test]
    fn stale_links_are_closed_once_at_startup() {
        let conn = open_in_memory().expect("db");
        link(&conn, 10, BlockAction::ExtensionConnected);
        close_stale_links(&conn, 500, EnforceMode::DryRun).expect("close");
        assert_eq!(unmonitored_ms(&conn, 0, 1000).expect("gap"), 10 + 500);
        close_stale_links(&conn, 600, EnforceMode::DryRun).expect("again");
        let closes = store::list_block_events(&conn, 10)
            .expect("events")
            .into_iter()
            .filter(|e| e.detail_code.as_deref() == Some(DETAIL_APP_RESTART))
            .count();
        assert_eq!(closes, 1, "nothing is left open the second time");
    }
}
