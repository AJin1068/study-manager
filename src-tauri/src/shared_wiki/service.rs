//! The shared-wiki service: what the Tauri commands call. Network calls and model calls always run
//! with the database lock released.

use std::sync::{Arc, Mutex, MutexGuard};

use chrono::Local;
use rusqlite::Connection;
use serde::Serialize;

use super::concepts::{self, NewConcept};
use super::copy;
use super::merge::{self, MergeError};
use super::model::{Published, RevisionKind, SharedCode, SharedError, WirePage, WireRevision, MAX_BODY_CHARS};
use super::port::SharedWikiPort;
use super::quiz::SharedQuiz;
use super::rest::RestSharedWiki;
use super::summary;
use crate::commands::lock_db;
use crate::error::{AppError, AppResult};
use crate::llm::service::{day_bounds_ms, LlmService};
use crate::llm::{LlmClient, LlmError, LlmRequest, LlmResponse};
use crate::quiz::ItemKind;
use crate::remote::runtime::RemoteRuntime;
use crate::remote::Clock;
use crate::store;
use crate::store::shared_wiki as marks;
use crate::wiki::fs::WikiFs;
use crate::wiki::index::content_hash;
use crate::wiki::service::WikiService;

const PREVIEW_CHARS: usize = 400;

/// Shown on the confirmation screen and in the room: what sharing means.
pub const SHARE_NOTICE: &str = "공유하면 이 방의 멤버가 이 페이지를 볼 수 있어요. 이미 본 사람의 사본은 철회해도 회수할 수 없어요.";
pub const MERGE_NOTICE: &str = "합치기를 허용하면 다른 멤버가 합칠 때 이 글이 그 멤버의 Anthropic 키로 AI에 전송돼요. 허용하지 않으면 합치지 않고 목록에만 나와요.";

/// Where the server port comes from: `None` when signed out or not configured.
pub trait PortSource: Send + Sync {
    fn port(&self) -> Option<Arc<dyn SharedWikiPort>>;
}

struct AppPorts {
    db: Arc<Mutex<Connection>>,
    runtime: Arc<RemoteRuntime>,
}

impl PortSource for AppPorts {
    fn port(&self) -> Option<Arc<dyn SharedWikiPort>> {
        let handle = {
            let conn = lock_db(&self.db).ok()?;
            self.runtime.supabase(&conn).ok()?
        };
        if !handle.auth.status().signed_in {
            return None;
        }
        Some(Arc::new(RestSharedWiki::new(handle.rest)))
    }
}

/// The gated `LlmService` (switch, key, budget, usage row) as an `LlmClient`.
struct GatedLlm(Arc<LlmService>);

impl LlmClient for GatedLlm {
    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.0.complete(request)
    }
}

pub(crate) type WikiLocator = dyn Fn(&Connection) -> AppResult<WikiFs> + Send + Sync;

pub struct SharedWikiService {
    db: Arc<Mutex<Connection>>,
    clock: Arc<dyn Clock>,
    ports: Arc<dyn PortSource>,
    llm: Arc<dyn LlmClient>,
    wiki: Arc<WikiLocator>,
    round: Mutex<Option<SharedQuiz>>,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

// ---- views ----

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SharePreview {
    pub page_name: String,
    pub preview: String,
    pub member_count: Option<i64>,
    pub share_notice: &'static str,
    pub merge_notice: &'static str,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionView {
    pub code: &'static str,
    pub branched: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncView {
    pub pushed: usize,
    pub failed: usize,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RoomPageView {
    pub title: String,
    pub contributors: Vec<String>,
    pub has_merge: bool,
    pub branched: bool,
    pub shared_by_me: bool,
    /// Contributions that may be sent to the model; a merge needs at least two.
    pub mergeable_parts: usize,
    /// Authors whose text is listed but not merged (no consent).
    pub listed_only: Vec<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NewConceptView {
    pub title: String,
    pub authors: Vec<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SharedRoomView {
    pub pages: Vec<RoomPageView>,
    pub new_concepts: Vec<NewConceptView>,
    pub summary: String,
    pub share_notice: &'static str,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MergeView {
    pub code: &'static str,
    pub used_llm: bool,
    pub fallback_reason: Option<&'static str>,
    pub contributors: Vec<String>,
    pub listed_only: Vec<String>,
    pub markdown: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuizQuestionView {
    pub item_id: i64,
    pub kind: ItemKind,
    pub prompt: String,
    pub choices: Vec<String>,
    pub page_name: String,
}

/// `Remote` failures are errors (the UI shows the link state); a refusal is an ordinary code.
fn coded<T>(result: Result<T, SharedError>) -> AppResult<Result<T, &'static str>> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(SharedError::Remote(err)) => Err(AppError::Remote(err.code().to_string())),
        Err(other) => Ok(Err(other.code())),
    }
}

fn live_edits(page: &WirePage) -> impl Iterator<Item = &WireRevision> {
    page.revisions.iter().filter(|r| r.kind == RevisionKind::Edit && !r.body.trim().is_empty())
}

impl SharedWikiService {
    pub fn new(db: Arc<Mutex<Connection>>, runtime: Arc<RemoteRuntime>, clock: Arc<dyn Clock>, llm: Arc<LlmService>) -> Self {
        let ports = Arc::new(AppPorts { db: Arc::clone(&db), runtime });
        Self::with_parts(db, clock, ports, Arc::new(GatedLlm(llm)), Arc::new(WikiService::wiki_fs))
    }

    pub(crate) fn with_parts(
        db: Arc<Mutex<Connection>>,
        clock: Arc<dyn Clock>,
        ports: Arc<dyn PortSource>,
        llm: Arc<dyn LlmClient>,
        wiki: Arc<WikiLocator>,
    ) -> Self {
        Self { db, clock, ports, llm, wiki, round: Mutex::new(None) }
    }

    fn now(&self) -> AppResult<i64> {
        self.clock.now_ms()
    }

    fn port(&self) -> AppResult<Arc<dyn SharedWikiPort>> {
        self.ports.port().ok_or_else(|| AppError::Remote(SharedCode::NotSignedIn.as_str().to_string()))
    }

    /// The text of a local wiki page and its name, read while the lock is held.
    fn read_local(&self, conn: &Connection, page_id: i64) -> AppResult<(String, String)> {
        let (subject_id, name) = marks::page_by_id(conn, page_id)?.ok_or_else(|| AppError::Invalid("unknown wiki page".into()))?;
        let subject = store::wiki::get_subject(conn, subject_id)?;
        let text = (self.wiki)(conn)?.read_page(&subject.slug, &name)?.ok_or_else(|| AppError::Invalid("the page does not exist".into()))?;
        Ok((name, text))
    }

    // ---- sharing ----

    /// The confirmation screen: the page name, the start of its text, and how many people see it.
    pub fn share_preview(&self, page_id: i64, room_id: &str) -> AppResult<SharePreview> {
        let conn = lock_db(&self.db)?;
        let (page_name, text) = self.read_local(&conn, page_id)?;
        let (rooms, _) = store::rooms::read_membership_cache(&conn)?;
        let member_count = rooms.iter().find(|r| r.room_id == room_id).map(|r| r.member_count);
        let preview = text.chars().take(PREVIEW_CHARS).collect();
        Ok(SharePreview { page_name, preview, member_count, share_notice: SHARE_NOTICE, merge_notice: MERGE_NOTICE })
    }

    /// Shares a page with a room. Nothing happens unless `confirmed` is true (the user saw the
    /// confirmation screen). `allow_llm_merge` is the separate consent for model merging.
    pub fn share(&self, page_id: i64, room_id: &str, allow_llm_merge: bool, confirmed: bool) -> AppResult<ActionView> {
        if !confirmed {
            return Ok(ActionView { code: "not_confirmed", branched: false });
        }
        let port = self.port()?;
        let now = self.now()?;
        let (name, text, base, was_marked) = {
            let conn = lock_db(&self.db)?;
            let (name, text) = self.read_local(&conn, page_id)?;
            if text.chars().count() > MAX_BODY_CHARS {
                return Ok(ActionView { code: SharedCode::TooLarge.as_str(), branched: false });
            }
            let previous = marks::get_mark(&conn, page_id, room_id)?;
            marks::set_mark(&conn, page_id, room_id, allow_llm_merge, now)?;
            let base = previous.as_ref().map(|m| m.last_revision_id.clone()).filter(|id| !id.is_empty());
            (name, text, base, previous.is_some())
        };
        match coded(port.publish(room_id, &name, &text, base.as_deref(), allow_llm_merge))? {
            Ok(Published { revision_id, branched }) => {
                let conn = lock_db(&self.db)?;
                marks::record_push(&conn, page_id, room_id, &content_hash(&text), &revision_id)?;
                Ok(ActionView { code: SharedCode::Ok.as_str(), branched })
            }
            Err(code) => {
                if !was_marked {
                    marks::remove_mark(&*lock_db(&self.db)?, page_id, room_id)?;
                }
                Ok(ActionView { code, branched: false })
            }
        }
    }

    /// Pushes the marked pages of a room whose text changed since the last push.
    pub fn sync_marked(&self, room_id: &str) -> AppResult<SyncView> {
        let port = self.port()?;
        let rows = marks::list_marks(&*lock_db(&self.db)?, room_id)?;
        let (mut pushed, mut failed) = (0, 0);
        for mark in rows {
            let text = {
                let conn = lock_db(&self.db)?;
                match self.read_local(&conn, mark.page_id) {
                    Ok((_, text)) => text,
                    Err(_) => {
                        failed += 1;
                        continue;
                    }
                }
            };
            let hash = content_hash(&text);
            if hash == mark.last_pushed_hash {
                continue;
            }
            let base = Some(mark.last_revision_id.as_str()).filter(|id| !id.is_empty());
            match coded(port.publish(room_id, &mark.page_name, &text, base, mark.allow_llm_merge))? {
                Ok(published) => {
                    marks::record_push(&*lock_db(&self.db)?, mark.page_id, room_id, &hash, &published.revision_id)?;
                    pushed += 1;
                }
                Err(_) => failed += 1,
            }
        }
        Ok(SyncView { pushed, failed })
    }

    /// Withdraws a page: the server tombstones the user's text, the local mark and the local copy
    /// go away. A failure that is not a refusal keeps the mark so the user can retry.
    pub fn withdraw(&self, page_id: i64, room_id: &str) -> AppResult<ActionView> {
        let port = self.port()?;
        let (name, root) = {
            let conn = lock_db(&self.db)?;
            let (_, name) = marks::page_by_id(&conn, page_id)?.ok_or_else(|| AppError::Invalid("unknown wiki page".into()))?;
            (name, (self.wiki)(&conn)?.root().to_path_buf())
        };
        match coded(port.withdraw(room_id, &name))? {
            Ok(_) | Err("not_shared" | "not_found") => {
                marks::remove_mark(&*lock_db(&self.db)?, page_id, room_id)?;
                copy::remove_copy(&root, room_id, &name)?;
                Ok(ActionView { code: SharedCode::Ok.as_str(), branched: false })
            }
            Err(code) => Ok(ActionView { code, branched: false }),
        }
    }

    // ---- reading the room ----

    fn pages(&self, room_id: &str) -> AppResult<Vec<WirePage>> {
        let listed = self.port()?.list(room_id);
        coded(listed)?.map_err(|code| AppError::Remote(code.to_string()))
    }

    /// The room's shared pages, concepts the user has no page for, and the day's recap. Also
    /// refreshes the local `.shared/` copies.
    pub fn room_view(&self, room_id: &str) -> AppResult<SharedRoomView> {
        let pages = self.pages(room_id)?;
        let now = self.now()?;
        let (local_names, seen, root) = {
            let conn = lock_db(&self.db)?;
            (marks::all_page_names(&conn)?, marks::seen_concepts(&conn, room_id)?, (self.wiki)(&conn)?.root().to_path_buf())
        };
        self.refresh_copies(&root, room_id, &pages)?;
        let news = concepts::new_concepts(&local_names, &pages, &seen);
        let (from, to) = day_bounds_ms(&Local, now);
        let recap = summary::daily(&pages, from, to, &news);
        let views = pages
            .iter()
            .map(|page| {
                let plan = merge::plan(page);
                RoomPageView {
                    title: page.title.clone(),
                    contributors: page.revisions.iter().flat_map(|r| r.contributors.iter().cloned()).fold(Vec::new(), |mut acc, n| {
                        if !acc.contains(&n) {
                            acc.push(n);
                        }
                        acc
                    }),
                    has_merge: page.revisions.iter().any(|r| r.kind == RevisionKind::Merge),
                    branched: page.revisions.iter().any(|r| r.branched),
                    shared_by_me: live_edits(page).any(|r| r.is_mine),
                    mergeable_parts: plan.parts.len(),
                    listed_only: plan.listed_only.into_iter().map(|l| l.author).collect(),
                }
            })
            .collect();
        Ok(SharedRoomView {
            pages: views,
            new_concepts: news.into_iter().map(|NewConcept { title, authors }| NewConceptView { title, authors }).collect(),
            summary: recap.text,
            share_notice: SHARE_NOTICE,
        })
    }

    fn refresh_copies(&self, root: &std::path::Path, room_id: &str, pages: &[WirePage]) -> AppResult<()> {
        let mut wanted = Vec::new();
        for page in pages {
            let merged = page.revisions.iter().filter(|r| r.kind == RevisionKind::Merge).max_by_key(|r| r.created_at_ms);
            let theirs = live_edits(page).filter(|r| !r.is_mine).max_by_key(|r| r.created_at_ms);
            if let Some(rev) = merged.or(theirs) {
                if copy::write_copy(root, room_id, &page.title, &rev.body).is_ok() {
                    wanted.push(page.title.clone());
                }
            }
        }
        for stale in copy::list_copies(root, room_id)? {
            if !wanted.contains(&stale) {
                copy::remove_copy(root, room_id, &stale)?;
            }
        }
        Ok(())
    }

    /// The user saw these concepts; they are not announced again.
    pub fn acknowledge_concepts(&self, room_id: &str, titles: &[String]) -> AppResult<()> {
        let now = self.now()?;
        let conn = lock_db(&self.db)?;
        for title in titles {
            marks::mark_concept_seen(&conn, room_id, &concepts::normalize(title), now)?;
        }
        Ok(())
    }

    // ---- merging ----

    /// Merges the consented contributions of a page, saves the result as a new revision (the
    /// originals stay) and refreshes the local copy.
    pub fn merge_page(&self, room_id: &str, page_name: &str) -> AppResult<MergeView> {
        let pages = self.pages(room_id)?;
        let Some(page) = pages.iter().find(|p| p.title == page_name) else {
            return Ok(MergeView { code: SharedCode::NotFound.as_str(), used_llm: false, fallback_reason: None, contributors: Vec::new(), listed_only: Vec::new(), markdown: String::new() });
        };
        let plan = merge::plan(page);
        let listed_only: Vec<String> = plan.listed_only.iter().map(|l| l.author.clone()).collect();
        let draft = match merge::merge(self.llm.as_ref(), &page.title, &plan) {
            Ok(draft) => draft,
            Err(err @ MergeError::NotEnough) => {
                return Ok(MergeView { code: err.code(), used_llm: false, fallback_reason: None, contributors: Vec::new(), listed_only, markdown: String::new() });
            }
        };
        let saved = self.port()?.save_merged(room_id, &page.title, &draft.markdown, &draft.source_revisions);
        if let Err(code) = coded(saved)? {
            return Ok(MergeView { code, used_llm: draft.used_llm, fallback_reason: draft.fallback_reason, contributors: draft.contributors, listed_only, markdown: String::new() });
        }
        let root = (self.wiki)(&*lock_db(&self.db)?)?.root().to_path_buf();
        copy::write_copy(&root, room_id, &page.title, &draft.markdown)?;
        Ok(MergeView {
            code: SharedCode::Ok.as_str(),
            used_llm: draft.used_llm,
            fallback_reason: draft.fallback_reason,
            contributors: draft.contributors,
            listed_only,
            markdown: draft.markdown,
        })
    }

    // ---- mutual quiz ----

    /// Starts a round of questions cut from the room's shared pages.
    pub fn quiz_start(&self, room_id: &str) -> AppResult<Vec<QuizQuestionView>> {
        let pages = self.pages(room_id)?;
        let seed = u64::try_from(self.now()?).unwrap_or(0);
        let round = SharedQuiz::draw(&pages, seed);
        let views = round
            .questions()
            .into_iter()
            .map(|(q, page_name)| QuizQuestionView { item_id: q.item_id, kind: q.kind, prompt: q.prompt, choices: q.choices, page_name })
            .collect();
        *guard(&self.round) = Some(round);
        Ok(views)
    }

    /// Whether the answer is right; `None` when there is no such question in the current round.
    pub fn quiz_answer(&self, item_id: i64, answer: &str) -> Option<bool> {
        guard(&self.round).as_ref().and_then(|round| round.grade(item_id, answer))
    }
}
