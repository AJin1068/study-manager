//! End to end over a fake browser link: the real timer decides whether a study block is locked,
//! the real presets and site rules decide each page, and the real tables record the result.
//! Only the browser side is fake (a byte stream with scripted frames).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use rusqlite::Connection;

use super::coverage;
use super::register;
use super::service::{LinkEnv, RulesJudge, Service};
use super::tests::{frames, handshake, tab, TestStream, EXT, TOKEN};
use super::{LinkHub, LinkStatus};
use crate::db::open_in_memory;
use crate::enforce::model::{BlockEvent, EnforceMode};
use crate::enforce::{self, StudyWindow};
use crate::error::AppResult;
use crate::plan::{create_plan, BlockKind, CreatePlanInput};
use crate::presets;
use crate::settings::UpdateOutcome;
use crate::site_rules::edit;
use crate::store;
use crate::testutil::capture_logs;
use crate::timer;

const DAY: i64 = 24 * 3600 * 1000;

fn t(h: i64, m: i64, s: i64) -> i64 {
    let base = Utc.with_ymd_and_hms(2026, 11, 5, 0, 0, 0).single().expect("date").timestamp_millis();
    base + (h * 3600 + m * 60 + s) * 1000
}

/// The app's own study-window lookup (timer state), on a clock the test steps by one second per call.
struct TimerEnv {
    now: AtomicI64,
}

impl LinkEnv for TimerEnv {
    fn now_ms(&self) -> AppResult<i64> {
        Ok(self.now.fetch_add(1000, Ordering::SeqCst))
    }

    fn study_window(&self, conn: &Connection, now_ms: i64) -> AppResult<Option<StudyWindow>> {
        Ok(enforce::study_window(&timer::current_state(conn, &Utc, now_ms)?))
    }
}

struct World {
    db: Arc<Mutex<Connection>>,
    hub: Arc<LinkHub>,
    service: Service,
    run_id: i64,
    study_started_ms: i64,
}

/// A 20:00 plan with the default presets chosen and one study playlist registered, then the run
/// started so the lock is held.
fn world() -> World {
    let mut conn = open_in_memory().expect("db");
    let input = CreatePlanInput {
        name: "수학".into(),
        subject: "수학".into(),
        start_date: "2026-11-05".into(),
        start_time: "20:00".into(),
        sets: 2,
        repeat: None,
    };
    create_plan(&mut conn, &Utc, input, t(0, 0, 0) - DAY).expect("plan");
    enforce::set_mode(&mut conn, EnforceMode::Enforce, enforce::ENFORCE_CONFIRMATION, t(12, 0, 0)).expect("mode");
    presets::set_categories(&mut conn, &BTreeMap::new(), t(12, 0, 1)).expect("presets");
    let rules = edit::read_config(&conn).expect("rules");
    let rules = crate::site_rules::SiteRulesConfig { youtube_playlists: vec!["PLstudy".into()], ..rules };
    edit::update_config(&mut conn, rules, t(12, 0, 2)).expect("playlist");
    timer::reconcile(&mut conn, &Utc, t(20, 0, 1)).expect("run starts");
    assert!(crate::study::lock_held(&conn).expect("lock"), "the study block must hold the lock");
    let run = store::list_active_runs(&conn).expect("runs").remove(0);
    let segment = store::list_segments(&conn, run.id)
        .expect("segments")
        .into_iter()
        .find(|s| s.kind == BlockKind::Study)
        .expect("study segment");

    let db = Arc::new(Mutex::new(conn));
    let hub = Arc::new(LinkHub::new(Box::new(|_: LinkStatus| {})));
    let service = Service::new(
        Arc::clone(&db),
        Arc::clone(&hub),
        Arc::new(TimerEnv { now: AtomicI64::new(t(20, 0, 10)) }),
        Arc::new(RulesJudge::new(Arc::clone(&db))),
        TOKEN.into(),
        vec![EXT.to_owned()],
    );
    World { db, hub, service, run_id: run.id, study_started_ms: segment.started_at_ms }
}

fn visit(world: &World, urls: &[&str]) -> Vec<(String, String)> {
    let mut messages = handshake();
    messages.extend(urls.iter().zip(1..).map(|(url, id)| tab(url, id)));
    let mut stream = TestStream::new(frames(&messages));
    world.service.serve(&mut stream);
    stream.replies()[1..]
        .iter()
        .map(|r| (r["verdict"].as_str().unwrap_or("").to_owned(), r["reasonCode"].as_str().unwrap_or("").to_owned()))
        .collect()
}

fn pair(verdict: &str, reason: &str) -> (String, String) {
    (verdict.to_owned(), reason.to_owned())
}

fn all_events(world: &World) -> Vec<BlockEvent> {
    let conn = world.db.lock().expect("db");
    let mut events = store::list_block_events(&conn, 500).expect("events");
    events.reverse();
    events
}

const PAGES: [&str; 6] = [
    "https://reddit.com/r/rust?q=SECRETQ#frag-secret",
    "https://www.coupang.com/vp/products/1",
    "https://www.youtube.com/watch?v=VIDSECRET&list=PLstudy",
    "https://www.youtube.com/watch?v=VIDSECRET",
    "https://docs.rs/serde",
    "chrome://newtab",
];

fn page_verdicts() -> Vec<(String, String)> {
    vec![
        pair("block", "blocked_site"),
        pair("allow", "unlisted_allowed"),
        pair("allow", "allowed_study_video"),
        pair("block", "blocked_site"),
        pair("allow", "unlisted_allowed"),
        pair("allow", "not_web"),
    ]
}

#[test]
fn tab_change_to_decision_to_block_event_to_reply_with_presets_and_summary_counts() {
    let world = world();
    let logs = capture_logs(|| {
        assert_eq!(visit(&world, &PAGES), page_verdicts());
    });

    let events = all_events(&world);
    let sites: Vec<_> = events.iter().filter(|e| e.target_kind == "site").collect();
    assert_eq!(sites.len(), 2, "only the two blocked pages are site events");
    for event in &sites {
        assert_eq!(
            (event.action.as_str(), event.target_id.as_str(), event.mode.as_str(), event.detail_code.as_deref()),
            ("site_blocked", if event.id == sites[0].id { "reddit.com" } else { "www.youtube.com" }, "enforce", Some("blocked_site"))
        );
        assert_eq!(event.run_id, Some(world.run_id));
        assert!(event.segment_id.is_some());
    }
    // The summary the timer shows counts the site blocks like app blocks.
    let conn = world.db.lock().expect("db");
    assert_eq!(store::count_attempts(&conn, world.run_id, None).expect("attempts"), 2);

    // Nothing from the URL, query, fragment, title, or playlist reached the log or any row.
    let dump: String = events.iter().map(|e| format!("{e:?}")).collect();
    for forbidden in ["SECRETQ", "frag-secret", "VIDSECRET", "PLstudy", "/r/rust", "products", "SECRET-TITLE"] {
        assert!(!dump.contains(forbidden), "row leaked `{forbidden}`: {dump}");
        assert!(!logs.contains(forbidden), "log leaked `{forbidden}`: {logs}");
    }
}

#[test]
fn every_rules_edit_is_refused_during_the_run_and_blocking_stays_as_it_was() {
    let world = world();
    {
        let mut conn = world.db.lock().expect("db");
        let host = std::path::Path::new("/tmp/study-manager-host");
        let dir = std::env::temp_dir().join(format!("study-manager-e2e-{}", std::process::id()));
        register::set_registered(&mut conn, &dir, host, true, t(12, 0, 3)).expect("register before the run");

        let loosened = crate::site_rules::SiteRulesConfig { strict: false, allow: Some(vec!["reddit.com".into()]), ..Default::default() };
        let attempts = [
            edit::update_config(&mut conn, loosened, t(20, 0, 5)).expect("rules"),
            edit::add_site(&mut conn, "newsite.example", t(20, 0, 6)).expect("site"),
            presets::set_categories(&mut conn, &BTreeMap::from([("video".to_owned(), false)]), t(20, 0, 7)).expect("presets"),
            register::set_registered(&mut conn, &dir, host, false, t(20, 0, 8)).expect("unregister"),
        ];
        for outcome in attempts {
            assert!(matches!(outcome, UpdateOutcome::RejectedLocked { .. }), "{outcome:?}");
        }
        assert!(crate::platform::is_host_registered(&dir), "the host manifest must survive the attempt");
        let rejected = store::list_history(&conn, 10).expect("history").into_iter().filter(|h| h.outcome == "rejected_locked").count();
        assert_eq!(rejected, 4, "each attempt leaves a record");
        let _ = std::fs::remove_dir_all(&dir);
    }
    // Same verdicts as before the attempts: reddit still blocked, the video category still on.
    assert_eq!(visit(&world, &PAGES), page_verdicts());
}

#[test]
fn a_dropped_link_is_recorded_and_its_gap_shows_up_in_the_run_coverage() {
    let world = world();
    // First link: connects, one tab, then the stream just ends (extension disabled or removed).
    assert_eq!(visit(&world, &PAGES[..1]), vec![pair("block", "blocked_site")]);
    let status = world.hub.status();
    assert!(!status.connected);
    assert_eq!(status.last_disconnect_reason.as_deref(), Some("closed"));

    let events = all_events(&world);
    let links: Vec<_> = events.iter().filter(|e| e.target_kind == "extension").collect();
    assert_eq!(links.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(), ["extension_connected", "extension_disconnected"]);
    assert_eq!(links[1].detail_code.as_deref(), Some("closed"));
    let watched = links[1].at_ms - links[0].at_ms;

    // Ten minutes in, with no link: everything outside the one watched stretch is unwatched.
    let now = t(20, 10, 0);
    let conn = world.db.lock().expect("db");
    let got = coverage::run_coverage(&conn, world.run_id, now).expect("coverage");
    assert_eq!(got.study_ms, now - world.study_started_ms);
    assert_eq!(got.unmonitored_ms, got.study_ms - watched);
    assert!(got.unmonitored_ms > got.study_ms / 2, "most of the run was unwatched");
}
