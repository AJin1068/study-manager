//! The HTTP port: one request, one response. The real transport lives in the Tauri shell
//! (`ureq`); tests use the scripted transport in `testing`.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

// Headers carry the bearer token and the body the pairing code: neither reaches a log line.
impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HttpRequest { .. }")
    }
}

impl HttpRequest {
    pub fn new(method: Method, url: String) -> Self {
        Self {
            method,
            url,
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn json_body(mut self, body: Vec<u8>) -> Self {
        self.headers
            .push(("content-type".to_string(), "application/json".to_string()));
        self.body = Some(body);
        self
    }

    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// `Retry-After` in seconds form, as milliseconds.
    pub fn retry_after_ms(&self) -> Option<u64> {
        let (_, value) = self
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))?;
        value.trim().parse::<u64>().ok()?.checked_mul(1000)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("timeout")]
    Timeout,
    #[error("network")]
    Network,
}

pub trait HttpTransport: Send + Sync {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError>;
}
