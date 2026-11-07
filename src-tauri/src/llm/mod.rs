//! LLM port. Callers hold an `LlmClient` (through `service::LlmService`) and never learn whether
//! the mock or the real Anthropic adapter answers. Everything that may leave the device is built
//! in `egress`; every call passes the gate in `service` (switch, consent, daily budget) and leaves
//! a content-free row in `llm_usage`.
//!
//! The LLM is never required: a refused or failed call (`Disabled`, `NotConfigured`,
//! `BudgetExceeded`, any transport failure) makes the caller use its local fallback. Locking,
//! timing, and blocking do not depend on this module.

pub mod anthropic;
pub mod config;
pub mod egress;
pub mod mock;
pub mod service;

use serde::Serialize;

use crate::error::AppError;
use crate::remote::http::TransportError;

/// Why a call is made. Part of `llm_usage` and the consent rule; the list is a superset reserved
/// for the later stage-3 stories (the table CHECK is drift-tested against `ALL`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    TabJudge,
    WindowClassify,
    WikiIngest,
    WikiAsk,
    WikiMerge,
    WikiLint,
    QuizGen,
    Message,
    PlanChat,
    PlanAdjust,
    ChatName,
    Insight,
    /// The "test connection" call.
    Probe,
}

impl Purpose {
    pub const ALL: [Purpose; 13] = [
        Purpose::TabJudge,
        Purpose::WindowClassify,
        Purpose::WikiIngest,
        Purpose::WikiAsk,
        Purpose::WikiMerge,
        Purpose::WikiLint,
        Purpose::QuizGen,
        Purpose::Message,
        Purpose::PlanChat,
        Purpose::PlanAdjust,
        Purpose::ChatName,
        Purpose::Insight,
        Purpose::Probe,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::TabJudge => "tab_judge",
            Purpose::WindowClassify => "window_classify",
            Purpose::WikiIngest => "wiki_ingest",
            Purpose::WikiAsk => "wiki_ask",
            Purpose::WikiMerge => "wiki_merge",
            Purpose::WikiLint => "wiki_lint",
            Purpose::QuizGen => "quiz_gen",
            Purpose::Message => "message",
            Purpose::PlanChat => "plan_chat",
            Purpose::PlanAdjust => "plan_adjust",
            Purpose::ChatName => "chat_name",
            Purpose::Insight => "insight",
            Purpose::Probe => "probe",
        }
    }

    /// Purposes that send what the user is looking at (tab or window descriptions) need their own
    /// opt-in on top of `llm.enabled` (`llm.judge_enabled`, off by default).
    pub fn needs_judge_consent(self) -> bool {
        matches!(self, Purpose::TabJudge | Purpose::WindowClassify)
    }
}

/// A wiki page proposal: what ingest and outline answers carry, and what the no-LLM fallback and
/// the mock produce. Names are validated by the wiki before anything is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageDraft {
    pub page_name: String,
    pub markdown: String,
    pub links: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LlmRequest {
    pub purpose: Purpose,
    pub prompt: egress::Prompt,
    pub max_tokens: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmResponse {
    pub text: String,
    pub in_tokens: u32,
    pub out_tokens: u32,
}

/// Failure of a call, as a stable code. No variant carries a URL, a message from the network
/// stack, or any prompt text, so formatting one is always safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    #[error("llm_not_configured")]
    NotConfigured,
    /// `llm.enabled` is off (or the purpose's own consent is).
    #[error("llm_disabled")]
    Disabled,
    #[error("llm_budget_exceeded")]
    BudgetExceeded,
    /// 401 / 403: the key is wrong or revoked.
    #[error("llm_unauthorized")]
    Unauthorized,
    #[error("llm_rate_limited")]
    RateLimited { retry_after_ms: Option<u64> },
    /// 529 and other 5xx: the service is busy or down.
    #[error("llm_overloaded")]
    Overloaded,
    /// Another 4xx: the request itself was refused.
    #[error("llm_rejected")]
    Rejected { status: u16 },
    #[error("llm_timeout")]
    Timeout,
    #[error("llm_host_denied")]
    HostDenied,
    /// Connection failed, I/O error, response too large.
    #[error("llm_unreachable")]
    Unreachable,
    /// The reply was not the JSON shape the API documents.
    #[error("llm_bad_response")]
    BadResponse,
    #[error("llm_internal")]
    Internal,
}

impl LlmError {
    pub fn code(self) -> &'static str {
        match self {
            LlmError::NotConfigured => "llm_not_configured",
            LlmError::Disabled => "llm_disabled",
            LlmError::BudgetExceeded => "llm_budget_exceeded",
            LlmError::Unauthorized => "llm_unauthorized",
            LlmError::RateLimited { .. } => "llm_rate_limited",
            LlmError::Overloaded => "llm_overloaded",
            LlmError::Rejected { .. } => "llm_rejected",
            LlmError::Timeout => "llm_timeout",
            LlmError::HostDenied => "llm_host_denied",
            LlmError::Unreachable => "llm_unreachable",
            LlmError::BadResponse => "llm_bad_response",
            LlmError::Internal => "llm_internal",
        }
    }

    /// Worth another attempt after a pause.
    pub fn is_retryable(self) -> bool {
        matches!(self, LlmError::RateLimited { .. } | LlmError::Overloaded)
    }

    /// The caller should fall back to its local behavior without surfacing a failure: the LLM is
    /// off, unconfigured, or out of budget on purpose.
    pub fn is_unavailable(self) -> bool {
        matches!(
            self,
            LlmError::NotConfigured | LlmError::Disabled | LlmError::BudgetExceeded
        )
    }
}

impl From<TransportError> for LlmError {
    fn from(err: TransportError) -> Self {
        match err {
            TransportError::HostDenied | TransportError::InvalidUrl => LlmError::HostDenied,
            TransportError::Timeout => LlmError::Timeout,
            TransportError::Connect
            | TransportError::Io
            | TransportError::ResponseTooLarge
            | TransportError::ScriptExhausted => LlmError::Unreachable,
        }
    }
}

impl From<LlmError> for AppError {
    fn from(err: LlmError) -> Self {
        match err {
            LlmError::NotConfigured => AppError::NotConfigured { port: "llm" },
            other => AppError::Remote(other.code().to_string()),
        }
    }
}

pub trait LlmClient: Send + Sync {
    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;
}

/// The adapter used when nothing is configured: every call is refused before anything is built.
pub struct NotConfiguredLlm;

impl LlmClient for NotConfiguredLlm {
    fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        Err(LlmError::NotConfigured)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purpose_names_are_unique_and_accepted_by_the_usage_table() {
        let conn = crate::db::open_in_memory().expect("db");
        let mut seen = std::collections::HashSet::new();
        for purpose in Purpose::ALL {
            assert!(seen.insert(purpose.as_str()), "duplicate {purpose:?}");
            crate::store::llm::insert_usage(
                &conn,
                &crate::store::llm::NewUsage {
                    at_ms: 1,
                    purpose: purpose.as_str(),
                    model: "m",
                    mode: "mock",
                    in_tokens: 0,
                    out_tokens: 0,
                    latency_ms: 0,
                    cached: false,
                    result_code: "ok",
                },
            )
            .unwrap_or_else(|err| panic!("{purpose:?} rejected by llm_usage CHECK: {err}"));
        }
        let json = serde_json::to_value(Purpose::WikiAsk).expect("json");
        assert_eq!(json, serde_json::json!("wiki_ask"));
    }

    #[test]
    fn only_screen_describing_purposes_need_the_judge_consent() {
        let needing: Vec<_> = Purpose::ALL
            .iter()
            .filter(|p| p.needs_judge_consent())
            .collect();
        assert_eq!(needing, [&Purpose::TabJudge, &Purpose::WindowClassify]);
    }

    #[test]
    fn errors_have_stable_codes_and_never_echo_content() {
        assert_eq!(LlmError::from(TransportError::Timeout), LlmError::Timeout);
        assert_eq!(
            LlmError::from(TransportError::HostDenied).code(),
            "llm_host_denied"
        );
        assert!(
            LlmError::RateLimited {
                retry_after_ms: Some(1)
            }
            .is_retryable()
        );
        assert!(!LlmError::Unauthorized.is_retryable());
        assert!(LlmError::BudgetExceeded.is_unavailable() && !LlmError::Timeout.is_unavailable());
        assert!(matches!(
            AppError::from(LlmError::NotConfigured),
            AppError::NotConfigured { port: "llm" }
        ));
        assert_eq!(
            AppError::from(LlmError::Overloaded).to_string(),
            "remote error: llm_overloaded"
        );
    }
}
