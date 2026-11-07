//! Local shared-wiki rows (migration m023): which wiki pages the user marked as shared with which
//! room, and which friends' concepts were already announced. A page without a mark row is private.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::AppResult;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkRow {
    pub page_id: i64,
    pub room_id: String,
    pub shared_at_ms: i64,
    pub last_pushed_hash: String,
    pub last_revision_id: String,
    pub allow_llm_merge: bool,
    pub page_name: String,
    pub subject_id: i64,
}

const MARK_SELECT: &str = "SELECT m.page_id, m.room_id, m.shared_at_ms, m.last_pushed_hash, m.last_revision_id, m.allow_llm_merge, p.page_name, p.subject_id
     FROM shared_page_marks m JOIN wiki_pages p ON p.id = m.page_id";

fn mark_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<MarkRow> {
    Ok(MarkRow {
        page_id: row.get(0)?,
        room_id: row.get(1)?,
        shared_at_ms: row.get(2)?,
        last_pushed_hash: row.get(3)?,
        last_revision_id: row.get(4)?,
        allow_llm_merge: row.get::<_, i64>(5)? == 1,
        page_name: row.get(6)?,
        subject_id: row.get(7)?,
    })
}

/// Marks a page as shared with a room. Marking again keeps `shared_at_ms` and the push state, and
/// changes only the merge consent; a changed consent forgets the pushed hash so the next sync
/// pushes the page again with the new flag.
pub fn set_mark(
    conn: &Connection,
    page_id: i64,
    room_id: &str,
    allow_llm_merge: bool,
    at_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO shared_page_marks (page_id, room_id, shared_at_ms, allow_llm_merge) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(page_id, room_id) DO UPDATE SET
           last_pushed_hash = CASE WHEN allow_llm_merge = excluded.allow_llm_merge THEN last_pushed_hash ELSE '' END,
           allow_llm_merge = excluded.allow_llm_merge",
        params![page_id, room_id, at_ms, i64::from(allow_llm_merge)],
    )?;
    Ok(())
}

pub fn get_mark(conn: &Connection, page_id: i64, room_id: &str) -> AppResult<Option<MarkRow>> {
    Ok(conn
        .query_row(
            &format!("{MARK_SELECT} WHERE m.page_id = ?1 AND m.room_id = ?2"),
            params![page_id, room_id],
            mark_from,
        )
        .optional()?)
}

pub fn list_marks(conn: &Connection, room_id: &str) -> AppResult<Vec<MarkRow>> {
    let mut stmt = conn.prepare(&format!(
        "{MARK_SELECT} WHERE m.room_id = ?1 ORDER BY p.page_name"
    ))?;
    let rows = stmt.query_map(params![room_id], mark_from)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn record_push(
    conn: &Connection,
    page_id: i64,
    room_id: &str,
    hash: &str,
    revision_id: &str,
) -> AppResult<()> {
    conn.execute(
        "UPDATE shared_page_marks SET last_pushed_hash = ?3, last_revision_id = ?4 WHERE page_id = ?1 AND room_id = ?2",
        params![page_id, room_id, hash, revision_id],
    )?;
    Ok(())
}

pub fn remove_mark(conn: &Connection, page_id: i64, room_id: &str) -> AppResult<bool> {
    Ok(conn.execute(
        "DELETE FROM shared_page_marks WHERE page_id = ?1 AND room_id = ?2",
        params![page_id, room_id],
    )? > 0)
}

/// `(subject_id, page_name)` of a wiki page row.
pub fn page_by_id(conn: &Connection, page_id: i64) -> AppResult<Option<(i64, String)>> {
    Ok(conn
        .query_row(
            "SELECT subject_id, page_name FROM wiki_pages WHERE id = ?1",
            params![page_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

/// Every wiki page name of the user, across subjects: the local side of the concept comparison.
pub fn all_page_names(conn: &Connection) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT page_name FROM wiki_pages")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn seen_concepts(conn: &Connection, room_id: &str) -> AppResult<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT concept FROM shared_concept_seen WHERE room_id = ?1")?;
    let rows = stmt.query_map(params![room_id], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn mark_concept_seen(
    conn: &Connection,
    room_id: &str,
    concept: &str,
    at_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO shared_concept_seen (room_id, concept, seen_ms) VALUES (?1, ?2, ?3)",
        params![room_id, concept, at_ms],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::store::wiki as repo;

    const ROOM: &str = "20000000-0000-4000-8000-000000000001";

    fn page(conn: &Connection, name: &str) -> i64 {
        let subject = repo::insert_subject(conn, "수학", "수학", 1).expect("subject");
        repo::upsert_page(conn, subject, name, &format!("pages/{name}.md"), "h", 1).expect("page");
        repo::get_page(conn, subject, name)
            .expect("get")
            .expect("row")
            .id
    }

    #[test]
    fn a_page_is_private_until_marked_and_remarking_keeps_the_push_state() {
        let conn = open_in_memory().expect("db");
        let id = page(&conn, "미분");
        assert_eq!(get_mark(&conn, id, ROOM).expect("get"), None);
        set_mark(&conn, id, ROOM, false, 10).expect("mark");
        record_push(&conn, id, ROOM, "abc", "rev1").expect("push");
        set_mark(&conn, id, ROOM, false, 99).expect("again");
        let row = get_mark(&conn, id, ROOM).expect("get").expect("row");
        assert_eq!(
            (
                row.shared_at_ms,
                row.last_pushed_hash.as_str(),
                row.last_revision_id.as_str(),
                row.allow_llm_merge
            ),
            (10, "abc", "rev1", false)
        );
        set_mark(&conn, id, ROOM, true, 100).expect("consent");
        let row = get_mark(&conn, id, ROOM).expect("get").expect("row");
        assert!(row.allow_llm_merge);
        assert_eq!(
            row.last_pushed_hash, "",
            "a changed consent must be pushed again"
        );
        assert!(remove_mark(&conn, id, ROOM).expect("remove"));
        assert!(list_marks(&conn, ROOM).expect("list").is_empty());
    }

    #[test]
    fn deleting_a_wiki_page_drops_its_marks_and_concepts_are_announced_once() {
        let conn = open_in_memory().expect("db");
        let id = page(&conn, "적분");
        set_mark(&conn, id, ROOM, false, 1).expect("mark");
        assert_eq!(all_page_names(&conn).expect("names"), ["적분"]);
        repo::delete_page(&conn, id).expect("delete");
        assert!(list_marks(&conn, ROOM).expect("list").is_empty());
        mark_concept_seen(&conn, ROOM, "연쇄법칙", 5).expect("seen");
        mark_concept_seen(&conn, ROOM, "연쇄법칙", 6).expect("seen again");
        assert_eq!(seen_concepts(&conn, ROOM).expect("seen").len(), 1);
    }
}
