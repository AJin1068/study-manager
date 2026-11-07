//! Bank generation and the review-card entry points. Generation reads indexed pages, asks the model
//! for questions when the LLM is on (purpose `quiz_gen`, the page inside a `<document>` block) and
//! checks every answer with `generate::parse_items`; when the LLM is off, refused, over budget, or
//! answers badly, the local template generator supplies the questions. So a bank exists with no
//! key and no network, and showing a question never calls the LLM.

use std::sync::{Arc, Mutex};

use chrono::Local;
use rusqlite::Connection;
use serde::Serialize;

use super::gate::{CardResult, QuizCard, QuizGate};
use super::generate::{NewItem, parse_items, template_items};
use crate::commands::lock_db;
use crate::error::{AppError, AppResult};
use crate::llm::egress::Prompt;
use crate::llm::service::LlmService;
use crate::llm::{LlmError, LlmRequest, Purpose};
use crate::remote::Clock;
use crate::store::quiz as repo;
use crate::store::wiki as wiki_repo;
use crate::wiki::service::WikiService;

const GEN_MAX_TOKENS: u32 = 1_500;
/// Pages turned into questions by one run.
const MAX_PAGES_PER_RUN: usize = 20;

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenerateReport {
    pub pages: usize,
    pub added: usize,
    /// Model questions rejected by validation (bad shape, no right answer, quote not on the page).
    pub dropped: usize,
    pub from_model: usize,
    pub from_template: usize,
}

pub struct QuizService {
    db: Arc<Mutex<Connection>>,
    wiki: Arc<WikiService>,
    llm: Arc<LlmService>,
    clock: Arc<dyn Clock>,
    gate: QuizGate,
}

impl QuizService {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        wiki: Arc<WikiService>,
        llm: Arc<LlmService>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            db,
            wiki,
            llm,
            clock,
            gate: QuizGate::default(),
        }
    }

    // ---- bank ----

    /// Writes questions for every page of the subject that has none yet. Runs the model calls with
    /// the database lock released.
    pub fn generate_for_subject(&self, subject_id: i64) -> AppResult<GenerateReport> {
        let (subject, pages) = {
            let conn = lock_db(&self.db)?;
            let subject = wiki_repo::get_subject(&conn, subject_id)?;
            if subject.archived_at_ms.is_some() {
                return Err(AppError::Invalid("the subject is archived".into()));
            }
            (subject, repo::pages_without_items(&conn, subject_id)?)
        };
        let mut report = GenerateReport::default();
        let mut model_usable = true;
        for (page_id, _, page_name) in pages.into_iter().take(MAX_PAGES_PER_RUN) {
            let body = match self.wiki.read_page(subject_id, &page_name) {
                Ok(content) => content.markdown,
                Err(_) => continue,
            };
            report.pages += 1;
            let mut items: Vec<NewItem> = Vec::new();
            if model_usable {
                let request = LlmRequest {
                    purpose: Purpose::QuizGen,
                    prompt: Prompt::quiz_gen(&subject.name, &page_name, &body),
                    max_tokens: GEN_MAX_TOKENS,
                };
                match self.llm.complete(&request) {
                    Ok(response) => {
                        let (kept, dropped) = parse_items(&response.text, &body);
                        report.dropped += dropped;
                        report.from_model += kept.len();
                        items = kept;
                    }
                    // Off, unconfigured, or out of budget: no point asking again for the next page.
                    Err(err) if err.is_unavailable() => model_usable = false,
                    Err(LlmError::Unauthorized) => model_usable = false,
                    Err(_) => {}
                }
            }
            if items.is_empty() {
                items = template_items(&body);
                report.from_template += items.len();
            }
            let at_ms = self.clock.now_ms()?;
            let conn = lock_db(&self.db)?;
            for item in &items {
                if repo::insert_item(&conn, page_id, item, at_ms)? {
                    report.added += 1;
                }
            }
        }
        tracing::info!(
            event = "quiz_bank_generated",
            pages = report.pages,
            added = report.added,
            dropped = report.dropped
        );
        Ok(report)
    }

    pub fn stats(&self) -> AppResult<repo::BankStats> {
        repo::bank_stats(&*lock_db(&self.db)?)
    }

    /// Reports a question (`wrong_answer`, `unclear`, `off_topic`, `other`); it is never asked again.
    pub fn flag(&self, item_id: i64, reason: &str) -> AppResult<()> {
        if !["wrong_answer", "unclear", "off_topic", "other"].contains(&reason) {
            return Err(AppError::Invalid("unknown report reason".into()));
        }
        let at_ms = self.clock.now_ms()?;
        super::gate::flag_item(&*lock_db(&self.db)?, item_id, reason, at_ms)
    }

    // ---- cards ----

    pub fn on_block_attempt(&self) -> AppResult<Option<QuizCard>> {
        let now = self.clock.now_ms()?;
        self.gate
            .offer_distraction(&*lock_db(&self.db)?, &Local, now)
    }

    /// The card for a check-in the window is showing right now.
    pub fn checkin_card(&self) -> AppResult<Option<QuizCard>> {
        let now = self.clock.now_ms()?;
        self.gate.offer_checkin(&*lock_db(&self.db)?, &Local, now)
    }

    pub fn session_end_card(&self) -> AppResult<Option<QuizCard>> {
        let now = self.clock.now_ms()?;
        self.gate
            .offer_session_end(&*lock_db(&self.db)?, &Local, now)
    }

    pub fn review_card(
        &self,
        subject_id: Option<i64>,
        count: usize,
    ) -> AppResult<Option<QuizCard>> {
        let now = self.clock.now_ms()?;
        self.gate
            .offer_review(&*lock_db(&self.db)?, now, subject_id, count)
    }

    pub fn answer_card(
        &self,
        card_id: u64,
        answers: &[String],
        skip: bool,
    ) -> AppResult<Option<CardResult>> {
        let now = self.clock.now_ms()?;
        self.gate.answer(
            &mut *lock_db(&self.db)?,
            &Local,
            card_id,
            answers,
            skip,
            now,
        )
    }
}
