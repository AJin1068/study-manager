//! Local pairing row (migration m025): the last known pairing state of this PC's phone.

use rusqlite::{Connection, params};

use crate::error::AppResult;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairRow {
    pub paired: bool,
    pub paired_at_ms: Option<i64>,
}

pub fn pair(conn: &Connection) -> AppResult<PairRow> {
    Ok(conn.query_row(
        "SELECT state, paired_at_ms FROM phone_pair WHERE id = 1",
        [],
        |r| {
            let state: String = r.get(0)?;
            Ok(PairRow {
                paired: state == "paired",
                paired_at_ms: r.get(1)?,
            })
        },
    )?)
}

pub fn save_pair(
    conn: &Connection,
    paired: bool,
    paired_at_ms: Option<i64>,
    at_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "UPDATE phone_pair SET state = ?1, paired_at_ms = ?2, updated_at_ms = ?3 WHERE id = 1",
        params![
            if paired { "paired" } else { "unpaired" },
            if paired { paired_at_ms } else { None },
            at_ms
        ],
    )?;
    Ok(())
}

// ---- the publisher's cursor (migration m026) ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishRow {
    pub seq: i64,
    pub last_key: Option<String>,
    pub last_state: Option<String>,
    pub last_cause: Option<String>,
    pub last_valid_until_ms: Option<i64>,
}

pub fn publish_row(conn: &Connection) -> AppResult<PublishRow> {
    Ok(conn.query_row("SELECT seq, last_key, last_state, last_cause, last_valid_until_ms FROM phone_publish WHERE id = 1", [], |r| {
        Ok(PublishRow { seq: r.get(0)?, last_key: r.get(1)?, last_state: r.get(2)?, last_cause: r.get(3)?, last_valid_until_ms: r.get(4)? })
    })?)
}

/// Takes the next sequence number: one above both the highest ever used here and `server_floor`
/// (what the server says it holds). The number is spent even if the publication then fails, so a
/// number is never used for two different leases.
pub fn reserve_seq(conn: &Connection, server_floor: i64, at_ms: i64) -> AppResult<i64> {
    let next = conn.query_row(
        "SELECT MAX(seq, ?1) + 1 FROM phone_publish WHERE id = 1",
        params![server_floor],
        |r| r.get::<_, i64>(0),
    )?;
    conn.execute(
        "UPDATE phone_publish SET seq = ?1, updated_at_ms = ?2 WHERE id = 1",
        params![next, at_ms],
    )?;
    Ok(next)
}

/// Remembers what the server now holds.
pub fn save_published(
    conn: &Connection,
    key: &str,
    state: &str,
    cause: Option<&str>,
    valid_until_ms: i64,
    at_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "UPDATE phone_publish SET last_key = ?1, last_state = ?2, last_cause = ?3, last_valid_until_ms = ?4, updated_at_ms = ?5 WHERE id = 1",
        params![key, state, cause, valid_until_ms, at_ms],
    )?;
    Ok(())
}

// ---- gaps (migration m026) ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GapRow {
    pub run_id: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub kind: String,
}

/// Appends one gap. The same (run, kind, start) again is ignored, so recording twice is harmless.
/// Returns whether a row was added.
pub fn insert_gap(conn: &Connection, gap: &GapRow) -> AppResult<bool> {
    let added = conn.execute(
        "INSERT OR IGNORE INTO phone_gap (run_id, start_ms, end_ms, kind) VALUES (?1, ?2, ?3, ?4)",
        params![
            gap.run_id,
            gap.start_ms,
            gap.end_ms.max(gap.start_ms),
            gap.kind
        ],
    )?;
    Ok(added > 0)
}

/// Gaps that overlap `[from_ms, to_ms)`, oldest first.
pub fn gaps_between(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<Vec<GapRow>> {
    let mut stmt = conn.prepare(
        "SELECT run_id, start_ms, end_ms, kind FROM phone_gap WHERE end_ms > ?1 AND start_ms < ?2 ORDER BY start_ms, id",
    )?;
    let rows = stmt.query_map(params![from_ms, to_ms], |r| {
        Ok(GapRow {
            run_id: r.get(0)?,
            start_ms: r.get(1)?,
            end_ms: r.get(2)?,
            kind: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- pick-up counts (migration m027) ----

/// Appends `count` for the segment when it is higher than anything the segment already has.
/// Returns whether a row was added: a repeat, or a lower number from an older report, adds nothing.
pub fn record_pickups(
    conn: &Connection,
    segment_id: i64,
    count: i64,
    at_ms: i64,
) -> AppResult<bool> {
    let added = conn.execute(
        "INSERT INTO phone_pickups (segment_id, count, reported_at_ms)
         SELECT ?1, ?2, ?3 WHERE ?2 > COALESCE((SELECT MAX(count) FROM phone_pickups WHERE segment_id = ?1), -1)",
        params![segment_id, count, at_ms],
    )?;
    Ok(added > 0)
}

/// The count of one segment (its largest reported value), `None` when it never reported.
#[cfg(test)]
pub fn pickups_of(conn: &Connection, segment_id: i64) -> AppResult<Option<i64>> {
    Ok(conn.query_row(
        "SELECT MAX(count) FROM phone_pickups WHERE segment_id = ?1",
        params![segment_id],
        |r| r.get(0),
    )?)
}

/// `(study segments that reported, segments with at least one pick-up, total pick-ups)` over the
/// study segments that began in `[from_ms, to_ms)`.
pub fn pickup_totals(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<(i64, i64, i64)> {
    Ok(conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(m > 0), 0), COALESCE(SUM(m), 0)
         FROM (SELECT p.segment_id, MAX(p.count) AS m FROM phone_pickups p
               JOIN timer_segments s ON s.id = p.segment_id
               WHERE s.kind = 'study' AND s.started_at_ms >= ?1 AND s.started_at_ms < ?2
               GROUP BY p.segment_id)",
        params![from_ms, to_ms],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?)
}
