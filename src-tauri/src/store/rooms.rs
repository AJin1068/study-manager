//! Local room rows (migration m020): the cached room list, the nickname used per room, the
//! timetable blocks already copied into the plan, and the distraction-attempt count the status
//! report needs. Nothing about other members and no invite code is stored.

use std::collections::HashSet;

use rusqlite::{params, Connection};

use crate::enforce::model::BlockAction;
use crate::error::AppResult;
use crate::rooms::model::{RoomRole, RoomSummary};

pub fn replace_membership_cache(conn: &mut Connection, rooms: &[RoomSummary], at_ms: i64) -> AppResult<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM room_membership_cache", [])?;
    for room in rooms {
        let role = match room.role {
            RoomRole::Owner => "owner",
            RoomRole::Member => "member",
        };
        tx.execute(
            "INSERT INTO room_membership_cache (room_id, room_name, role, member_count, capacity, refreshed_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![room.room_id, room.name, role, room.member_count, room.capacity, at_ms],
        )?;
    }
    // A room that left the list also loses its local nickname and copy marks.
    tx.execute("DELETE FROM room_prefs WHERE room_id NOT IN (SELECT room_id FROM room_membership_cache)", [])?;
    tx.execute("DELETE FROM together_imports WHERE room_id NOT IN (SELECT room_id FROM room_membership_cache)", [])?;
    tx.commit()?;
    Ok(())
}

/// The last known list and when it was fetched (`None` when never).
pub fn read_membership_cache(conn: &Connection) -> AppResult<(Vec<RoomSummary>, Option<i64>)> {
    let mut stmt = conn.prepare(
        "SELECT room_id, room_name, role, member_count, capacity, refreshed_at_ms
         FROM room_membership_cache ORDER BY room_name, room_id",
    )?;
    let rows = stmt.query_map([], |row| {
        let role: String = row.get(2)?;
        Ok((
            RoomSummary {
                room_id: row.get(0)?,
                name: row.get(1)?,
                role: if role == "owner" { RoomRole::Owner } else { RoomRole::Member },
                member_count: row.get(3)?,
                capacity: row.get(4)?,
            },
            row.get::<_, i64>(5)?,
        ))
    })?;
    let mut rooms = Vec::new();
    let mut refreshed = None;
    for row in rows {
        let (room, at) = row?;
        refreshed = Some(refreshed.map_or(at, |prev: i64| prev.max(at)));
        rooms.push(room);
    }
    Ok((rooms, refreshed))
}

pub fn has_rooms(conn: &Connection) -> AppResult<bool> {
    Ok(conn.query_row("SELECT EXISTS (SELECT 1 FROM room_membership_cache)", [], |row| row.get::<_, i64>(0))? == 1)
}

pub fn set_room_pref(conn: &Connection, room_id: &str, nickname: &str, ack_ms: i64) -> AppResult<()> {
    conn.execute(
        "INSERT INTO room_prefs (room_id, nickname, notice_ack_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(room_id) DO UPDATE SET nickname = excluded.nickname, notice_ack_ms = excluded.notice_ack_ms",
        params![room_id, nickname, ack_ms],
    )?;
    Ok(())
}

pub fn forget_room(conn: &Connection, room_id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM room_membership_cache WHERE room_id = ?1", params![room_id])?;
    conn.execute("DELETE FROM room_prefs WHERE room_id = ?1", params![room_id])?;
    conn.execute("DELETE FROM together_imports WHERE room_id = ?1", params![room_id])?;
    Ok(())
}

/// Returns false when the block was already copied (the primary key refused the second row).
pub fn record_import(conn: &Connection, room_id: &str, item_key: &str, plan_id: i64, at_ms: i64) -> AppResult<bool> {
    let added = conn.execute(
        "INSERT OR IGNORE INTO together_imports (room_id, item_key, plan_id, at_ms) VALUES (?1, ?2, ?3, ?4)",
        params![room_id, item_key, plan_id, at_ms],
    )?;
    Ok(added == 1)
}

pub fn imported_keys(conn: &Connection, room_id: &str) -> AppResult<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT item_key FROM together_imports WHERE room_id = ?1")?;
    let keys = stmt.query_map(params![room_id], |row| row.get::<_, String>(0))?;
    Ok(keys.collect::<Result<_, _>>()?)
}

/// Distraction attempts (an app asked to quit, would be quit in dry-run, or a blocked site)
/// recorded in `[from_ms, to_ms)`. Same event set as `store::count_attempts`, across all runs.
pub fn count_attempts_between(conn: &Connection, from_ms: i64, to_ms: i64) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM block_events WHERE at_ms >= ?1 AND at_ms < ?2 AND action IN (?3, ?4, ?5)",
        params![
            from_ms,
            to_ms,
            BlockAction::TerminateRequested.as_str(),
            BlockAction::WouldTerminate.as_str(),
            BlockAction::SiteBlocked.as_str(),
        ],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use rusqlite::OptionalExtension;

    fn room_nickname(conn: &Connection, room_id: &str) -> AppResult<Option<String>> {
        Ok(conn.query_row("SELECT nickname FROM room_prefs WHERE room_id = ?1", params![room_id], |row| row.get(0)).optional()?)
    }
    use super::*;
    use crate::db::open_in_memory;

    fn room(id: &str, name: &str) -> RoomSummary {
        RoomSummary { room_id: id.into(), name: name.into(), role: RoomRole::Member, member_count: 2, capacity: 8 }
    }

    #[test]
    fn the_cache_is_replaced_as_a_whole_and_drops_local_facts_of_rooms_that_left() {
        let mut conn = open_in_memory().expect("db");
        replace_membership_cache(&mut conn, &[room("room-aaaa-1", "A"), room("room-bbbb-2", "B")], 10).expect("cache");
        set_room_pref(&conn, "room-aaaa-1", "나", 10).expect("pref");
        set_room_pref(&conn, "room-bbbb-2", "나", 10).expect("pref");
        assert!(record_import(&conn, "room-bbbb-2", "123", 7, 11).expect("import"));
        assert!(!record_import(&conn, "room-bbbb-2", "123", 8, 12).expect("second"), "the same block twice is refused");

        replace_membership_cache(&mut conn, &[room("room-aaaa-1", "A")], 20).expect("cache");
        let (rooms, refreshed) = read_membership_cache(&conn).expect("read");
        assert_eq!((rooms.len(), refreshed), (1, Some(20)));
        assert_eq!(room_nickname(&conn, "room-bbbb-2").expect("nick"), None);
        assert!(imported_keys(&conn, "room-bbbb-2").expect("keys").is_empty());
        assert_eq!(room_nickname(&conn, "room-aaaa-1").expect("nick").as_deref(), Some("나"));
    }

    #[test]
    fn an_empty_cache_has_no_rooms_and_no_refresh_time() {
        let conn = open_in_memory().expect("db");
        assert!(!has_rooms(&conn).expect("has"));
        assert_eq!(read_membership_cache(&conn).expect("read"), (Vec::new(), None));
    }

    #[test]
    fn attempts_are_counted_inside_the_half_open_window_only() {
        let conn = open_in_memory().expect("db");
        for (at, action) in [(5, "would_terminate"), (10, "terminate_requested"), (10, "terminated"), (11, "site_blocked"), (20, "would_terminate")] {
            conn.execute(
                "INSERT INTO block_events (at_ms, target_kind, target_id, action, mode) VALUES (?1, 'app', 'x', ?2, 'dryRun')",
                params![at, action],
            )
            .expect("event");
        }
        assert_eq!(count_attempts_between(&conn, 10, 20).expect("count"), 2, "[10,20): the attempt and the site block, not the outcome row");
    }
}
