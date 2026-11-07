//! The local copy of shared pages: `<wiki root>/.shared/<room id>/<page name>.md`. A dot-folder
//! cannot collide with a subject folder (subject slugs never start with a dot), so the personal wiki
//! and the room's copies stay apart: reindexing and ingest never see these files. Text written here
//! came from the server; it is never read back as the user's own page.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::model::MAX_BODY_CHARS;
use crate::error::{AppError, AppResult};
use crate::wiki::fs::reject_symlink;
use crate::wiki::names;

const SHARED_DIR: &str = ".shared";

fn io_error(op: &str, err: &std::io::Error) -> AppError {
    AppError::Internal(format!("shared copy {op} failed: {:?}", err.kind()))
}

fn room_dir(root: &Path, room_id: &str) -> AppResult<PathBuf> {
    let ok = (8..=64).contains(&room_id.len())
        && room_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if !ok {
        return Err(AppError::Invalid("not a valid room id".into()));
    }
    Ok(root.join(SHARED_DIR).join(room_id))
}

fn page_path(root: &Path, room_id: &str, page_name: &str) -> AppResult<PathBuf> {
    let name = names::page_name(page_name)
        .filter(|n| n == page_name)
        .ok_or_else(|| AppError::Invalid("not a valid page name".into()))?;
    Ok(room_dir(root, room_id)?.join(names::page_file_name(&name)))
}

/// Writes (replaces) the copy atomically and returns its path.
pub fn write_copy(root: &Path, room_id: &str, page_name: &str, body: &str) -> AppResult<PathBuf> {
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(AppError::Invalid("the shared page is too large".into()));
    }
    let path = page_path(root, room_id, page_name)?;
    let dir = room_dir(root, room_id)?;
    reject_symlink(&root.join(SHARED_DIR))?;
    reject_symlink(&dir)?;
    reject_symlink(&path)?;
    fs::create_dir_all(&dir).map_err(|e| io_error("create folder", &e))?;
    let temp = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    let write = || -> std::io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()
    };
    if let Err(err) = write().and_then(|()| fs::rename(&temp, &path)) {
        let _ = fs::remove_file(&temp);
        return Err(io_error("write", &err));
    }
    Ok(path)
}

/// Deletes the copy if there is one (a withdrawn page loses its local copy too).
pub fn remove_copy(root: &Path, room_id: &str, page_name: &str) -> AppResult<bool> {
    let path = page_path(root, room_id, page_name)?;
    reject_symlink(&path)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(io_error("remove", &err)),
    }
}

/// Page names that have a local copy for a room.
pub fn list_copies(root: &Path, room_id: &str) -> AppResult<Vec<String>> {
    let dir = room_dir(root, room_id)?;
    reject_symlink(&dir)?;
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(io_error("list", &err)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| io_error("list", &e))?;
        if entry.file_type().is_ok_and(|t| t.is_file()) {
            if let Some(name) = entry
                .file_name()
                .to_str()
                .and_then(names::page_name_of_file)
            {
                found.push(name);
            }
        }
    }
    found.sort();
    Ok(found)
}

#[cfg(test)]
pub fn read_copy(root: &Path, room_id: &str, page_name: &str) -> AppResult<Option<String>> {
    let path = page_path(root, room_id, page_name)?;
    reject_symlink(&path)?;
    match fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_error("read", &err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    const ROOM: &str = "20000000-0000-4000-8000-000000000001";

    #[test]
    fn a_copy_round_trips_and_lives_outside_the_personal_wiki_folders() {
        let dir = TempDir::new("shared-copy");
        let path = write_copy(dir.path(), ROOM, "미분", "본문").expect("write");
        assert!(path.starts_with(dir.path().join(".shared")));
        assert_eq!(
            read_copy(dir.path(), ROOM, "미분")
                .expect("read")
                .as_deref(),
            Some("본문")
        );
        write_copy(dir.path(), ROOM, "미분", "새 본문").expect("replace");
        assert_eq!(
            read_copy(dir.path(), ROOM, "미분")
                .expect("read")
                .as_deref(),
            Some("새 본문")
        );
        assert!(remove_copy(dir.path(), ROOM, "미분").expect("remove"));
        assert!(!remove_copy(dir.path(), ROOM, "미분").expect("again"));
        assert_eq!(read_copy(dir.path(), ROOM, "미분").expect("read"), None);
    }

    #[test]
    fn names_that_could_escape_the_folder_are_refused() {
        let dir = TempDir::new("shared-copy-bad");
        for page in ["../x", "a/b", "..", "", "a\\b", "con"] {
            assert!(write_copy(dir.path(), ROOM, page, "x").is_err(), "{page}");
        }
        for room in ["../../etc", "short", "a/b/c/d/e/f/g", ""] {
            assert!(write_copy(dir.path(), room, "미분", "x").is_err(), "{room}");
        }
        assert!(write_copy(dir.path(), ROOM, "미분", &"가".repeat(MAX_BODY_CHARS + 1)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_in_the_way_is_refused() {
        let dir = TempDir::new("shared-copy-link");
        let outside = TempDir::new("shared-copy-outside");
        std::os::unix::fs::symlink(outside.path(), dir.path().join(".shared")).expect("link");
        assert!(write_copy(dir.path(), ROOM, "미분", "x").is_err());
        assert!(fs::read_dir(outside.path()).expect("dir").next().is_none());
    }
}
