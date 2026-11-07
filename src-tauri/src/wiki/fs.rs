//! The wiki folder: `<root>/<subject-slug>/{raw/, pages/*.md, RULES.md, .versions/}`. The files
//! are the truth; the database only indexes them.
//!
//! Every path is built here from a validated slug and page name, never from model output or UI
//! text directly. Symbolic links are refused wherever one could redirect a read or a write out of
//! the folder. Writes go to a temporary file in the same folder and replace the page atomically;
//! the previous version is copied to `.versions/` first.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::names;
use crate::error::{AppError, AppResult};

pub const MAX_PAGE_BYTES: usize = 200 * 1024;
pub const MAX_PAGES_PER_SUBJECT: usize = 500;
/// Largest file read back (pages and imported documents alike).
pub const MAX_READ_BYTES: u64 = 1024 * 1024;

const PAGES_DIR: &str = "pages";
const RAW_DIR: &str = "raw";
const VERSIONS_DIR: &str = ".versions";
const RULES_FILE: &str = "RULES.md";

fn io_error(op: &str, err: &io::Error) -> AppError {
    // The kind only: the message of an OS error can contain a path.
    AppError::Internal(format!("wiki {op} failed: {:?}", err.kind()))
}

/// Refuses a path that is a symbolic link. A missing path is fine.
pub fn reject_symlink(path: &Path) -> AppResult<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(AppError::Invalid(
            "the wiki does not follow symbolic links".into(),
        )),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io_error("stat", &err)),
    }
}

/// `~/StudyWiki` from the home directory the OS reports.
pub fn default_root() -> AppResult<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .ok_or_else(|| AppError::Internal("no home directory is set".into()))?;
    Ok(PathBuf::from(home).join("StudyWiki"))
}

/// What a page write did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteReport {
    pub bytes: usize,
    /// The file the previous version was copied to, when there was one.
    pub backup: Option<PathBuf>,
}

pub struct WikiFs {
    root: PathBuf,
}

impl WikiFs {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `llm.wiki_dir` as stored: empty means the default folder.
    pub fn from_setting(configured: &str) -> AppResult<Self> {
        if configured.is_empty() {
            Ok(Self::new(default_root()?))
        } else {
            Ok(Self::new(PathBuf::from(configured)))
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn subject_path(&self, slug: &str) -> AppResult<PathBuf> {
        if names::subject_slug(slug).as_deref().ok() != Some(slug) {
            return Err(AppError::Invalid("not a valid subject folder name".into()));
        }
        Ok(self.root.join(slug))
    }

    /// Subject folder with `pages/` verified, not created.
    fn pages_path(&self, slug: &str) -> AppResult<PathBuf> {
        let subject = self.subject_path(slug)?;
        reject_symlink(&subject)?;
        let pages = subject.join(PAGES_DIR);
        reject_symlink(&pages)?;
        Ok(pages)
    }

    fn ensure_dir(path: &Path) -> AppResult<()> {
        reject_symlink(path)?;
        fs::create_dir_all(path).map_err(|e| io_error("create folder", &e))
    }

    fn ensure_layout(&self, slug: &str) -> AppResult<PathBuf> {
        let subject = self.subject_path(slug)?;
        Self::ensure_dir(&self.root)?;
        Self::ensure_dir(&subject)?;
        for dir in [PAGES_DIR, RAW_DIR, VERSIONS_DIR] {
            Self::ensure_dir(&subject.join(dir))?;
        }
        Ok(subject)
    }

    /// Creates the folder layout and a starter `RULES.md` (never overwritten once edited).
    pub fn create_subject(&self, slug: &str, name: &str) -> AppResult<()> {
        let subject = self.ensure_layout(slug)?;
        let rules = subject.join(RULES_FILE);
        reject_symlink(&rules)?;
        if !rules.exists() {
            let text = format!(
                "# {name} 위키 규칙\n\n- 페이지는 `pages/`에 마크다운으로 둡니다.\n- 자료 원본은 `raw/`에 둡니다.\n- 앱이 페이지를 바꾸기 전 이전 버전은 `.versions/`에 남습니다.\n"
            );
            fs::write(&rules, text).map_err(|e| io_error("write rules", &e))?;
        }
        Ok(())
    }

    pub fn subject_dir_exists(&self, slug: &str) -> bool {
        self.subject_path(slug)
            .is_ok_and(|p| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
    }

    fn page_path(&self, slug: &str, name: &str) -> AppResult<PathBuf> {
        if names::page_name(name).as_deref() != Some(name) {
            return Err(AppError::Invalid("not a valid page name".into()));
        }
        Ok(self.pages_path(slug)?.join(names::page_file_name(name)))
    }

    /// Page text, or `None` when the page (or the whole folder) is missing.
    pub fn read_page(&self, slug: &str, name: &str) -> AppResult<Option<String>> {
        let path = self.page_path(slug, name)?;
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(AppError::Invalid(
                    "the wiki does not follow symbolic links".into(),
                ));
            }
            Ok(meta) if meta.len() > MAX_READ_BYTES => {
                return Err(AppError::Invalid("the page is too large to open".into()));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(io_error("stat page", &err)),
        }
        match fs::read(&path) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| AppError::Invalid("the page is not valid UTF-8 text".into())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(io_error("read page", &err)),
        }
    }

    /// Text of a backup the app made (`write_page` returns its file name). The name must be a bare
    /// `*.md` file name: no separators, no `..`, no hidden file.
    pub fn read_version(&self, slug: &str, file_name: &str) -> AppResult<Option<String>> {
        let bare = !file_name.is_empty()
            && file_name.ends_with(".md")
            && !file_name.starts_with('.')
            && !file_name.contains(['/', '\\', '\0'])
            && !file_name.contains("..");
        if !bare {
            return Err(AppError::Invalid("not a valid backup name".into()));
        }
        let subject = self.subject_path(slug)?;
        reject_symlink(&subject)?;
        let versions = subject.join(VERSIONS_DIR);
        reject_symlink(&versions)?;
        let path = versions.join(file_name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(AppError::Invalid(
                    "the wiki does not follow symbolic links".into(),
                ));
            }
            Ok(meta) if meta.len() > MAX_READ_BYTES => {
                return Err(AppError::Invalid("the backup is too large to open".into()));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(io_error("stat backup", &err)),
        }
        match fs::read(&path) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| AppError::Invalid("the backup is not valid UTF-8 text".into())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(io_error("read backup", &err)),
        }
    }

    /// Names of the valid page files, sorted. Symbolic links, odd names, and temp files are skipped.
    pub fn list_pages(&self, slug: &str) -> AppResult<Vec<String>> {
        let pages = self.pages_path(slug)?;
        let entries = match fs::read_dir(&pages) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(io_error("list pages", &err)),
        };
        let mut found = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| io_error("list pages", &e))?;
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_file() {
                continue;
            }
            if let Some(name) = entry
                .file_name()
                .to_str()
                .and_then(names::page_name_of_file)
            {
                found.push(name);
            }
        }
        found.sort();
        Ok(found)
    }

    pub fn write_page(&self, slug: &str, name: &str, content: &str) -> AppResult<WriteReport> {
        self.write_page_with(slug, name, content, now_ms_for_backup(), || Ok(()))
    }

    /// `before_rename` runs after the new text is safely on disk and before it replaces the page;
    /// tests make it fail to model a crash at the worst moment.
    pub(crate) fn write_page_with(
        &self,
        slug: &str,
        name: &str,
        content: &str,
        at_ms: i64,
        before_rename: impl FnOnce() -> io::Result<()>,
    ) -> AppResult<WriteReport> {
        if content.len() > MAX_PAGE_BYTES {
            return Err(AppError::Invalid("the page is too large".into()));
        }
        let page = self.page_path(slug, name)?;
        let subject = self.ensure_layout(slug)?;
        reject_symlink(&page)?;
        let existing = page.exists();
        if !existing && self.list_pages(slug)?.len() >= MAX_PAGES_PER_SUBJECT {
            return Err(AppError::Invalid(
                "this subject already has the most pages the wiki allows".into(),
            ));
        }

        let temp = subject
            .join(PAGES_DIR)
            .join(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
        let write_temp = || -> io::Result<()> {
            let mut file = fs::File::create(&temp)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()
        };
        if let Err(err) = write_temp() {
            let _ = fs::remove_file(&temp);
            return Err(io_error("write page", &err));
        }
        let backup = if existing {
            match self.backup(&subject, name, &page, at_ms) {
                Ok(path) => Some(path),
                Err(err) => {
                    let _ = fs::remove_file(&temp);
                    return Err(err);
                }
            }
        } else {
            None
        };
        let replaced = before_rename().and_then(|()| fs::rename(&temp, &page));
        if let Err(err) = replaced {
            let _ = fs::remove_file(&temp);
            return Err(io_error("replace page", &err));
        }
        Ok(WriteReport {
            bytes: content.len(),
            backup,
        })
    }

    fn backup(&self, subject: &Path, name: &str, page: &Path, at_ms: i64) -> AppResult<PathBuf> {
        let versions = subject.join(VERSIONS_DIR);
        let mut target = versions.join(format!("{name}.{at_ms}.md"));
        let mut counter = 1;
        while target.exists() {
            target = versions.join(format!("{name}.{at_ms}-{counter}.md"));
            counter += 1;
        }
        fs::copy(page, &target).map_err(|e| io_error("back up page", &e))?;
        Ok(target)
    }
}

fn now_ms_for_backup() -> i64 {
    crate::clock::now_ms().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn wiki() -> (TempDir, WikiFs) {
        let dir = TempDir::new("wikifs");
        let fs = WikiFs::new(dir.path().join("StudyWiki"));
        (dir, fs)
    }

    fn files_under(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            let Ok(entries) = fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(
                        path.strip_prefix(dir)
                            .expect("under dir")
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn creating_a_subject_lays_out_the_folder_and_keeps_edited_rules() {
        let (_dir, wiki) = wiki();
        wiki.create_subject("수학", "수학").expect("create");
        let base = wiki.root().join("수학");
        for sub in ["pages", "raw", ".versions"] {
            assert!(base.join(sub).is_dir(), "{sub}");
        }
        let rules = fs::read_to_string(base.join("RULES.md")).expect("rules");
        assert!(rules.contains("수학 위키 규칙"));
        fs::write(base.join("RULES.md"), "내가 고친 규칙").expect("edit");
        wiki.create_subject("수학", "수학").expect("again");
        assert_eq!(
            fs::read_to_string(base.join("RULES.md")).expect("rules"),
            "내가 고친 규칙"
        );
        assert!(wiki.subject_dir_exists("수학") && !wiki.subject_dir_exists("영어"));
    }

    #[test]
    fn hangul_and_emoji_names_work_for_folders_and_pages() {
        let (_dir, wiki) = wiki();
        wiki.create_subject("📚-영어", "📚 영어").expect("create");
        wiki.write_page("📚-영어", "관계대명사 😀", "# 본문")
            .expect("write");
        assert_eq!(wiki.list_pages("📚-영어").expect("list"), ["관계대명사 😀"]);
        assert_eq!(
            wiki.read_page("📚-영어", "관계대명사 😀")
                .expect("read")
                .as_deref(),
            Some("# 본문")
        );
    }

    #[test]
    fn rewriting_a_page_keeps_the_previous_version_and_new_pages_have_none() {
        let (_dir, wiki) = wiki();
        wiki.create_subject("s", "s").expect("create");
        let first = wiki
            .write_page_with("s", "p", "v1", 100, || Ok(()))
            .expect("first");
        assert_eq!(first.backup, None);
        let second = wiki
            .write_page_with("s", "p", "v2", 100, || Ok(()))
            .expect("second");
        let backup = second.backup.expect("backup of v1");
        assert_eq!(fs::read_to_string(&backup).expect("backup text"), "v1");
        assert_eq!(
            wiki.read_page("s", "p").expect("read").as_deref(),
            Some("v2")
        );
        let third = wiki
            .write_page_with("s", "p", "v3", 100, || Ok(()))
            .expect("third, same millisecond");
        assert_ne!(
            third.backup.as_ref(),
            Some(&backup),
            "a same-millisecond backup does not overwrite the earlier one"
        );
        assert_eq!(fs::read_to_string(&backup).expect("still v1"), "v1");
    }

    #[test]
    fn a_crash_between_writing_and_replacing_leaves_the_original_page_and_no_temp_file() {
        let (_dir, wiki) = wiki();
        wiki.create_subject("s", "s").expect("create");
        wiki.write_page("s", "p", "original").expect("write");
        let err = wiki
            .write_page_with("s", "p", "half-done replacement", 5, || {
                Err(io::Error::other("simulated crash"))
            })
            .expect_err("aborted");
        assert!(err.to_string().contains("replace page"), "{err}");
        assert_eq!(
            wiki.read_page("s", "p").expect("read").as_deref(),
            Some("original")
        );
        let leftovers: Vec<_> = files_under(wiki.root())
            .into_iter()
            .filter(|f| f.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        assert_eq!(wiki.list_pages("s").expect("list"), ["p"]);
    }

    #[test]
    fn names_that_could_leave_the_folder_are_refused_everywhere() {
        let (dir, wiki) = wiki();
        wiki.create_subject("s", "s").expect("create");
        for slug in ["..", "../x", "a/b", "", ".hidden"] {
            assert!(wiki.write_page(slug, "p", "x").is_err(), "slug {slug:?}");
            assert!(wiki.list_pages(slug).is_err(), "slug {slug:?}");
        }
        for name in ["../escape", "a/b", "..", "", "sub\\dir"] {
            assert!(wiki.write_page("s", name, "x").is_err(), "name {name:?}");
            assert!(wiki.read_page("s", name).is_err(), "name {name:?}");
        }
        assert!(
            files_under(dir.path())
                .iter()
                .all(|f| f.starts_with("StudyWiki")),
            "nothing outside the wiki root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_never_followed_for_reads_writes_or_listing() {
        use std::os::unix::fs::symlink;
        let (dir, wiki) = wiki();
        wiki.create_subject("s", "s").expect("create");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("secret.md"), "do not touch").expect("secret");

        // A page file that is a link to a file outside.
        let pages = wiki.root().join("s").join("pages");
        symlink(outside.join("secret.md"), pages.join("link.md")).expect("link");
        assert!(wiki.read_page("s", "link").is_err());
        assert!(wiki.write_page("s", "link", "overwrite").is_err());
        assert_eq!(
            fs::read_to_string(outside.join("secret.md")).expect("secret"),
            "do not touch"
        );
        assert!(
            wiki.list_pages("s").expect("list").is_empty(),
            "the link is not a page"
        );

        // A pages folder that is a link to a folder outside.
        wiki.create_subject("t", "t").expect("create");
        let t_pages = wiki.root().join("t").join("pages");
        fs::remove_dir_all(&t_pages).expect("remove");
        symlink(&outside, &t_pages).expect("link dir");
        assert!(wiki.write_page("t", "p", "x").is_err());
        assert!(wiki.list_pages("t").is_err());
        assert!(!outside.join("p.md").exists());

        // A subject folder that is a link.
        symlink(&outside, wiki.root().join("u")).expect("link subject");
        assert!(wiki.create_subject("u", "u").is_err());
        assert!(!outside.join("pages").exists());
    }

    #[test]
    fn size_and_count_limits_hold() {
        let (_dir, wiki) = wiki();
        wiki.create_subject("s", "s").expect("create");
        assert!(
            wiki.write_page("s", "big", &"x".repeat(MAX_PAGE_BYTES + 1))
                .is_err()
        );
        assert!(
            wiki.write_page("s", "ok", &"x".repeat(MAX_PAGE_BYTES))
                .is_ok()
        );
        let pages = wiki.root().join("s").join("pages");
        for i in 0..MAX_PAGES_PER_SUBJECT - 1 {
            fs::write(pages.join(format!("p{i}.md")), "x").expect("fill");
        }
        assert_eq!(
            wiki.list_pages("s").expect("list").len(),
            MAX_PAGES_PER_SUBJECT
        );
        assert!(wiki.write_page("s", "one-too-many", "x").is_err());
        assert!(
            wiki.write_page("s", "ok", "rewriting an existing page is still fine")
                .is_ok()
        );
        fs::write(
            pages.join("huge.md"),
            vec![b'x'; usize::try_from(MAX_READ_BYTES).expect("fits") + 1],
        )
        .expect("huge");
        assert!(
            wiki.read_page("s", "huge").is_err(),
            "oversized files are not read"
        );
    }

    #[test]
    fn a_deleted_folder_reads_as_empty_and_a_write_rebuilds_it() {
        let (_dir, wiki) = wiki();
        wiki.create_subject("s", "s").expect("create");
        wiki.write_page("s", "p", "x").expect("write");
        fs::remove_dir_all(wiki.root().join("s")).expect("delete");
        assert!(!wiki.subject_dir_exists("s"));
        assert!(wiki.list_pages("s").expect("list").is_empty());
        assert_eq!(wiki.read_page("s", "p").expect("read"), None);
        wiki.write_page("s", "p", "again")
            .expect("rebuild on write");
        assert_eq!(
            wiki.read_page("s", "p").expect("read").as_deref(),
            Some("again")
        );
        fs::remove_dir_all(wiki.root()).expect("delete root");
        assert!(wiki.list_pages("s").expect("list").is_empty());
    }

    #[test]
    fn errors_name_the_operation_but_never_a_path() {
        let (_dir, wiki) = wiki();
        // The root is a file, so creating a folder under it fails with an OS error.
        fs::create_dir_all(wiki.root().parent().expect("parent")).expect("parent");
        fs::write(wiki.root(), "not a folder").expect("file");
        let err = wiki
            .create_subject("s", "s")
            .expect_err("fails")
            .to_string();
        assert!(
            err.contains("create folder") && !err.contains("StudyWiki"),
            "{err}"
        );
    }
}
