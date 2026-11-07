//! Wiki index tables. The markdown files are the truth; page rows are a rebuildable index, and
//! `wiki_subjects` / `wiki_sources` are the only state that exists nowhere else.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use crate::error::{AppError, AppResult};

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubjectRow {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub archived_at_ms: Option<i64>,
    pub created_at_ms: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PageRow {
    pub id: i64,
    pub subject_id: i64,
    pub page_name: String,
    pub rel_path: String,
    pub content_hash: String,
    pub updated_at_ms: i64,
    pub quiz_total: i64,
    pub quiz_correct: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceRow {
    pub id: i64,
    pub subject_id: i64,
    pub kind: String,
    pub source_ref: String,
    pub imported_at_ms: i64,
    pub status: String,
}

const SUBJECT_COLUMNS: &str = "id, slug, name, archived_at_ms, created_at_ms";
const PAGE_COLUMNS: &str =
    "id, subject_id, page_name, rel_path, content_hash, updated_at_ms, quiz_total, quiz_correct";

fn subject_from(row: &Row<'_>) -> rusqlite::Result<SubjectRow> {
    Ok(SubjectRow {
        id: row.get(0)?,
        slug: row.get(1)?,
        name: row.get(2)?,
        archived_at_ms: row.get(3)?,
        created_at_ms: row.get(4)?,
    })
}

fn page_from(row: &Row<'_>) -> rusqlite::Result<PageRow> {
    Ok(PageRow {
        id: row.get(0)?,
        subject_id: row.get(1)?,
        page_name: row.get(2)?,
        rel_path: row.get(3)?,
        content_hash: row.get(4)?,
        updated_at_ms: row.get(5)?,
        quiz_total: row.get(6)?,
        quiz_correct: row.get(7)?,
    })
}

pub fn insert_subject(conn: &Connection, slug: &str, name: &str, at_ms: i64) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO wiki_subjects (slug, name, created_at_ms) VALUES (?1, ?2, ?3)",
        params![slug, name, at_ms],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_subject(conn: &Connection, id: i64) -> AppResult<SubjectRow> {
    conn.query_row(
        &format!("SELECT {SUBJECT_COLUMNS} FROM wiki_subjects WHERE id = ?1"),
        params![id],
        subject_from,
    )
    .optional()?
    .ok_or_else(|| AppError::Invalid(format!("unknown wiki subject {id}")))
}

pub fn find_subject_by_slug(conn: &Connection, slug: &str) -> AppResult<Option<SubjectRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {SUBJECT_COLUMNS} FROM wiki_subjects WHERE slug = ?1"),
            params![slug],
            subject_from,
        )
        .optional()?)
}

/// Active (not archived) subject whose name equals `name` ignoring case; how a plan's free-text
/// subject finds its wiki.
pub fn find_active_subject_by_name(conn: &Connection, name: &str) -> AppResult<Option<SubjectRow>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {SUBJECT_COLUMNS} FROM wiki_subjects
                 WHERE archived_at_ms IS NULL AND lower(name) = lower(?1) ORDER BY id LIMIT 1"
            ),
            params![name],
            subject_from,
        )
        .optional()?)
}

pub fn list_subjects(conn: &Connection, include_archived: bool) -> AppResult<Vec<SubjectRow>> {
    let sql = if include_archived {
        format!("SELECT {SUBJECT_COLUMNS} FROM wiki_subjects ORDER BY id")
    } else {
        format!(
            "SELECT {SUBJECT_COLUMNS} FROM wiki_subjects WHERE archived_at_ms IS NULL ORDER BY id"
        )
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], subject_from)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn set_archived(conn: &Connection, id: i64, archived_at_ms: Option<i64>) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE wiki_subjects SET archived_at_ms = ?2 WHERE id = ?1",
        params![id, archived_at_ms],
    )?;
    if changed == 0 {
        return Err(AppError::Invalid(format!("unknown wiki subject {id}")));
    }
    Ok(())
}

/// Inserts or refreshes a page row. Quiz counters survive a refresh: they belong to the page, not
/// to one version of its text.
pub fn upsert_page(
    conn: &Connection,
    subject_id: i64,
    page_name: &str,
    rel_path: &str,
    content_hash: &str,
    at_ms: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO wiki_pages (subject_id, page_name, rel_path, content_hash, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(subject_id, page_name) DO UPDATE SET
           rel_path = excluded.rel_path, content_hash = excluded.content_hash, updated_at_ms = excluded.updated_at_ms",
        params![subject_id, page_name, rel_path, content_hash, at_ms],
    )?;
    Ok(())
}

pub fn list_pages(conn: &Connection, subject_id: i64) -> AppResult<Vec<PageRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PAGE_COLUMNS} FROM wiki_pages WHERE subject_id = ?1 ORDER BY page_name"
    ))?;
    let rows = stmt.query_map(params![subject_id], page_from)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[cfg(test)]
pub fn get_page(conn: &Connection, subject_id: i64, page_name: &str) -> AppResult<Option<PageRow>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {PAGE_COLUMNS} FROM wiki_pages WHERE subject_id = ?1 AND page_name = ?2"
            ),
            params![subject_id, page_name],
            page_from,
        )
        .optional()?)
}

pub fn delete_page(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM wiki_pages WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn insert_source(
    conn: &Connection,
    subject_id: i64,
    kind: &str,
    source_ref: &str,
    status: &str,
    at_ms: i64,
) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO wiki_sources (subject_id, kind, source_ref, imported_at_ms, status) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![subject_id, kind, source_ref, at_ms, status],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_sources(conn: &Connection, subject_id: i64) -> AppResult<Vec<SourceRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subject_id, kind, source_ref, imported_at_ms, status FROM wiki_sources WHERE subject_id = ?1 ORDER BY id DESC",
    )?;
    let rows = stmt.query_map(params![subject_id], |row| {
        Ok(SourceRow {
            id: row.get(0)?,
            subject_id: row.get(1)?,
            kind: row.get(2)?,
            source_ref: row.get(3)?,
            imported_at_ms: row.get(4)?,
            status: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn subjects_are_unique_by_slug_and_archivable() {
        let conn = open_in_memory().expect("db");
        let id = insert_subject(&conn, "수학", "수학", 1).expect("insert");
        assert!(
            insert_subject(&conn, "수학", "Math", 2).is_err(),
            "slug is unique"
        );
        assert_eq!(
            find_active_subject_by_name(&conn, "수학")
                .expect("find")
                .map(|s| s.id),
            Some(id)
        );
        set_archived(&conn, id, Some(5)).expect("archive");
        assert!(
            find_active_subject_by_name(&conn, "수학")
                .expect("find")
                .is_none()
        );
        assert!(list_subjects(&conn, false).expect("list").is_empty());
        assert_eq!(
            list_subjects(&conn, true).expect("list")[0].archived_at_ms,
            Some(5)
        );
        set_archived(&conn, id, None).expect("restore");
        assert_eq!(list_subjects(&conn, false).expect("list").len(), 1);
        assert!(set_archived(&conn, 999, Some(1)).is_err());
    }

    #[test]
    fn page_refresh_keeps_quiz_counters_and_cascades_with_the_subject() {
        let conn = open_in_memory().expect("db");
        let id = insert_subject(&conn, "s", "S", 1).expect("subject");
        upsert_page(&conn, id, "p", "pages/p.md", "h1", 1).expect("page");
        conn.execute("UPDATE wiki_pages SET quiz_total = 4, quiz_correct = 3", [])
            .expect("quiz");
        upsert_page(&conn, id, "p", "pages/p.md", "h2", 2).expect("refresh");
        let page = get_page(&conn, id, "p").expect("get").expect("row");
        assert_eq!(
            (
                page.content_hash.as_str(),
                page.quiz_total,
                page.quiz_correct
            ),
            ("h2", 4, 3)
        );
        assert!(
            conn.execute("UPDATE wiki_pages SET quiz_correct = 9", [])
                .is_err(),
            "correct cannot exceed total"
        );
        insert_source(&conn, id, "paste", "memo", "ingested", 3).expect("source");
        conn.execute("DELETE FROM wiki_subjects WHERE id = ?1", params![id])
            .expect("delete");
        assert!(list_pages(&conn, id).expect("pages").is_empty());
        assert!(list_sources(&conn, id).expect("sources").is_empty());
    }
}
