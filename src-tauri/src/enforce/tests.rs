//! Enforcement through a fake `ProcessControl`: no real process is ever ended in these tests.

use std::sync::Mutex;

use rusqlite::Connection;

use super::decide::Tracker;
use super::model::{BlockAction, EnforceMode, EntryKind, EntrySource, TargetKind};
use super::*;
use crate::db::open_in_memory;
use crate::testutil::{FakeNotifier, capture_logs, set_lock_for_test};
use crate::timer::model::{Phase, TimerState};

const GAME: &str = "com.example.game";
const SECRET_NAME: &str = "Secret Diary - bank.example.com/account?token=abc";

fn app(id: &str, pid: u32, protected: bool) -> AppIdentity {
    AppIdentity {
        id: id.into(),
        display_name: SECRET_NAME.into(),
        pid,
        is_protected: protected,
    }
}

#[derive(Default)]
struct FakeState {
    apps: Vec<AppIdentity>,
    calls: Vec<String>,
    /// A quit request makes the app disappear, like an app that closes politely.
    quit_removes: bool,
    fail_quit: bool,
    fail_list: bool,
}

#[derive(Default)]
struct FakeControl(Mutex<FakeState>);

impl FakeControl {
    fn with(apps: Vec<AppIdentity>) -> FakeControl {
        FakeControl(Mutex::new(FakeState {
            apps,
            ..FakeState::default()
        }))
    }

    fn calls(&self) -> Vec<String> {
        self.0.lock().expect("fake").calls.clone()
    }

    fn edit(&self, f: impl FnOnce(&mut FakeState)) {
        f(&mut self.0.lock().expect("fake"));
    }
}

impl ProcessControl for FakeControl {
    fn list_running(&self) -> Result<Vec<AppIdentity>, PlatformError> {
        let state = self.0.lock().expect("fake");
        if state.fail_list {
            return Err(PlatformError::Unsupported("test"));
        }
        Ok(state.apps.clone())
    }

    fn request_quit(&self, app: &AppIdentity) -> Result<(), PlatformError> {
        let mut state = self.0.lock().expect("fake");
        state.calls.push(format!("quit:{}", app.pid));
        if state.fail_quit {
            return Err(PlatformError::Unsupported("test"));
        }
        if state.quit_removes {
            state.apps.retain(|a| a.pid != app.pid);
        }
        Ok(())
    }

    fn force_kill(&self, app: &AppIdentity) -> Result<(), PlatformError> {
        let mut state = self.0.lock().expect("fake");
        state.calls.push(format!("kill:{}", app.pid));
        state.apps.retain(|a| a.pid != app.pid);
        Ok(())
    }
}

struct Rig {
    db: Mutex<Connection>,
    control: FakeControl,
    notifier: FakeNotifier,
    tracker: Tracker,
}

const WINDOW: StudyWindow = StudyWindow {
    run_id: Some(1),
    segment_id: Some(10),
};

impl Rig {
    /// A locked database with `GAME` on the block list and the given apps running.
    fn new(apps: Vec<AppIdentity>) -> Rig {
        let conn = open_in_memory().expect("db");
        store::insert_block_entry(&conn, EntryKind::App, GAME, EntrySource::User, None, 1)
            .expect("entry");
        set_lock_for_test(&conn, true);
        Rig {
            db: Mutex::new(conn),
            control: FakeControl::with(apps),
            notifier: FakeNotifier::default(),
            tracker: Tracker::default(),
        }
    }

    fn enforce(&self, force_allowed: bool) {
        let conn = self.db.lock().expect("db");
        store::put_setting(&conn, "enforcementMode", "enforcement", "enforce", 1).expect("mode");
        store::put_setting(
            &conn,
            "forceQuitAllowed",
            "general",
            &force_allowed.to_string(),
            1,
        )
        .expect("force");
    }

    fn poll_at(&mut self, window: Option<StudyWindow>, at_ms: i64) -> Vec<Attempt> {
        poll(
            &self.db,
            window,
            &self.control,
            &self.notifier,
            &mut self.tracker,
            at_ms,
        )
        .expect("poll")
    }

    fn actions(&self) -> Vec<(String, String, Option<String>)> {
        let conn = self.db.lock().expect("db");
        let mut events = store::list_block_events(&conn, 100).expect("events");
        events.reverse();
        events
            .into_iter()
            .map(|e| (e.action, e.mode, e.detail_code))
            .collect()
    }
}

fn names(rig: &Rig) -> Vec<String> {
    rig.actions()
        .into_iter()
        .map(|(action, _, _)| action)
        .collect()
}

#[test]
fn dry_run_is_the_default_and_ends_nothing() {
    let mut rig = Rig::new(vec![app(GAME, 5, false)]);
    for t in [0, 3_000, 60_000] {
        rig.poll_at(Some(WINDOW), t);
    }
    assert!(
        rig.control.calls().is_empty(),
        "dry-run must never ask or kill"
    );
    assert_eq!(
        names(&rig),
        ["would_terminate"],
        "reported once per pid, not every poll"
    );
    assert_eq!(rig.actions()[0].1, "dryRun");
    assert!(rig.notifier.sent().is_empty());
}

#[test]
fn enforce_asks_first_then_kills_only_after_the_grace_time_when_allowed() {
    let mut rig = Rig::new(vec![app(GAME, 5, false)]);
    rig.enforce(true);
    rig.poll_at(Some(WINDOW), 0);
    assert_eq!(rig.control.calls(), ["quit:5"]);
    rig.poll_at(Some(WINDOW), 9_999);
    assert_eq!(
        rig.control.calls(),
        ["quit:5"],
        "still inside the grace time (default 10 s)"
    );
    rig.poll_at(Some(WINDOW), 10_000);
    assert_eq!(
        rig.control.calls(),
        ["quit:5", "kill:5"],
        "graceful request strictly before the force kill"
    );
    rig.poll_at(Some(WINDOW), 13_000);
    assert_eq!(rig.control.calls().len(), 2, "no repeat after the kill");
    assert_eq!(names(&rig), ["terminate_requested", "force_killed"]);
    assert_eq!(
        rig.notifier.sent().len(),
        1,
        "the user is told once the app is gone"
    );
}

#[test]
fn a_polite_quit_is_recorded_as_terminated_and_never_forced() {
    let mut rig = Rig::new(vec![app(GAME, 5, false)]);
    rig.enforce(true);
    rig.control.edit(|s| s.quit_removes = true);
    rig.poll_at(Some(WINDOW), 0);
    rig.poll_at(Some(WINDOW), 3_000);
    rig.poll_at(Some(WINDOW), 30_000);
    assert_eq!(rig.control.calls(), ["quit:5"]);
    assert_eq!(names(&rig), ["terminate_requested", "terminated"]);
    let sent = rig.notifier.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].1.contains("앱을 종료했습니다"));
}

#[test]
fn without_force_permission_an_app_that_ignores_the_request_is_left_running_and_recorded() {
    let mut rig = Rig::new(vec![app(GAME, 5, false)]);
    rig.enforce(false);
    rig.poll_at(Some(WINDOW), 0);
    rig.poll_at(Some(WINDOW), 20_000);
    rig.poll_at(Some(WINDOW), 40_000);
    assert_eq!(
        rig.control.calls(),
        ["quit:5"],
        "force quit is off by default"
    );
    assert_eq!(names(&rig), ["terminate_requested", "failed"]);
    assert_eq!(
        rig.actions()[1].2.as_deref(),
        Some("grace_expired_force_not_allowed")
    );
}

#[test]
fn a_failed_quit_request_is_recorded_and_the_grace_clock_still_runs() {
    let mut rig = Rig::new(vec![app(GAME, 5, false)]);
    rig.enforce(true);
    rig.control.edit(|s| s.fail_quit = true);
    rig.poll_at(Some(WINDOW), 0);
    rig.poll_at(Some(WINDOW), 10_000);
    assert_eq!(rig.control.calls(), ["quit:5", "kill:5"]);
    assert_eq!(names(&rig), ["failed", "force_killed"]);
    assert_eq!(rig.actions()[0].2.as_deref(), Some("unsupported"));
}

#[test]
fn protected_apps_are_never_touched_even_when_on_the_list() {
    let mut rig = Rig::new(vec![app(GAME, 5, true)]);
    rig.enforce(true);
    rig.poll_at(Some(WINDOW), 0);
    rig.poll_at(Some(WINDOW), 60_000);
    assert!(rig.control.calls().is_empty());
    assert_eq!(names(&rig), ["skipped_protected"]);
}

#[test]
fn protected_and_self_apps_cannot_be_added_to_the_list() {
    let mut conn = open_in_memory().expect("db");
    for id in [
        "com.apple.finder",
        "com.apple.loginwindow",
        "com.studymanager.app",
        "C:\\Windows\\System32\\lsass.exe",
        "  ",
    ] {
        assert!(add_entry(&mut conn, id, None, 1).is_err(), "{id}");
    }
    assert!(store::list_block_entries(&conn).expect("list").is_empty());
}

#[test]
fn breaks_release_blocking_while_the_lock_stays() {
    let mut rig = Rig::new(vec![app(GAME, 5, false)]);
    rig.enforce(true);
    rig.poll_at(Some(WINDOW), 0);
    assert_eq!(rig.control.calls(), ["quit:5"]);
    // Break begins: the timer state is break + lock held.
    let state = TimerState {
        phase: Phase::Break,
        run_id: Some(1),
        segment_id: Some(11),
        set_index: 1,
        sets_total: 2,
        planned_end_ms: Some(1),
        lock_held: true,
        now_ms: 0,
        next: None,
    };
    assert_eq!(study_window(&state), None);
    assert!(rig.poll_at(study_window(&state), 40_000).is_empty());
    assert_eq!(
        rig.control.calls(),
        ["quit:5"],
        "no force kill during the break"
    );
    assert!(
        crate::study::lock_held(&rig.db.lock().expect("db")).expect("lock"),
        "the lock itself is untouched"
    );
    // The next study segment starts clean: the app is asked again, not killed on the old clock.
    let next = StudyWindow {
        run_id: Some(1),
        segment_id: Some(12),
    };
    rig.poll_at(Some(next), 50_000);
    assert_eq!(rig.control.calls(), ["quit:5", "quit:5"]);
}

#[test]
fn study_window_needs_both_the_lock_and_a_study_phase() {
    let base = TimerState {
        phase: Phase::Study,
        run_id: Some(2),
        segment_id: Some(3),
        set_index: 1,
        sets_total: 1,
        planned_end_ms: Some(1),
        lock_held: true,
        now_ms: 0,
        next: None,
    };
    assert_eq!(
        study_window(&base),
        Some(StudyWindow {
            run_id: Some(2),
            segment_id: Some(3)
        })
    );
    assert_eq!(
        study_window(&TimerState {
            lock_held: false,
            ..base.clone()
        }),
        None
    );
    for phase in [Phase::Idle, Phase::Break, Phase::LongBreak] {
        assert_eq!(
            study_window(&TimerState {
                phase,
                ..base.clone()
            }),
            None,
            "{phase:?}"
        );
    }
}

#[test]
fn a_released_lock_stops_a_pending_kill() {
    let mut rig = Rig::new(vec![app(GAME, 5, false)]);
    rig.enforce(true);
    // The poll was handed a study window, but the emergency release already freed the lock.
    set_lock_for_test(&rig.db.lock().expect("db"), false);
    rig.poll_at(Some(WINDOW), 0);
    assert!(rig.control.calls().is_empty());
    assert!(rig.actions().is_empty());
}

#[test]
fn an_empty_block_list_or_a_failing_listing_does_nothing() {
    let mut rig = Rig::new(vec![app("com.example.other", 6, false)]);
    rig.enforce(true);
    rig.poll_at(Some(WINDOW), 0);
    assert!(
        rig.control.calls().is_empty() && rig.actions().is_empty(),
        "unlisted apps are ignored"
    );
    rig.control.edit(|s| s.fail_list = true);
    assert!(rig.poll_at(Some(WINDOW), 3_000).is_empty());
    assert!(rig.control.calls().is_empty());
}

#[test]
fn a_reused_pid_is_not_mistaken_for_the_app_we_asked() {
    let mut rig = Rig::new(vec![app(GAME, 5, false)]);
    rig.enforce(true);
    rig.poll_at(Some(WINDOW), 0);
    // The original exited and an unrelated app took pid 5.
    rig.control
        .edit(|s| s.apps = vec![app("com.example.calendar", 5, false)]);
    rig.poll_at(Some(WINDOW), 15_000);
    assert_eq!(
        rig.control.calls(),
        ["quit:5"],
        "the unrelated app is not killed"
    );
    assert_eq!(names(&rig), ["terminate_requested", "terminated"]);
}

#[test]
fn block_list_edits_are_locked_during_study_and_the_attempt_is_recorded() {
    let mut conn = open_in_memory().expect("db");
    assert_eq!(
        add_entry(&mut conn, " Com.Example.Game ", Some("game"), 1).expect("add"),
        UpdateOutcome::Applied {
            key: "blockList".into()
        }
    );
    assert_eq!(
        store::list_enabled_patterns(&conn, EntryKind::App).expect("list"),
        [GAME]
    );
    assert!(
        add_entry(&mut conn, GAME, None, 2).is_err(),
        "duplicates are refused"
    );
    let id = store::list_block_entries(&conn).expect("list")[0].id;

    set_lock_for_test(&conn, true);
    assert_eq!(
        add_entry(&mut conn, "com.example.other", None, 3).expect("add"),
        UpdateOutcome::RejectedLocked {
            key: "blockList".into()
        }
    );
    assert_eq!(
        remove_entry(&mut conn, id, 4).expect("remove"),
        UpdateOutcome::RejectedLocked {
            key: "blockList".into()
        }
    );
    assert_eq!(store::list_block_entries(&conn).expect("list").len(), 1);
    let history = settings::read_history(&conn, 2).expect("history");
    assert!(
        history
            .iter()
            .all(|h| h.key == "blockList" && h.outcome == "rejected_locked")
    );

    set_lock_for_test(&conn, false);
    assert_eq!(
        remove_entry(&mut conn, id, 5).expect("remove"),
        UpdateOutcome::Applied {
            key: "blockList".into()
        }
    );
    assert!(store::list_block_entries(&conn).expect("list").is_empty());
    assert!(remove_entry(&mut conn, id, 6).is_err(), "unknown id");
}

#[test]
fn enforce_mode_needs_the_confirmation_sentence_and_is_locked_during_study() {
    let mut conn = open_in_memory().expect("db");
    assert_eq!(
        read_config(&conn).expect("cfg").mode,
        EnforceMode::DryRun,
        "dry-run is the default"
    );
    assert!(!read_config(&conn).expect("cfg").force_quit_allowed);
    assert_eq!(read_config(&conn).expect("cfg").grace_ms, 10_000);
    assert!(set_mode(&mut conn, EnforceMode::Enforce, "wrong", 1).is_err());
    assert_eq!(read_config(&conn).expect("cfg").mode, EnforceMode::DryRun);

    set_lock_for_test(&conn, true);
    let outcome =
        set_mode(&mut conn, EnforceMode::Enforce, ENFORCE_CONFIRMATION, 2).expect("attempt");
    assert_eq!(
        outcome,
        UpdateOutcome::RejectedLocked {
            key: "enforcementMode".into()
        }
    );
    assert_eq!(read_config(&conn).expect("cfg").mode, EnforceMode::DryRun);

    set_lock_for_test(&conn, false);
    set_mode(&mut conn, EnforceMode::Enforce, ENFORCE_CONFIRMATION, 3).expect("enforce");
    assert_eq!(read_config(&conn).expect("cfg").mode, EnforceMode::Enforce);
    let latest = &settings::read_history(&conn, 1).expect("h")[0];
    assert_eq!(
        (
            latest.key.as_str(),
            latest.after.as_str(),
            latest.outcome.as_str()
        ),
        ("enforcementMode", "enforce", "applied")
    );
    assert!(
        !latest.after.contains(ENFORCE_CONFIRMATION),
        "the typed sentence is not stored"
    );
    set_mode(&mut conn, EnforceMode::DryRun, "", 4).expect("back to dry-run needs no sentence");
    assert_eq!(read_config(&conn).expect("cfg").mode, EnforceMode::DryRun);
}

#[test]
fn block_events_are_append_only() {
    let conn = open_in_memory().expect("db");
    let event = NewBlockEvent {
        at_ms: 1,
        run_id: None,
        segment_id: None,
        target_kind: TargetKind::App,
        target_id: GAME,
        action: BlockAction::WouldTerminate,
        mode: EnforceMode::DryRun,
        detail_code: None,
    };
    store::insert_block_event(&conn, &event).expect("insert");
    assert!(
        conn.execute("UPDATE block_events SET action = 'terminated'", [])
            .is_err()
    );
    assert!(conn.execute("DELETE FROM block_events", []).is_err());
}

fn check_list(conn: &Connection, table: &str, column: &str) -> Vec<String> {
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = ?1",
            [table],
            |r| r.get(0),
        )
        .expect("sql");
    let start = sql
        .find(&format!("{column} "))
        .and_then(|i| sql[i..].find("IN (").map(|j| i + j + 4))
        .expect("check");
    let list = &sql[start..start + sql[start..].find(')').expect("end")];
    list.split(',')
        .map(|v| v.trim().trim_matches('\'').to_owned())
        .collect()
}

#[test]
fn check_constraints_list_exactly_the_rust_enum_values() {
    let conn = open_in_memory().expect("db");
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    let rust = |all: &[&str]| sorted(all.iter().map(|s| (*s).to_owned()).collect());
    assert_eq!(
        sorted(check_list(&conn, "block_events", "action")),
        rust(&BlockAction::ALL.map(BlockAction::as_str))
    );
    assert_eq!(
        sorted(check_list(&conn, "block_events", "target_kind")),
        rust(&TargetKind::ALL.map(TargetKind::as_str))
    );
    assert_eq!(
        sorted(check_list(&conn, "block_entries", "kind")),
        rust(&EntryKind::ALL.map(EntryKind::as_str))
    );
    assert_eq!(
        sorted(check_list(&conn, "block_entries", "source")),
        rust(&EntrySource::ALL.map(EntrySource::as_str))
    );
    // Every Rust value is accepted by the real CHECK, not just listed in it.
    for action in BlockAction::ALL {
        let event = NewBlockEvent {
            at_ms: 1,
            run_id: None,
            segment_id: None,
            target_kind: TargetKind::App,
            target_id: "x",
            action,
            mode: EnforceMode::DryRun,
            detail_code: None,
        };
        store::insert_block_event(&conn, &event).expect("accepted");
    }
}

#[test]
fn logs_and_rows_carry_app_ids_only_never_names_titles_or_urls() {
    let mut rig = Rig::new(vec![app(GAME, 5, false), app("com.apple.finder", 6, true)]);
    {
        let conn = rig.db.lock().expect("db");
        store::insert_block_entry(
            &conn,
            EntryKind::App,
            "com.apple.finder",
            EntrySource::User,
            None,
            2,
        )
        .expect("entry");
    }
    rig.enforce(true);
    rig.control.edit(|s| s.quit_removes = true);
    let logs = capture_logs(|| {
        rig.poll_at(Some(WINDOW), 0);
        rig.poll_at(Some(WINDOW), 3_000);
    });
    for needle in ["Secret", "Diary", "bank.example", "token", "account"] {
        assert!(!logs.contains(needle), "log leaked {needle}: {logs}");
    }
    for action in ["terminate_requested", "terminated", "skipped_protected"] {
        assert!(logs.contains(action), "missing {action}: {logs}");
    }
    assert!(logs.contains(GAME));
    let rows = serde_json::to_string(
        &store::list_block_events(&rig.db.lock().expect("db"), 50).expect("events"),
    )
    .expect("json");
    assert!(
        !rows.contains("Secret") && !rows.contains("bank.example"),
        "{rows}"
    );
}

#[test]
fn every_action_uses_one_log_format() {
    let mut rig = Rig::new(vec![app(GAME, 5, false)]);
    rig.enforce(true);
    let logs = capture_logs(|| {
        rig.poll_at(Some(WINDOW), 0);
        rig.poll_at(Some(WINDOW), 10_000);
    });
    for action in ["terminate_requested", "force_killed"] {
        let line = logs
            .lines()
            .find(|l| {
                l.contains(&format!("event=\"{action}\"")) || l.contains(&format!("event={action}"))
            })
            .unwrap_or_else(|| panic!("no {action}: {logs}"));
        for field in ["target_kind", "target_id", "mode", "detail_code"] {
            assert!(line.contains(field), "{action} lacks {field}: {line}");
        }
    }
}

#[test]
fn running_apps_are_deduplicated_and_sorted_for_the_picker() {
    let control = FakeControl::with(vec![
        AppIdentity {
            id: "b".into(),
            display_name: "Beta".into(),
            pid: 1,
            is_protected: false,
        },
        AppIdentity {
            id: "a".into(),
            display_name: "alpha".into(),
            pid: 2,
            is_protected: true,
        },
        AppIdentity {
            id: "b".into(),
            display_name: "Beta".into(),
            pid: 3,
            is_protected: false,
        },
    ]);
    let apps = running_apps(&control).expect("apps");
    assert_eq!(
        apps.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(apps[0].is_protected);
    control.edit(|s| s.fail_list = true);
    assert!(running_apps(&control).is_err());
}

#[test]
fn status_reports_activity_only_for_a_locked_study_block() {
    let conn = open_in_memory().expect("db");
    let state = TimerState {
        phase: Phase::Study,
        run_id: Some(1),
        segment_id: Some(1),
        set_index: 1,
        sets_total: 1,
        planned_end_ms: Some(1),
        lock_held: true,
        now_ms: 0,
        next: None,
    };
    let on = status(&conn, &state).expect("status");
    assert!(on.active);
    assert_eq!(
        (on.mode.as_str(), on.grace_wait_sec, on.force_quit_allowed),
        ("dryRun", 10, false)
    );
    assert!(
        !status(
            &conn,
            &TimerState {
                phase: Phase::Break,
                ..state
            }
        )
        .expect("status")
        .active
    );
}

#[test]
fn an_emergency_pass_pauses_enforcement_until_it_expires() {
    let mut rig = Rig::new(vec![app(GAME, 5, false)]);
    rig.enforce(true);
    rig.db
        .lock()
        .expect("db")
        .execute(
            "INSERT INTO study_passes (at_ms, date, kind, expires_at_ms, challenge_len, typing_ms, penalty_points)
             VALUES (0, '2026-11-05', 'emergency_pass', 600000, 90, 1000, 5)",
            [],
        )
        .expect("pass");
    for t in [0, 3_000, 599_999] {
        rig.poll_at(Some(WINDOW), t);
    }
    assert!(
        rig.control.calls().is_empty(),
        "no quit request while the pass runs"
    );
    assert!(rig.actions().is_empty());
    rig.poll_at(Some(WINDOW), 600_000);
    assert_eq!(
        rig.control.calls(),
        ["quit:5"],
        "enforcement resumes the moment the pass ends"
    );
}
