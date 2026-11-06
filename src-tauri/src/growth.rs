//! Growth view (PLAN 11) and the weekly one-line insight.
//!
//! The growth numbers are local folds: recognized study time over the app's life, level progress
//! from the point ledger, wiki page count, quiz accuracy, and the page to review next (the wiki's
//! own ranking, `wiki::recommend::rank`). Nothing here needs the LLM.
//!
//! The insight sends the model one `InsightInput` of aggregate numbers (no titles, no URLs, no
//! window or app names). The weakest subject's name is part of it only while
//! `growth.insight_subject` is on (default on: subject names are the user's own wiki concepts,
//! like plan data). Without a usable LLM (off, unconfigured, over budget, failing, or answering
//! outside the schema) a local rule writes the sentence and the result says why.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{Local, TimeZone};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::commands::lock_db;
use crate::dashboard;
use crate::error::{AppError, AppResult};
use crate::llm::egress::{InsightInput, Prompt};
use crate::llm::service::LlmService;
use crate::llm::{LlmRequest, Purpose};
use crate::remote::Clock;
use crate::settings::{self, SettingSpec, SettingValue, UpdateOutcome, ValueKind};
use crate::store::growth as repo;
use crate::store::wiki as wiki_repo;
use crate::wiki::recommend::{self, PageStat, Reason};

const CATEGORY: &str = "growth";
const MINUTE_MS: i64 = 60_000;
const INSIGHT_MAX_TOKENS: u32 = 120;
/// Longest sentence accepted from the model.
const INSIGHT_MAX_CHARS: usize = 120;
/// Quiz accuracy (percent) below which the insight points at the quiz.
const LOW_QUIZ_PCT: i64 = 60;
/// Streak from which the insight praises it.
const STREAK_PRAISE_DAYS: i64 = 3;

pub const INSIGHT_SUBJECT: SettingSpec =
    SettingSpec { name: "growth.insight_subject", category: CATEGORY, kind: ValueKind::Bool, default: "true", tighten_to: Some("false") };

pub fn insight_subject_enabled(conn: &Connection) -> AppResult<bool> {
    match settings::read_value(conn, &INSIGHT_SUBJECT)? {
        SettingValue::Bool(v) => Ok(v),
        other => Err(AppError::Internal(format!("{} has the wrong type: {other:?}", INSIGHT_SUBJECT.name))),
    }
}

/// Switching it off narrows what is sent, so it stays allowed during a planned run; switching on
/// is locked like any setting.
pub fn set_insight_subject(conn: &mut Connection, on: bool, at_ms: i64) -> AppResult<UpdateOutcome> {
    settings::apply_setting(conn, &INSIGHT_SUBJECT, &SettingValue::Bool(on), at_ms, || Ok(()))
}

// ---- wire types ----

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NextReview {
    pub subject: String,
    pub page_name: String,
    pub reason: Reason,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GrowthStatus {
    /// Recognized study time since the first record.
    pub total_study_ms: i64,
    pub level: i64,
    pub stage_label: String,
    pub earned_points: i64,
    /// Total earned points at which the next level starts; `None` at the top level.
    pub next_level_at: Option<i64>,
    /// 0-100 inside the current level; 100 at the top level.
    pub level_progress_pct: i64,
    pub wiki_pages: i64,
    pub quiz_answered: i64,
    pub quiz_correct: i64,
    /// `None` until a question has been answered.
    pub quiz_accuracy_pct: Option<i64>,
    pub next_review: Option<NextReview>,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InsightSource {
    Llm,
    Rule,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeeklyInsight {
    pub week_start: String,
    pub text: String,
    pub source: InsightSource,
    /// Why the rule wrote it instead of the model: a stable code (`llm_disabled`, ...), or
    /// `bad_answer`. `None` when the model wrote it.
    pub fallback_reason: Option<String>,
}

// ---- pure ----

fn pct(part: i64, whole: i64) -> Option<i64> {
    (whole > 0).then(|| ((part * 100 + whole / 2) / whole).clamp(0, 100))
}

/// Progress inside the current level, 0-100. The lower bound is the previous threshold.
pub fn level_progress_pct(level: i64, earned: i64, next_level_at: Option<i64>) -> i64 {
    let Some(next) = next_level_at else { return 100 };
    let index = usize::try_from(level - 1).unwrap_or(0);
    let floor = crate::points::rewards::LEVEL_THRESHOLDS.get(index).copied().unwrap_or(0);
    pct(earned - floor, next - floor).unwrap_or(0)
}

/// The page to study next across all given subjects: the wiki's ranking over the union of their
/// pages. `pages` pairs a subject name with its page facts (page ids are global).
pub fn pick_next_review(pages: &[(String, PageStat)], now_ms: i64) -> Option<NextReview> {
    let stats: Vec<PageStat> = pages.iter().map(|(_, p)| p.clone()).collect();
    let pick = recommend::rank(&stats, now_ms).into_iter().next()?;
    let subject = pages.iter().find(|(_, p)| p.page_id == pick.page_id).map(|(s, _)| s.clone())?;
    Some(NextReview { subject, page_name: pick.page_name, reason: pick.reason })
}

/// The sentence written without a model. Deterministic: first matching rule wins.
pub fn rule_insight(input: &InsightInput) -> String {
    if input.planned_min == 0 && input.study_min == 0 {
        return "이번 주는 기록이 없어요. 다음 주 계획을 하나만 먼저 세워 보세요.".to_string();
    }
    if input.planned_min > 0 && input.study_min * 100 < input.planned_min * 80 {
        let done = pct(input.study_min, input.planned_min).unwrap_or(0);
        return format!("이번 주 계획의 {done}%를 했어요. 다음 주는 하루 세트를 하나 줄여서 지킬 수 있는 계획으로 만들어 보세요.");
    }
    if let Some(acc) = input.quiz_accuracy_pct.filter(|a| *a < LOW_QUIZ_PCT) {
        return match &input.weakest_subject {
            Some(subject) => format!("{subject} 퀴즈 정답률이 {acc}%예요. 위키를 먼저 복습하고 시작해 보세요."),
            None => format!("퀴즈 정답률이 {acc}%예요. 위키를 먼저 복습하고 시작해 보세요."),
        };
    }
    if input.streak_days >= STREAK_PRAISE_DAYS {
        return format!("{}일 연속으로 달성 중이에요. 이 페이스를 다음 주에도 이어 가요.", input.streak_days);
    }
    "이번 주 계획을 잘 지켰어요. 오늘 배운 것을 위키에 한 페이지 정리해 보세요.".to_string()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInsight {
    insight: String,
}

/// The sentence from a model answer, or `None` when the answer is not the documented shape, is
/// empty, spans lines, or is too long.
pub fn parse_insight(answer: &str) -> Option<String> {
    let raw: RawInsight = serde_json::from_str(answer.trim()).ok()?;
    let text = raw.insight.trim().to_string();
    let ok = !text.is_empty() && text.chars().count() <= INSIGHT_MAX_CHARS && !text.chars().any(char::is_control);
    ok.then_some(text)
}

// ---- reads ----

fn review_pages(conn: &Connection) -> AppResult<Vec<(String, PageStat)>> {
    let mut out = Vec::new();
    for subject in wiki_repo::list_subjects(conn, false)? {
        for stat in recommend::page_stats(conn, subject.id)? {
            out.push((subject.name.clone(), stat));
        }
    }
    Ok(out)
}

/// Settles the point ledger first (like the dashboard) so level and progress are current.
pub fn status<Tz: TimeZone>(conn: &mut Connection, tz: &Tz, now_ms: i64) -> AppResult<GrowthStatus> {
    let today = dashboard::today(conn, tz, now_ms)?;
    let points = today.points;
    let (answered, correct) = repo::quiz_counts(conn)?;
    Ok(GrowthStatus {
        total_study_ms: repo::total_credited_ms(conn)?,
        level: points.level,
        stage_label: points.stage_label,
        earned_points: points.earned,
        next_level_at: points.next_level_at,
        level_progress_pct: level_progress_pct(points.level, points.earned, points.next_level_at),
        wiki_pages: repo::active_page_count(conn)?,
        quiz_answered: answered,
        quiz_correct: correct,
        quiz_accuracy_pct: pct(correct, answered),
        next_review: pick_next_review(&review_pages(conn)?, now_ms),
    })
}

/// This week's numbers for the model. The weakest subject (lowest quiz accuracy with enough
/// answers) is included only when `include_subject`.
pub fn insight_input<Tz: TimeZone>(conn: &mut Connection, tz: &Tz, now_ms: i64, include_subject: bool) -> AppResult<InsightInput> {
    let week = dashboard::week(conn, tz, now_ms)?;
    let today = dashboard::today(conn, tz, now_ms)?;
    let (answered, correct) = repo::quiz_counts(conn)?;
    let weakest_subject = if include_subject { weakest_subject(conn)? } else { None };
    Ok(InsightInput {
        study_min: week.actual_ms / MINUTE_MS,
        planned_min: week.planned_ms / MINUTE_MS,
        achieved_days: week.achieved_days,
        planned_days: week.planned_days,
        streak_days: today.streak_days,
        quiz_accuracy_pct: pct(correct, answered),
        wiki_pages: repo::active_page_count(conn)?,
        weakest_subject,
    })
}

fn weakest_subject(conn: &Connection) -> AppResult<Option<String>> {
    let mut totals: HashMap<String, (i64, i64)> = HashMap::new();
    for subject in wiki_repo::list_subjects(conn, false)? {
        for page in wiki_repo::list_pages(conn, subject.id)? {
            let entry = totals.entry(subject.name.clone()).or_default();
            entry.0 += page.quiz_total;
            entry.1 += page.quiz_correct;
        }
    }
    let mut ranked: Vec<(i64, String)> = totals
        .into_iter()
        .filter(|(_, (total, _))| *total >= recommend::MIN_ANSWERS)
        .filter_map(|(name, (total, correct))| pct(correct, total).map(|p| (p, name)))
        .collect();
    ranked.sort();
    Ok(ranked.into_iter().next().map(|(_, name)| name))
}

// ---- service ----

pub struct GrowthService {
    db: Arc<Mutex<Connection>>,
    llm: Arc<LlmService>,
    clock: Arc<dyn Clock>,
    /// The model's sentence for the week it was written (rule sentences are cheap and not kept,
    /// so turning the LLM on takes effect at once).
    cache: Mutex<Option<(WeeklyInsight, bool)>>,
}

impl GrowthService {
    pub fn new(db: Arc<Mutex<Connection>>, llm: Arc<LlmService>, clock: Arc<dyn Clock>) -> Self {
        Self { db, llm, clock, cache: Mutex::new(None) }
    }

    pub fn status(&self) -> AppResult<GrowthStatus> {
        let now = self.clock.now_ms()?;
        let mut conn = lock_db(&self.db)?;
        status(&mut conn, &Local, now)
    }

    /// The week's one-line insight. `refresh` asks the model again even if it already answered.
    pub fn weekly_insight(&self, refresh: bool) -> AppResult<WeeklyInsight> {
        let now = self.clock.now_ms()?;
        let (input, include_subject, week_start) = {
            let mut conn = lock_db(&self.db)?;
            let include = insight_subject_enabled(&conn)?;
            let week_start = dashboard::week(&conn, &Local, now)?.week_start;
            (insight_input(&mut conn, &Local, now, include)?, include, week_start)
        };
        if !refresh {
            let cache = self.cache.lock().map_err(|_| AppError::Internal("insight cache poisoned".into()))?;
            if let Some((cached, with_subject)) = cache.as_ref() {
                if cached.week_start == week_start && *with_subject == include_subject {
                    return Ok(cached.clone());
                }
            }
        }
        // The database is unlocked from here on: the call may take as long as the model needs.
        let insight = self.ask_or_rule(&input, week_start);
        if insight.source == InsightSource::Llm {
            *self.cache.lock().map_err(|_| AppError::Internal("insight cache poisoned".into()))? = Some((insight.clone(), include_subject));
        }
        Ok(insight)
    }

    fn ask_or_rule(&self, input: &InsightInput, week_start: String) -> WeeklyInsight {
        let request = LlmRequest { purpose: Purpose::Insight, prompt: Prompt::insight(input.clone()), max_tokens: INSIGHT_MAX_TOKENS };
        let rule = |reason: String| WeeklyInsight { week_start: week_start.clone(), text: rule_insight(input), source: InsightSource::Rule, fallback_reason: Some(reason) };
        match self.llm.complete(&request) {
            Ok(response) => match parse_insight(&response.text) {
                Some(text) => WeeklyInsight { week_start: week_start.clone(), text, source: InsightSource::Llm, fallback_reason: None },
                None => rule("bad_answer".to_string()),
            },
            Err(err) => rule(err.code().to_string()),
        }
    }
}

#[cfg(test)]
mod tests;
