//! One browser connection from first frame to disconnect: authenticate, judge tab events, answer
//! each with exactly one verdict, record the link's coming and going.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Local;
use rusqlite::Connection;

use super::protocol::{
    domain_of, encode, is_valid_ext_id, is_valid_version, parse_incoming, read_frame, write_frame, FrameError,
    Incoming, Outgoing, TabKind, VerdictKind, PROTOCOL_VERSION,
};
use super::LinkHub;
use crate::clock;
use crate::commands::lock_db;
use crate::enforce::model::{BlockAction, EnforceMode, NewBlockEvent, TargetKind};
use crate::enforce::{self, StudyWindow};
use crate::error::AppResult;
use crate::platform::LinkStream;
use crate::site_rules::{self, Situation, SiteRuleEngine};
use crate::store;
use crate::timer::{self, HighWater};

/// Reason codes the extension and the log see. Fixed identifiers, never free text.
pub const REASON_NOT_WEB: &str = site_rules::REASON_NOT_WEB;
pub const REASON_OUTSIDE_STUDY: &str = site_rules::REASON_OUTSIDE_STUDY;
pub const REASON_RATE_LIMITED: &str = "rate_limited";
pub const REASON_DRY_RUN: &str = "dry_run";
pub const REASON_APP_ERROR: &str = site_rules::REASON_APP_ERROR;
/// The judge returns this when a pass lets a site through that rules would block.
pub const REASON_SITE_ALLOWED_PASS: &str = site_rules::REASON_SITE_ALLOWED_PASS;

/// Pre-auth reads give up quickly so a silent local process cannot hold a thread.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// At most this many tab events per window; the rest are answered "allow" without judging.
const RATE_LIMIT_EVENTS: u32 = 60;
const RATE_WINDOW_MS: i64 = 10_000;

/// What a judge sees of one tab inside a locked study block. `url` and `title` live only for the
/// call: nothing here may store or log them.
pub struct TabContext<'a> {
    pub url: &'a str,
    /// The tab's caption, when the extension sent one. Only the LLM judge reads it.
    pub title: Option<&'a str>,
    pub window: StudyWindow,
    /// An emergency pass is running: blocks become `site_allowed_pass`.
    pub pass_active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub kind: VerdictKind,
    pub reason_code: &'static str,
}

impl Verdict {
    pub fn allow(reason_code: &'static str) -> Verdict {
        Verdict { kind: VerdictKind::Allow, reason_code }
    }

    pub fn block(reason_code: &'static str) -> Verdict {
        Verdict { kind: VerdictKind::Block, reason_code }
    }
}

/// Decides a tab inside a locked study block. The LLM judge (`llm_judge`) sits behind the same
/// trait, in front of `RulesJudge`.
pub trait TabJudge: Send + Sync {
    fn judge(&self, tab: &TabContext<'_>) -> Verdict;
}

/// The site-rule engine behind the link: block/allow lists, strict mode, YouTube study entries,
/// with "LLM off" as the fallback for unlisted sites.
pub struct RulesJudge {
    engine: SiteRuleEngine,
}

impl RulesJudge {
    pub fn new(db: Arc<Mutex<Connection>>) -> RulesJudge {
        RulesJudge { engine: SiteRuleEngine::new(db) }
    }
}

impl TabJudge for RulesJudge {
    fn judge(&self, tab: &TabContext<'_>) -> Verdict {
        let scope = (tab.window.run_id, tab.window.segment_id);
        let situation = Situation { in_study: true, pass_active: tab.pass_active };
        let decision = self.engine.decide(tab.url, scope, situation);
        if decision.block {
            Verdict::block(decision.reason)
        } else {
            Verdict::allow(decision.reason)
        }
    }
}

/// Clock and study-window lookup, replaceable in tests.
pub trait LinkEnv: Send + Sync {
    fn now_ms(&self) -> AppResult<i64>;
    /// `Some` only inside a locked study block.
    fn study_window(&self, conn: &Connection, now_ms: i64) -> AppResult<Option<StudyWindow>>;
}

pub struct SystemEnv {
    high_water: Arc<HighWater>,
}

impl SystemEnv {
    pub fn new(high_water: Arc<HighWater>) -> SystemEnv {
        SystemEnv { high_water }
    }
}

impl LinkEnv for SystemEnv {
    fn now_ms(&self) -> AppResult<i64> {
        Ok(self.high_water.observe(clock::now_ms()?))
    }

    fn study_window(&self, conn: &Connection, now_ms: i64) -> AppResult<Option<StudyWindow>> {
        let state = timer::current_state(conn, &Local, now_ms)?;
        Ok(enforce::study_window(&state))
    }
}

/// Fixed-window counter: cheap and enough to stop a runaway sender.
struct RateLimiter {
    window_start_ms: i64,
    count: u32,
}

impl RateLimiter {
    fn new() -> RateLimiter {
        RateLimiter { window_start_ms: i64::MIN, count: 0 }
    }

    /// Returns whether this event is within budget, and whether it is the first one over.
    fn admit(&mut self, now_ms: i64) -> Admission {
        if self.window_start_ms == i64::MIN || now_ms.saturating_sub(self.window_start_ms) >= RATE_WINDOW_MS {
            self.window_start_ms = now_ms;
            self.count = 0;
        }
        self.count += 1;
        match self.count {
            n if n <= RATE_LIMIT_EVENTS => Admission::Admitted,
            n if n == RATE_LIMIT_EVENTS + 1 => Admission::FirstOver,
            _ => Admission::Over,
        }
    }
}

#[derive(PartialEq, Eq)]
enum Admission {
    Admitted,
    FirstOver,
    Over,
}

pub struct Service {
    db: Arc<Mutex<Connection>>,
    hub: Arc<LinkHub>,
    env: Arc<dyn LinkEnv>,
    judge: Arc<dyn TabJudge>,
    token: String,
    allowed_ext_ids: Vec<String>,
}

fn tokens_match(expected: &str, given: &str) -> bool {
    // No early exit on the first differing byte.
    expected.len() == given.len()
        && expected.bytes().zip(given.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

fn send(stream: &mut dyn LinkStream, message: &Outgoing) -> bool {
    write_frame(stream, &encode(message)).is_ok()
}

impl Service {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        hub: Arc<LinkHub>,
        env: Arc<dyn LinkEnv>,
        judge: Arc<dyn TabJudge>,
        token: String,
        allowed_ext_ids: Vec<String>,
    ) -> Service {
        Service { db, hub, env, judge, token, allowed_ext_ids }
    }

    /// Runs one connection to its end. The disconnect is recorded on every path out of an
    /// established link, including a panic inside the message loop.
    pub fn serve(&self, stream: &mut dyn LinkStream) {
        let Some(ext_id) = self.handshake(stream) else { return };
        let connected_at = self.record_link(&ext_id, BlockAction::ExtensionConnected, None);
        self.hub.connected(connected_at);
        tracing::info!(event = "extension_connected");
        let reason = catch_unwind(AssertUnwindSafe(|| self.pump(stream))).unwrap_or("internal_error");
        let at = self.record_link(&ext_id, BlockAction::ExtensionDisconnected, Some(reason));
        self.hub.disconnected(at, reason);
        tracing::info!(event = "extension_disconnected", reason);
    }

    /// Auth frame (from the host) then hello (from the extension). Returns the verified
    /// extension ID, or `None` after refusing the peer without any record or status change.
    fn handshake(&self, stream: &mut dyn LinkStream) -> Option<String> {
        if stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).is_err() {
            return None;
        }
        let refuse = |stream: &mut dyn LinkStream, code: &'static str| {
            tracing::warn!(event = "browser_link_refused", reason = code);
            send(stream, &Outgoing::Error { code });
            None
        };
        let origin = match self.read_message(stream) {
            Ok(Incoming::Auth { token, origin_ext_id }) if tokens_match(&self.token, &token) => origin_ext_id,
            Ok(Incoming::Auth { .. }) => return refuse(stream, "unauthorized"),
            Ok(_) => return refuse(stream, "protocol_error"),
            Err(code) => return refuse(stream, code),
        };
        let ext_id = match self.read_message(stream) {
            Ok(Incoming::Hello { ext_id, version }) if is_valid_version(&version) => ext_id,
            Ok(Incoming::Hello { .. }) => return refuse(stream, "bad_version"),
            Ok(_) => return refuse(stream, "protocol_error"),
            Err(code) => return refuse(stream, code),
        };
        // The origin comes from Chrome via the host; the hello is the extension's own claim.
        let known = is_valid_ext_id(&ext_id) && self.allowed_ext_ids.contains(&ext_id);
        if !known || ext_id != origin {
            return refuse(stream, "unknown_extension");
        }
        if stream.set_read_timeout(None).is_err() || !send(stream, &Outgoing::HelloAck { protocol: PROTOCOL_VERSION }) {
            return None;
        }
        Some(ext_id)
    }

    fn read_message(&self, stream: &mut dyn LinkStream) -> Result<Incoming, &'static str> {
        match read_frame(stream) {
            Ok(Some(frame)) => parse_incoming(&frame).map_err(|_| "bad_message"),
            Ok(None) => Err("closed"),
            Err(FrameError::TooLarge) => Err("frame_too_large"),
            Err(FrameError::Truncated) => Err("truncated"),
            Err(FrameError::Io(_)) => Err("io_error"),
        }
    }

    /// Answers tab events until the link ends; returns the disconnect reason.
    fn pump(&self, stream: &mut dyn LinkStream) -> &'static str {
        let mut limiter = RateLimiter::new();
        loop {
            let message = match self.read_message(stream) {
                Ok(message) => message,
                Err(code) => {
                    // A peer that is gone cannot read the error; a misbehaving one can.
                    if matches!(code, "bad_message" | "frame_too_large") {
                        send(stream, &Outgoing::Error { code });
                    }
                    return if code == "bad_message" { "protocol_error" } else { code };
                }
            };
            let Incoming::TabEvent { url, tab_id, kind, title } = message else {
                send(stream, &Outgoing::Error { code: "protocol_error" });
                return "protocol_error";
            };
            let verdict = self.handle_tab(&url, title.as_deref(), kind, &mut limiter);
            drop(url);
            drop(title);
            let reply = Outgoing::Verdict { tab_id, verdict: verdict.kind, reason_code: verdict.reason_code };
            if !send(stream, &reply) {
                return "write_failed";
            }
        }
    }

    /// Every failure inside resolves to allow: the link never blocks on its own malfunction.
    fn handle_tab(&self, url: &str, title: Option<&str>, kind: TabKind, limiter: &mut RateLimiter) -> Verdict {
        let now = match self.env.now_ms() {
            Ok(now) => now,
            Err(err) => {
                tracing::error!(error = %err, "browser link clock unavailable");
                return Verdict::allow(REASON_APP_ERROR);
            }
        };
        match limiter.admit(now) {
            Admission::Admitted => {}
            Admission::FirstOver => {
                tracing::warn!(event = "browser_link_rate_limited", limit = RATE_LIMIT_EVENTS);
                return Verdict::allow(REASON_RATE_LIMITED);
            }
            Admission::Over => return Verdict::allow(REASON_RATE_LIMITED),
        }
        let Some(domain) = domain_of(url) else { return Verdict::allow(REASON_NOT_WEB) };
        let context = (|| -> AppResult<(Option<StudyWindow>, EnforceMode, bool)> {
            let conn = lock_db(&self.db)?;
            let window = self.env.study_window(&conn, now)?;
            let pass_active = timer::study_pass::active_until(&conn, now)?.is_some();
            Ok((window, enforce::read_config(&conn)?.mode, pass_active))
        })();
        let (window, mode, pass_active) = match context {
            Ok(context) => context,
            Err(err) => {
                tracing::error!(error = %err, "browser link context unavailable");
                return Verdict::allow(REASON_APP_ERROR);
            }
        };
        let Some(window) = window else { return Verdict::allow(REASON_OUTSIDE_STUDY) };
        let verdict = self.judge.judge(&TabContext { url, title, window, pass_active });
        let action = match (verdict.kind, verdict.reason_code) {
            (VerdictKind::Block, _) => Some(BlockAction::SiteBlocked),
            (VerdictKind::Allow, REASON_SITE_ALLOWED_PASS) => Some(BlockAction::SiteAllowedPass),
            _ => None,
        };
        if let Some(action) = action {
            self.record_site(now, window, mode, &domain, action, verdict.reason_code);
            tracing::info!(
                event = action.as_str(),
                domain = %domain,
                reason = verdict.reason_code,
                tab_event = kind.as_str()
            );
        }
        // Dry-run records what would have been blocked and lets the page load.
        if verdict.kind == VerdictKind::Block && mode == EnforceMode::DryRun {
            return Verdict::allow(REASON_DRY_RUN);
        }
        verdict
    }

    fn record_site(
        &self,
        at_ms: i64,
        window: StudyWindow,
        mode: EnforceMode,
        domain: &str,
        action: BlockAction,
        reason: &str,
    ) {
        let event = NewBlockEvent {
            at_ms,
            run_id: window.run_id,
            segment_id: window.segment_id,
            target_kind: TargetKind::Site,
            target_id: domain,
            action,
            mode,
            detail_code: Some(reason),
        };
        let written = lock_db(&self.db).and_then(|conn| store::insert_block_event(&conn, &event));
        if let Err(err) = written {
            tracing::error!(error = %err, "failed to record site event");
        }
    }

    /// Records a connection event and returns its timestamp (0 only if the clock failed, which
    /// is logged; the status still updates).
    fn record_link(&self, ext_id: &str, action: BlockAction, reason: Option<&str>) -> i64 {
        let now = match self.env.now_ms() {
            Ok(now) => now,
            Err(err) => {
                tracing::error!(error = %err, "browser link clock unavailable; event not recorded");
                return 0;
            }
        };
        let written = lock_db(&self.db).and_then(|conn| {
            let window = self.env.study_window(&conn, now).unwrap_or(None);
            let mode = enforce::read_config(&conn)?.mode;
            store::insert_block_event(
                &conn,
                &NewBlockEvent {
                    at_ms: now,
                    run_id: window.and_then(|w| w.run_id),
                    segment_id: window.and_then(|w| w.segment_id),
                    target_kind: TargetKind::Extension,
                    target_id: ext_id,
                    action,
                    mode,
                    detail_code: reason,
                },
            )
        });
        if let Err(err) = written {
            tracing::error!(error = %err, "failed to record extension link event");
        }
        now
    }
}
