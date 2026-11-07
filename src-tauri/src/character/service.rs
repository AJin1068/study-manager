//! Candidate generation with names. The look comes from the rules; the name and one-line intro
//! come from the model when it is usable, otherwise from the built-in dictionary. The model gets
//! part labels only (never MBTI, birth year, or gender) and its answer is screened (length,
//! control characters, protected names) per candidate.
//!
//! Model call purpose: `chat_name` (the existing "short name" purpose; the usage table's purpose
//! CHECK is not widened for this).

use std::sync::Arc;

use chrono::{Datelike, Local, TimeZone};

use super::catalog::catalog;
use super::compose::{builtin_blurb, builtin_name, compose, parse_names};
use super::{CandidateSet, CandidateView, Equipped, NameSource, SetupInput, labels, render};
use crate::error::AppResult;
use crate::llm::egress::{CharacterLook, Prompt};
use crate::llm::service::LlmService;
use crate::llm::{LlmRequest, Purpose};
use crate::points::rewards::Stage;
use crate::remote::Clock;

const NAMES_MAX_TOKENS: u32 = 300;

pub struct CharacterService {
    llm: Arc<LlmService>,
    clock: Arc<dyn Clock>,
}

pub fn year_of(now_ms: i64) -> i32 {
    Local
        .timestamp_millis_opt(now_ms)
        .single()
        .map_or(2026, |t| t.year())
}

impl CharacterService {
    pub fn new(llm: Arc<LlmService>, clock: Arc<dyn Clock>) -> Self {
        Self { llm, clock }
    }

    /// Never touches the database: a roll changes nothing that was saved. May call the model, so
    /// callers run it off the UI thread.
    pub fn candidates(&self, input: &SetupInput, seed: u32) -> AppResult<CandidateSet> {
        let now = self.clock.now_ms()?;
        let traits = input.normalize(year_of(now))?;
        let cat = catalog()?;
        let picked = compose(cat, &traits, u64::from(seed));
        let mut views = Vec::new();
        for (i, c) in picked.iter().enumerate() {
            views.push(CandidateView {
                labels: labels(&c.look)?,
                svg: render::render(&c.look, Stage::Seed, &Equipped::default())?,
                look: c.look.clone(),
                starter_top: c.starter_top.clone(),
                name: builtin_name(u64::from(seed), i),
                blurb: builtin_blurb(cat, &c.look),
                name_source: NameSource::Builtin,
            });
        }

        let request = LlmRequest {
            purpose: Purpose::ChatName,
            prompt: Prompt::character_names(
                views
                    .iter()
                    .map(|v| CharacterLook {
                        species: v.labels.species.clone(),
                        palette: v.labels.palette.clone(),
                        expression: v.labels.expression.clone(),
                        prop: v.labels.prop.clone(),
                    })
                    .collect(),
            ),
            max_tokens: NAMES_MAX_TOKENS,
        };
        let fallback_reason = match self.llm.complete(&request) {
            Ok(response) => {
                let names = parse_names(&response.text, views.len());
                let mut all = true;
                for (view, named) in views.iter_mut().zip(names) {
                    match named {
                        Some((name, blurb)) => {
                            view.name = name;
                            view.blurb = blurb;
                            view.name_source = NameSource::Llm;
                        }
                        None => all = false,
                    }
                }
                (!all).then(|| "bad_answer".to_string())
            }
            Err(err) => Some(err.code().to_string()),
        };
        Ok(CandidateSet {
            seed,
            candidates: views,
            fallback_reason,
        })
    }
}
