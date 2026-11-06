//! Session retrospective merge and wiki lint behind the commands.
//!
//! After a run the user may write what they learned. The note is stored locally (`session_retro`),
//! then the LLM folds it into one wiki page. Nothing replaces a page until the merged text has
//! passed the length checks of `retro::check_merge` and the user accepted the diff (or switched on
//! auto-apply); every replacement backs the old page up first and can be undone with one call.
//! If the LLM cannot answer, the note stays `pending` and `retry` runs it again later.
//!
//! The database lock is never held while the model is called.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::fs::WikiFs;
use super::index;
use super::lint::{self, LintFinding};
use super::names;
use super::retrieve::rank_pages;
use super::retro::{check_merge, diff_lines, parse_advice, parse_merge, DiffLine, Suggestion};
use crate::commands::lock_db;
use crate::error::{AppError, AppResult};
use crate::llm::config::LlmConfig;
use crate::llm::egress::{Prompt, PromptPage};
use crate::llm::service::LlmService;
use crate::llm::{LlmRequest, Purpose};
use crate::remote::Clock;
use crate::settings::{self, SettingSpec, SettingValue, UpdateOutcome, ValueKind};
use crate::store::retro::{self as repo, RetroRow, RetroStatus};
use crate::store::wiki::{self as wiki_repo, SubjectRow};
use crate::study_context;

/// The rolling page a retrospective lands on when no existing page matches it.
pub const DEFAULT_PAGE: &str = "회고 노트";
pub const MAX_NOTE_CHARS: usize = 2_000;
const MERGE_MAX_TOKENS: u32 = 4_096;
const LINT_MAX_TOKENS: u32 = 1_024;
const MAX_PREVIEWS: usize = 8;

/// Merge without asking for the diff first. Off by default.
pub const AUTO_APPLY: SettingSpec =
    SettingSpec { name: "retro.auto_apply", category: "retro", kind: ValueKind::Bool, default: "false", tighten_to: Some("false") };

pub fn auto_apply(conn: &Connection) -> AppResult<bool> {
    match settings::read_value(conn, &AUTO_APPLY)? {
        SettingValue::Bool(v) => Ok(v),
        other => Err(AppError::Internal(format!("{} has the wrong type: {other:?}", AUTO_APPLY.name))),
    }
}

pub fn set_auto_apply(conn: &mut Connection, value: bool, at_ms: i64) -> AppResult<UpdateOutcome> {
    settings::apply_setting(conn, &AUTO_APPLY, &SettingValue::Bool(value), at_ms, || Ok(()))
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RetroOutcome {
    /// A merged page waits for the user's decision: `apply_retro` writes it.
    Preview { preview_id: String, run_id: i64, subject_id: i64, page_name: String, created: bool, diff: Vec<DiffLine> },
    /// The page was written.
    Applied { run_id: i64, subject_id: i64, page_name: String, created: bool, backed_up: bool },
    /// Nothing changed; the note is kept and `retry_retro` can run it again. `code` says why.
    Pending { run_id: i64, code: String },
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RetroView {
    pub run_id: i64,
    pub status: &'static str,
    pub subject_id: Option<i64>,
    pub page_name: Option<String>,
    pub attempts: i64,
    pub last_code: Option<String>,
    /// The merge replaced an existing page, so its backup can be restored.
    pub can_undo: bool,
    pub created_at_ms: i64,
    pub merged_at_ms: Option<i64>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LintReport {
    pub subject_id: i64,
    pub pages: usize,
    /// Found by local rules; present whether or not the LLM answers.
    pub findings: Vec<LintFinding>,
    pub suggestions: Vec<Suggestion>,
    /// Why there is no advice (LLM off, failed, or nothing to advise on); `None` when it came.
    pub advice_code: Option<String>,
}

struct Prepared {
    run_id: i64,
    subject_id: i64,
    page_name: String,
    /// The page text the merge was built from; `None` when the page did not exist.
    original: Option<String>,
    merged: String,
}

#[derive(Default)]
struct Hub {
    previews: HashMap<String, Prepared>,
    order: VecDeque<String>,
}

pub struct RetroService {
    db: Arc<Mutex<Connection>>,
    llm: Arc<LlmService>,
    clock: Arc<dyn Clock>,
    hub: Mutex<Hub>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn clean_note(raw: &str) -> AppResult<String> {
    let note: String = raw.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect();
    let note = note.trim().to_string();
    if note.is_empty() || note.chars().count() > MAX_NOTE_CHARS {
        return Err(AppError::Invalid(format!("the retrospective must be 1 to {MAX_NOTE_CHARS} characters")));
    }
    Ok(note)
}

fn view(row: &RetroRow) -> RetroView {
    RetroView {
        run_id: row.run_id,
        status: row.status.as_str(),
        subject_id: row.subject_id,
        page_name: row.page_name.clone(),
        attempts: row.attempts,
        last_code: row.last_code.clone(),
        can_undo: row.status == RetroStatus::Merged && row.backup_name.is_some(),
        created_at_ms: row.created_at_ms,
        merged_at_ms: row.merged_at_ms,
    }
}

impl RetroService {
    pub fn new(db: Arc<Mutex<Connection>>, llm: Arc<LlmService>, clock: Arc<dyn Clock>) -> Self {
        Self { db, llm, clock, hub: Mutex::new(Hub::default()) }
    }

    fn wiki_fs(conn: &Connection) -> AppResult<WikiFs> {
        WikiFs::from_setting(&LlmConfig::load(conn)?.wiki_dir)
    }

    fn active_subject(conn: &Connection, id: i64) -> AppResult<SubjectRow> {
        let subject = wiki_repo::get_subject(conn, id)?;
        if subject.archived_at_ms.is_some() {
            return Err(AppError::Invalid("the subject is archived".into()));
        }
        Ok(subject)
    }

    // ---- retrospective ----

    /// Stores the note for a finished run and prepares the merge. `page_name` of `None` picks the
    /// best matching page of the subject by local keywords (the note and, when the study context
    /// is on, the labels of the run), else the rolling `회고 노트` page.
    pub fn submit(&self, run_id: i64, subject_id: i64, page_name: Option<&str>, note: &str) -> AppResult<RetroOutcome> {
        let note = clean_note(note)?;
        let at_ms = self.clock.now_ms()?;
        {
            let conn = lock_db(&self.db)?;
            let run_exists: Option<i64> =
                conn.query_row("SELECT id FROM plan_runs WHERE id = ?1", [run_id], |r| r.get(0)).optional()?;
            if run_exists.is_none() {
                return Err(AppError::Invalid("unknown run".into()));
            }
            if repo::get(&conn, run_id)?.is_some_and(|r| r.status == RetroStatus::Merged) {
                return Err(AppError::Invalid("this retrospective is already merged".into()));
            }
            let subject = Self::active_subject(&conn, subject_id)?;
            let wiki = Self::wiki_fs(&conn)?;
            let page = match page_name {
                Some(raw) => names::page_name(raw).ok_or_else(|| AppError::Invalid("not a valid page name".into()))?,
                None => Self::pick_page(&conn, &wiki, &subject, run_id, &note)?,
            };
            repo::upsert_pending(&conn, run_id, &note, Some(subject.id), Some(&page), at_ms)?;
        }
        self.prepare(run_id)
    }

    /// The best page for a note by local keyword matching; the default page when nothing matches.
    fn pick_page(conn: &Connection, wiki: &WikiFs, subject: &SubjectRow, run_id: i64, note: &str) -> AppResult<String> {
        let mut query = note.to_string();
        for label in study_context::local_labels(conn, run_id)? {
            query.push(' ');
            query.push_str(&label);
        }
        let mut pages = Vec::new();
        for name in wiki.list_pages(&subject.slug)? {
            if let Ok(Some(body)) = wiki.read_page(&subject.slug, &name) {
                pages.push((name, body));
            }
        }
        Ok(rank_pages(&query, &pages, 1).first().map_or_else(|| DEFAULT_PAGE.to_string(), |r| pages[r.index].0.clone()))
    }

    /// Runs a waiting note again (after the LLM was off, over budget, or its answer was refused).
    pub fn retry(&self, run_id: i64) -> AppResult<RetroOutcome> {
        self.prepare(run_id)
    }

    fn prepare(&self, run_id: i64) -> AppResult<RetroOutcome> {
        let (row, subject, original, labels, auto) = {
            let conn = lock_db(&self.db)?;
            let row = repo::get(&conn, run_id)?.ok_or_else(|| AppError::Invalid("no retrospective for that run".into()))?;
            if !matches!(row.status, RetroStatus::Pending | RetroStatus::Failed) {
                return Err(AppError::Invalid("this retrospective is not waiting for a merge".into()));
            }
            let subject_id = row.subject_id.ok_or_else(|| AppError::Invalid("the retrospective has no subject".into()))?;
            let subject = Self::active_subject(&conn, subject_id)?;
            let page = row.page_name.clone().ok_or_else(|| AppError::Invalid("the retrospective has no page".into()))?;
            let original = Self::wiki_fs(&conn)?.read_page(&subject.slug, &page)?;
            (row, subject, original, study_context::llm_labels(&conn, run_id)?, auto_apply(&conn)?)
        };
        let page_name = row.page_name.clone().unwrap_or_else(|| DEFAULT_PAGE.to_string());
        let original_text = original.clone().unwrap_or_default();

        let request = LlmRequest {
            purpose: Purpose::WikiMerge,
            prompt: Prompt::merge(&subject.name, &page_name, &original_text, &row.note, &labels),
            max_tokens: MERGE_MAX_TOKENS,
        };
        let outcome = self.llm.complete(&request);
        let held = |status: RetroStatus, code: &str| -> AppResult<RetroOutcome> {
            repo::record_attempt(&*lock_db(&self.db)?, run_id, status, Some(code))?;
            Ok(RetroOutcome::Pending { run_id, code: code.to_string() })
        };
        let response = match outcome {
            Ok(response) => response,
            Err(err) => return held(RetroStatus::Pending, err.code()),
        };
        let merged = match parse_merge(&response.text).and_then(|m| check_merge(&original_text, &m, &row.note)) {
            Ok(merged) => merged,
            Err(rejected) => {
                tracing::info!(event = "retro_merge_refused", code = rejected.code());
                return held(RetroStatus::Failed, rejected.code());
            }
        };
        if merged.trim() == original_text.trim() {
            return held(RetroStatus::Failed, "merge_no_change");
        }
        let prepared = Prepared { run_id, subject_id: subject.id, page_name, original, merged };
        if auto {
            return self.write(prepared);
        }
        let diff = diff_lines(&original_text, &prepared.merged);
        let preview = RetroOutcome::Preview {
            preview_id: uuid::Uuid::new_v4().to_string(),
            run_id,
            subject_id: prepared.subject_id,
            page_name: prepared.page_name.clone(),
            created: prepared.original.is_none(),
            diff,
        };
        if let RetroOutcome::Preview { preview_id, .. } = &preview {
            let mut hub = lock(&self.hub);
            hub.previews.insert(preview_id.clone(), prepared);
            hub.order.push_back(preview_id.clone());
            while hub.order.len() > MAX_PREVIEWS {
                if let Some(old) = hub.order.pop_front() {
                    hub.previews.remove(&old);
                }
            }
        }
        Ok(preview)
    }

    /// Writes a previewed merge. A preview is single use, and it is refused if the page changed
    /// after it was made, so the user's later edit is never overwritten blindly.
    pub fn apply(&self, preview_id: &str) -> AppResult<RetroOutcome> {
        let prepared = {
            let mut hub = lock(&self.hub);
            hub.order.retain(|id| id != preview_id);
            hub.previews.remove(preview_id)
        }
        .ok_or_else(|| AppError::Invalid("the preview no longer exists".into()))?;
        self.write(prepared)
    }

    pub fn cancel(&self, preview_id: &str) {
        let mut hub = lock(&self.hub);
        hub.order.retain(|id| id != preview_id);
        hub.previews.remove(preview_id);
    }

    fn write(&self, prepared: Prepared) -> AppResult<RetroOutcome> {
        let at_ms = self.clock.now_ms()?;
        let conn = lock_db(&self.db)?;
        let subject = Self::active_subject(&conn, prepared.subject_id)?;
        let wiki = Self::wiki_fs(&conn)?;
        let current = wiki.read_page(&subject.slug, &prepared.page_name)?;
        if current != prepared.original {
            return Err(AppError::Invalid("the page changed after the preview; submit the retrospective again".into()));
        }
        let report = wiki.write_page(&subject.slug, &prepared.page_name, &prepared.merged)?;
        index::index_page(&conn, &subject, &prepared.page_name, &prepared.merged, at_ms)?;
        let backup_name = report.backup.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str()).map(str::to_string);
        repo::mark_merged(&conn, prepared.run_id, &prepared.page_name, backup_name.as_deref(), at_ms)?;
        tracing::info!(event = "wiki_write", op = "retro_merge", bytes = report.bytes, backup = report.backup.is_some());
        Ok(RetroOutcome::Applied {
            run_id: prepared.run_id,
            subject_id: subject.id,
            page_name: prepared.page_name,
            created: prepared.original.is_none(),
            backed_up: report.backup.is_some(),
        })
    }

    /// Restores the page text from before the merge. The merged text is itself backed up by the
    /// write, so an undo does not lose anything either.
    pub fn undo(&self, run_id: i64) -> AppResult<RetroView> {
        let at_ms = self.clock.now_ms()?;
        let conn = lock_db(&self.db)?;
        let row = repo::get(&conn, run_id)?.ok_or_else(|| AppError::Invalid("no retrospective for that run".into()))?;
        let (Some(subject_id), Some(page), Some(backup)) = (row.subject_id, row.page_name.as_deref(), row.backup_name.as_deref()) else {
            return Err(AppError::Invalid("there is nothing to restore for this retrospective".into()));
        };
        if row.status != RetroStatus::Merged {
            return Err(AppError::Invalid("this retrospective is not merged".into()));
        }
        let subject = wiki_repo::get_subject(&conn, subject_id)?;
        let wiki = Self::wiki_fs(&conn)?;
        let previous = wiki.read_version(&subject.slug, backup)?.ok_or_else(|| AppError::Invalid("the backup file is gone".into()))?;
        let report = wiki.write_page(&subject.slug, page, &previous)?;
        index::index_page(&conn, &subject, page, &previous, at_ms)?;
        repo::mark_undone(&conn, run_id)?;
        tracing::info!(event = "wiki_write", op = "retro_undo", bytes = report.bytes, backup = report.backup.is_some());
        repo::get(&conn, run_id)?.map(|r| view(&r)).ok_or_else(|| AppError::Internal("retrospective vanished".into()))
    }

    /// The user chose not to merge (or to drop a waiting note).
    pub fn skip(&self, run_id: i64) -> AppResult<()> {
        repo::mark_skipped(&*lock_db(&self.db)?, run_id)
    }

    pub fn get(&self, run_id: i64) -> AppResult<Option<RetroView>> {
        Ok(repo::get(&*lock_db(&self.db)?, run_id)?.as_ref().map(view))
    }

    /// Notes still waiting for a merge.
    pub fn list_open(&self) -> AppResult<Vec<RetroView>> {
        Ok(repo::list_open(&*lock_db(&self.db)?)?.iter().map(view).collect())
    }

    // ---- lint ----

    /// Local checks of a subject's pages, then, only when something was found and the LLM answers,
    /// improvement advice for the flagged pages. The advice is for display; nothing is changed.
    pub fn lint(&self, subject_id: i64) -> AppResult<LintReport> {
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
        let findings = lint::lint_pages(&pages);
        let mut report = LintReport { subject_id, pages: pages.len(), findings, suggestions: Vec::new(), advice_code: None };
        if report.findings.is_empty() {
            report.advice_code = Some("nothing_to_advise".into());
            return Ok(report);
        }
        let mut flagged: Vec<String> = Vec::new();
        for finding in &report.findings {
            if !flagged.contains(&finding.page_name) {
                flagged.push(finding.page_name.clone());
            }
        }
        let prompt_pages: Vec<PromptPage> = flagged
            .iter()
            .filter_map(|name| pages.iter().find(|(n, _)| n == name))
            .map(|(name, body)| PromptPage { name: name.clone(), body: body.clone() })
            .collect();
        let offered: Vec<String> = prompt_pages.iter().map(|p| p.name.clone()).collect();
        let request = LlmRequest {
            purpose: Purpose::WikiLint,
            prompt: Prompt::lint(&subject.name, &lint::describe(&report.findings), prompt_pages),
            max_tokens: LINT_MAX_TOKENS,
        };
        match self.llm.complete(&request) {
            Ok(response) => match parse_advice(&response.text, &offered) {
                Some(suggestions) => report.suggestions = suggestions,
                None => report.advice_code = Some("advice_malformed".into()),
            },
            Err(err) => report.advice_code = Some(err.code().to_string()),
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::config::{update_setting, LlmSettingKey};
    use crate::llm::mock::MockStep;
    use crate::llm::service::tests::{mock_harness, Harness};
    use crate::llm::LlmError;
    use crate::study_context::{update_setting as update_context, ContextSettingKey};
    use crate::testutil::{capture_logs, insert_run_for_test, TempDir};
    use crate::wiki::service::{SubjectView, WikiService};

    struct Rig {
        h: Harness,
        retro: RetroService,
        wiki: WikiService,
        dir: TempDir,
        subject: SubjectView,
        run_id: i64,
    }

    fn rig() -> Rig {
        let h = mock_harness();
        let dir = TempDir::new("retro");
        {
            let mut conn = h.db.lock().expect("db");
            let root = dir.path().join("wiki");
            update_setting(&mut conn, LlmSettingKey::WikiDir, SettingValue::Text(root.to_string_lossy().into_owned()), 1).expect("dir");
        }
        let wiki = WikiService::new(Arc::clone(&h.db), Arc::clone(&h.service), h.clock.clone(), Arc::new(crate::wiki::pdf::NoPdfExtractor));
        let subject = wiki.create_subject("수학").expect("subject");
        let run_id = insert_run_for_test(&h.db.lock().expect("db"), "수학").0;
        let retro = RetroService::new(Arc::clone(&h.db), Arc::clone(&h.service), h.clock.clone());
        Rig { h, retro, wiki, dir, subject, run_id }
    }

    fn write_page(r: &Rig, name: &str, text: &str) {
        let conn = r.h.db.lock().expect("db");
        let wiki = RetroService::wiki_fs(&conn).expect("fs");
        wiki.write_page(&r.subject.slug, name, text).expect("write");
    }

    fn page(r: &Rig, name: &str) -> Option<String> {
        std::fs::read_to_string(r.dir.path().join("wiki").join(&r.subject.slug).join("pages").join(format!("{name}.md"))).ok()
    }

    fn backups(r: &Rig) -> usize {
        std::fs::read_dir(r.dir.path().join("wiki").join(&r.subject.slug).join(".versions")).map(|d| d.count()).unwrap_or(0)
    }

    fn status(r: &Rig) -> (String, Option<String>) {
        let row = r.retro.get(r.run_id).expect("get").expect("row");
        (row.status.to_string(), row.last_code)
    }

    const ORIGINAL: &str = "# 미분\n\n도함수는 순간 변화율이다.\n극한으로 정의한다.";

    #[test]
    fn a_retro_is_previewed_then_written_with_a_backup_and_marked_merged() {
        let r = rig();
        write_page(&r, "미분", ORIGINAL);
        let outcome = r.retro.submit(r.run_id, r.subject.id, Some("미분"), "합성함수 미분은 연쇄법칙").expect("submit");
        let RetroOutcome::Preview { preview_id, diff, created, .. } = outcome else { panic!("expected a preview: {outcome:?}") };
        assert!(!created);
        assert!(diff.iter().any(|l| l.text.contains("연쇄법칙")), "{diff:?}");
        assert_eq!(page(&r, "미분").as_deref(), Some(ORIGINAL), "a preview writes nothing");
        assert_eq!(backups(&r), 0);
        let applied = r.retro.apply(&preview_id).expect("apply");
        assert!(matches!(applied, RetroOutcome::Applied { backed_up: true, created: false, .. }), "{applied:?}");
        let merged = page(&r, "미분").expect("page");
        assert!(merged.contains("도함수는 순간 변화율이다.") && merged.contains("연쇄법칙"));
        assert_eq!(backups(&r), 1);
        assert_eq!(status(&r).0, "merged");
        assert!(r.retro.get(r.run_id).expect("get").expect("row").can_undo);
        assert!(r.retro.apply(&preview_id).is_err(), "a preview is single use");
        assert!(r.retro.submit(r.run_id, r.subject.id, Some("미분"), "또").is_err(), "a merged retro is final");
    }

    #[test]
    fn undo_restores_the_text_from_before_the_merge() {
        let r = rig();
        write_page(&r, "미분", ORIGINAL);
        let RetroOutcome::Preview { preview_id, .. } = r.retro.submit(r.run_id, r.subject.id, Some("미분"), "연쇄법칙").expect("submit") else {
            panic!("preview")
        };
        r.retro.apply(&preview_id).expect("apply");
        let undone = r.retro.undo(r.run_id).expect("undo");
        assert_eq!(undone.status, "undone");
        assert_eq!(page(&r, "미분").as_deref(), Some(ORIGINAL));
        assert!(r.retro.undo(r.run_id).is_err(), "undo works once");
        let pages = r.wiki.list_pages(r.subject.id).expect("pages");
        assert_eq!(pages.len(), 1);
    }

    #[test]
    fn a_merge_that_would_destroy_the_page_is_refused_and_nothing_is_written() {
        let r = rig();
        let long = format!("# 미분\n\n{}", "내용 ".repeat(100));
        write_page(&r, "미분", &long);
        r.h.remote.mock_llm().push(MockStep::Text("{\"markdown\":\"# 미분\"}".into()));
        let outcome = r.retro.submit(r.run_id, r.subject.id, Some("미분"), "메모").expect("submit");
        assert_eq!(outcome, RetroOutcome::Pending { run_id: r.run_id, code: "merge_too_short".into() });
        assert_eq!(page(&r, "미분").as_deref(), Some(long.as_str()));
        assert_eq!(backups(&r), 0);
        assert_eq!(status(&r), ("failed".into(), Some("merge_too_short".into())));
        // Garbage and unknown fields are refused too, then a good answer goes through on retry.
        r.h.remote.mock_llm().push(MockStep::Text("{\"markdown\":\"x\",\"path\":\"../x\"}".into()));
        assert!(matches!(r.retro.retry(r.run_id).expect("retry"), RetroOutcome::Pending { code, .. } if code == "merge_not_json"));
        assert!(matches!(r.retro.retry(r.run_id).expect("retry"), RetroOutcome::Preview { .. }));
    }

    #[test]
    fn when_the_llm_cannot_answer_the_note_stays_pending_and_retry_works_later() {
        let r = rig();
        write_page(&r, "미분", ORIGINAL);
        r.h.remote.mock_llm().push(MockStep::Fail(LlmError::Overloaded));
        let first = r.retro.submit(r.run_id, r.subject.id, Some("미분"), "연쇄법칙").expect("submit");
        assert_eq!(first, RetroOutcome::Pending { run_id: r.run_id, code: "llm_overloaded".into() });
        assert_eq!(status(&r), ("pending".into(), Some("llm_overloaded".into())));
        assert_eq!(r.retro.list_open().expect("open").len(), 1);

        {
            let mut conn = r.h.db.lock().expect("db");
            update_setting(&mut conn, LlmSettingKey::Enabled, SettingValue::Bool(false), 5).expect("off");
        }
        let off = r.retro.retry(r.run_id).expect("retry");
        assert_eq!(off, RetroOutcome::Pending { run_id: r.run_id, code: "llm_disabled".into() });
        assert_eq!(r.retro.get(r.run_id).expect("get").expect("row").attempts, 2);
        {
            let mut conn = r.h.db.lock().expect("db");
            update_setting(&mut conn, LlmSettingKey::Enabled, SettingValue::Bool(true), 6).expect("on");
        }
        assert!(matches!(r.retro.retry(r.run_id).expect("retry"), RetroOutcome::Preview { .. }));
        r.retro.skip(r.run_id).expect("skip");
        assert_eq!(status(&r).0, "skipped");
        assert!(r.retro.list_open().expect("open").is_empty());
    }

    #[test]
    fn auto_apply_writes_without_a_preview_and_a_changed_page_blocks_a_stale_preview() {
        let r = rig();
        write_page(&r, "미분", ORIGINAL);
        let RetroOutcome::Preview { preview_id, .. } = r.retro.submit(r.run_id, r.subject.id, Some("미분"), "연쇄법칙").expect("submit") else {
            panic!("preview")
        };
        write_page(&r, "미분", "# 미분\n\n그 사이 내가 고친 내용이다.");
        assert!(r.retro.apply(&preview_id).is_err(), "the page changed after the preview");
        assert!(page(&r, "미분").expect("page").contains("내가 고친"));

        {
            let mut conn = r.h.db.lock().expect("db");
            assert!(set_auto_apply(&mut conn, true, 7).is_ok());
        }
        let outcome = r.retro.retry(r.run_id).expect("auto");
        assert!(matches!(outcome, RetroOutcome::Applied { backed_up: true, .. }), "{outcome:?}");
        assert!(page(&r, "미분").expect("page").contains("연쇄법칙"));
    }

    #[test]
    fn the_page_is_picked_locally_and_a_new_page_has_nothing_to_undo() {
        let r = rig();
        write_page(&r, "적분", "# 적분\n\n넓이를 구하는 방법이다.");
        write_page(&r, "미분", ORIGINAL);
        let RetroOutcome::Preview { page_name, .. } = r.retro.submit(r.run_id, r.subject.id, None, "오늘 미분의 극한 정의를 복습했다").expect("submit") else {
            panic!("preview")
        };
        assert_eq!(page_name, "미분");

        let r2 = rig();
        let RetroOutcome::Preview { page_name, created, preview_id, .. } = r2.retro.submit(r2.run_id, r2.subject.id, None, "아무 관련 없는 메모").expect("submit") else {
            panic!("preview")
        };
        assert_eq!((page_name.as_str(), created), (DEFAULT_PAGE, true));
        r2.retro.apply(&preview_id).expect("apply");
        assert!(page(&r2, DEFAULT_PAGE).expect("created").contains("아무 관련 없는 메모"));
        let view = r2.retro.get(r2.run_id).expect("get").expect("row");
        assert!(!view.can_undo);
        assert!(r2.retro.undo(r2.run_id).is_err());
    }

    #[test]
    fn invalid_input_is_refused_before_anything_is_stored() {
        let r = rig();
        assert!(r.retro.submit(r.run_id, r.subject.id, Some("미분"), "   ").is_err());
        assert!(r.retro.submit(r.run_id, r.subject.id, Some("미분"), &"가".repeat(MAX_NOTE_CHARS + 1)).is_err());
        assert!(r.retro.submit(9_999, r.subject.id, Some("미분"), "메모").is_err(), "unknown run");
        assert!(r.retro.submit(r.run_id, r.subject.id, Some("../escape"), "메모").is_err());
        assert!(r.retro.submit(r.run_id, 9_999, Some("미분"), "메모").is_err(), "unknown subject");
        assert!(r.retro.get(r.run_id).expect("get").is_none());
        assert_eq!(r.h.remote.mock_llm().call_count(), 0);
    }

    #[test]
    fn study_context_labels_reach_the_model_only_with_the_separate_consent() {
        let r = rig();
        write_page(&r, "미분", ORIGINAL);
        {
            let mut conn = r.h.db.lock().expect("db");
            update_context(&mut conn, ContextSettingKey::Enabled, SettingValue::Bool(true), 3).expect("on");
            let segment: i64 = conn.query_row("SELECT id FROM timer_segments WHERE run_id = ?1", [r.run_id], |row| row.get(0)).expect("segment");
            crate::store::context::insert(
                &conn,
                &crate::store::context::ContextRow { segment_id: segment, at_ms: 1, app_id: "a".into(), context_label: "CANARY-LABEL.pdf".into() },
            )
            .expect("label");
        }
        r.retro.submit(r.run_id, r.subject.id, Some("미분"), "연쇄법칙").expect("submit");
        let sent = r.h.remote.mock_llm().calls();
        assert!(sent.iter().all(|c| !c.user.contains("CANARY-LABEL")), "no consent, no labels");
        {
            let mut conn = r.h.db.lock().expect("db");
            update_context(&mut conn, ContextSettingKey::LlmConsent, SettingValue::Bool(true), 4).expect("consent");
        }
        r.retro.retry(r.run_id).expect("retry");
        let sent = r.h.remote.mock_llm().calls();
        assert!(sent.last().is_some_and(|c| c.user.contains("CANARY-LABEL.pdf") && c.purpose == Purpose::WikiMerge));
    }

    #[test]
    fn the_note_stays_out_of_the_log() {
        let r = rig();
        write_page(&r, "미분", ORIGINAL);
        let mut seen = String::new();
        for _ in 0..10 {
            seen = capture_logs(|| {
                r.retro.submit(r.run_id, r.subject.id, Some("미분"), "CANARY-NOTE-77").ok();
            });
            if seen.contains("llm_call") {
                break;
            }
        }
        assert!(seen.contains("llm_call"), "{seen}");
        assert!(!seen.contains("CANARY-NOTE-77"), "{seen}");
        let usage: i64 = r.h.db.lock().expect("db").query_row("SELECT COUNT(*) FROM llm_usage WHERE purpose = 'wiki_merge'", [], |row| row.get(0)).expect("count");
        assert!(usage >= 1);
    }

    #[test]
    fn lint_reports_local_findings_without_a_model_and_adds_advice_when_one_answers() {
        let r = rig();
        write_page(&r, "개요", "# 개요\n\n미분과 적분의 관계를 설명한다. [[없는 페이지]]");
        write_page(&r, "빈 페이지", "# 빈 페이지\n");
        {
            let mut conn = r.h.db.lock().expect("db");
            update_setting(&mut conn, LlmSettingKey::Enabled, SettingValue::Bool(false), 5).expect("off");
        }
        let local = r.retro.lint(r.subject.id).expect("lint");
        assert!(local.findings.len() >= 3, "{:?}", local.findings);
        assert!(local.suggestions.is_empty());
        assert_eq!(local.advice_code.as_deref(), Some("llm_disabled"));
        assert_eq!(r.h.remote.mock_llm().call_count(), 0);

        {
            let mut conn = r.h.db.lock().expect("db");
            update_setting(&mut conn, LlmSettingKey::Enabled, SettingValue::Bool(true), 6).expect("on");
        }
        let with_advice = r.retro.lint(r.subject.id).expect("lint");
        assert_eq!(with_advice.findings, local.findings);
        assert!(!with_advice.suggestions.is_empty());
        assert!(with_advice.suggestions.iter().all(|s| ["개요", "빈 페이지"].contains(&s.page_name.as_str())));
        assert_eq!(with_advice.advice_code, None);
        assert_eq!(page(&r, "개요").as_deref(), Some("# 개요\n\n미분과 적분의 관계를 설명한다. [[없는 페이지]]"), "lint never edits");
    }

    #[test]
    fn a_clean_subject_needs_no_model_call() {
        let r = rig();
        write_page(&r, "가", "# 가\n\n충분히 긴 본문이 여기에 있다. [[나]]");
        write_page(&r, "나", "# 나\n\n충분히 긴 본문이 여기에 있다. [[가]]");
        let report = r.retro.lint(r.subject.id).expect("lint");
        assert!(report.findings.is_empty());
        assert_eq!(report.advice_code.as_deref(), Some("nothing_to_advise"));
        assert_eq!(r.h.remote.mock_llm().call_count(), 0);
    }
}
