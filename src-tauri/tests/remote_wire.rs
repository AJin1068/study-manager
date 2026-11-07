//! Wire-level tests of the real HTTP adapters against a loopback fake server. No external host is
//! ever contacted: the transport under test is built with `HostPolicy::LoopbackOnly`, the same
//! public policy type the product uses, and a request to any other host is refused before a
//! socket exists.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use study_manager_lib::remote::SystemClock;
use study_manager_lib::remote::auth::{AccessTokenSource, AuthProvider, AuthService, SupabaseAuth};
use study_manager_lib::remote::fake::FakeBackend;
use study_manager_lib::remote::http::{
    HostPolicy, HttpRequest, HttpTransport, Method, RequestTag, TransportError, UreqTransport,
};
use study_manager_lib::remote::payload::{Body, PresenceBeat};
use study_manager_lib::remote::secrets::MemorySecretStore;
use study_manager_lib::remote::supabase::SupabaseRest;

const TAG: RequestTag = RequestTag {
    port: "supabase",
    kind: "test",
};
const CANARY_TITLE: &str = "CANARY_TITLE_7f3a";
const CANARY_URL: &str = "https://canary.example/secret-path";

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn reply(status: u16, body: &str) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: body.to_string(),
    }
}

/// Minimal HTTP/1.1 server: one request per connection, `Connection: close`.
struct FakeServer {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    connections: Arc<AtomicUsize>,
}

fn read_request(stream: &mut TcpStream) -> Option<Seen> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    while buf.len() < header_end + length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Some(Seen {
        method,
        path,
        headers,
        body: buf[header_end..].to_vec(),
    })
}

impl FakeServer {
    fn start(handler: impl Fn(&Seen) -> Option<Reply> + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let (seen_t, conns_t) = (seen.clone(), connections.clone());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                conns_t.fetch_add(1, Ordering::SeqCst);
                let Some(request) = read_request(&mut stream) else {
                    continue;
                };
                seen_t.lock().expect("seen").push(request.clone());
                // None = hang without answering (timeout scenario).
                let Some(reply) = handler(&request) else {
                    thread::sleep(Duration::from_secs(3));
                    continue;
                };
                let mut head = format!(
                    "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reply.body.len()
                );
                for (k, v) in &reply.headers {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(reply.body.as_bytes());
            }
        });
        Self {
            port,
            seen,
            connections,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("seen").clone()
    }
}

const TOKEN_JSON: &str = r#"{"access_token":"acc-1","token_type":"bearer","expires_in":3600,"refresh_token":"ref-1","user":{"id":"u-1"}}"#;

fn gotrue_and_rest(request: &Seen) -> Option<Reply> {
    Some(match request.path.as_str() {
        "/auth/v1/otp" => reply(200, "{}"),
        "/auth/v1/verify" => reply(200, TOKEN_JSON),
        "/auth/v1/token?grant_type=refresh_token" => reply(200, TOKEN_JSON),
        "/rest/v1/rpc/heartbeat" => reply(200, "\"ok\""),
        "/rest/v1/devices" => reply(201, ""),
        _ => reply(404, "{}"),
    })
}

#[test]
fn loopback_only_policy_refuses_example_com_before_any_connection() {
    let transport = UreqTransport::new(HostPolicy::LoopbackOnly);
    let started = Instant::now();
    for url in [
        "https://example.com/",
        "http://example.com/",
        "http://93.184.216.34/",
        "http://10.0.0.5:80/",
    ] {
        let result = transport.send(HttpRequest::new(TAG, Method::Get, url.to_string()));
        assert_eq!(result, Err(TransportError::HostDenied), "{url}");
    }
    // No DNS lookup or connect attempt happens, so refusal is immediate.
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn configured_policy_denies_other_hosts_and_plain_http_to_the_endpoint() {
    let transport = UreqTransport::new(HostPolicy::configured(["proj.supabase.co"]));
    assert_eq!(
        transport.send(HttpRequest::new(
            TAG,
            Method::Get,
            "https://example.com/".into()
        )),
        Err(TransportError::HostDenied)
    );
    assert_eq!(
        transport.send(HttpRequest::new(
            TAG,
            Method::Get,
            "http://proj.supabase.co/".into()
        )),
        Err(TransportError::HostDenied)
    );
}

#[test]
fn ureq_transport_talks_to_a_loopback_server_and_never_follows_redirects() {
    let server = FakeServer::start(|request| {
        if request.path == "/redirect" {
            return Some(Reply {
                status: 302,
                headers: vec![("Location", "http://example.com/stolen".into())],
                body: String::new(),
            });
        }
        Some(Reply {
            status: 200,
            headers: vec![("Retry-After", "7".into())],
            body: format!("echo:{}", request.method),
        })
    });
    let transport = UreqTransport::new(HostPolicy::LoopbackOnly);

    let ok = transport
        .send(HttpRequest::new(TAG, Method::Post, server.url("/echo")).header("x-test", "1"))
        .expect("response");
    assert_eq!(
        (ok.status, ok.body.as_slice(), ok.retry_after_ms()),
        (200, b"echo:POST".as_slice(), Some(7000))
    );
    assert!(
        server.seen()[0]
            .headers
            .iter()
            .any(|(k, v)| k == "x-test" && v == "1")
    );

    let redirected = transport
        .send(HttpRequest::new(TAG, Method::Get, server.url("/redirect")))
        .expect("redirect is a response");
    assert_eq!(redirected.status, 302);
    assert_eq!(
        server.connections.load(Ordering::SeqCst),
        2,
        "the Location target was never contacted"
    );
}

#[test]
fn a_silent_server_times_out_instead_of_hanging() {
    let server = FakeServer::start(|_| None);
    let transport = UreqTransport::with_timeouts(
        HostPolicy::LoopbackOnly,
        Duration::from_millis(200),
        Duration::from_millis(400),
    );
    let started = Instant::now();
    let result = transport.send(HttpRequest::new(TAG, Method::Get, server.url("/slow")));
    assert_eq!(result, Err(TransportError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn auth_and_rest_adapters_speak_gotrue_and_postgrest_on_the_wire() {
    let server = FakeServer::start(gotrue_and_rest);
    let transport: Arc<dyn HttpTransport> = Arc::new(UreqTransport::new(HostPolicy::LoopbackOnly));
    let endpoint = server.url("");
    let provider = Arc::new(SupabaseAuth::new(
        &endpoint,
        "anon-public-key",
        transport.clone(),
    ));
    // A throwaway fake backend only supplies the (unused here) device directory.
    let directory = Arc::new(FakeBackend::new(Arc::new(SystemClock)));
    let service = Arc::new(AuthService::new(
        provider,
        Arc::new(MemorySecretStore::in_memory()),
        directory,
        Arc::new(SystemClock),
    ));

    service.request_otp("person@example.test").expect("otp");
    assert_eq!(
        service
            .verify_otp("person@example.test", "123456")
            .expect("verify"),
        "u-1"
    );
    let tokens: Arc<dyn AccessTokenSource> = service.clone();
    let rest = SupabaseRest::new(&endpoint, "anon-public-key", transport, tokens);
    let beat = PresenceBeat {
        device_id: "dev-1".into(),
        app_version: "0.1.0".into(),
        studying: true,
    };
    assert_eq!(
        rest.rpc("rpc_heartbeat", "heartbeat", &beat).expect("rpc"),
        serde_json::json!("ok")
    );

    let seen = server.seen();
    let paths: Vec<&str> = seen.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(
        paths,
        ["/auth/v1/otp", "/auth/v1/verify", "/rest/v1/rpc/heartbeat"]
    );
    let heartbeat = &seen[2];
    assert!(
        heartbeat
            .headers
            .iter()
            .any(|(k, v)| k == "apikey" && v == "anon-public-key")
    );
    assert!(
        heartbeat
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer acc-1")
    );
    let body: serde_json::Value = serde_json::from_slice(&heartbeat.body).expect("json body");
    assert_eq!(
        body,
        serde_json::json!({"p_device_id": "dev-1", "p_app_version": "0.1.0", "p_studying": true})
    );

    // Egress audit at the receiving end: nothing the server saw may carry a canary or a forbidden key.
    for request in &seen {
        let text = String::from_utf8_lossy(&request.body);
        assert!(
            !text.contains(CANARY_TITLE) && !text.contains(CANARY_URL),
            "{}",
            request.path
        );
        for forbidden in ["window_title", "\"title\"", "\"url\"", "typed", "sentence"] {
            assert!(
                !text.contains(forbidden),
                "{} sent {forbidden}",
                request.path
            );
        }
    }
}

#[test]
fn an_expired_session_gets_one_refresh_and_a_retry_over_the_wire() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let server = FakeServer::start(move |request| {
        if request.path == "/rest/v1/rpc/heartbeat" {
            // First attempt carries the stale token and is refused; the retry succeeds.
            return Some(if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                reply(401, "{\"message\":\"JWT expired\"}")
            } else {
                reply(200, "\"ok\"")
            });
        }
        gotrue_and_rest(request)
    });
    let transport: Arc<dyn HttpTransport> = Arc::new(UreqTransport::new(HostPolicy::LoopbackOnly));
    let endpoint = server.url("");
    let provider: Arc<dyn AuthProvider> =
        Arc::new(SupabaseAuth::new(&endpoint, "k", transport.clone()));
    let service = Arc::new(AuthService::new(
        provider,
        Arc::new(MemorySecretStore::in_memory()),
        Arc::new(FakeBackend::new(Arc::new(SystemClock))),
        Arc::new(SystemClock),
    ));
    service
        .verify_otp("person@example.test", "123456")
        .expect("verify");
    let rest = SupabaseRest::new(&endpoint, "k", transport, service);
    let beat = PresenceBeat {
        device_id: "d".into(),
        app_version: "1".into(),
        studying: false,
    };
    assert_eq!(
        rest.rpc("rpc_heartbeat", "heartbeat", &beat)
            .expect("retried"),
        serde_json::json!("ok")
    );
    let paths: Vec<String> = server.seen().into_iter().map(|s| s.path).collect();
    assert_eq!(
        paths,
        [
            "/auth/v1/verify",
            "/rest/v1/rpc/heartbeat",
            "/auth/v1/token?grant_type=refresh_token",
            "/rest/v1/rpc/heartbeat"
        ]
    );
}

#[test]
fn bodies_can_only_be_built_from_payload_types() {
    // The positive half of the compile_fail doctests in remote::payload.
    let body = Body::from_payload(&PresenceBeat {
        device_id: "d".into(),
        app_version: "1".into(),
        studying: false,
    })
    .expect("body");
    assert!(!body.is_empty());
}
