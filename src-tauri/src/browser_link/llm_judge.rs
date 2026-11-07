//! LLM judgement of tabs the rules cannot decide (feature 2, PLAN 5; default OFF, O-7).
//!
//! Order: the rule judge first. Only a tab the rules leave open ("unlisted" in normal or strict
//! mode) is a candidate. A candidate is looked up in `tab_verdict_cache`; only a miss starts a
//! model call, and that call runs on a worker thread. The tab that triggered it is answered at
//! once with the rule verdict (normal mode allows, strict mode blocks: strict stays fail-closed);
//! the model's answer lands in the cache and decides the next event for that page. Nothing here
//! can slow a verdict, and a failing model changes nothing.
//!
//! What may leave the device (checked again at every call by `llm.enabled` + `llm.judge_enabled`):
//! the study topic, the tab's domain, and a shortened tab label. Never a path, query, fragment, the
//! full address, or anything about another tab. Neither the label nor the address is stored: the
//! cache keeps a hash of (domain, label) and the domain.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;

use super::protocol::domain_of;
use super::service::{REASON_SITE_ALLOWED_PASS, TabContext, TabJudge, Verdict};
use crate::commands::lock_db;
use crate::llm::config::LlmConfig;
use crate::llm::egress::{MAX_TAB_LABEL_CHARS, Prompt};
use crate::llm::service::LlmService;
use crate::llm::{LlmRequest, Purpose};
use crate::remote::Clock;
use crate::site_rules::{REASON_STRICT_UNLISTED, REASON_UNLISTED_ALLOWED};
use crate::store::tabcache::{self, CachedVerdict};
use crate::wiki::index::content_hash;

pub const REASON_LLM_STUDY: &str = "llm_allowed";
pub const REASON_LLM_DISTRACTION: &str = "llm_blocked";

/// A cached verdict is trusted this long.
const CACHE_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// After a call starts (or fails) the same page is not asked about again for this long.
const RETRY_AFTER_MS: i64 = 60_000;
const DEFAULT_TOPIC: &str = "공부";
const TAB_MAX_TOKENS: u32 = 64;

/// Where the model call runs. Production uses a thread; tests queue the job and run it by hand.
pub trait JobRunner: Send + Sync {
    fn run(&self, job: Box<dyn FnOnce() + Send>);
}

pub struct ThreadRunner;

impl JobRunner for ThreadRunner {
    fn run(&self, job: Box<dyn FnOnce() + Send>) {
        thread::spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
                tracing::error!("tab judge job panicked");
            }
        });
    }
}

pub struct LlmTabJudge {
    rules: Arc<dyn TabJudge>,
    db: Arc<Mutex<Connection>>,
    llm: Arc<LlmService>,
    clock: Arc<dyn Clock>,
    runner: Arc<dyn JobRunner>,
    /// Cache key -> time before which the page is not asked about again.
    busy_until: Arc<Mutex<HashMap<String, i64>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVerdict {
    verdict: String,
}

/// The model's answer: exactly `{"verdict":"study"}` or `{"verdict":"distraction"}`.
pub fn parse_verdict(answer: &str) -> Option<CachedVerdict> {
    let raw: RawVerdict = serde_json::from_str(crate::wiki::ingest::json_object(answer)?).ok()?;
    match raw.verdict.as_str() {
        "study" => Some(CachedVerdict::Study),
        "distraction" => Some(CachedVerdict::Distraction),
        _ => None,
    }
}

/// The rules left this tab undecided.
fn is_ambiguous(reason: &str) -> bool {
    reason == REASON_UNLISTED_ALLOWED || reason == REASON_STRICT_UNLISTED
}

/// A tab caption as the model may see it: one line, no control characters, shortened; empty when
/// it looks like an address (which would carry a path or query).
pub fn tab_label(raw: Option<&str>) -> String {
    let Some(raw) = raw else { return String::new() };
    let flat: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let text = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.contains("://") || (text.contains('?') && text.contains('=')) {
        return String::new();
    }
    text.chars().take(MAX_TAB_LABEL_CHARS).collect()
}

/// Hash of (domain, normalized label): the cache key. 16 hex characters.
pub fn cache_key(domain: &str, label: &str) -> String {
    content_hash(&format!("{domain}\0{}", label.to_lowercase()))
}

impl LlmTabJudge {
    pub fn new(
        rules: Arc<dyn TabJudge>,
        db: Arc<Mutex<Connection>>,
        llm: Arc<LlmService>,
        clock: Arc<dyn Clock>,
        runner: Arc<dyn JobRunner>,
    ) -> Self {
        Self {
            rules,
            db,
            llm,
            clock,
            runner,
            busy_until: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn topic(conn: &Connection, run_id: Option<i64>) -> String {
        let Some(run_id) = run_id else {
            return DEFAULT_TOPIC.to_string();
        };
        conn.query_row(
            "SELECT p.subject FROM plan_runs r JOIN plans p ON p.id = r.plan_id WHERE r.id = ?1",
            [run_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_TOPIC.to_string())
    }

    fn from_cache(verdict: CachedVerdict, pass_active: bool) -> Verdict {
        match verdict {
            CachedVerdict::Study => Verdict::allow(REASON_LLM_STUDY),
            CachedVerdict::Distraction if pass_active => Verdict::allow(REASON_SITE_ALLOWED_PASS),
            CachedVerdict::Distraction => Verdict::block(REASON_LLM_DISTRACTION),
        }
    }

    /// Starts one model call for a page unless it was asked about a moment ago.
    fn schedule(&self, key: String, domain: String, label: String, topic: String, now: i64) {
        {
            let mut busy = self
                .busy_until
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if busy.get(&key).is_some_and(|until| *until > now) {
                return;
            }
            if busy.len() > 512 {
                busy.retain(|_, until| *until > now);
            }
            busy.insert(key.clone(), now + RETRY_AFTER_MS);
        }
        let (llm, db, clock, busy_until) = (
            Arc::clone(&self.llm),
            Arc::clone(&self.db),
            Arc::clone(&self.clock),
            Arc::clone(&self.busy_until),
        );
        self.runner.run(Box::new(move || {
            let request = LlmRequest {
                purpose: Purpose::TabJudge,
                prompt: Prompt::tab_judge(&topic, &domain, &label),
                max_tokens: TAB_MAX_TOKENS,
            };
            let verdict = llm
                .complete(&request)
                .ok()
                .and_then(|reply| parse_verdict(&reply.text));
            let Some(verdict) = verdict else {
                tracing::info!(event = "tab_judge_no_verdict");
                return;
            };
            let stored = clock.now_ms().and_then(|at| {
                lock_db(&db).and_then(|conn| tabcache::put(&conn, &key, &domain, verdict, at))
            });
            match stored {
                Ok(()) => {
                    busy_until
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&key);
                    tracing::info!(event = "tab_judge_cached", verdict = verdict.as_str());
                }
                Err(err) => tracing::error!(event = "tab_judge_cache_failed", error = %err),
            }
        }));
    }
}

impl TabJudge for LlmTabJudge {
    fn judge(&self, tab: &TabContext<'_>) -> Verdict {
        let base = self.rules.judge(tab);
        if !is_ambiguous(base.reason_code) {
            return base;
        }
        let Some(domain) = domain_of(tab.url) else {
            return base;
        };
        let Ok(now) = self.clock.now_ms() else {
            return base;
        };
        let label = tab_label(tab.title);
        let key = cache_key(&domain, &label);
        let (topic, cached) = {
            let Ok(conn) = lock_db(&self.db) else {
                return base;
            };
            match LlmConfig::load(&conn) {
                Ok(config) if config.enabled && config.judge_enabled => {}
                _ => return base,
            }
            (
                Self::topic(&conn, tab.window.run_id),
                tabcache::get(&conn, &key, now - CACHE_TTL_MS)
                    .ok()
                    .flatten(),
            )
        };
        if let Some(verdict) = cached {
            return Self::from_cache(verdict, tab.pass_active);
        }
        self.schedule(key, domain, label, topic, now);
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enforce::StudyWindow;
    use crate::llm::LlmError;
    use crate::llm::config::{LlmSettingKey, update_setting};
    use crate::llm::mock::MockStep;
    use crate::llm::service::tests::{Harness, mock_harness};
    use crate::settings::SettingValue;

    /// Queues jobs so a test decides when "the worker thread" runs.
    #[derive(Default)]
    struct ManualRunner {
        jobs: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    }

    impl ManualRunner {
        fn pending(&self) -> usize {
            self.jobs.lock().expect("jobs").len()
        }

        fn run_all(&self) {
            let jobs: Vec<_> = std::mem::take(&mut *self.jobs.lock().expect("jobs"));
            for job in jobs {
                job();
            }
        }
    }

    impl JobRunner for ManualRunner {
        fn run(&self, job: Box<dyn FnOnce() + Send>) {
            self.jobs.lock().expect("jobs").push(job);
        }
    }

    struct FixedRules(Verdict);

    impl TabJudge for FixedRules {
        fn judge(&self, _tab: &TabContext<'_>) -> Verdict {
            self.0
        }
    }

    struct Rig {
        h: Harness,
        judge: LlmTabJudge,
        runner: Arc<ManualRunner>,
    }

    fn rig(rules: Verdict, judge_enabled: bool) -> Rig {
        let h = mock_harness();
        {
            let mut conn = h.db.lock().expect("db");
            update_setting(
                &mut conn,
                LlmSettingKey::JudgeEnabled,
                SettingValue::Bool(judge_enabled),
                1,
            )
            .expect("judge switch");
        }
        let runner = Arc::new(ManualRunner::default());
        let judge = LlmTabJudge::new(
            Arc::new(FixedRules(rules)),
            Arc::clone(&h.db),
            Arc::clone(&h.service),
            h.clock.clone(),
            runner.clone(),
        );
        Rig { h, judge, runner }
    }

    fn tab<'a>(url: &'a str, title: Option<&'a str>) -> TabContext<'a> {
        TabContext {
            url,
            title,
            window: StudyWindow {
                run_id: None,
                segment_id: None,
            },
            pass_active: false,
        }
    }

    const UNLISTED: fn() -> Verdict = || Verdict::allow(REASON_UNLISTED_ALLOWED);
    const STRICT: fn() -> Verdict = || Verdict::block(REASON_STRICT_UNLISTED);
    const URL: &str = "https://site.example:8443/private/path-xyz?q=SECRETQ#frag-secret";

    #[test]
    fn a_tab_the_rules_decide_never_reaches_the_model_or_the_cache() {
        for rules in [
            Verdict::block("blocked_site"),
            Verdict::allow("allowed_listed"),
            Verdict::allow("allowed_study_video"),
            Verdict::allow("site_allowed_pass"),
        ] {
            let r = rig(rules, true);
            assert_eq!(r.judge.judge(&tab(URL, Some("title"))), rules);
            assert_eq!(r.runner.pending(), 0);
            assert_eq!(r.h.remote.mock_llm().call_count(), 0);
        }
    }

    #[test]
    fn with_the_judge_off_nothing_is_ever_sent_even_for_undecided_tabs() {
        let r = rig(UNLISTED(), false);
        for _ in 0..3 {
            assert_eq!(r.judge.judge(&tab(URL, Some("title"))), UNLISTED());
        }
        assert_eq!(r.runner.pending(), 0);
        r.runner.run_all();
        assert_eq!(r.h.remote.mock_llm().call_count(), 0);

        // The master switch off with the judge consent on: still nothing.
        let r = rig(UNLISTED(), true);
        update_setting(
            &mut r.h.db.lock().expect("db"),
            LlmSettingKey::Enabled,
            SettingValue::Bool(false),
            2,
        )
        .expect("off");
        r.judge.judge(&tab(URL, Some("title")));
        assert_eq!(r.runner.pending(), 0);
        assert_eq!(r.h.remote.mock_llm().call_count(), 0);
    }

    #[test]
    fn the_first_event_gets_the_rule_verdict_at_once_and_the_answer_decides_the_next_one() {
        let r = rig(UNLISTED(), true);
        r.h.remote
            .mock_llm()
            .push(MockStep::Text("{\"verdict\":\"distraction\"}".into()));
        assert_eq!(
            r.judge.judge(&tab(URL, Some("재밌는 영상"))),
            UNLISTED(),
            "normal mode allows while the model thinks"
        );
        assert_eq!(
            r.h.remote.mock_llm().call_count(),
            0,
            "no model call on the judging path"
        );
        assert_eq!(r.runner.pending(), 1);
        assert_eq!(
            r.judge.judge(&tab(URL, Some("재밌는 영상"))),
            UNLISTED(),
            "still waiting: no duplicate job"
        );
        assert_eq!(r.runner.pending(), 1);
        r.runner.run_all();
        assert_eq!(r.h.remote.mock_llm().call_count(), 1);
        assert_eq!(
            r.judge.judge(&tab(URL, Some("재밌는 영상"))),
            Verdict::block(REASON_LLM_DISTRACTION)
        );
        assert_eq!(
            r.h.remote.mock_llm().call_count(),
            1,
            "a cache hit costs nothing"
        );
        assert_eq!(r.runner.pending(), 0);
    }

    #[test]
    fn a_study_verdict_opens_an_unlisted_site_in_strict_mode_and_failure_keeps_it_closed() {
        let r = rig(STRICT(), true);
        r.h.remote
            .mock_llm()
            .push(MockStep::Fail(LlmError::Timeout));
        assert_eq!(
            r.judge
                .judge(&tab("https://docs.example/guide", Some("API 가이드"))),
            STRICT()
        );
        r.runner.run_all();
        assert_eq!(
            r.judge
                .judge(&tab("https://docs.example/guide", Some("API 가이드"))),
            STRICT(),
            "a failed call changes nothing"
        );
        assert_eq!(
            r.runner.pending(),
            0,
            "and the page is not asked again at once"
        );

        let r = rig(STRICT(), true);
        r.h.remote
            .mock_llm()
            .push(MockStep::Text("{\"verdict\":\"study\"}".into()));
        r.judge
            .judge(&tab("https://docs.example/guide", Some("API 가이드")));
        r.runner.run_all();
        assert_eq!(
            r.judge
                .judge(&tab("https://docs.example/guide", Some("API 가이드"))),
            Verdict::allow(REASON_LLM_STUDY)
        );
    }

    #[test]
    fn a_pass_turns_a_cached_distraction_into_a_pass_allowance() {
        let r = rig(UNLISTED(), true);
        r.h.remote
            .mock_llm()
            .push(MockStep::Text("{\"verdict\":\"distraction\"}".into()));
        r.judge.judge(&tab(URL, None));
        r.runner.run_all();
        let mut during_pass = tab(URL, None);
        during_pass.pass_active = true;
        assert_eq!(
            r.judge.judge(&during_pass),
            Verdict::allow(REASON_SITE_ALLOWED_PASS)
        );
    }

    #[test]
    fn the_payload_has_the_domain_and_a_short_label_and_nothing_else_of_the_address_or_other_tabs()
    {
        let r = rig(UNLISTED(), true);
        r.judge
            .judge(&tab(URL, Some("Study notes OTHER-TAB-CANARY")));
        r.judge.judge(&tab(
            "https://second.example/page?token=abc",
            Some("Second tab"),
        ));
        r.runner.run_all();
        let calls = r.h.remote.mock_llm().calls();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|c| c.purpose == Purpose::TabJudge));
        let first = &calls[0].user;
        assert!(
            first.contains("site.example") && first.contains("Study notes"),
            "{first}"
        );
        for forbidden in [
            "private",
            "path-xyz",
            "SECRETQ",
            "frag-secret",
            "8443",
            "https://",
            "token=abc",
            "second.example",
            "Second tab",
        ] {
            assert!(
                !first.contains(forbidden),
                "first request leaked `{forbidden}`: {first}"
            );
            assert!(!calls[0].system.contains(forbidden));
        }
        assert!(
            !calls[1].user.contains("OTHER-TAB-CANARY") && !calls[1].user.contains("site.example"),
            "{}",
            calls[1].user
        );
        assert!(!calls[1].user.contains("token=abc"));
    }

    #[test]
    fn the_cache_holds_a_hash_and_the_domain_never_the_label_or_address() {
        let r = rig(UNLISTED(), true);
        r.judge.judge(&tab(URL, Some("Study notes CANARY-LABEL")));
        r.runner.run_all();
        let conn = r.h.db.lock().expect("db");
        let rows: Vec<(String, String, String)> = {
            let mut stmt = conn
                .prepare("SELECT key_hash, domain, verdict FROM tab_verdict_cache")
                .expect("prepare");
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .expect("rows")
                .collect::<Result<_, _>>()
                .expect("collect")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "site.example");
        let dump = format!("{rows:?}");
        for forbidden in [
            "CANARY-LABEL",
            "Study notes",
            "private",
            "path-xyz",
            "SECRETQ",
            "frag-secret",
            "https",
        ] {
            assert!(
                !dump.contains(forbidden),
                "cache leaked `{forbidden}`: {dump}"
            );
        }
        assert_eq!(rows[0].0.len(), 16);
    }

    #[test]
    fn labels_that_look_like_addresses_are_dropped_and_long_ones_cut() {
        assert_eq!(tab_label(Some("https://x.example/a?b=c")), "");
        assert_eq!(tab_label(Some("search?q=secret")), "");
        assert_eq!(tab_label(None), "");
        assert_eq!(tab_label(Some("a\nb\t c")), "a b c");
        assert_eq!(
            tab_label(Some(&"가".repeat(500))).chars().count(),
            MAX_TAB_LABEL_CHARS
        );
        assert_ne!(cache_key("a.test", "x"), cache_key("a.test", "y"));
        assert_eq!(cache_key("a.test", "X"), cache_key("a.test", "x"));
    }

    #[test]
    fn answers_are_read_strictly() {
        assert_eq!(
            parse_verdict("{\"verdict\":\"study\"}"),
            Some(CachedVerdict::Study)
        );
        assert_eq!(
            parse_verdict("```json\n{\"verdict\":\"distraction\"}\n```"),
            Some(CachedVerdict::Distraction)
        );
        assert_eq!(parse_verdict("{\"verdict\":\"maybe\"}"), None);
        assert_eq!(parse_verdict("{\"verdict\":\"study\",\"why\":\"x\"}"), None);
        assert_eq!(parse_verdict("study"), None);
    }
}
