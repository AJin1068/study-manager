//! Wiki operations behind the commands. Database locks are held only for short reads and writes;
//! model calls run unlocked. Files change in exactly two places: creating a subject folder, and
//! `apply_ingest` after a preview.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::Serialize;

use super::ask::{check_answer, AnswerCheck};
use super::fs::{WikiFs, MAX_READ_BYTES};
use super::index::{self, ReindexReport};
use super::ingest::{chunk_text, IngestSession, SkippedFile, SourceKind, Step, BATCH_STEPS, MAX_INPUT_CHARS, MAX_STEPS};
use super::names;
use super::pdf::{extract_isolated, PdfExtractor};
use super::retrieve::rank_pages;
use crate::commands::lock_db;
use crate::error::{AppError, AppResult};
use crate::llm::config::LlmConfig;
use crate::llm::egress::{Prompt, PromptPage, MAX_ASK_PAGES, MAX_CHUNK_CHARS, MAX_QUESTION_CHARS};
use crate::llm::service::LlmService;
use crate::llm::{LlmRequest, PageDraft, Purpose};
use crate::remote::Clock;
use crate::store::wiki::{self as repo, PageRow, SourceRow, SubjectRow};

const MAX_PDF_BYTES: u64 = 20 * 1024 * 1024;
const MAX_FOLDER_FILES: usize = 200;
const MAX_FOLDER_DEPTH: usize = 2;
const MAX_SKIPPED_SHOWN: usize = 50;
const MAX_PREVIEWS: usize = 8;
const MAX_ASKS: usize = 16;
const MAX_NAME_INPUT_CHARS: usize = 80;
const EXCERPT_CHARS: usize = 120;
const ASK_MAX_TOKENS: u32 = 1_024;

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubjectView {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub archived_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub page_count: i64,
    /// The subject's folder was deleted by hand; writing a page rebuilds it.
    pub folder_missing: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PageContent {
    pub page_name: String,
    pub markdown: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProposalView {
    pub page_name: String,
    pub markdown: String,
    pub links: Vec<String>,
    /// `create`, or `append` to a page that already exists (its old text is backed up first).
    pub mode: &'static str,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IngestView {
    pub preview_id: Option<String>,
    pub subject_id: i64,
    /// `ready`, `partial` (more steps wait for `resume`), `stopped` (a failure; `stopped_by` says
    /// which), `empty` (nothing usable came out), or `unsupported` (nothing could be read).
    pub status: &'static str,
    pub processed_steps: usize,
    pub total_steps: usize,
    pub rejected_steps: usize,
    /// The local heading-based import produced the proposals because the LLM is off.
    pub used_fallback: bool,
    pub stopped_by: Option<String>,
    pub proposals: Vec<ProposalView>,
    pub skipped: Vec<SkippedFile>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WrittenPage {
    pub page_name: String,
    pub mode: &'static str,
    pub backed_up: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkippedPage {
    pub page_name: String,
    pub reason: &'static str,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub written: Vec<WrittenPage>,
    pub skipped: Vec<SkippedPage>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MatchView {
    pub page_name: String,
    pub excerpt: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AskOutcome {
    /// No page matched the question: no model was called.
    NotInWiki { hint: &'static str },
    Answer { ask_id: String, answer: String, citations: Vec<String>, dropped_citations: usize },
    /// The model answered without citing any page it was given; the answer was discarded.
    Ungrounded { matches: Vec<MatchView> },
    /// The model could not answer (off, unconfigured, over budget, failed): the pages that matched
    /// are shown instead.
    Unavailable { code: String, matches: Vec<MatchView> },
}

struct AskRecord {
    subject_id: i64,
    question: String,
    answer: String,
    citations: Vec<String>,
}

#[derive(Default)]
struct Hub {
    sessions: HashMap<String, Arc<Mutex<IngestSession>>>,
    session_order: VecDeque<String>,
    asks: HashMap<String, AskRecord>,
    ask_order: VecDeque<String>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub struct WikiService {
    db: Arc<Mutex<Connection>>,
    llm: Arc<LlmService>,
    clock: Arc<dyn Clock>,
    pdf: Arc<dyn PdfExtractor>,
    hub: Mutex<Hub>,
}

fn is_bare_url(text: &str) -> bool {
    let t = text.trim();
    !t.contains(char::is_whitespace) && (t.starts_with("http://") || t.starts_with("https://"))
}

fn file_name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn extension_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn excerpt(body: &str) -> String {
    body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or("").chars().take(EXCERPT_CHARS).collect()
}

/// The text of a page as written: the proposal, then its links, appended after existing text.
fn compose_page(existing: Option<&str>, draft: &PageDraft) -> String {
    let mut addition = draft.markdown.trim().to_string();
    if !draft.links.is_empty() {
        let links: Vec<String> = draft.links.iter().map(|l| format!("[[{l}]]")).collect();
        addition.push_str(&format!("\n\n관련: {}", links.join(" ")));
    }
    match existing {
        Some(old) => format!("{}\n\n{addition}\n", old.trim_end()),
        None => format!("{addition}\n"),
    }
}

impl WikiService {
    pub fn new(db: Arc<Mutex<Connection>>, llm: Arc<LlmService>, clock: Arc<dyn Clock>, pdf: Arc<dyn PdfExtractor>) -> Self {
        Self { db, llm, clock, pdf, hub: Mutex::new(Hub::default()) }
    }

    fn now(&self) -> AppResult<i64> {
        self.clock.now_ms()
    }

    pub(crate) fn wiki_fs(conn: &Connection) -> AppResult<WikiFs> {
        WikiFs::from_setting(&LlmConfig::load(conn)?.wiki_dir)
    }

    fn view(conn: &Connection, wiki: &WikiFs, row: SubjectRow) -> AppResult<SubjectView> {
        Ok(SubjectView {
            page_count: i64::try_from(repo::list_pages(conn, row.id)?.len()).unwrap_or(i64::MAX),
            folder_missing: !wiki.subject_dir_exists(&row.slug),
            id: row.id,
            slug: row.slug,
            name: row.name,
            archived_at_ms: row.archived_at_ms,
            created_at_ms: row.created_at_ms,
        })
    }

    fn clean_subject_name(name: &str) -> AppResult<String> {
        let clean = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if clean.is_empty() || clean.chars().count() > MAX_NAME_INPUT_CHARS {
            return Err(AppError::Invalid("the subject name must be 1 to 80 characters".into()));
        }
        Ok(clean)
    }

    fn active_subject(conn: &Connection, id: i64) -> AppResult<SubjectRow> {
        let row = repo::get_subject(conn, id)?;
        if row.archived_at_ms.is_some() {
            return Err(AppError::Invalid("the subject is archived".into()));
        }
        Ok(row)
    }

    fn active(&self, id: i64) -> AppResult<SubjectRow> {
        let conn = lock_db(&self.db)?;
        Self::active_subject(&conn, id)
    }

    // ---- subjects ----

    pub fn list_subjects(&self, include_archived: bool) -> AppResult<Vec<SubjectView>> {
        let conn = lock_db(&self.db)?;
        let wiki = Self::wiki_fs(&conn)?;
        repo::list_subjects(&conn, include_archived)?.into_iter().map(|row| Self::view(&conn, &wiki, row)).collect()
    }

    /// Creates a subject and its folder. A second subject with the same folder name is refused.
    pub fn create_subject(&self, name: &str) -> AppResult<SubjectView> {
        let name = Self::clean_subject_name(name)?;
        let slug = names::subject_slug(&name)?;
        let at_ms = self.now()?;
        let conn = lock_db(&self.db)?;
        if repo::find_subject_by_slug(&conn, &slug)?.is_some() {
            return Err(AppError::Invalid("a subject with the same folder name already exists".into()));
        }
        let wiki = Self::wiki_fs(&conn)?;
        wiki.create_subject(&slug, &name)?;
        let id = repo::insert_subject(&conn, &slug, &name, at_ms)?;
        Self::view(&conn, &wiki, repo::get_subject(&conn, id)?)
    }

    /// The subject for a name, created when missing and brought back when archived: what the
    /// plan form calls after a plan with a new subject is saved.
    pub fn ensure_subject(&self, name: &str) -> AppResult<SubjectView> {
        let clean = Self::clean_subject_name(name)?;
        let slug = names::subject_slug(&clean)?;
        let existing = {
            let conn = lock_db(&self.db)?;
            repo::find_subject_by_slug(&conn, &slug)?
        };
        match existing {
            None => self.create_subject(&clean),
            Some(row) => {
                let conn = lock_db(&self.db)?;
                if row.archived_at_ms.is_some() {
                    repo::set_archived(&conn, row.id, None)?;
                }
                let wiki = Self::wiki_fs(&conn)?;
                wiki.create_subject(&row.slug, &row.name)?;
                Self::view(&conn, &wiki, repo::get_subject(&conn, row.id)?)
            }
        }
    }

    /// Archiving hides the subject and keeps its folder; restoring brings it back. Archiving is
    /// refused while the study lock is held: it shrinks the quiz bank, which must not be possible
    /// from inside a locked block (the early-release gate reads the bank size).
    pub fn set_archived(&self, id: i64, archived: bool) -> AppResult<SubjectView> {
        let at_ms = self.now()?;
        let conn = lock_db(&self.db)?;
        if archived && crate::study::lock_held(&conn)? {
            return Err(AppError::Invalid("subjects cannot be archived while the lock is held".into()));
        }
        repo::set_archived(&conn, id, archived.then_some(at_ms))?;
        let wiki = Self::wiki_fs(&conn)?;
        Self::view(&conn, &wiki, repo::get_subject(&conn, id)?)
    }

    /// The wiki subject a plan's free-text subject belongs to.
    pub fn subject_for_plan(&self, plan_subject: &str) -> AppResult<Option<SubjectView>> {
        let conn = lock_db(&self.db)?;
        let Some(row) = subject_for_plan(&conn, plan_subject)? else { return Ok(None) };
        let wiki = Self::wiki_fs(&conn)?;
        Ok(Some(Self::view(&conn, &wiki, row)?))
    }

    // ---- pages ----

    pub fn list_pages(&self, subject_id: i64) -> AppResult<Vec<PageRow>> {
        let conn = lock_db(&self.db)?;
        repo::list_pages(&conn, subject_id)
    }

    pub fn list_sources(&self, subject_id: i64) -> AppResult<Vec<SourceRow>> {
        let conn = lock_db(&self.db)?;
        repo::list_sources(&conn, subject_id)
    }

    pub fn read_page(&self, subject_id: i64, page_name: &str) -> AppResult<PageContent> {
        let conn = lock_db(&self.db)?;
        let subject = repo::get_subject(&conn, subject_id)?;
        let wiki = Self::wiki_fs(&conn)?;
        let markdown = wiki
            .read_page(&subject.slug, page_name)?
            .ok_or_else(|| AppError::Invalid("the page does not exist".into()))?;
        Ok(PageContent { page_name: page_name.to_string(), markdown })
    }

    pub fn reindex(&self, subject_id: i64) -> AppResult<ReindexReport> {
        let at_ms = self.now()?;
        let conn = lock_db(&self.db)?;
        let subject = repo::get_subject(&conn, subject_id)?;
        let wiki = Self::wiki_fs(&conn)?;
        index::reindex(&conn, &wiki, &subject, at_ms)
    }

    // ---- ingest ----

    fn register(&self, session: IngestSession) -> (String, Arc<Mutex<IngestSession>>) {
        let id = uuid::Uuid::new_v4().to_string();
        let shared = Arc::new(Mutex::new(session));
        let mut hub = lock(&self.hub);
        hub.sessions.insert(id.clone(), Arc::clone(&shared));
        hub.session_order.push_back(id.clone());
        while hub.session_order.len() > MAX_PREVIEWS {
            if let Some(old) = hub.session_order.pop_front() {
                hub.sessions.remove(&old);
            }
        }
        (id, shared)
    }

    fn session(&self, preview_id: &str) -> AppResult<Arc<Mutex<IngestSession>>> {
        lock(&self.hub).sessions.get(preview_id).cloned().ok_or_else(|| AppError::Invalid("the preview no longer exists".into()))
    }

    fn drop_session(&self, preview_id: &str) {
        let mut hub = lock(&self.hub);
        hub.sessions.remove(preview_id);
        hub.session_order.retain(|id| id != preview_id);
    }

    fn ingest_view(&self, preview_id: Option<&str>, session: &IngestSession) -> AppResult<IngestView> {
        let (subject, wiki) = {
            let conn = lock_db(&self.db)?;
            (repo::get_subject(&conn, session.subject_id)?, Self::wiki_fs(&conn)?)
        };
        let mut proposals = Vec::new();
        for draft in session.proposals() {
            let exists = wiki.read_page(&subject.slug, &draft.page_name)?.is_some();
            proposals.push(ProposalView {
                page_name: draft.page_name.clone(),
                markdown: draft.markdown.clone(),
                links: draft.links.clone(),
                mode: if exists { "append" } else { "create" },
            });
        }
        let status = if session.total_steps() == 0 && proposals.is_empty() {
            "unsupported"
        } else if session.stopped_by().is_some() {
            "stopped"
        } else if !session.is_finished() {
            "partial"
        } else if proposals.is_empty() {
            "empty"
        } else {
            "ready"
        };
        Ok(IngestView {
            preview_id: preview_id.map(str::to_string),
            subject_id: session.subject_id,
            status,
            processed_steps: session.processed_steps(),
            total_steps: session.total_steps(),
            rejected_steps: session.rejected_steps(),
            used_fallback: session.used_fallback(),
            stopped_by: session.stopped_by().map(str::to_string),
            proposals,
            skipped: session.skipped.clone(),
        })
    }

    fn start(&self, mut session: IngestSession) -> AppResult<IngestView> {
        if session.total_steps() == 0 {
            return self.ingest_view(None, &session);
        }
        session.run(&self.llm, BATCH_STEPS);
        let (id, shared) = self.register(session);
        let guard = lock(&shared);
        self.ingest_view(Some(&id), &guard)
    }

    /// Pasted text, a note, or a memo about a link. The link itself is never fetched.
    pub fn ingest_text(&self, subject_id: i64, kind: SourceKind, label: &str, text: &str) -> AppResult<IngestView> {
        let subject = self.active(subject_id)?;
        if kind == SourceKind::Pdf {
            return Err(AppError::Invalid("PDF files are imported from a file".into()));
        }
        if text.trim().is_empty() {
            return Err(AppError::Invalid("there is no text to import".into()));
        }
        if kind == SourceKind::Link && is_bare_url(text) {
            return Err(AppError::Invalid("a link needs a memo about it: the app does not open links".into()));
        }
        if text.chars().count() > MAX_INPUT_CHARS {
            return Err(AppError::Invalid("the text is too long to import at once".into()));
        }
        let steps: Vec<Step> = chunk_text(text, MAX_CHUNK_CHARS).into_iter().map(Step::Chunk).collect();
        let default_label = match kind {
            SourceKind::Link => "링크 메모",
            SourceKind::Note => "메모",
            _ => "붙여넣기",
        };
        let label = label.trim();
        let label = if label.is_empty() { default_label } else { label };
        self.start(IngestSession::new(subject.id, &subject.name, kind, &label.chars().take(120).collect::<String>(), steps))
    }

    fn read_text_file(&self, path: &Path) -> Result<(SourceKind, String), &'static str> {
        let meta = std::fs::symlink_metadata(path).map_err(|_| "unreadable")?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err("not_a_file");
        }
        match extension_of(path).as_str() {
            "md" | "markdown" | "txt" => {
                if meta.len() > MAX_READ_BYTES {
                    return Err("too_large");
                }
                let bytes = std::fs::read(path).map_err(|_| "unreadable")?;
                String::from_utf8(bytes).map(|t| (SourceKind::Note, t)).map_err(|_| "not_text")
            }
            "pdf" => {
                if meta.len() > MAX_PDF_BYTES {
                    return Err("too_large");
                }
                let bytes = std::fs::read(path).map_err(|_| "unreadable")?;
                extract_isolated(&self.pdf, bytes).map(|t| (SourceKind::Pdf, t)).map_err(|e| e.code())
            }
            _ => Err("unsupported_type"),
        }
    }

    /// One `.md`, `.txt`, or `.pdf` file.
    pub fn ingest_file(&self, subject_id: i64, path: &str) -> AppResult<IngestView> {
        let subject = self.active(subject_id)?;
        let path = Path::new(path);
        if !path.is_absolute() {
            return Err(AppError::Invalid("the file path must be absolute".into()));
        }
        let name = file_name_of(path);
        match self.read_text_file(path) {
            Ok((kind, text)) => {
                let steps: Vec<Step> = chunk_text(&text, MAX_CHUNK_CHARS).into_iter().map(Step::Chunk).collect();
                if steps.len() > MAX_STEPS {
                    return Err(AppError::Invalid("the file is too long to import at once".into()));
                }
                self.start(IngestSession::new(subject.id, &subject.name, kind, &name, steps))
            }
            Err(reason) => {
                let mut session = IngestSession::new(subject.id, &subject.name, SourceKind::Note, &name, Vec::new());
                session.skipped.push(SkippedFile { name, reason });
                self.ingest_view(None, &session)
            }
        }
    }

    fn collect_files(dir: &Path, depth: usize, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if out.len() > MAX_FOLDER_FILES {
                return;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };
            if name.starts_with('.') || kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if depth < MAX_FOLDER_DEPTH {
                    Self::collect_files(&entry.path(), depth + 1, out);
                }
            } else if kind.is_file() {
                out.push(entry.path());
            }
        }
    }

    /// Every readable `.md`, `.txt`, and `.pdf` file in a folder (two levels deep). Files that
    /// cannot be read are listed with the reason instead of failing the whole import.
    pub fn ingest_folder(&self, subject_id: i64, dir: &str) -> AppResult<IngestView> {
        let subject = self.active(subject_id)?;
        let dir = Path::new(dir);
        if !dir.is_absolute() {
            return Err(AppError::Invalid("the folder path must be absolute".into()));
        }
        let meta = std::fs::symlink_metadata(dir).map_err(|_| AppError::Invalid("the folder cannot be opened".into()))?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(AppError::Invalid("the path is not a folder".into()));
        }
        let mut files = Vec::new();
        Self::collect_files(dir, 0, &mut files);
        let mut skipped = Vec::new();
        let mut steps = Vec::new();
        let mut imported = 0usize;
        if files.len() > MAX_FOLDER_FILES {
            files.truncate(MAX_FOLDER_FILES);
            skipped.push(SkippedFile { name: "…".into(), reason: "too_many_files" });
        }
        for path in files {
            if steps.len() >= MAX_STEPS {
                skipped.push(SkippedFile { name: file_name_of(&path), reason: "too_many_steps" });
                continue;
            }
            match self.read_text_file(&path) {
                Ok((_, text)) => {
                    let room = MAX_STEPS - steps.len();
                    let chunks = chunk_text(&text, MAX_CHUNK_CHARS);
                    if chunks.len() > room {
                        skipped.push(SkippedFile { name: file_name_of(&path), reason: "too_many_steps" });
                        continue;
                    }
                    steps.extend(chunks.into_iter().map(Step::Chunk));
                    imported += 1;
                }
                Err("unsupported_type") => {}
                Err(reason) => {
                    if skipped.len() < MAX_SKIPPED_SHOWN {
                        skipped.push(SkippedFile { name: file_name_of(&path), reason });
                    }
                }
            }
        }
        let label = format!("폴더 {} ({imported}개 파일)", file_name_of(dir));
        let mut session = IngestSession::new(subject.id, &subject.name, SourceKind::Note, &label, steps);
        session.skipped = skipped;
        self.start(session)
    }

    /// A table-of-contents draft from the subject name, for a subject with no material yet.
    pub fn outline(&self, subject_id: i64) -> AppResult<IngestView> {
        let subject = self.active(subject_id)?;
        let mut session = IngestSession::new(subject.id, &subject.name, SourceKind::Note, "목차 초안", vec![Step::Outline]);
        session.record_source = false;
        self.start(session)
    }

    /// Continues a preview that stopped at its batch limit, the budget, or a failure.
    pub fn resume(&self, preview_id: &str) -> AppResult<IngestView> {
        let shared = self.session(preview_id)?;
        let mut session = lock(&shared);
        session.run(&self.llm, BATCH_STEPS);
        self.ingest_view(Some(preview_id), &session)
    }

    /// Throws a preview away. Nothing was written, so nothing is undone.
    pub fn cancel(&self, preview_id: &str) {
        self.drop_session(preview_id);
    }

    /// Writes the chosen proposals (all when `selected` is `None`). An existing page gets the new
    /// text appended after a backup of the old one.
    pub fn apply_ingest(&self, preview_id: &str, selected: Option<Vec<String>>) -> AppResult<ApplyReport> {
        let shared = self.session(preview_id)?;
        let session = lock(&shared);
        let at_ms = self.now()?;
        let conn = lock_db(&self.db)?;
        let subject = Self::active_subject(&conn, session.subject_id)?;
        let wiki = Self::wiki_fs(&conn)?;
        let wanted: Vec<String> = selected.unwrap_or_else(|| session.proposals().map(|d| d.page_name.clone()).collect());
        let (mut written, mut skipped) = (Vec::new(), Vec::new());
        for name in wanted {
            let Some(draft) = session.proposal(&name) else {
                skipped.push(SkippedPage { page_name: name, reason: "not_in_preview" });
                continue;
            };
            let existing = match wiki.read_page(&subject.slug, &name) {
                Ok(text) => text,
                Err(_) => {
                    skipped.push(SkippedPage { page_name: name, reason: "unreadable" });
                    continue;
                }
            };
            let content = compose_page(existing.as_deref(), draft);
            match wiki.write_page(&subject.slug, &name, &content) {
                Ok(report) => {
                    index::index_page(&conn, &subject, &name, &content, at_ms)?;
                    tracing::info!(event = "wiki_write", op = "ingest", bytes = report.bytes, backup = report.backup.is_some());
                    written.push(WrittenPage { page_name: name, mode: if existing.is_some() { "append" } else { "create" }, backed_up: report.backup.is_some() });
                }
                Err(_) => skipped.push(SkippedPage { page_name: name, reason: "write_failed" }),
            }
        }
        if session.record_source {
            let status = if written.is_empty() { "nothing_written" } else { "ingested" };
            repo::insert_source(&conn, subject.id, session.kind.as_str(), &session.source_ref, status, at_ms)?;
        }
        drop(conn);
        drop(session);
        self.drop_session(preview_id);
        Ok(ApplyReport { written, skipped })
    }

    // ---- ask ----

    fn matches_view(ranked: &[(String, String)]) -> Vec<MatchView> {
        ranked.iter().map(|(name, body)| MatchView { page_name: name.clone(), excerpt: excerpt(body) }).collect()
    }

    /// Answers from the subject's own pages. The pages are found locally; only the best few,
    /// together with the question, go to the model, and only citations of those pages survive.
    pub fn ask(&self, subject_id: i64, question: &str) -> AppResult<AskOutcome> {
        let question = question.split_whitespace().collect::<Vec<_>>().join(" ");
        if question.is_empty() || question.chars().count() > MAX_QUESTION_CHARS {
            return Err(AppError::Invalid(format!("the question must be 1 to {MAX_QUESTION_CHARS} characters")));
        }
        let (subject, pages) = {
            let conn = lock_db(&self.db)?;
            let subject = Self::active_subject(&conn, subject_id)?;
            let wiki = Self::wiki_fs(&conn)?;
            let mut pages = Vec::new();
            for name in wiki.list_pages(&subject.slug)? {
                if let Ok(Some(body)) = wiki.read_page(&subject.slug, &name) {
                    pages.push((name, body));
                }
            }
            (subject, pages)
        };
        let ranked = rank_pages(&question, &pages, MAX_ASK_PAGES);
        if ranked.is_empty() {
            return Ok(AskOutcome::NotInWiki { hint: "위키에서 찾지 못했어요. 자료를 먼저 넣어 보세요." });
        }
        let top: Vec<(String, String)> = ranked.iter().map(|r| pages[r.index].clone()).collect();
        let prompt_pages: Vec<PromptPage> = top.iter().map(|(name, body)| PromptPage { name: name.clone(), body: body.clone() }).collect();
        let request = LlmRequest { purpose: Purpose::WikiAsk, prompt: Prompt::ask(&subject.name, &question, prompt_pages), max_tokens: ASK_MAX_TOKENS };
        let response = match self.llm.complete(&request) {
            Ok(response) => response,
            Err(err) => return Ok(AskOutcome::Unavailable { code: err.code().to_string(), matches: Self::matches_view(&top) }),
        };
        let retrieved: Vec<String> = top.iter().map(|(name, _)| name.clone()).collect();
        match check_answer(&response.text, &retrieved) {
            AnswerCheck::Grounded { answer, citations, dropped } => {
                let ask_id = uuid::Uuid::new_v4().to_string();
                let mut hub = lock(&self.hub);
                hub.asks.insert(ask_id.clone(), AskRecord { subject_id, question, answer: answer.clone(), citations: citations.clone() });
                hub.ask_order.push_back(ask_id.clone());
                while hub.ask_order.len() > MAX_ASKS {
                    if let Some(old) = hub.ask_order.pop_front() {
                        hub.asks.remove(&old);
                    }
                }
                Ok(AskOutcome::Answer { ask_id, answer, citations, dropped_citations: dropped })
            }
            AnswerCheck::NotInWiki => Ok(AskOutcome::NotInWiki { hint: "위키에서 찾지 못했어요. 자료를 먼저 넣어 보세요." }),
            AnswerCheck::Ungrounded | AnswerCheck::Malformed => Ok(AskOutcome::Ungrounded { matches: Self::matches_view(&top) }),
        }
    }

    /// A preview that saves an answer as a page (written only by `apply_ingest`).
    pub fn save_answer_preview(&self, ask_id: &str) -> AppResult<IngestView> {
        let (subject_id, question, answer, citations) = {
            let hub = lock(&self.hub);
            let record = hub.asks.get(ask_id).ok_or_else(|| AppError::Invalid("the answer is no longer available".into()))?;
            (record.subject_id, record.question.clone(), record.answer.clone(), record.citations.clone())
        };
        let subject = self.active(subject_id)?;
        let prefix: String = question.chars().take(24).collect();
        let page_name = names::page_name(&format!("질문 {prefix}")).unwrap_or_else(|| "질문 답변".to_string());
        let draft = PageDraft { page_name, markdown: format!("# {question}\n\n{answer}"), links: citations };
        let session = IngestSession::from_drafts(subject.id, &subject.name, SourceKind::Note, "묻기 답변", vec![draft]);
        let (id, shared) = self.register(session);
        let guard = lock(&shared);
        self.ingest_view(Some(&id), &guard)
    }
}

/// The wiki subject a plan's free-text subject belongs to: same name ignoring case, else same
/// folder name. Archived subjects do not match.
pub fn subject_for_plan(conn: &Connection, plan_subject: &str) -> AppResult<Option<SubjectRow>> {
    let name = plan_subject.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Ok(None);
    }
    if let Some(row) = repo::find_active_subject_by_name(conn, &name)? {
        return Ok(Some(row));
    }
    let Ok(slug) = names::subject_slug(&name) else { return Ok(None) };
    Ok(repo::find_subject_by_slug(conn, &slug)?.filter(|row| row.archived_at_ms.is_none()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::config::{update_setting, LlmSettingKey};
    use crate::llm::mock::MockStep;
    use crate::llm::service::tests::{mock_harness, Harness};
    use crate::settings::SettingValue;
    use crate::testutil::TempDir;
    use crate::wiki::pdf::NoPdfExtractor;

    struct Rig {
        h: Harness,
        wiki: WikiService,
        dir: TempDir,
    }

    fn rig_with(h: Harness) -> Rig {
        let dir = TempDir::new("wiki-service");
        {
            let mut conn = h.db.lock().expect("db");
            let root = dir.path().join("wiki");
            update_setting(&mut conn, LlmSettingKey::WikiDir, SettingValue::Text(root.to_string_lossy().into_owned()), 1).expect("dir");
        }
        let wiki = WikiService::new(Arc::clone(&h.db), Arc::clone(&h.service), h.clock.clone(), Arc::new(NoPdfExtractor));
        Rig { h, wiki, dir }
    }

    fn rig() -> Rig {
        rig_with(mock_harness())
    }

    /// LLM switched off entirely (the default state of a fresh install).
    fn rig_off() -> Rig {
        let h = mock_harness();
        {
            let mut conn = h.db.lock().expect("db");
            update_setting(&mut conn, LlmSettingKey::Enabled, SettingValue::Bool(false), 2).expect("off");
        }
        rig_with(h)
    }

    fn page_on_disk(r: &Rig, subject: &SubjectView, page: &str) -> Option<String> {
        std::fs::read_to_string(r.dir.path().join("wiki").join(&subject.slug).join("pages").join(format!("{page}.md"))).ok()
    }

    #[test]
    fn subjects_are_created_listed_archived_and_restored() {
        let r = rig();
        let math = r.wiki.create_subject("  수학   기초 ").expect("create");
        assert_eq!(math.name, "수학 기초");
        assert!(!math.folder_missing);
        assert!(r.wiki.create_subject("수학 기초").is_err(), "same folder name twice");
        assert!(r.wiki.create_subject("   ").is_err());
        assert!(r.wiki.create_subject("..").is_err(), "path-like names are refused");
        let archived = r.wiki.set_archived(math.id, true).expect("archive");
        assert!(archived.archived_at_ms.is_some());
        assert!(r.wiki.list_subjects(false).expect("list").is_empty());
        assert_eq!(r.wiki.list_subjects(true).expect("list").len(), 1);
        assert!(r.wiki.ingest_text(math.id, SourceKind::Paste, "", "# a\nb").is_err(), "archived subjects take no material");
        assert!(r.wiki.set_archived(math.id, false).expect("restore").archived_at_ms.is_none());
    }

    #[test]
    fn ensure_subject_reuses_restores_and_rebuilds_a_deleted_folder() {
        let r = rig();
        let first = r.wiki.ensure_subject("물리").expect("ensure");
        assert_eq!(r.wiki.ensure_subject("물리").expect("again").id, first.id);
        r.wiki.set_archived(first.id, true).expect("archive");
        assert!(r.wiki.ensure_subject("물리").expect("restore").archived_at_ms.is_none());
        std::fs::remove_dir_all(r.dir.path().join("wiki").join(&first.slug)).expect("delete folder");
        assert!(r.wiki.list_subjects(false).expect("list")[0].folder_missing);
        let rebuilt = r.wiki.ensure_subject("물리").expect("rebuild");
        assert!(!rebuilt.folder_missing);
    }

    #[test]
    fn archiving_is_refused_while_the_lock_is_held_but_restoring_is_not() {
        let r = rig();
        let math = r.wiki.create_subject("수학").expect("create");
        crate::testutil::set_lock_for_test(&r.h.db.lock().expect("db"), true);
        assert!(r.wiki.set_archived(math.id, true).is_err(), "the quiz bank must not shrink inside a locked block");
        assert!(r.wiki.list_subjects(false).expect("list").iter().any(|s| s.id == math.id && s.archived_at_ms.is_none()));
        crate::testutil::set_lock_for_test(&r.h.db.lock().expect("db"), false);
        r.wiki.set_archived(math.id, true).expect("archive when free");
        crate::testutil::set_lock_for_test(&r.h.db.lock().expect("db"), true);
        assert!(r.wiki.set_archived(math.id, false).expect("restoring only grows the bank").archived_at_ms.is_none());
    }

    #[test]
    fn a_plan_subject_finds_its_wiki_subject() {
        let r = rig();
        let subject = r.wiki.create_subject("Linear Algebra").expect("create");
        assert_eq!(r.wiki.subject_for_plan(" linear  algebra ").expect("find").map(|s| s.id), Some(subject.id));
        assert!(r.wiki.subject_for_plan("other").expect("find").is_none());
        r.wiki.set_archived(subject.id, true).expect("archive");
        assert!(r.wiki.subject_for_plan("Linear Algebra").expect("find").is_none());
    }

    #[test]
    fn ingest_previews_first_and_writes_only_on_apply() {
        let r = rig();
        let s = r.wiki.create_subject("수학").expect("create");
        let view = r.wiki.ingest_text(s.id, SourceKind::Paste, "", "# 미분\n도함수는 변화율이다.\n# 적분\n넓이를 구한다.").expect("ingest");
        assert_eq!(view.status, "ready");
        assert!(!view.used_fallback);
        assert_eq!(view.proposals.iter().map(|p| p.page_name.as_str()).collect::<Vec<_>>(), ["미분", "적분"]);
        assert!(page_on_disk(&r, &s, "미분").is_none(), "a preview writes nothing");
        let id = view.preview_id.expect("preview id");
        let report = r.wiki.apply_ingest(&id, Some(vec!["미분".into(), "없는 페이지".into()])).expect("apply");
        assert_eq!(report.written.len(), 1);
        assert_eq!(report.skipped, vec![SkippedPage { page_name: "없는 페이지".into(), reason: "not_in_preview" }]);
        assert!(page_on_disk(&r, &s, "미분").expect("page").contains("도함수"));
        assert!(page_on_disk(&r, &s, "적분").is_none(), "unselected proposals are not written");
        assert_eq!(r.wiki.list_pages(s.id).expect("pages").len(), 1);
        assert_eq!(r.wiki.list_sources(s.id).expect("sources").len(), 1);
        assert!(r.wiki.apply_ingest(&id, None).is_err(), "a preview is single-use");
    }

    #[test]
    fn applying_onto_an_existing_page_appends_and_backs_up() {
        let r = rig();
        let s = r.wiki.create_subject("수학").expect("create");
        let ingest = |text: &str| {
            let view = r.wiki.ingest_text(s.id, SourceKind::Paste, "", text).expect("ingest");
            let mode = view.proposals[0].mode;
            (r.wiki.apply_ingest(&view.preview_id.expect("id"), None).expect("apply"), mode)
        };
        let (_, first_mode) = ingest("# 미분\n처음 내용");
        assert_eq!(first_mode, "create");
        let (report, second_mode) = ingest("# 미분\n새 내용");
        assert_eq!(second_mode, "append");
        assert!(report.written[0].backed_up);
        let text = page_on_disk(&r, &s, "미분").expect("page");
        assert!(text.contains("처음 내용") && text.contains("새 내용"));
    }

    #[test]
    fn with_the_llm_off_the_local_import_still_proposes_pages_and_calls_nothing() {
        let r = rig_off();
        let s = r.wiki.create_subject("수학").expect("create");
        let view = r.wiki.ingest_text(s.id, SourceKind::Note, "", "# 미분\n내용").expect("ingest");
        assert!(view.used_fallback);
        assert_eq!(view.status, "ready");
        assert_eq!(r.h.remote.mock_llm().call_count(), 0);
        let rows: i64 = r.h.db.lock().expect("db").query_row("SELECT COUNT(*) FROM llm_usage", [], |row| row.get(0)).expect("count");
        assert_eq!(rows, 0);
    }

    #[test]
    fn a_bare_link_is_refused_but_a_link_with_a_memo_is_taken() {
        let r = rig();
        let s = r.wiki.create_subject("수학").expect("create");
        assert!(r.wiki.ingest_text(s.id, SourceKind::Link, "", " https://example.com/page ").is_err());
        let view = r.wiki.ingest_text(s.id, SourceKind::Link, "강의", "https://example.com 강의 요약\n미분의 정의").expect("memo");
        assert_eq!(view.status, "ready");
        assert!(r.wiki.ingest_text(s.id, SourceKind::Paste, "", "   ").is_err());
        assert!(r.wiki.ingest_text(s.id, SourceKind::Pdf, "", "x").is_err());
    }

    #[test]
    fn files_and_folders_are_read_with_reasons_for_what_is_skipped() {
        let r = rig();
        let s = r.wiki.create_subject("수학").expect("create");
        let src = r.dir.path().join("src");
        std::fs::create_dir_all(src.join("sub")).expect("dirs");
        std::fs::write(src.join("a.md"), "# 미분\n내용").expect("write");
        std::fs::write(src.join("sub").join("b.txt"), "적분 메모").expect("write");
        std::fs::write(src.join("c.pdf"), b"%PDF-1.4").expect("write");
        std::fs::write(src.join("d.bin"), [0u8, 1]).expect("write");
        std::fs::write(src.join("e.md"), [0xffu8, 0xfe]).expect("write");
        let view = r.wiki.ingest_folder(s.id, &src.to_string_lossy()).expect("folder");
        assert_eq!(view.status, "ready");
        let reasons: Vec<(&str, &str)> = view.skipped.iter().map(|f| (f.name.as_str(), f.reason)).collect();
        assert!(reasons.contains(&("e.md", "not_text")), "{reasons:?}");
        assert!(reasons.iter().any(|(n, _)| *n == "c.pdf"), "an unreadable PDF is listed, not fatal: {reasons:?}");
        assert!(!reasons.iter().any(|(n, _)| *n == "d.bin"), "other file types are ignored silently");

        let one = r.wiki.ingest_file(s.id, &src.join("c.pdf").to_string_lossy()).expect("pdf file");
        assert_eq!(one.status, "unsupported");
        assert_eq!(one.skipped.len(), 1);
        assert!(r.wiki.ingest_file(s.id, "relative.md").is_err());
        assert!(r.wiki.ingest_folder(s.id, &src.join("a.md").to_string_lossy()).is_err(), "a file is not a folder");
    }

    #[test]
    fn an_outline_proposes_starter_pages_from_the_name_and_records_no_source() {
        let r = rig();
        let s = r.wiki.create_subject("해부학").expect("create");
        let view = r.wiki.outline(s.id).expect("outline");
        assert_eq!(view.status, "ready");
        assert_eq!(view.proposals.len(), 3);
        r.wiki.apply_ingest(&view.preview_id.expect("id"), None).expect("apply");
        assert_eq!(r.wiki.list_pages(s.id).expect("pages").len(), 3);
        assert!(r.wiki.list_sources(s.id).expect("sources").is_empty());
    }

    #[test]
    fn a_bad_model_answer_is_rejected_and_nothing_is_proposed() {
        let r = rig();
        let s = r.wiki.create_subject("수학").expect("create");
        r.h.remote.mock_llm().push(MockStep::Text("not json at all".into()));
        let view = r.wiki.ingest_text(s.id, SourceKind::Paste, "", "# 미분\n내용").expect("ingest");
        assert_eq!(view.status, "empty");
        assert_eq!(view.rejected_steps, 1);
        assert!(view.proposals.is_empty());
    }

    #[test]
    fn ask_answers_with_citations_from_the_retrieved_pages_only() {
        let r = rig();
        let s = r.wiki.create_subject("수학").expect("create");
        let view = r.wiki.ingest_text(s.id, SourceKind::Paste, "", "# 미분\n도함수는 변화율이다.\n# 적분\n넓이를 구한다.").expect("ingest");
        r.wiki.apply_ingest(&view.preview_id.expect("id"), None).expect("apply");
        match r.wiki.ask(s.id, "미분 도함수").expect("ask") {
            AskOutcome::Answer { citations, answer, .. } => {
                assert!(citations.contains(&"미분".to_string()), "{citations:?}");
                assert!(!answer.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
        let sent = r.h.remote.mock_llm().calls().into_iter().filter(|c| c.purpose == Purpose::WikiAsk).count();
        assert_eq!(sent, 1);
        assert!(matches!(r.wiki.ask(s.id, "전혀 무관한 양자역학 질문").expect("ask"), AskOutcome::NotInWiki { .. }));
        assert_eq!(r.h.remote.mock_llm().calls().into_iter().filter(|c| c.purpose == Purpose::WikiAsk).count(), 1, "no match means no model call");
        assert!(r.wiki.ask(s.id, "  ").is_err());
    }

    #[test]
    fn an_ungrounded_or_unavailable_answer_falls_back_to_the_matching_pages() {
        let r = rig();
        let s = r.wiki.create_subject("수학").expect("create");
        let view = r.wiki.ingest_text(s.id, SourceKind::Paste, "", "# 미분\n도함수는 변화율이다.").expect("ingest");
        r.wiki.apply_ingest(&view.preview_id.expect("id"), None).expect("apply");
        r.h.remote.mock_llm().push(MockStep::Text(r#"{"answer":"지어낸 답","citations":["없는 페이지"]}"#.into()));
        match r.wiki.ask(s.id, "미분 도함수").expect("ask") {
            AskOutcome::Ungrounded { matches } => assert_eq!(matches[0].page_name, "미분"),
            other => panic!("unexpected {other:?}"),
        }
        r.h.remote.mock_llm().push(MockStep::Fail(crate::llm::LlmError::Overloaded));
        match r.wiki.ask(s.id, "미분 도함수").expect("ask") {
            AskOutcome::Unavailable { code, matches } => {
                assert_eq!(code, "llm_overloaded");
                assert_eq!(matches[0].page_name, "미분");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_saved_answer_is_a_preview_like_any_other_ingest() {
        let r = rig();
        let s = r.wiki.create_subject("수학").expect("create");
        let view = r.wiki.ingest_text(s.id, SourceKind::Paste, "", "# 미분\n도함수는 변화율이다.").expect("ingest");
        r.wiki.apply_ingest(&view.preview_id.expect("id"), None).expect("apply");
        let AskOutcome::Answer { ask_id, .. } = r.wiki.ask(s.id, "미분 도함수").expect("ask") else { panic!("no answer") };
        let preview = r.wiki.save_answer_preview(&ask_id).expect("preview");
        assert_eq!(preview.proposals.len(), 1);
        assert_eq!(r.wiki.list_pages(s.id).expect("pages").len(), 1, "nothing is written by the preview");
        r.wiki.apply_ingest(&preview.preview_id.expect("id"), None).expect("apply");
        assert_eq!(r.wiki.list_pages(s.id).expect("pages").len(), 2);
        assert!(r.wiki.save_answer_preview("unknown").is_err());
    }

    #[test]
    fn cancelling_a_preview_discards_it() {
        let r = rig();
        let s = r.wiki.create_subject("수학").expect("create");
        let view = r.wiki.ingest_text(s.id, SourceKind::Paste, "", "# 미분\n내용").expect("ingest");
        let id = view.preview_id.expect("id");
        r.wiki.cancel(&id);
        assert!(r.wiki.apply_ingest(&id, None).is_err());
        assert!(r.wiki.list_pages(s.id).expect("pages").is_empty());
    }

    #[test]
    fn wire_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/wiki-contract.json");
        let expected: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let actual = serde_json::json!({
            "subject": SubjectView { id: 1, slug: "math".into(), name: "Math".into(), archived_at_ms: None, created_at_ms: 1_773_000_000_000, page_count: 2, folder_missing: false },
            "page": PageRow { id: 5, subject_id: 1, page_name: "Derivative".into(), rel_path: "math/pages/Derivative.md".into(), content_hash: "00ff".into(), updated_at_ms: 1_773_000_001_000, quiz_total: 0, quiz_correct: 0 },
            "ingest": IngestView {
                preview_id: Some("p-1".into()),
                subject_id: 1,
                status: "partial",
                processed_steps: 6,
                total_steps: 9,
                rejected_steps: 1,
                used_fallback: false,
                stopped_by: None,
                proposals: vec![ProposalView { page_name: "Derivative".into(), markdown: "# Derivative".into(), links: vec!["Integral".into()], mode: "append" }],
                skipped: vec![SkippedFile { name: "x.bin".into(), reason: "not_text" }],
            },
            "applyReport": ApplyReport {
                written: vec![WrittenPage { page_name: "Derivative".into(), mode: "create", backed_up: false }],
                skipped: vec![SkippedPage { page_name: "Gone".into(), reason: "not_in_preview" }],
            },
            "askOutcomes": [
                AskOutcome::NotInWiki { hint: "h" },
                AskOutcome::Answer { ask_id: "a-1".into(), answer: "x".into(), citations: vec!["Derivative".into()], dropped_citations: 1 },
                AskOutcome::Ungrounded { matches: vec![MatchView { page_name: "Derivative".into(), excerpt: "e".into() }] },
                AskOutcome::Unavailable { code: "llm_overloaded".into(), matches: vec![] },
            ],
            "llmStatus": crate::llm::service::LlmStatus {
                enabled: true,
                judge_enabled: false,
                model: "m".into(),
                mode: crate::remote::IntegrationMode::Mock,
                daily_budget_tokens: 200_000,
                used_today_tokens: 40,
                by_purpose: vec![crate::store::llm::PurposeUsage { purpose: "wiki_ask".into(), calls: 2, tokens: 40 }],
            },
        });
        assert_eq!(actual, expected);
    }
}
