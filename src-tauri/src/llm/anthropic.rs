//! Anthropic Messages API adapter (`POST {base}/v1/messages`). The key rides in the `x-api-key`
//! header only: it is never in a body, a URL, a log line, or a `Debug` output. The transport
//! enforces the host policy (product: exactly `api.anthropic.com`); this file is tested against
//! scripted responses and a loopback fake server, never against the real host.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;

use super::egress::MessagesRequest;
use super::{LlmClient, LlmError, LlmRequest, LlmResponse};
use crate::remote::http::{
    CONNECT_TIMEOUT, HttpRequest, HttpResponse, HttpTransport, Method, RequestTag, TransportError,
    UreqTransport, backoff_ms,
};
use crate::remote::payload::Body;
use crate::remote::runtime::TransportFactory;
use crate::remote::secrets::SecretValue;

pub const DEFAULT_BASE: &str = "https://api.anthropic.com";
pub const HOST: &str = "api.anthropic.com";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Whole-request limit for a model call.
pub const LLM_TIMEOUT: Duration = Duration::from_secs(30);
/// Attempts per call, counting the first. Only 429 and 5xx are retried.
pub const MAX_ATTEMPTS: u32 = 3;
/// A server asking for a longer pause than this is not waited for: the caller falls back.
const MAX_WAIT_MS: u64 = 30_000;

const TAG: RequestTag = RequestTag {
    port: "llm",
    kind: "messages",
};

/// Transport for the product: real network, 30 second limit.
pub fn ureq_factory() -> TransportFactory {
    Arc::new(|policy| {
        Arc::new(UreqTransport::with_timeouts(
            policy,
            CONNECT_TIMEOUT,
            LLM_TIMEOUT,
        ))
    })
}

/// Pause between retries; injectable so backoff is tested without sleeping.
pub trait Sleeper: Send + Sync {
    fn sleep_ms(&self, ms: u64);
}

pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// Records the requested pauses instead of waiting.
#[derive(Default)]
pub struct RecordingSleeper {
    slept: Mutex<Vec<u64>>,
}

impl RecordingSleeper {
    pub fn slept(&self) -> Vec<u64> {
        match self.slept.lock() {
            Ok(slept) => slept.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl Sleeper for RecordingSleeper {
    fn sleep_ms(&self, ms: u64) {
        match self.slept.lock() {
            Ok(mut slept) => slept.push(ms),
            Err(poisoned) => poisoned.into_inner().push(ms),
        }
    }
}

pub struct AnthropicHttp {
    base: String,
    api_key: SecretValue,
    model: String,
    transport: Arc<dyn HttpTransport>,
    sleeper: Arc<dyn Sleeper>,
}

impl AnthropicHttp {
    pub fn new(
        base: &str,
        api_key: SecretValue,
        model: &str,
        transport: Arc<dyn HttpTransport>,
        sleeper: Arc<dyn Sleeper>,
    ) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            api_key,
            model: model.to_string(),
            transport,
            sleeper,
        }
    }

    fn attempt(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let payload = MessagesRequest::build(&self.model, &request.prompt, request.max_tokens);
        let body = Body::from_payload(&payload).map_err(|_| LlmError::Internal)?;
        let http = HttpRequest::new(TAG, Method::Post, format!("{}/v1/messages", self.base))
            .header("x-api-key", self.api_key.expose())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .with_body(body);
        interpret(self.transport.send(http))
    }
}

impl LlmClient for AnthropicHttp {
    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let mut failures = 0u32;
        loop {
            let outcome = self.attempt(request);
            let err = match outcome {
                Ok(response) => return Ok(response),
                Err(err) => err,
            };
            failures += 1;
            if !err.is_retryable() || failures >= MAX_ATTEMPTS {
                return Err(err);
            }
            let retry_after = match err {
                LlmError::RateLimited { retry_after_ms } => retry_after_ms,
                _ => None,
            };
            let wait = backoff_ms(failures, retry_after, 0);
            if wait > MAX_WAIT_MS {
                return Err(err);
            }
            self.sleeper.sleep_ms(wait);
        }
    }
}

#[derive(Deserialize)]
struct WireResponse {
    content: Vec<WireBlock>,
    usage: WireUsage,
}

#[derive(Deserialize)]
struct WireBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct WireUsage {
    input_tokens: u32,
    output_tokens: u32,
}

/// Maps one transport result to the port's result. The mock uses the same function for scripted
/// raw outcomes, so both adapters classify 401/429/529/timeouts/broken JSON identically.
pub fn interpret(result: Result<HttpResponse, TransportError>) -> Result<LlmResponse, LlmError> {
    let response = result.map_err(LlmError::from)?;
    match response.status {
        200..=299 => {}
        401 | 403 => return Err(LlmError::Unauthorized),
        429 => {
            return Err(LlmError::RateLimited {
                retry_after_ms: response.retry_after_ms(),
            });
        }
        500..=599 => return Err(LlmError::Overloaded),
        status => return Err(LlmError::Rejected { status }),
    }
    let wire: WireResponse =
        serde_json::from_slice(&response.body).map_err(|_| LlmError::BadResponse)?;
    let text: String = wire
        .content
        .iter()
        .filter(|b| b.kind == "text")
        .filter_map(|b| b.text.as_deref())
        .collect();
    if text.is_empty() {
        return Err(LlmError::BadResponse);
    }
    Ok(LlmResponse {
        text,
        in_tokens: wire.usage.input_tokens,
        out_tokens: wire.usage.output_tokens,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::llm::Purpose;
    use crate::llm::egress::Prompt;
    use crate::remote::http::{HostPolicy, ScriptedTransport};

    pub(crate) fn ok_body(text: &str, in_tokens: u32, out_tokens: u32) -> String {
        serde_json::json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "m",
            "content": [{"type": "text", "text": text}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": in_tokens, "output_tokens": out_tokens}
        })
        .to_string()
    }

    fn request() -> LlmRequest {
        LlmRequest {
            purpose: Purpose::Probe,
            prompt: Prompt::probe(),
            max_tokens: 64,
        }
    }

    fn adapter(
        steps: Vec<Result<HttpResponse, TransportError>>,
    ) -> (AnthropicHttp, Arc<ScriptedTransport>, Arc<RecordingSleeper>) {
        let transport = Arc::new(ScriptedTransport::new(
            HostPolicy::configured([HOST]),
            steps,
        ));
        let sleeper = Arc::new(RecordingSleeper::default());
        let client = AnthropicHttp::new(
            DEFAULT_BASE,
            SecretValue::new("sk-ant-KEY"),
            "claude-test",
            transport.clone(),
            sleeper.clone(),
        );
        (client, transport, sleeper)
    }

    #[test]
    fn request_snapshot_pins_headers_path_and_body_sections() {
        let (client, transport, _) = adapter(vec![Ok(HttpResponse::ok_json(
            200,
            &ok_body("pong", 11, 2),
        ))]);
        let response = client.complete(&request()).expect("reply");
        assert_eq!(
            response,
            LlmResponse {
                text: "pong".into(),
                in_tokens: 11,
                out_tokens: 2
            }
        );

        let seen = transport.requests();
        assert_eq!(seen.len(), 1);
        let sent = &seen[0];
        assert_eq!(
            (sent.method, sent.url.as_str()),
            (Method::Post, "https://api.anthropic.com/v1/messages")
        );
        let header = |name: &str| {
            sent.headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(header("x-api-key"), Some("sk-ant-KEY"));
        assert_eq!(header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(sent.content_type, Some("application/json"));
        let body: serde_json::Value = serde_json::from_slice(&sent.body).expect("json body");
        assert_eq!(body["model"], "claude-test");
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "ping");
        assert!(body["system"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(
            body.as_object().map(|o| o.len()),
            Some(4),
            "exactly model, max_tokens, system, messages"
        );
        let text = String::from_utf8_lossy(&sent.body);
        assert!(
            !text.contains("sk-ant-KEY"),
            "the key is a header, never a body field"
        );
    }

    #[test]
    fn model_name_is_the_configured_value_and_text_blocks_are_concatenated() {
        let body = serde_json::json!({
            "content": [{"type": "text", "text": "a"}, {"type": "tool_use", "id": "x"}, {"type": "text", "text": "b"}],
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })
        .to_string();
        let (client, _, _) = adapter(vec![Ok(HttpResponse::ok_json(200, &body))]);
        assert_eq!(client.complete(&request()).expect("reply").text, "ab");
    }

    pub(crate) fn contract_cases()
    -> Vec<(&'static str, Result<HttpResponse, TransportError>, LlmError)> {
        let retry_after = HttpResponse {
            status: 429,
            headers: vec![("retry-after".into(), "7".into())],
            body: Vec::new(),
        };
        vec![
            (
                "401",
                Ok(HttpResponse::ok_json(401, "{}")),
                LlmError::Unauthorized,
            ),
            (
                "403",
                Ok(HttpResponse::ok_json(403, "{}")),
                LlmError::Unauthorized,
            ),
            (
                "429",
                Ok(retry_after),
                LlmError::RateLimited {
                    retry_after_ms: Some(7_000),
                },
            ),
            (
                "429 plain",
                Ok(HttpResponse::ok_json(429, "{}")),
                LlmError::RateLimited {
                    retry_after_ms: None,
                },
            ),
            (
                "529",
                Ok(HttpResponse::ok_json(529, "{}")),
                LlmError::Overloaded,
            ),
            (
                "500",
                Ok(HttpResponse::ok_json(500, "")),
                LlmError::Overloaded,
            ),
            (
                "400",
                Ok(HttpResponse::ok_json(400, "{}")),
                LlmError::Rejected { status: 400 },
            ),
            ("timeout", Err(TransportError::Timeout), LlmError::Timeout),
            (
                "connect",
                Err(TransportError::Connect),
                LlmError::Unreachable,
            ),
            (
                "host denied",
                Err(TransportError::HostDenied),
                LlmError::HostDenied,
            ),
            (
                "broken json",
                Ok(HttpResponse::ok_json(200, "{not json")),
                LlmError::BadResponse,
            ),
            (
                "no text",
                Ok(HttpResponse::ok_json(
                    200,
                    r#"{"content":[],"usage":{"input_tokens":1,"output_tokens":1}}"#,
                )),
                LlmError::BadResponse,
            ),
            (
                "no usage",
                Ok(HttpResponse::ok_json(
                    200,
                    r#"{"content":[{"type":"text","text":"x"}]}"#,
                )),
                LlmError::BadResponse,
            ),
        ]
    }

    #[test]
    fn failures_map_to_stable_errors_and_only_429_and_5xx_are_retried() {
        for (name, result, expected) in contract_cases() {
            let steps = vec![result.clone(), result.clone(), result.clone(), result];
            let (client, transport, sleeper) = adapter(steps);
            assert_eq!(client.complete(&request()), Err(expected), "{name}");
            let attempts = transport.requests().len();
            if expected.is_retryable() {
                let waits = sleeper.slept().len();
                assert_eq!(
                    (attempts, waits),
                    (MAX_ATTEMPTS as usize, MAX_ATTEMPTS as usize - 1),
                    "{name}"
                );
            } else {
                assert_eq!(
                    (attempts, sleeper.slept().len()),
                    (1, 0),
                    "{name}: no retry"
                );
            }
        }
    }

    #[test]
    fn backoff_doubles_and_honors_retry_after_then_a_later_success_wins() {
        let busy = || Ok(HttpResponse::ok_json(529, "{}"));
        let (client, _, sleeper) = adapter(vec![
            busy(),
            busy(),
            Ok(HttpResponse::ok_json(200, &ok_body("fine", 1, 1))),
        ]);
        assert_eq!(
            client.complete(&request()).expect("third attempt").text,
            "fine"
        );
        assert_eq!(sleeper.slept(), [1_000, 2_000]);

        let limited = Ok(HttpResponse {
            status: 429,
            headers: vec![("retry-after".into(), "9".into())],
            body: Vec::new(),
        });
        let (client, _, sleeper) = adapter(vec![
            limited,
            Ok(HttpResponse::ok_json(200, &ok_body("ok", 1, 1))),
        ]);
        client.complete(&request()).expect("retry");
        assert_eq!(
            sleeper.slept(),
            [9_000],
            "the server's Retry-After is the floor"
        );
    }

    #[test]
    fn a_retry_after_beyond_the_wait_limit_is_not_waited_for() {
        let long = Ok(HttpResponse {
            status: 429,
            headers: vec![("retry-after".into(), "120".into())],
            body: Vec::new(),
        });
        let (client, transport, sleeper) = adapter(vec![long]);
        assert_eq!(
            client.complete(&request()),
            Err(LlmError::RateLimited {
                retry_after_ms: Some(120_000)
            })
        );
        assert_eq!((transport.requests().len(), sleeper.slept().len()), (1, 0));
    }

    #[test]
    fn a_host_other_than_the_policy_is_refused_before_anything_is_sent() {
        let transport = Arc::new(ScriptedTransport::new(
            HostPolicy::configured([HOST]),
            vec![Ok(HttpResponse::ok_json(200, &ok_body("x", 1, 1)))],
        ));
        let client = AnthropicHttp::new(
            "https://example.com",
            SecretValue::new("k"),
            "m",
            transport.clone(),
            Arc::new(RecordingSleeper::default()),
        );
        assert_eq!(client.complete(&request()), Err(LlmError::HostDenied));
        assert!(transport.requests().is_empty());
    }

    #[test]
    fn debug_of_the_request_path_never_shows_the_key() {
        let (client, _, _) = adapter(vec![]);
        let shown = format!("{:?}", SecretValue::new("sk-ant-KEY"));
        assert!(!shown.contains("sk-ant"));
        assert_eq!(client.base, DEFAULT_BASE);
    }
}
