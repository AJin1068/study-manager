//! The real HTTP transport: blocking ureq with rustls. Redirects are never followed (a redirect
//! target would leave the host the pairing offer named) and the endpoint must be https, or plain
//! http on a loopback address for local development.

use std::time::Duration;

use study_phone_core::http::{HttpRequest, HttpResponse, HttpTransport, Method, TransportError};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(TOTAL_TIMEOUT))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        Self { agent }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// `https://...`, or `http://localhost` / `http://127.0.0.1` (the same rule as the pairing offer).
pub fn url_allowed(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.split(':').next().unwrap_or_default();
    match scheme {
        "https" => !host.is_empty(),
        "http" => host == "localhost" || host == "127.0.0.1",
        _ => false,
    }
}

fn map_error(err: ureq::Error) -> TransportError {
    match err {
        ureq::Error::Timeout(_) => TransportError::Timeout,
        _ => TransportError::Network,
    }
}

impl HttpTransport for UreqTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        if !url_allowed(&request.url) {
            return Err(TransportError::Network);
        }
        let method = match request.method {
            Method::Get => "GET",
            Method::Post => "POST",
        };
        let mut builder = ureq::http::Request::builder()
            .method(method)
            .uri(request.url.as_str());
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let built = builder
            .body(request.body.unwrap_or_default())
            .map_err(|_| TransportError::Network)?;
        let mut response = self.agent.run(built).map_err(map_error)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|v| (name.as_str().to_string(), v.to_string()))
            })
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(map_error)?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_loopback_http_may_be_called() {
        assert!(url_allowed(
            "https://proj.supabase.co/rest/v1/rpc/phone_lease"
        ));
        assert!(url_allowed("http://127.0.0.1:54321/auth/v1/signup"));
        assert!(url_allowed("http://localhost:54321/x"));
        for url in [
            "http://proj.supabase.co/x",
            "http://127.0.0.1.evil.example/x",
            "ftp://x",
            "file:///etc/passwd",
            "https:///x",
            "nothing",
        ] {
            assert!(!url_allowed(url), "{url}");
        }
    }

    #[test]
    fn a_refused_url_makes_no_connection() {
        let transport = UreqTransport::new();
        let request = HttpRequest::new(Method::Get, "http://example.invalid/x".into());
        assert_eq!(transport.send(request), Err(TransportError::Network));
    }
}
