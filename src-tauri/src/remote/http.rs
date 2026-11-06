//! HTTP port: one request/response pair, no streaming. Real network access goes through
//! `UreqTransport`, which cannot be built without a `HostPolicy` and checks it before any socket
//! is opened. Tests use `ScriptedTransport` (recorded responses) or a loopback fake server.
//!
//! Logging here carries host, status, byte count, and latency only. Paths (an ntfy topic lives in
//! the path), queries, headers, and bodies never reach a log line or a `Debug` output.

use std::collections::VecDeque;
use std::fmt;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use url::{Host, Url};

use super::payload::Body;

/// Largest response body read, to bound memory when a server misbehaves.
pub const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Delete => "DELETE",
        }
    }
}

/// Which integration a request belongs to and what it is for. Both are static labels recorded in
/// `remote_events`; neither can carry request content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestTag {
    pub port: &'static str,
    pub kind: &'static str,
}

pub struct HttpRequest {
    pub tag: RequestTag,
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Body>,
}

impl HttpRequest {
    pub fn new(tag: RequestTag, method: Method, url: String) -> Self {
        Self { tag, method, url, headers: Vec::new(), body: None }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn with_body(mut self, body: Body) -> Self {
        self.body = Some(body);
        self
    }
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("tag", &self.tag)
            .field("method", &self.method)
            .field("host", &host_of(&self.url).unwrap_or_default())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn ok_json(status: u16, body: &str) -> Self {
        Self { status, headers: Vec::new(), body: body.as_bytes().to_vec() }
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// `Retry-After` in seconds form, as milliseconds. HTTP-date form is not honored (the
    /// backoff floor applies instead).
    pub fn retry_after_ms(&self) -> Option<u64> {
        let secs: u64 = self.header("retry-after")?.trim().parse().ok()?;
        secs.checked_mul(1000)
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse").field("status", &self.status).field("bytes", &self.body.len()).finish()
    }
}

/// Transport failure with a stable code. No variant carries a URL, path, or message from the
/// network stack, so formatting one is always safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("host_denied")]
    HostDenied,
    #[error("invalid_url")]
    InvalidUrl,
    #[error("timeout")]
    Timeout,
    #[error("connect_failed")]
    Connect,
    #[error("io_error")]
    Io,
    #[error("response_too_large")]
    ResponseTooLarge,
    #[error("scripted_exhausted")]
    ScriptExhausted,
}

impl TransportError {
    pub fn code(self) -> &'static str {
        match self {
            TransportError::HostDenied => "host_denied",
            TransportError::InvalidUrl => "invalid_url",
            TransportError::Timeout => "timeout",
            TransportError::Connect => "connect_failed",
            TransportError::Io => "io_error",
            TransportError::ResponseTooLarge => "response_too_large",
            TransportError::ScriptExhausted => "scripted_exhausted",
        }
    }
}

pub trait HttpTransport: Send + Sync {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// Where a transport may connect. Public and never gated by `cfg(test)`, so integration tests in
/// `tests/` and the shipped app apply the same rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostPolicy {
    /// Product policy: https to exactly the listed hosts (the configured endpoints), plus
    /// loopback over http or https.
    Configured { hosts: Vec<String> },
    /// Test policy: loopback addresses only.
    LoopbackOnly,
}

impl HostPolicy {
    pub fn configured<I: IntoIterator<Item = S>, S: AsRef<str>>(hosts: I) -> Self {
        HostPolicy::Configured { hosts: hosts.into_iter().map(|h| h.as_ref().to_ascii_lowercase()).collect() }
    }

    /// Checks a URL before any connection exists. Returns the host for logging.
    pub fn check(&self, raw: &str) -> Result<String, TransportError> {
        let url = Url::parse(raw).map_err(|_| TransportError::InvalidUrl)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(TransportError::HostDenied);
        }
        let host = url.host().ok_or(TransportError::InvalidUrl)?;
        let loopback = is_loopback_host(&host);
        let name = host.to_string().to_ascii_lowercase();
        let allowed = match self {
            HostPolicy::LoopbackOnly => loopback && matches!(url.scheme(), "http" | "https"),
            HostPolicy::Configured { hosts } => {
                (url.scheme() == "https" && hosts.contains(&name))
                    || (loopback && matches!(url.scheme(), "http" | "https"))
            }
        };
        if allowed {
            Ok(name)
        } else {
            Err(TransportError::HostDenied)
        }
    }
}

fn is_loopback_host(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(ip) => IpAddr::V4(*ip).is_loopback(),
        Host::Ipv6(ip) => IpAddr::V6(*ip).is_loopback(),
    }
}

/// Host part of a URL, for logs. `None` when the URL does not parse.
pub fn host_of(raw: &str) -> Option<String> {
    Url::parse(raw).ok()?.host_str().map(str::to_ascii_lowercase)
}

/// Shape check for an endpoint setting: `https://host[/path]`, or `http://` to a loopback host.
/// No credentials, query, or fragment (a key placed in a URL would be logged or stored in history).
pub fn is_valid_endpoint(raw: &str) -> bool {
    let Ok(url) = Url::parse(raw) else { return false };
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return false;
    }
    let Some(host) = url.host() else { return false };
    match url.scheme() {
        "https" => true,
        "http" => is_loopback_host(&host),
        _ => false,
    }
}

/// Real transport (blocking ureq + rustls). Redirects are never followed: a redirect target would
/// bypass the host policy, so a 3xx is returned to the caller as an ordinary response.
pub struct UreqTransport {
    policy: HostPolicy,
    agent: ureq::Agent,
}

impl UreqTransport {
    /// The policy is mandatory: there is no constructor that skips the host check.
    pub fn new(policy: HostPolicy) -> Self {
        Self::with_timeouts(policy, CONNECT_TIMEOUT, TOTAL_TIMEOUT)
    }

    pub fn with_timeouts(policy: HostPolicy, connect: Duration, total: Duration) -> Self {
        let mut builder = ureq::Agent::config_builder()
            .timeout_connect(Some(connect))
            .timeout_global(Some(total))
            .max_redirects(0)
            .http_status_as_error(false);
        if policy == HostPolicy::LoopbackOnly {
            // A proxy environment variable must not carry a loopback-only test off the machine.
            builder = builder.proxy(None);
        }
        Self { policy, agent: builder.build().into() }
    }
}

impl HttpTransport for UreqTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let host = self.policy.check(&request.url)?;
        let started = Instant::now();
        let sent_bytes = request.body.as_ref().map_or(0, Body::len);
        let outcome = self.execute(request);
        let latency_ms = started.elapsed().as_millis();
        match &outcome {
            Ok(response) => {
                tracing::info!(event = "http", host = %host, status = response.status, bytes_out = sent_bytes, bytes_in = response.body.len(), latency_ms = latency_ms as u64);
            }
            Err(err) => {
                tracing::warn!(event = "http_failed", host = %host, code = err.code(), latency_ms = latency_ms as u64);
            }
        }
        outcome
    }
}

impl UreqTransport {
    fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let mut builder = ureq::http::Request::builder().method(request.method.as_str()).uri(request.url.as_str());
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let (bytes, content_type) = match request.body {
            Some(body) => body.into_parts(),
            None => (Vec::new(), None),
        };
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        let built = builder.body(bytes).map_err(|_| TransportError::InvalidUrl)?;
        let mut response = self.agent.run(built).map_err(map_ureq_error)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|v| (name.as_str().to_string(), v.to_string())))
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(map_ureq_error)?;
        Ok(HttpResponse { status, headers, body })
    }
}

fn map_ureq_error(err: ureq::Error) -> TransportError {
    match err {
        ureq::Error::Timeout(_) => TransportError::Timeout,
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => TransportError::Connect,
        ureq::Error::BodyExceedsLimit(_) => TransportError::ResponseTooLarge,
        ureq::Error::BadUri(_) | ureq::Error::Http(_) => TransportError::InvalidUrl,
        _ => TransportError::Io,
    }
}

/// What a scripted transport saw. Body bytes are kept so tests can assert what would have left
/// the device (for example that no canary string was sent).
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub tag: RequestTag,
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub content_type: Option<&'static str>,
}

/// Plays back prepared responses in order and records every request. The policy is applied too,
/// so a scripted run still proves that a request targets an allowed host.
pub struct ScriptedTransport {
    policy: HostPolicy,
    steps: Mutex<VecDeque<Result<HttpResponse, TransportError>>>,
    seen: Mutex<Vec<RecordedRequest>>,
}

impl ScriptedTransport {
    pub fn new(policy: HostPolicy, steps: Vec<Result<HttpResponse, TransportError>>) -> Self {
        Self { policy, steps: Mutex::new(steps.into()), seen: Mutex::new(Vec::new()) }
    }

    /// Builds the script from a recorded fixture: a JSON array of
    /// `{ "status": 200, "headers": {"retry-after": "3"}, "body": <json or string> }`
    /// or `{ "error": "timeout" | "connect_failed" | "io_error" }`.
    pub fn from_fixture(policy: HostPolicy, fixture: &str) -> Result<Self, String> {
        let entries: Vec<serde_json::Value> = serde_json::from_str(fixture).map_err(|e| e.to_string())?;
        let mut steps = Vec::with_capacity(entries.len());
        for entry in entries {
            if let Some(code) = entry.get("error").and_then(|v| v.as_str()) {
                steps.push(Err(match code {
                    "timeout" => TransportError::Timeout,
                    "connect_failed" => TransportError::Connect,
                    "io_error" => TransportError::Io,
                    other => return Err(format!("unknown scripted error {other}")),
                }));
                continue;
            }
            let status = entry
                .get("status")
                .and_then(|v| v.as_u64())
                .and_then(|v| u16::try_from(v).ok())
                .ok_or("fixture step needs a status")?;
            let headers = entry
                .get("headers")
                .and_then(|v| v.as_object())
                .map(|map| map.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
                .unwrap_or_default();
            let body = match entry.get("body") {
                None | Some(serde_json::Value::Null) => Vec::new(),
                Some(serde_json::Value::String(text)) => text.clone().into_bytes(),
                Some(other) => other.to_string().into_bytes(),
            };
            steps.push(Ok(HttpResponse { status, headers, body }));
        }
        Ok(Self::new(policy, steps))
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.seen.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl HttpTransport for ScriptedTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.policy.check(&request.url)?;
        let (body, content_type) = match request.body {
            Some(b) => b.into_parts(),
            None => (Vec::new(), None),
        };
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(RecordedRequest {
                tag: request.tag,
                method: request.method,
                url: request.url,
                headers: request.headers,
                body,
                content_type,
            });
        }
        self.steps.lock().map_err(|_| TransportError::Io)?.pop_front().unwrap_or(Err(TransportError::ScriptExhausted))
    }
}

/// Retry delay after `failures` consecutive failures: 1s doubling to a 60s cap, with the server's
/// `Retry-After` as a floor. `jitter_permille` (0..=1000) shrinks the delay by up to 20% so
/// several workers do not retry in lockstep; pass 0 for the deterministic delay.
pub fn backoff_ms(failures: u32, retry_after_ms: Option<u64>, jitter_permille: u32) -> u64 {
    let exponent = failures.saturating_sub(1).min(6);
    let base = (1000u64 << exponent).min(60_000);
    let jitter = base * u64::from(jitter_permille.min(1000)) / 1000 / 5;
    let delay = base - jitter;
    match retry_after_ms {
        Some(floor) => delay.max(floor.min(10 * 60_000)),
        None => delay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_policy_is_https_and_exact_host_with_loopback_allowed() {
        let policy = HostPolicy::configured(["abc.supabase.co"]);
        assert_eq!(policy.check("https://abc.supabase.co/rest/v1/x").as_deref(), Ok("abc.supabase.co"));
        assert_eq!(policy.check("http://abc.supabase.co/x"), Err(TransportError::HostDenied));
        assert_eq!(policy.check("https://evil.supabase.co/x"), Err(TransportError::HostDenied));
        assert_eq!(policy.check("https://abc.supabase.co.evil.test/x"), Err(TransportError::HostDenied));
        assert_eq!(policy.check("https://abc.supabase.co@evil.test/x"), Err(TransportError::HostDenied));
        assert_eq!(policy.check("https://user:pw@abc.supabase.co/x"), Err(TransportError::HostDenied));
        assert!(policy.check("http://127.0.0.1:9/x").is_ok());
        assert!(policy.check("http://[::1]:9/x").is_ok());
        assert_eq!(policy.check("not a url"), Err(TransportError::InvalidUrl));
    }

    #[test]
    fn loopback_only_denies_everything_else() {
        let policy = HostPolicy::LoopbackOnly;
        assert!(policy.check("http://127.0.0.1:8080/a").is_ok());
        assert!(policy.check("http://localhost:8080/a").is_ok());
        assert_eq!(policy.check("https://example.com/"), Err(TransportError::HostDenied));
        assert_eq!(policy.check("http://10.0.0.5/"), Err(TransportError::HostDenied));
    }

    #[test]
    fn endpoint_shape_rejects_credentials_queries_and_plain_http() {
        assert!(is_valid_endpoint("https://abc.supabase.co"));
        assert!(is_valid_endpoint("http://127.0.0.1:54321"));
        assert!(!is_valid_endpoint("http://abc.supabase.co"));
        assert!(!is_valid_endpoint("https://u:p@abc.supabase.co"));
        assert!(!is_valid_endpoint("https://abc.supabase.co/?apikey=k"));
        assert!(!is_valid_endpoint("ftp://abc.supabase.co"));
        assert!(!is_valid_endpoint("abc.supabase.co"));
    }

    #[test]
    fn backoff_doubles_to_the_cap_and_honors_retry_after() {
        let delays: Vec<u64> = (1..=9).map(|n| backoff_ms(n, None, 0)).collect();
        assert_eq!(delays, vec![1000, 2000, 4000, 8000, 16000, 32000, 60000, 60000, 60000]);
        assert_eq!(backoff_ms(1, Some(30_000), 0), 30_000);
        assert_eq!(backoff_ms(7, Some(1_000), 0), 60_000);
        assert_eq!(backoff_ms(3, None, 1000), 3200);
        assert!(backoff_ms(0, None, 0) >= 1000);
    }

    #[test]
    fn scripted_transport_replays_in_order_and_records_requests() {
        let script = ScriptedTransport::new(
            HostPolicy::LoopbackOnly,
            vec![Ok(HttpResponse::ok_json(200, "{}")), Err(TransportError::Timeout)],
        );
        let tag = RequestTag { port: "supabase", kind: "probe" };
        let make = || HttpRequest::new(tag, Method::Get, "http://127.0.0.1:1/p".into());
        assert_eq!(script.send(make()).map(|r| r.status), Ok(200));
        assert_eq!(script.send(make()), Err(TransportError::Timeout));
        assert_eq!(script.send(make()), Err(TransportError::ScriptExhausted));
        assert_eq!(script.requests().len(), 3);
    }

    #[test]
    fn scripted_transport_still_applies_the_host_policy() {
        let script = ScriptedTransport::new(HostPolicy::LoopbackOnly, vec![Ok(HttpResponse::ok_json(200, "{}"))]);
        let tag = RequestTag { port: "supabase", kind: "probe" };
        let denied = script.send(HttpRequest::new(tag, Method::Get, "https://example.com/".into()));
        assert_eq!(denied, Err(TransportError::HostDenied));
        assert!(script.requests().is_empty());
    }

    #[test]
    fn request_debug_hides_path_headers_and_body() {
        let tag = RequestTag { port: "notify", kind: "ntfy_publish" };
        let request = HttpRequest::new(tag, Method::Post, "https://ntfy.sh/SECRETTOPIC123".into())
            .header("Authorization", "Bearer SECRETTOKEN");
        let shown = format!("{request:?}");
        assert!(shown.contains("ntfy.sh"));
        assert!(!shown.contains("SECRETTOPIC123") && !shown.contains("SECRETTOKEN"), "{shown}");
    }

    #[test]
    fn retry_after_parses_seconds_only() {
        let mut response = HttpResponse::ok_json(429, "");
        response.headers.push(("Retry-After".into(), "7".into()));
        assert_eq!(response.retry_after_ms(), Some(7000));
        response.headers[0].1 = "Wed, 21 Oct 2015 07:28:00 GMT".into();
        assert_eq!(response.retry_after_ms(), None);
    }
}
