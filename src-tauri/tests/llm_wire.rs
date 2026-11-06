//! Wire-level test of the Anthropic adapter against a loopback fake. No external host is ever
//! contacted; the transport is built with `HostPolicy::LoopbackOnly`, and the key is a dummy.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use study_manager_lib::llm::anthropic::{AnthropicHttp, RecordingSleeper, ANTHROPIC_VERSION};
use study_manager_lib::llm::egress::Prompt;
use study_manager_lib::llm::{LlmClient, LlmError, LlmRequest, Purpose};
use study_manager_lib::remote::http::{HostPolicy, UreqTransport};
use study_manager_lib::remote::secrets::SecretValue;

const DUMMY_KEY: &str = "sk-ant-DUMMY-LOOPBACK";
const OK_BODY: &str = r#"{"content":[{"type":"text","text":"{\"pages\":[]}"}],"usage":{"input_tokens":12,"output_tokens":5}}"#;

struct Seen {
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

fn serve(replies: Vec<(u16, &'static str)>) -> (u16, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_t = Arc::clone(&seen);
    thread::spawn(move || {
        for (status, body) in replies {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buf = Vec::new();
            let mut chunk = [0u8; 2048];
            let (head_end, length) = loop {
                let Ok(n) = stream.read(&mut chunk) else { return };
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                    let length = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse().ok()).unwrap_or(0usize);
                    break (pos + 4, length);
                }
            };
            while buf.len() < head_end + length {
                let Ok(n) = stream.read(&mut chunk) else { return };
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let mut lines = head.split("\r\n");
            let path = lines.next().and_then(|l| l.split(' ').nth(1)).unwrap_or("").to_string();
            let headers = lines.filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))).collect();
            seen_t.lock().expect("seen").push(Seen { path, headers, body: String::from_utf8_lossy(&buf[head_end..]).into_owned() });
            let reply = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (port, seen)
}

fn client(port: u16, sleeper: Arc<RecordingSleeper>) -> AnthropicHttp {
    let transport = Arc::new(UreqTransport::new(HostPolicy::LoopbackOnly));
    AnthropicHttp::new(&format!("http://127.0.0.1:{port}"), SecretValue::new(DUMMY_KEY), "test-model", transport, sleeper)
}

fn request() -> LlmRequest {
    LlmRequest { purpose: Purpose::WikiIngest, prompt: Prompt::ingest("수학", "# 미분\n도함수"), max_tokens: 256 }
}

#[test]
fn messages_request_has_the_documented_shape_and_the_reply_is_parsed() {
    let (port, seen) = serve(vec![(200, OK_BODY)]);
    let reply = client(port, Arc::new(RecordingSleeper::default())).complete(&request()).expect("reply");
    assert_eq!((reply.in_tokens, reply.out_tokens), (12, 5));
    let seen = seen.lock().expect("seen");
    assert_eq!(seen[0].path, "/v1/messages");
    let header = |name: &str| seen[0].headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    assert_eq!(header("x-api-key").as_deref(), Some(DUMMY_KEY));
    assert_eq!(header("anthropic-version").as_deref(), Some(ANTHROPIC_VERSION));
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).expect("json body");
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["max_tokens"], 256);
    assert_eq!(body["messages"][0]["role"], "user");
    assert!(body["messages"][0]["content"].as_str().is_some_and(|c| c.contains("도함수")));
    assert!(!seen[0].body.contains(DUMMY_KEY), "the key travels in a header only");
}

#[test]
fn a_rate_limit_is_retried_with_the_server_pause_and_then_succeeds() {
    let (port, seen) = serve(vec![(429, r#"{"error":{"type":"rate_limit_error"}}"#), (200, OK_BODY)]);
    let sleeper = Arc::new(RecordingSleeper::default());
    client(port, Arc::clone(&sleeper)).complete(&request()).expect("second attempt succeeds");
    assert_eq!(seen.lock().expect("seen").len(), 2);
    assert_eq!(sleeper.slept().len(), 1, "one pause between the attempts");
}

#[test]
fn an_auth_failure_is_not_retried() {
    let (port, seen) = serve(vec![(401, r#"{"error":{"type":"authentication_error"}}"#)]);
    let sleeper = Arc::new(RecordingSleeper::default());
    let err = client(port, Arc::clone(&sleeper)).complete(&request()).expect_err("fails");
    assert_eq!(err, LlmError::Unauthorized);
    assert_eq!(seen.lock().expect("seen").len(), 1);
    assert!(sleeper.slept().is_empty());
}

#[test]
fn a_non_loopback_base_is_refused_before_any_connection() {
    let transport = Arc::new(UreqTransport::new(HostPolicy::LoopbackOnly));
    let llm = AnthropicHttp::new("https://api.anthropic.com", SecretValue::new(DUMMY_KEY), "m", transport, Arc::new(RecordingSleeper::default()));
    let err = llm.complete(&request()).expect_err("denied");
    assert_eq!(err, LlmError::HostDenied);
}
