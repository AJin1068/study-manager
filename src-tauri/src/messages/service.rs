//! Builds, logs, and delivers messages. A pass reads the timer state, asks the engine whether
//! one is due, picks the text, writes `message_log` (kind, tone, source, run; never the text),
//! and then shows it: a toast event for the window (may name a wiki concept) and, if on, a system
//! notification through the existing `Notifier` that carries only the general phrase.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chrono::{Local, TimeZone};
use rusqlite::Connection;
use tauri::{AppHandle, Emitter};

use super::config::MessageConfig;
use super::engine::{Engine, Trigger};
use super::{compose, Kind, Message, MESSAGE_EVENT};
use crate::commands::lock_db;
use crate::error::AppResult;
use crate::platform::Notifier;
use crate::quiz::select::mix;
use crate::remote::Clock;
use crate::store;
use crate::store::messages as repo;
use crate::timer::model::TimerState;
use crate::wiki::recommend::{self, Reason};
use crate::wiki::service::subject_for_plan;

const POLL: Duration = Duration::from_secs(4);
/// The notification title: the app, never a subject.
const NOTICE_TITLE: &str = "Study Manager";
/// Days in a row from which a finished set is celebrated as a streak.
const STREAK_FROM_DAYS: i64 = 2;

pub struct MessageService {
    db: Arc<Mutex<Connection>>,
    clock: Arc<dyn Clock>,
    notifier: Arc<dyn Notifier>,
    engine: Engine,
}

/// A wiki page worth asking about: the first page of the run's subject that is not empty, in the
/// order of `recommend::rank` (weak first). `None` when the subject has no wiki or no such page.
fn concept_for(conn: &Connection, run_id: Option<i64>, now_ms: i64) -> Option<String> {
    let run = store::get_run(conn, run_id?).ok()??;
    let plan = store::get_plan(conn, run.plan_id).ok()??;
    let subject = subject_for_plan(conn, &plan.subject).ok()??;
    let stats = recommend::page_stats(conn, subject.id).ok()?;
    recommend::rank(&stats, now_ms).into_iter().find(|p| p.reason != Reason::Empty).map(|p| p.page_name)
}

fn streak_days<Tz: TimeZone>(conn: &mut Connection, tz: &Tz, now_ms: i64) -> i64 {
    crate::dashboard::today(conn, tz, now_ms).map_or(0, |t| t.streak_days)
}

impl MessageService {
    pub fn new(db: Arc<Mutex<Connection>>, clock: Arc<dyn Clock>, notifier: Arc<dyn Notifier>) -> Self {
        Self { db, clock, notifier, engine: Engine::default() }
    }

    /// One pass. Returns the message that was sent, if any.
    pub fn tick(&self) -> AppResult<Option<Message>> {
        let now = self.clock.now_ms()?;
        let (message, cfg) = {
            let mut conn = lock_db(&self.db)?;
            let cfg = MessageConfig::load(&conn)?;
            let state = crate::timer::current_state(&conn, &Local, now)?;
            (self.produce(&mut conn, &Local, &state, &cfg, now)?, cfg)
        };
        if let Some(message) = &message {
            self.deliver(&cfg, message);
        }
        Ok(message)
    }

    /// Decides, composes, and logs. Does not show anything.
    pub fn produce<Tz: TimeZone>(&self, conn: &mut Connection, tz: &Tz, state: &TimerState, cfg: &MessageConfig, now_ms: i64) -> AppResult<Option<Message>> {
        let Some(trigger) = self.engine.decide(conn, state, cfg, now_ms)? else { return Ok(None) };
        let kind = match trigger {
            Trigger::Distraction => Kind::Distraction,
            Trigger::Random => Kind::Random,
            Trigger::SetDone if streak_days(conn, tz, now_ms) >= STREAK_FROM_DAYS => Kind::Streak,
            Trigger::SetDone => Kind::SetEnd,
        };
        let concept = if kind == Kind::Distraction { None } else { concept_for(conn, state.run_id, now_ms) };
        let seed = mix(u64::try_from(now_ms).unwrap_or(0) ^ u64::try_from(state.set_index).unwrap_or(0));
        let message = compose(kind, cfg.tone, concept.as_deref(), seed);
        repo::insert(conn, now_ms, kind.as_str(), cfg.tone.as_str(), message.source.as_str(), state.run_id)?;
        tracing::info!(event = "cheer_message", kind = kind.as_str(), tone = cfg.tone.as_str(), source = message.source.as_str());
        Ok(Some(message))
    }

    /// The system notification (general phrase only, no subject or page).
    fn deliver(&self, cfg: &MessageConfig, message: &Message) {
        if !cfg.notify_enabled {
            return;
        }
        if let Err(err) = self.notifier.notify(NOTICE_TITLE, &message.notice) {
            tracing::warn!(error = %err, "message notification not delivered");
        }
    }
}

pub fn spawn(app: AppHandle, service: Arc<MessageService>) {
    thread::spawn(move || loop {
        thread::sleep(POLL);
        match catch_unwind(AssertUnwindSafe(|| service.tick())) {
            Ok(Ok(Some(message))) => {
                if let Err(err) = app.emit(MESSAGE_EVENT, message) {
                    tracing::error!(error = %err, "failed to emit cheer message");
                }
            }
            Ok(Ok(None)) => {}
            Ok(Err(err)) => tracing::warn!(event = "cheer_pass_failed", error = %err),
            Err(_) => tracing::error!(event = "cheer_pass_panicked", "message pass panicked; retrying"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::llm::config::{update_setting, LlmSettingKey};
    use crate::messages::config::{update_setting as set_message, MessageSettingKey};
    use crate::messages::Tone;
    use crate::remote::ManualClock;
    use crate::settings::SettingValue;
    use crate::testutil::{FakeNotifier, TempDir};
    use crate::timer::model::Phase;
    use crate::store::wiki as wiki_repo;

    const T0: i64 = 1_800_000_000_000;
    const MIN: i64 = 60_000;

    struct Rig {
        conn: Connection,
        service: MessageService,
        notifier: Arc<FakeNotifier>,
        run_id: i64,
        dir: TempDir,
    }

    fn rig() -> Rig {
        let mut conn = open_in_memory().expect("db");
        let dir = TempDir::new("cheer");
        let root = dir.path().join("wiki");
        update_setting(&mut conn, LlmSettingKey::WikiDir, SettingValue::Text(root.to_string_lossy().into_owned()), 1).expect("dir");
        let (run_id, _) = crate::testutil::insert_run_for_test(&conn, "수학");
        let notifier = Arc::new(FakeNotifier::default());
        let service = MessageService::new(Arc::new(std::sync::Mutex::new(open_in_memory().expect("unused"))), Arc::new(ManualClock::new(T0)), notifier.clone());
        Rig { conn, service, notifier, run_id, dir }
    }

    fn state(r: &Rig, phase: Phase, lock_held: bool, set_index: i64) -> TimerState {
        TimerState { phase, run_id: Some(r.run_id), segment_id: Some(1), set_index, sets_total: 4, planned_end_ms: None, lock_held, now_ms: T0, next: None }
    }

    fn attempt(r: &Rig, at_ms: i64) {
        r.conn
            .execute(
                "INSERT INTO block_events (at_ms, run_id, segment_id, target_kind, target_id, action, mode)
                 VALUES (?1, ?2, 1, 'app', 'game', 'terminate_requested', 'enforce')",
                rusqlite::params![at_ms, r.run_id],
            )
            .expect("event");
    }

    fn step(r: &mut Rig, st: &TimerState, at: i64) -> Option<Message> {
        let cfg = MessageConfig::load(&r.conn).expect("cfg");
        let message = r.service.produce(&mut r.conn, &chrono::Utc, st, &cfg, at).expect("produce");
        if let Some(m) = &message {
            r.service.deliver(&cfg, m);
        }
        message
    }

    fn logged(r: &Rig) -> i64 {
        r.conn.query_row("SELECT COUNT(*) FROM message_log", [], |x| x.get(0)).expect("count")
    }

    #[test]
    fn a_blocked_app_attempt_in_a_study_block_sends_one_message_and_a_lock_screen_safe_notice() {
        let mut r = rig();
        let study = state(&r, Phase::Study, true, 1);
        assert!(step(&mut r, &study, T0).is_none(), "the first look only records marks");
        attempt(&r, T0 + 1);
        let message = step(&mut r, &study, T0 + 2).expect("message");
        assert_eq!(message.kind, Kind::Distraction);
        assert_eq!(message.tone, Tone::Normal);
        assert_eq!(logged(&r), 1);
        let sent = r.notifier.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1, message.notice);
        assert!(step(&mut r, &study, T0 + 3).is_none(), "the same attempt is not announced twice");
    }

    #[test]
    fn nothing_is_sent_in_a_break_between_runs_or_with_the_lock_free() {
        let mut r = rig();
        let study = state(&r, Phase::Study, true, 1);
        step(&mut r, &study, T0);
        for (i, st) in [state(&r, Phase::Break, true, 1), state(&r, Phase::LongBreak, true, 1), state(&r, Phase::Idle, false, 0), state(&r, Phase::Study, false, 1)]
            .iter()
            .enumerate()
        {
            attempt(&r, T0 + 10 + i64::try_from(i).expect("i"));
            assert!(step(&mut r, st, T0 + 20 + i64::try_from(i).expect("i")).is_none(), "{:?}", st.phase);
        }
        assert_eq!(logged(&r), 0);
        assert!(r.notifier.sent().is_empty());
        // An attempt made during the break is not delivered late once studying resumes.
        assert!(step(&mut r, &study, T0 + 100).is_none());
    }

    #[test]
    fn the_hourly_cap_limits_messages_and_an_hour_later_they_resume() {
        let mut r = rig();
        set_message(&mut r.conn, MessageSettingKey::MaxPerHour, SettingValue::Int(2), 1).expect("cap");
        let study = state(&r, Phase::Study, true, 1);
        step(&mut r, &study, T0);
        let mut sent = 0;
        for i in 1..=5 {
            attempt(&r, T0 + i);
            sent += i64::from(step(&mut r, &study, T0 + i).is_some());
        }
        assert_eq!(sent, 2);
        attempt(&r, T0 + 61 * MIN);
        assert!(step(&mut r, &study, T0 + 61 * MIN).is_some());
    }

    #[test]
    fn a_finished_set_sends_a_set_end_message() {
        let mut r = rig();
        let (first, rest, second) = (state(&r, Phase::Study, true, 1), state(&r, Phase::Break, true, 1), state(&r, Phase::Study, true, 2));
        step(&mut r, &first, T0);
        step(&mut r, &rest, T0 + MIN);
        let message = step(&mut r, &second, T0 + 2 * MIN).expect("message");
        assert_eq!(message.kind, Kind::SetEnd);
    }

    #[test]
    fn random_messages_wait_for_the_quiet_time_and_only_when_switched_on() {
        let mut r = rig();
        let study = state(&r, Phase::Study, true, 1);
        step(&mut r, &study, T0);
        assert!(step(&mut r, &study, T0 + 40 * MIN).is_none(), "random is off by default");
        set_message(&mut r.conn, MessageSettingKey::RandomEnabled, SettingValue::Bool(true), 2).expect("on");
        assert!(step(&mut r, &study, T0 + 20 * MIN).is_none(), "30 minutes have not passed");
        let message = step(&mut r, &study, T0 + 31 * MIN).expect("message");
        assert_eq!(message.kind, Kind::Random);
        assert!(step(&mut r, &study, T0 + 40 * MIN).is_none(), "the quiet time restarts after a message");
    }

    #[test]
    fn switched_off_or_events_off_sends_nothing() {
        let mut r = rig();
        let study = state(&r, Phase::Study, true, 1);
        step(&mut r, &study, T0);
        set_message(&mut r.conn, MessageSettingKey::EventsEnabled, SettingValue::Bool(false), 1).expect("events off");
        attempt(&r, T0 + 1);
        assert!(step(&mut r, &study, T0 + 2).is_none());
        set_message(&mut r.conn, MessageSettingKey::EventsEnabled, SettingValue::Bool(true), 2).expect("events on");
        set_message(&mut r.conn, MessageSettingKey::Enabled, SettingValue::Bool(false), 3).expect("off");
        attempt(&r, T0 + 3);
        assert!(step(&mut r, &study, T0 + 4).is_none());
        assert_eq!(logged(&r), 0);
    }

    #[test]
    fn the_tone_setting_changes_the_phrase() {
        let mut r = rig();
        set_message(&mut r.conn, MessageSettingKey::Tone, SettingValue::Text("harsh".into()), 1).expect("tone");
        let study = state(&r, Phase::Study, true, 1);
        step(&mut r, &study, T0);
        attempt(&r, T0 + 1);
        let message = step(&mut r, &study, T0 + 2).expect("message");
        assert_eq!(message.tone, Tone::Harsh);
        let tone: String = r.conn.query_row("SELECT tone FROM message_log", [], |x| x.get(0)).expect("tone");
        assert_eq!(tone, "harsh");
    }

    #[test]
    fn a_wiki_page_makes_a_wiki_message_but_the_notification_never_names_it() {
        let mut r = rig();
        let root = r.dir.path().join("wiki");
        let subject_id = wiki_repo::insert_subject(&r.conn, "math", "수학", 1).expect("subject");
        std::fs::create_dir_all(root.join("math").join("pages")).expect("dirs");
        std::fs::write(root.join("math").join("pages").join("고유값분해.md"), "가".repeat(900)).expect("page");
        wiki_repo::upsert_page(&r.conn, subject_id, "고유값분해", "pages/고유값분해.md", "h", 1).expect("row");
        set_message(&mut r.conn, MessageSettingKey::RandomEnabled, SettingValue::Bool(true), 2).expect("on");
        let study = state(&r, Phase::Study, true, 1);
        step(&mut r, &study, T0);
        let message = step(&mut r, &study, T0 + 31 * MIN).expect("message");
        assert_eq!(message.source, crate::messages::Source::Wiki);
        assert!(message.text.contains("고유값분해"));
        let sent = r.notifier.sent();
        assert_eq!(sent.len(), 1);
        for (title, body) in &sent {
            for secret in ["고유값분해", "수학"] {
                assert!(!title.contains(secret) && !body.contains(secret));
            }
        }
        let source: String = r.conn.query_row("SELECT source FROM message_log", [], |x| x.get(0)).expect("source");
        assert_eq!(source, "wiki");
    }

    #[test]
    fn without_a_wiki_the_general_template_is_used() {
        let mut r = rig();
        set_message(&mut r.conn, MessageSettingKey::RandomEnabled, SettingValue::Bool(true), 2).expect("on");
        let study = state(&r, Phase::Study, true, 1);
        step(&mut r, &study, T0);
        let message = step(&mut r, &study, T0 + 31 * MIN).expect("message");
        assert_eq!(message.source, crate::messages::Source::Template);
        assert_eq!(message.text, message.notice);
    }

    #[test]
    fn the_notification_can_be_switched_off_while_the_toast_still_comes() {
        let mut r = rig();
        set_message(&mut r.conn, MessageSettingKey::NotifyEnabled, SettingValue::Bool(false), 1).expect("off");
        let study = state(&r, Phase::Study, true, 1);
        step(&mut r, &study, T0);
        attempt(&r, T0 + 1);
        assert!(step(&mut r, &study, T0 + 2).is_some());
        assert!(r.notifier.sent().is_empty());
    }

    #[test]
    fn the_enums_match_the_schema_check_lists() {
        let conn = open_in_memory().expect("db");
        let sql: String = conn.query_row("SELECT sql FROM sqlite_master WHERE name = 'message_log'", [], |r| r.get(0)).expect("schema");
        let list = |column: &str| -> Vec<String> {
            let from = sql.find(&format!("{column} IN (")).expect("list") + column.len() + 5;
            let body = &sql[from..from + sql[from..].find(')').expect("end")];
            let mut v: Vec<String> = body.split(',').map(|s| s.trim().trim_matches('\'').to_string()).collect();
            v.sort();
            v
        };
        let sorted = |mut v: Vec<String>| {
            v.sort();
            v
        };
        assert_eq!(list("kind"), sorted(Kind::ALL.iter().map(|k| k.as_str().to_string()).collect()));
        assert_eq!(list("tone"), sorted(Tone::ALL.iter().map(|t| t.as_str().to_string()).collect()));
        assert_eq!(list("source"), sorted(crate::messages::Source::ALL.iter().map(|s| s.as_str().to_string()).collect()));
    }
}
