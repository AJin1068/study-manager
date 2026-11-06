//! Rebuilds the page index from the files. The folder is the truth: pages added by hand show up,
//! pages deleted by hand disappear, and quiz counters of surviving pages are kept.

use rusqlite::Connection;
use serde::Serialize;

use super::fs::WikiFs;
use super::names;
use crate::error::AppResult;
use crate::store::wiki as repo;
use crate::store::wiki::SubjectRow;

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReindexReport {
    pub pages: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// The subject folder is gone: the index is empty and the UI says so.
    pub folder_missing: bool,
}

/// FNV-1a over the bytes: a stable change marker (not a security hash, and not stored anywhere
/// else, so it can change freely).
pub fn content_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Records one page's current text in the index.
pub fn index_page(conn: &Connection, subject: &SubjectRow, name: &str, text: &str, at_ms: i64) -> AppResult<()> {
    let rel_path = format!("pages/{}", names::page_file_name(name));
    repo::upsert_page(conn, subject.id, name, &rel_path, &content_hash(text), at_ms)
}

pub fn reindex(conn: &Connection, wiki: &WikiFs, subject: &SubjectRow, at_ms: i64) -> AppResult<ReindexReport> {
    let on_disk = wiki.list_pages(&subject.slug)?;
    let known = repo::list_pages(conn, subject.id)?;
    let (mut added, mut updated, mut removed) = (0, 0, 0);

    for name in &on_disk {
        let Some(text) = wiki.read_page(&subject.slug, name)? else { continue };
        match known.iter().find(|p| &p.page_name == name) {
            None => added += 1,
            Some(row) if row.content_hash != content_hash(&text) => updated += 1,
            Some(_) => continue,
        }
        index_page(conn, subject, name, &text, at_ms)?;
    }
    for row in &known {
        if !on_disk.contains(&row.page_name) {
            repo::delete_page(conn, row.id)?;
            removed += 1;
        }
    }
    Ok(ReindexReport { pages: on_disk.len(), added, updated, removed, folder_missing: !wiki.subject_dir_exists(&subject.slug) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::testutil::TempDir;

    fn setup() -> (TempDir, WikiFs, Connection, SubjectRow) {
        let dir = TempDir::new("index");
        let wiki = WikiFs::new(dir.path().join("w"));
        let conn = open_in_memory().expect("db");
        wiki.create_subject("수학", "수학").expect("folder");
        let id = repo::insert_subject(&conn, "수학", "수학", 1).expect("subject");
        let subject = repo::get_subject(&conn, id).expect("row");
        (dir, wiki, conn, subject)
    }

    fn names_in_index(conn: &Connection, subject: &SubjectRow) -> Vec<String> {
        repo::list_pages(conn, subject.id).expect("pages").into_iter().map(|p| p.page_name).collect()
    }

    #[test]
    fn the_index_matches_the_files_after_adds_edits_and_deletes_by_hand() {
        let (_dir, wiki, conn, subject) = setup();
        wiki.write_page("수학", "미분", "# 미분").expect("write");
        wiki.write_page("수학", "적분", "# 적분").expect("write");
        let first = reindex(&conn, &wiki, &subject, 10).expect("reindex");
        assert_eq!((first.pages, first.added, first.updated, first.removed, first.folder_missing), (2, 2, 0, 0, false));
        assert_eq!(names_in_index(&conn, &subject), ["미분", "적분"]);

        let pages = wiki.root().join("수학").join("pages");
        std::fs::write(pages.join("미분.md"), "# 미분\n고쳤다").expect("hand edit");
        std::fs::write(pages.join("확률.md"), "손으로 추가").expect("hand add");
        std::fs::remove_file(pages.join("적분.md")).expect("hand delete");
        std::fs::write(pages.join("notes.txt"), "not a page").expect("stray");
        let second = reindex(&conn, &wiki, &subject, 20).expect("reindex");
        assert_eq!((second.pages, second.added, second.updated, second.removed), (2, 1, 1, 1));
        assert_eq!(names_in_index(&conn, &subject), ["미분", "확률"]);

        let third = reindex(&conn, &wiki, &subject, 30).expect("reindex");
        assert_eq!((third.added, third.updated, third.removed), (0, 0, 0), "nothing changed, nothing rewritten");
        let row = repo::get_page(&conn, subject.id, "미분").expect("get").expect("row");
        assert_eq!((row.updated_at_ms, row.rel_path.as_str()), (20, "pages/미분.md"));
    }

    #[test]
    fn quiz_counters_survive_a_reindex_of_an_edited_page() {
        let (_dir, wiki, conn, subject) = setup();
        wiki.write_page("수학", "미분", "v1").expect("write");
        reindex(&conn, &wiki, &subject, 1).expect("reindex");
        conn.execute("UPDATE wiki_pages SET quiz_total = 3, quiz_correct = 2", []).expect("quiz");
        wiki.write_page("수학", "미분", "v2").expect("edit");
        reindex(&conn, &wiki, &subject, 2).expect("reindex");
        let row = repo::get_page(&conn, subject.id, "미분").expect("get").expect("row");
        assert_eq!((row.quiz_total, row.quiz_correct), (3, 2));
    }

    #[test]
    fn a_deleted_folder_empties_the_index_without_failing() {
        let (_dir, wiki, conn, subject) = setup();
        wiki.write_page("수학", "미분", "x").expect("write");
        reindex(&conn, &wiki, &subject, 1).expect("reindex");
        std::fs::remove_dir_all(wiki.root().join("수학")).expect("delete folder");
        let report = reindex(&conn, &wiki, &subject, 2).expect("reindex");
        assert_eq!((report.pages, report.removed, report.folder_missing), (0, 1, true));
        assert!(names_in_index(&conn, &subject).is_empty());
    }

    #[test]
    fn content_hash_is_stable_and_sensitive() {
        assert_eq!(content_hash(""), "cbf29ce484222325");
        assert_ne!(content_hash("a"), content_hash("b"));
        assert_eq!(content_hash("미분"), content_hash("미분"));
    }
}
