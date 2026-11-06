//! Connection behaviour through an in-memory stream: no socket, no browser, no real clock.

use std::io::{self, Cursor, Read, Write};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::Connection;

use super::protocol::{write_frame, MAX_FRAME_BYTES};
use super::service::{LinkEnv, RulesJudge, Service, TabContext, TabJudge, Verdict, REASON_SITE_ALLOWED_PASS};
use super::{LinkHub, LinkStatus};
use crate::db::open_in_memory;
use crate::enforce::model::{EnforceMode, EntryKind, EntrySource};
use crate::enforce::{self, StudyWindow};
use crate::error::AppResult;
use crate::platform::LinkStream;
use crate::store;
use crate::testutil::capture_logs;

pub(super) const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
pub(super) const EXT: &str = "abcdefghijklmnopabcdefghijklmnop";
const OTHER_EXT: &str = "ponmlkjihgfedcbaponmlkjihgfedcba";
const SECRET_URL: &str = "https://Docs.Example.com:8443/private/path-xyz?session=SECRETQ#frag-secret";

pub(super) struct TestStream {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
    /// Writes after this many succeed no more.
    writes_left: Option<u32>,
}

impl TestStream {
    pub(super) fn new(input: Vec<u8>) -> TestStream {
        TestStream { input: Cursor::new(input), output: Vec::new(), writes_left: None }
    }

    pub(super) fn replies(&self) -> Vec<serde_json::Value> {
        let mut cursor = Cursor::new(self.output.clone());
        let mut out = Vec::new();
        while let Some(frame) = super::protocol::read_frame(&mut cursor).expect("frame") {
            out.push(serde_json::from_slice(&frame).expect("json"));
        }
        out
    }
}

impl Read for TestStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for TestStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(left) = self.writes_left.as_mut() {
            if *left == 0 {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "peer gone"));
            }
            *left -= 1;
        }
        self.output.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl LinkStream for TestStream {
    fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    fn try_clone_stream(&self) -> io::Result<Box<dyn LinkStream>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "not needed"))
    }
}

struct FakeEnv {
    now: AtomicI64,
    window: Mutex<Option<StudyWindow>>,
}

impl LinkEnv for FakeEnv {
    fn now_ms(&self) -> AppResult<i64> {
        Ok(self.now.fetch_add(1, Ordering::SeqCst))
    }

    fn study_window(&self, _conn: &Connection, _now_ms: i64) -> AppResult<Option<StudyWindow>> {
        Ok(*self.window.lock().expect("window"))
    }
}

struct FixedJudge {
    verdict: Verdict,
    calls: AtomicU32,
}

impl TabJudge for FixedJudge {
    fn judge(&self, _tab: &TabContext<'_>) -> Verdict {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.verdict
    }
}

struct Rig {
    service: Service,
    db: Arc<Mutex<Connection>>,
    hub: Arc<LinkHub>,
    judge: Arc<FixedJudge>,
    changes: Arc<Mutex<Vec<LinkStatus>>>,
}

fn rig(verdict: Verdict, in_study: bool, mode: EnforceMode) -> Rig {
    let mut conn = open_in_memory().expect("db");
    if mode == EnforceMode::Enforce {
        enforce::set_mode(&mut conn, mode, enforce::ENFORCE_CONFIRMATION, 1).expect("mode");
    }
    let db = Arc::new(Mutex::new(conn));
    let changes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&changes);
    let hub = Arc::new(LinkHub::new(Box::new(move |s| sink.lock().expect("sink").push(s))));
    let window = in_study.then_some(StudyWindow { run_id: None, segment_id: None });
    let env = Arc::new(FakeEnv { now: AtomicI64::new(1_000_000), window: Mutex::new(window) });
    let judge = Arc::new(FixedJudge { verdict, calls: AtomicU32::new(0) });
    let service = Service::new(Arc::clone(&db), Arc::clone(&hub), env, judge.clone(), TOKEN.into(), vec![EXT.to_owned()]);
    Rig { service, db, hub, judge, changes }
}

pub(super) fn frames(messages: &[serde_json::Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for m in messages {
        write_frame(&mut out, m.to_string().as_bytes()).expect("frame");
    }
    out
}

fn auth(token: &str, origin: &str) -> serde_json::Value {
    serde_json::json!({"type":"auth","token":token,"originExtId":origin})
}

fn hello(ext: &str) -> serde_json::Value {
    serde_json::json!({"type":"hello","extId":ext,"version":"0.1.0"})
}

pub(super) fn tab(url: &str, id: i64) -> serde_json::Value {
    serde_json::json!({"type":"tab_event","url":url,"title":"SECRET-TITLE Bank Statement","tabId":id,"kind":"updated"})
}

pub(super) fn handshake() -> Vec<serde_json::Value> {
    vec![auth(TOKEN, EXT), hello(EXT)]
}

fn run(rig: &Rig, input: Vec<u8>) -> TestStream {
    let mut stream = TestStream::new(input);
    rig.service.serve(&mut stream);
    stream
}

fn events(rig: &Rig) -> Vec<(String, String, String, Option<String>)> {
    let conn = rig.db.lock().expect("db");
    let mut rows = store::list_block_events(&conn, 100).expect("events");
    rows.reverse();
    rows.into_iter().map(|e| (e.target_kind, e.target_id, e.action, e.detail_code)).collect()
}

#[test]
fn wrong_token_or_origin_is_refused_without_records_or_status_change() {
    let cases = [
        vec![auth("f".repeat(64).as_str(), EXT), hello(EXT)],
        vec![auth("short", EXT), hello(EXT)],
        vec![auth(TOKEN, OTHER_EXT), hello(EXT)],
        vec![auth(TOKEN, EXT), hello(OTHER_EXT)],
        vec![auth(TOKEN, "not-an-id"), hello("not-an-id")],
        vec![hello(EXT)],
        vec![tab("https://a.example/", 1)],
    ];
    for messages in cases {
        let rig = rig(Verdict::allow("x"), true, EnforceMode::DryRun);
        let stream = run(&rig, frames(&messages));
        let replies = stream.replies();
        assert_eq!(replies.len(), 1, "exactly one error reply for {messages:?}");
        assert_eq!(replies[0]["type"], "error");
        assert!(events(&rig).is_empty(), "no block_events for a refused peer");
        assert!(!rig.hub.status().connected);
        assert!(rig.changes.lock().expect("changes").is_empty());
    }
}

#[test]
fn bad_version_is_refused() {
    let rig = rig(Verdict::allow("x"), true, EnforceMode::DryRun);
    let bad = serde_json::json!({"type":"hello","extId":EXT,"version":""});
    let stream = run(&rig, frames(&[auth(TOKEN, EXT), bad]));
    assert_eq!(stream.replies()[0]["code"], "bad_version");
}

#[test]
fn no_allowed_extensions_means_everyone_is_refused() {
    let rig = rig(Verdict::allow("x"), true, EnforceMode::DryRun);
    let service = Service::new(
        Arc::clone(&rig.db),
        Arc::clone(&rig.hub),
        Arc::new(FakeEnv { now: AtomicI64::new(5), window: Mutex::new(None) }),
        rig.judge.clone(),
        TOKEN.into(),
        Vec::new(),
    );
    let mut stream = TestStream::new(frames(&handshake()));
    service.serve(&mut stream);
    assert_eq!(stream.replies()[0]["code"], "unknown_extension");
}

#[test]
fn connect_and_disconnect_are_recorded_and_reported() {
    let rig = rig(Verdict::allow("x"), false, EnforceMode::DryRun);
    let stream = run(&rig, frames(&handshake()));
    let replies = stream.replies();
    assert_eq!(replies[0]["type"], "hello_ack");
    assert_eq!(
        events(&rig),
        vec![
            ("extension".into(), EXT.into(), "extension_connected".into(), None),
            ("extension".into(), EXT.into(), "extension_disconnected".into(), Some("closed".into())),
        ]
    );
    let status = rig.hub.status();
    assert!(!status.connected);
    assert_eq!(status.connections, 0);
    assert_eq!(status.last_disconnect_reason.as_deref(), Some("closed"));
    assert!(status.last_connected_at_ms.is_some() && status.last_disconnected_at_ms.is_some());
    let seen = rig.changes.lock().expect("changes");
    assert_eq!(seen.iter().map(|s| s.connected).collect::<Vec<_>>(), vec![true, false]);
}

#[test]
fn every_way_a_link_can_end_records_a_disconnect() {
    let hs = frames(&handshake());
    let mut truncated = hs.clone();
    truncated.extend_from_slice(&[9, 0, 0, 0, b'{']);
    let mut oversize = hs.clone();
    oversize.extend_from_slice(&((MAX_FRAME_BYTES + 1) as u32).to_ne_bytes());
    let mut garbage = hs.clone();
    write_frame(&mut garbage, b"not json").expect("frame");
    let mut hello_again = hs.clone();
    hello_again.extend(frames(&[hello(EXT)]));
    let cases = [
        (truncated, "truncated"),
        (oversize, "frame_too_large"),
        (garbage, "protocol_error"),
        (hello_again, "protocol_error"),
        (hs, "closed"),
    ];
    for (input, reason) in cases {
        let rig = rig(Verdict::allow("x"), true, EnforceMode::DryRun);
        run(&rig, input);
        let log = events(&rig);
        assert_eq!(log.last().map(|e| e.2.as_str()), Some("extension_disconnected"), "{reason}");
        assert_eq!(log.last().and_then(|e| e.3.as_deref()), Some(reason));
        assert_eq!(rig.hub.status().last_disconnect_reason.as_deref(), Some(reason));
        assert!(!rig.hub.status().connected);
    }
}

#[test]
fn a_dead_peer_write_failure_still_records_the_disconnect() {
    let rig = rig(Verdict::allow("x"), true, EnforceMode::DryRun);
    let mut input = frames(&handshake());
    input.extend(frames(&[tab("https://a.example/", 1)]));
    let mut stream = TestStream::new(input);
    stream.writes_left = Some(1); // the hello ack gets through, the verdict does not
    rig.service.serve(&mut stream);
    assert_eq!(events(&rig).last().and_then(|e| e.3.clone()).as_deref(), Some("write_failed"));
}

#[test]
fn each_tab_event_gets_exactly_one_verdict_with_its_tab_id() {
    let rig = rig(Verdict::allow("ok"), true, EnforceMode::Enforce);
    let mut messages = handshake();
    messages.extend([tab("https://a.example/", 11), tab("https://b.example/", 12)]);
    let stream = run(&rig, frames(&messages));
    let replies = stream.replies();
    assert_eq!(replies.len(), 3);
    assert_eq!((replies[1]["tabId"].as_i64(), replies[1]["type"].as_str()), (Some(11), Some("verdict")));
    assert_eq!(replies[2]["tabId"].as_i64(), Some(12));
    assert_eq!(replies[1]["verdict"], "allow");
}

#[test]
fn outside_study_and_non_web_tabs_are_allowed_without_judging() {
    let outside = rig(Verdict::block("blocked_site"), false, EnforceMode::Enforce);
    let mut messages = handshake();
    messages.push(tab("https://a.example/", 1));
    let replies = run(&outside, frames(&messages)).replies();
    assert_eq!(replies[1]["verdict"], "allow");
    assert_eq!(replies[1]["reasonCode"], "outside_study");
    assert_eq!(outside.judge.calls.load(Ordering::SeqCst), 0);

    let non_web = rig(Verdict::block("blocked_site"), true, EnforceMode::Enforce);
    let mut messages = handshake();
    messages.push(tab("chrome://settings", 2));
    let replies = run(&non_web, frames(&messages)).replies();
    assert_eq!((replies[1]["verdict"].as_str(), replies[1]["reasonCode"].as_str()), (Some("allow"), Some("not_web")));
    assert_eq!(non_web.judge.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn block_verdict_is_enforced_in_enforce_mode_and_recorded_by_domain_only() {
    let rig = rig(Verdict::block("blocked_site"), true, EnforceMode::Enforce);
    let mut messages = handshake();
    messages.push(tab(SECRET_URL, 5));
    let replies = run(&rig, frames(&messages)).replies();
    assert_eq!((replies[1]["verdict"].as_str(), replies[1]["reasonCode"].as_str()), (Some("block"), Some("blocked_site")));
    let blocked: Vec<_> = events(&rig).into_iter().filter(|e| e.2 == "site_blocked").collect();
    assert_eq!(blocked, vec![("site".into(), "docs.example.com".into(), "site_blocked".into(), Some("blocked_site".into()))]);
}

#[test]
fn dry_run_records_the_block_but_lets_the_page_load() {
    let rig = rig(Verdict::block("blocked_site"), true, EnforceMode::DryRun);
    let mut messages = handshake();
    messages.push(tab("https://games.example/play", 5));
    let replies = run(&rig, frames(&messages)).replies();
    assert_eq!((replies[1]["verdict"].as_str(), replies[1]["reasonCode"].as_str()), (Some("allow"), Some("dry_run")));
    assert!(events(&rig).iter().any(|e| e.2 == "site_blocked" && e.1 == "games.example"));
}

#[test]
fn pass_allowance_is_recorded() {
    let rig = rig(Verdict::allow(REASON_SITE_ALLOWED_PASS), true, EnforceMode::Enforce);
    let mut messages = handshake();
    messages.push(tab("https://games.example/play", 5));
    let replies = run(&rig, frames(&messages)).replies();
    assert_eq!(replies[1]["verdict"], "allow");
    assert!(events(&rig).iter().any(|e| e.2 == "site_allowed_pass" && e.1 == "games.example"));
}

#[test]
fn flood_is_rate_limited_and_fails_open() {
    let rig = rig(Verdict::block("blocked_site"), true, EnforceMode::Enforce);
    let mut messages = handshake();
    for i in 0..80 {
        messages.push(tab("https://a.example/", i));
    }
    let replies = run(&rig, frames(&messages)).replies();
    assert_eq!(replies.len(), 81);
    assert_eq!(rig.judge.calls.load(Ordering::SeqCst), 60);
    assert_eq!(replies[61]["verdict"], "allow");
    assert_eq!(replies[61]["reasonCode"], "rate_limited");
    assert_eq!(replies[80]["reasonCode"], "rate_limited");
}

#[test]
fn logs_and_rows_never_contain_url_path_query_or_title() {
    // tracing caches per-callsite interest process-wide; a callsite first hit by a parallel test
    // thread can miss this thread's capture. Retry until the domain line shows up.
    let mut seen = None;
    for _ in 0..10 {
        let rig = rig(Verdict::block("blocked_site"), true, EnforceMode::Enforce);
        let mut messages = handshake();
        messages.push(tab(SECRET_URL, 5));
        messages.push(tab("https://bank.example/account?token=abc", 6));
        let input = frames(&messages);
        let logs = capture_logs(|| {
            run(&rig, input);
        });
        let rows = format!("{:?}", events(&rig));
        for forbidden in ["private", "path-xyz", "SECRETQ", "frag-secret", "SECRET-TITLE", "token=abc", "/account", "8443"] {
            assert!(!logs.contains(forbidden), "log leaked `{forbidden}`: {logs}");
            assert!(!rows.contains(forbidden), "rows leaked `{forbidden}`: {rows}");
        }
        assert!(!logs.contains(TOKEN));
        if logs.contains("docs.example.com") {
            seen = Some(logs);
            break;
        }
    }
    let logs = seen.expect("the blocked domain is logged");
    assert!(logs.contains("site_blocked"), "{logs}");
}

#[test]
fn panicking_judge_still_records_the_disconnect() {
    struct Boom;
    impl TabJudge for Boom {
        fn judge(&self, _tab: &TabContext<'_>) -> Verdict {
            panic!("judge failure");
        }
    }
    let base = rig(Verdict::allow("x"), true, EnforceMode::Enforce);
    let service = Service::new(
        Arc::clone(&base.db),
        Arc::clone(&base.hub),
        Arc::new(FakeEnv { now: AtomicI64::new(9), window: Mutex::new(Some(StudyWindow { run_id: None, segment_id: None })) }),
        Arc::new(Boom),
        TOKEN.into(),
        vec![EXT.to_owned()],
    );
    let mut messages = handshake();
    messages.push(tab("https://a.example/", 1));
    let mut stream = TestStream::new(frames(&messages));
    service.serve(&mut stream);
    assert_eq!(events(&base).last().and_then(|e| e.3.clone()).as_deref(), Some("internal_error"));
    assert!(!base.hub.status().connected, "the lock must not be left poisoned-open");
}

#[test]
fn status_serializes_for_the_ui() {
    let rig = rig(Verdict::allow("x"), false, EnforceMode::DryRun);
    rig.hub.set_transport(Some("no listener".into()));
    let value = serde_json::to_value(rig.hub.status()).expect("json");
    assert_eq!(value["transportActive"], false);
    assert_eq!(value["transportError"], "no listener");
    assert_eq!(value["connected"], false);
    assert!(value["lastDisconnectReason"].is_null());
}

// ---- real site rules behind the link ----

/// A link whose judge is the site-rule engine, with `entries` on the site block list.
fn rules_rig(entries: &[&str], stored: Option<&str>, in_study: bool, mode: EnforceMode) -> Rig {
    let base = rig(Verdict::allow("unused"), in_study, mode);
    {
        let conn = base.db.lock().expect("db");
        for entry in entries {
            store::insert_block_entry(&conn, EntryKind::Site, entry, EntrySource::User, None, 1).expect("entry");
        }
        if let Some(json) = stored {
            store::put_setting(&conn, crate::site_rules::SITE_RULES_KEY, "browser", json, 1).expect("rules");
        }
    }
    let window = in_study.then_some(StudyWindow { run_id: None, segment_id: None });
    let service = Service::new(
        Arc::clone(&base.db),
        Arc::clone(&base.hub),
        Arc::new(FakeEnv { now: AtomicI64::new(1_000_000), window: Mutex::new(window) }),
        Arc::new(RulesJudge::new(Arc::clone(&base.db))),
        TOKEN.into(),
        vec![EXT.to_owned()],
    );
    Rig { service, ..base }
}

fn verdicts(rig: &Rig, urls: &[&str]) -> Vec<(String, String)> {
    let mut messages = handshake();
    messages.extend(urls.iter().zip(1..).map(|(url, id)| tab(url, id)));
    let replies = run(rig, frames(&messages)).replies();
    replies[1..]
        .iter()
        .map(|r| (r["verdict"].as_str().unwrap_or("").to_owned(), r["reasonCode"].as_str().unwrap_or("").to_owned()))
        .collect()
}

#[test]
fn tab_change_in_a_locked_study_block_gets_a_rule_verdict() {
    let rig = rules_rig(&["reddit.com"], None, true, EnforceMode::Enforce);
    let got = verdicts(&rig, &["https://old.reddit.com/r/rust?x=SECRETQ", "https://docs.rs/serde", "chrome://newtab"]);
    let expected: Vec<(String, String)> =
        [("block", "blocked_site"), ("allow", "unlisted_allowed"), ("allow", "not_web")].iter().map(|(a, b)| ((*a).into(), (*b).into())).collect();
    assert_eq!(got, expected);
    let blocked: Vec<_> = events(&rig).into_iter().filter(|e| e.2 == "site_blocked").collect();
    assert_eq!(blocked, vec![("site".into(), "old.reddit.com".into(), "site_blocked".into(), Some("blocked_site".into()))]);
}

#[test]
fn non_canonical_urls_are_judged_not_waved_through_as_not_web() {
    let rig = rules_rig(&["reddit.com"], None, true, EnforceMode::Enforce);
    let urls = [
        "https:reddit.com",
        "https:/reddit.com/r/rust",
        "https://red\tdit.com/",
        "https://reddit%2ecom/",
        "https://.reddit.com/",
        "https://REDDIT.com./",
    ];
    for (url, got) in urls.iter().zip(verdicts(&rig, &urls)) {
        assert_eq!(got.0, "block", "url: {url}");
    }
    let not_web = verdicts(&rig, &["chrome://settings", "about:blank", "https://exa mple.com/"]);
    assert!(not_web.iter().all(|v| v == &("allow".to_owned(), "not_web".to_owned())));
}

#[test]
fn strict_mode_blocks_unlisted_sites_and_rules_stay_off_outside_study() {
    let strict = rules_rig(&[], Some(r#"{"strict":true,"allow":["github.com"]}"#), true, EnforceMode::Enforce);
    let got = verdicts(&strict, &["https://github.com/a", "https://example.org/b"]);
    assert_eq!(got[0], ("allow".to_owned(), "allowed_listed".to_owned()));
    assert_eq!(got[1], ("block".to_owned(), "strict_unlisted".to_owned()));

    let outside = rules_rig(&["reddit.com"], Some(r#"{"strict":true}"#), false, EnforceMode::Enforce);
    assert_eq!(verdicts(&outside, &["https://reddit.com/"]), vec![("allow".to_owned(), "outside_study".to_owned())]);
    assert!(events(&outside).iter().all(|e| e.0 == "extension"));
}

#[test]
fn emergency_pass_turns_a_block_into_a_recorded_allow() {
    let rig = rules_rig(&["reddit.com"], None, true, EnforceMode::Enforce);
    rig.db
        .lock()
        .expect("db")
        .execute(
            "INSERT INTO study_passes (at_ms, date, kind, expires_at_ms, challenge_len, typing_ms, penalty_points)
             VALUES (1, '2026-11-05', 'emergency_pass', 9999999999, 10, 1000, 5)",
            [],
        )
        .expect("pass");
    assert_eq!(verdicts(&rig, &["https://reddit.com/r/x"]), vec![("allow".to_owned(), "site_allowed_pass".to_owned())]);
    assert!(events(&rig).iter().any(|e| e.2 == "site_allowed_pass" && e.1 == "reddit.com"));
    assert!(events(&rig).iter().all(|e| e.2 != "site_blocked"));
}

#[test]
fn rule_decisions_never_store_or_log_url_parts_or_titles() {
    let rig = rules_rig(&["bank.example"], Some(r#"{"youtubePlaylists":["PLstudy"]}"#), true, EnforceMode::Enforce);
    let mut messages = handshake();
    messages.push(tab(SECRET_URL, 1));
    messages.push(tab("https://bank.example/account?token=abc#frag-secret", 2));
    messages.push(tab("https://www.youtube.com/watch?v=VIDSECRET&list=PLstudy", 3));
    let input = frames(&messages);
    let logs = capture_logs(|| {
        run(&rig, input);
    });
    let rows = format!("{:?}", events(&rig));
    let all_rows = {
        let conn = rig.db.lock().expect("db");
        let mut stmt = conn.prepare("SELECT target_id, detail_code FROM block_events").expect("prepare");
        let rows: Vec<(String, Option<String>)> =
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).expect("query").collect::<Result<_, _>>().expect("rows");
        format!("{rows:?}")
    };
    for forbidden in ["private", "path-xyz", "SECRETQ", "frag-secret", "SECRET-TITLE", "token=abc", "/account", "VIDSECRET", "PLstudy", "8443"] {
        assert!(!logs.contains(forbidden), "log leaked `{forbidden}`: {logs}");
        assert!(!rows.contains(forbidden), "rows leaked `{forbidden}`: {rows}");
        assert!(!all_rows.contains(forbidden), "db leaked `{forbidden}`: {all_rows}");
    }
}

#[test]
fn allowed_extension_id_is_valid_and_documented_with_the_extension() {
    assert_eq!(super::ALLOWED_EXTENSION_IDS.len(), 1);
    let id = super::ALLOWED_EXTENSION_IDS[0];
    assert!(super::protocol::is_valid_ext_id(id));
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../extension/README.md")).expect("readme");
    assert!(readme.contains(id), "extension/README.md must name the ID the app allows");
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../extension/public/manifest.json")).expect("manifest");
    assert!(manifest.contains("\"key\""), "the ID is only stable while the manifest pins a key");
}
