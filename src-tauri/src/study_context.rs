//! "What did I study" context (PLAN 5, opt-in). While a study block runs and the user has turned
//! this on, the sampler keeps a short label of what the front study app shows: the app's name, or
//! (with the extra switch) a shortened form of the window caption such as a document name.
//!
//! Privacy rules, all enforced here:
//! * Off by default. Off means nothing is read and nothing is stored; switching it off deletes
//!   what was stored.
//! * The raw window caption lives in a local variable only long enough to be shortened. It is never
//!   stored, logged, or sent; the stored `context_label` is at most 80 sanitized characters.
//! * Labels are kept 30 days and can be deleted at any time.
//! * Labels reach the LLM only with a separate consent (`context.llm_consent`), and then only
//!   inside the retrospective merge prompt.
//! * Wiki pages are linked to a run by local keyword matching only.

use std::sync::Mutex;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::commands::lock_db;
use crate::error::{AppError, AppResult};
use crate::platform::{AppIdentity, FocusProbe, TitleAccess};
use crate::settings::{self, SettingSpec, SettingValue, UpdateOutcome, ValueKind};
use crate::store::context::{self as repo, ContextRow};

const CATEGORY: &str = "context";
pub const RETENTION_DAYS: i64 = 30;
const DAY_MS: i64 = 86_400_000;
pub const MAX_LABEL_CHARS: usize = 80;
/// The sampler reads a caption at most this often.
pub const SAMPLE_EVERY_MS: i64 = 30_000;

pub const ENABLED: SettingSpec =
    SettingSpec { name: "context.enabled", category: CATEGORY, kind: ValueKind::Bool, default: "false", tighten_to: Some("false") };
pub const WINDOW_TITLES: SettingSpec =
    SettingSpec { name: "context.window_titles", category: CATEGORY, kind: ValueKind::Bool, default: "false", tighten_to: Some("false") };
pub const LLM_CONSENT: SettingSpec =
    SettingSpec { name: "context.llm_consent", category: CATEGORY, kind: ValueKind::Bool, default: "false", tighten_to: Some("false") };

/// API name = camelCase of the variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextSettingKey {
    Enabled,
    WindowTitles,
    LlmConsent,
}

impl ContextSettingKey {
    fn spec(self) -> SettingSpec {
        match self {
            ContextSettingKey::Enabled => ENABLED,
            ContextSettingKey::WindowTitles => WINDOW_TITLES,
            ContextSettingKey::LlmConsent => LLM_CONSENT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextConfig {
    pub enabled: bool,
    pub window_titles: bool,
    pub llm_consent: bool,
}

fn read_bool(conn: &Connection, spec: &SettingSpec) -> AppResult<bool> {
    match settings::read_value(conn, spec)? {
        SettingValue::Bool(v) => Ok(v),
        other => Err(AppError::Internal(format!("{} has the wrong type: {other:?}", spec.name))),
    }
}

impl ContextConfig {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        Ok(Self { enabled: read_bool(conn, &ENABLED)?, window_titles: read_bool(conn, &WINDOW_TITLES)?, llm_consent: read_bool(conn, &LLM_CONSENT)? })
    }
}

/// Changes one switch. Turning the feature off also deletes what it stored. All three may be
/// switched off during a planned run (they only narrow collection); switching on is locked.
pub fn update_setting(conn: &mut Connection, key: ContextSettingKey, value: SettingValue, at_ms: i64) -> AppResult<UpdateOutcome> {
    let outcome = settings::apply_setting(conn, &key.spec(), &value, at_ms, || Ok(()))?;
    if key == ContextSettingKey::Enabled && matches!(outcome, UpdateOutcome::Applied { .. }) && value == SettingValue::Bool(false) {
        repo::delete_all(conn)?;
    }
    Ok(outcome)
}

// ---- labels ----

/// A short, safe label from a window caption. Captions that look like addresses are dropped; the
/// app-name suffix many apps append is cut; control characters and long digit runs (ids, numbers)
/// are removed; the rest is shortened to 80 characters.
pub fn label_from_caption(app_name: &str, caption: &str) -> Option<String> {
    let flat: String = caption.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut text = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.contains("://") || text.to_ascii_lowercase().contains("www.") {
        return None;
    }
    let app = app_name.trim();
    if !app.is_empty() {
        for separator in [" — ", " – ", " - ", " | "] {
            let suffix = format!("{separator}{app}");
            if let Some(stripped) = text.strip_suffix(&suffix) {
                text = stripped.to_string();
                break;
            }
        }
    }
    let cleaned = drop_digit_runs(&text);
    let label: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_LABEL_CHARS).collect();
    let label = label.trim().to_string();
    (!label.is_empty()).then_some(label)
}

/// Removes runs of 6+ digits (account numbers, ids) from a caption.
fn drop_digit_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() {
            run.push(c);
            continue;
        }
        if run.len() < 6 {
            out.push_str(&run);
        }
        run.clear();
        out.push(c);
    }
    if run.len() < 6 {
        out.push_str(&run);
    }
    out
}

fn app_label(app: &AppIdentity) -> Option<String> {
    label_from_caption("", &app.display_name)
}

// ---- recording ----

/// One sampling step, called by the focus sampler for the front app when it is a study app.
/// Does nothing unless the user switched the feature on. The caption is read without the
/// database lock held and dropped as soon as the label exists.
pub fn capture(db: &Mutex<Connection>, probe: &dyn FocusProbe, app: &AppIdentity, segment_id: i64, now_ms: i64) -> AppResult<()> {
    let config = ContextConfig::load(&*lock_db(db)?)?;
    if !config.enabled {
        return Ok(());
    }
    let label = if config.window_titles {
        let caption = probe.front_window_title(app.pid);
        caption.and_then(|c| label_from_caption(&app.display_name, &c)).or_else(|| app_label(app))
    } else {
        app_label(app)
    };
    let Some(label) = label else { return Ok(()) };
    let conn = lock_db(db)?;
    if repo::last_label(&conn, segment_id)?.is_some_and(|(id, last)| id == app.id && last == label) {
        return Ok(());
    }
    repo::insert(&conn, &ContextRow { segment_id, at_ms: now_ms, app_id: app.id.clone(), context_label: label })?;
    repo::prune_before(&conn, now_ms - RETENTION_DAYS * DAY_MS)?;
    tracing::info!(event = "study_context_recorded");
    Ok(())
}

// ---- what the UI and the retrospective read ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextStatus {
    #[serde(flatten)]
    pub config: ContextConfig,
    pub retention_days: i64,
    pub row_count: i64,
    pub title_access: TitleAccess,
}

/// Settings and the stored count; rows past the retention window are removed first.
pub fn status(conn: &Connection, probe: &dyn FocusProbe, now_ms: i64) -> AppResult<ContextStatus> {
    repo::prune_before(conn, now_ms - RETENTION_DAYS * DAY_MS)?;
    Ok(ContextStatus {
        config: ContextConfig::load(conn)?,
        retention_days: RETENTION_DAYS,
        row_count: repo::count(conn)?,
        title_access: probe.window_title_access(),
    })
}

/// The "delete everything" button. Returns how many rows went.
pub fn delete_all(conn: &Connection) -> AppResult<usize> {
    repo::delete_all(conn)
}

/// Labels of a run for local keyword matching. Empty when the feature is off.
pub fn local_labels(conn: &Connection, run_id: i64) -> AppResult<Vec<String>> {
    if !ContextConfig::load(conn)?.enabled {
        return Ok(Vec::new());
    }
    repo::labels_for_run(conn, run_id, 10)
}

/// Labels that may go into an LLM prompt: only with the separate consent (and the feature on).
pub fn llm_labels(conn: &Connection, run_id: i64) -> AppResult<Vec<String>> {
    let config = ContextConfig::load(conn)?;
    if !(config.enabled && config.llm_consent) {
        return Ok(Vec::new());
    }
    repo::labels_for_run(conn, run_id, 5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::platform::PlatformError;
    use crate::testutil::{capture_logs, set_lock_for_test};

    const CANARY: &str = "CANARY-CAPTION-9f3";

    struct Probe {
        caption: Option<String>,
    }

    impl FocusProbe for Probe {
        fn frontmost_app(&self) -> Result<Option<AppIdentity>, PlatformError> {
            Ok(None)
        }
        fn idle_seconds(&self) -> Result<u64, PlatformError> {
            Ok(0)
        }
        fn window_title_access(&self) -> TitleAccess {
            TitleAccess::Granted
        }
        fn front_window_title(&self, _pid: u32) -> Option<String> {
            self.caption.clone()
        }
    }

    fn app() -> AppIdentity {
        AppIdentity { id: "com.example.preview".into(), display_name: "Preview".into(), pid: 7, is_protected: false }
    }

    fn db_with(settings_on: &[ContextSettingKey]) -> Mutex<Connection> {
        let mut conn = open_in_memory().expect("db");
        for key in settings_on {
            update_setting(&mut conn, *key, SettingValue::Bool(true), 1).expect("on");
        }
        Mutex::new(conn)
    }

    fn rows(db: &Mutex<Connection>) -> Vec<(i64, String, String)> {
        let conn = db.lock().expect("db");
        let mut stmt = conn.prepare("SELECT segment_id, app_id, context_label FROM study_context ORDER BY id").expect("prepare");
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).expect("rows").collect::<Result<_, _>>().expect("collect")
    }

    #[test]
    fn off_means_no_row_and_no_caption_read() {
        struct Panicky;
        impl FocusProbe for Panicky {
            fn frontmost_app(&self) -> Result<Option<AppIdentity>, PlatformError> {
                Ok(None)
            }
            fn idle_seconds(&self) -> Result<u64, PlatformError> {
                Ok(0)
            }
            fn window_title_access(&self) -> TitleAccess {
                TitleAccess::Granted
            }
            fn front_window_title(&self, _pid: u32) -> Option<String> {
                panic!("the caption must not be read while the feature is off")
            }
        }
        let db = db_with(&[]);
        capture(&db, &Panicky, &app(), 1, 1_000).expect("capture");
        assert!(rows(&db).is_empty());
        // The title switch alone does nothing either.
        let db = db_with(&[ContextSettingKey::WindowTitles]);
        capture(&db, &Panicky, &app(), 1, 1_000).expect("capture");
        assert!(rows(&db).is_empty());
    }

    #[test]
    fn app_name_only_unless_titles_are_switched_on() {
        let db = db_with(&[ContextSettingKey::Enabled]);
        let probe = Probe { caption: Some(format!("{CANARY} — Preview")) };
        capture(&db, &probe, &app(), 1, 1_000).expect("capture");
        assert_eq!(rows(&db), [(1, "com.example.preview".to_string(), "Preview".to_string())]);
        assert!(!format!("{:?}", rows(&db)).contains(CANARY));
    }

    #[test]
    fn a_caption_becomes_a_short_label_without_the_app_suffix_and_repeats_are_not_stored() {
        let db = db_with(&[ContextSettingKey::Enabled, ContextSettingKey::WindowTitles]);
        let probe = Probe { caption: Some("미분과 적분 정리.pdf — Preview".into()) };
        capture(&db, &probe, &app(), 1, 1_000).expect("first");
        capture(&db, &probe, &app(), 1, 2_000).expect("same label");
        assert_eq!(rows(&db).len(), 1);
        assert_eq!(rows(&db)[0].2, "미분과 적분 정리.pdf");
        let other = Probe { caption: Some("선형대수.pdf — Preview".into()) };
        capture(&db, &other, &app(), 1, 3_000).expect("new label");
        capture(&db, &other, &app(), 2, 4_000).expect("new segment");
        assert_eq!(rows(&db).len(), 3);
    }

    #[test]
    fn captions_that_look_like_addresses_fall_back_to_the_app_name() {
        let db = db_with(&[ContextSettingKey::Enabled, ContextSettingKey::WindowTitles]);
        capture(&db, &Probe { caption: Some("https://private.example/path?token=abc — Chrome".into()) }, &app(), 1, 1).expect("capture");
        assert_eq!(rows(&db)[0].2, "Preview");
        assert_eq!(label_from_caption("", "계좌 1234567890 확인"), Some("계좌 확인".to_string()));
        assert_eq!(label_from_caption("", &"가".repeat(200)).map(|l| l.chars().count()), Some(MAX_LABEL_CHARS));
        assert_eq!(label_from_caption("", " \n\t "), None);
    }

    #[test]
    fn rows_expire_after_thirty_days_and_delete_all_leaves_zero() {
        let db = db_with(&[ContextSettingKey::Enabled]);
        capture(&db, &Probe { caption: None }, &app(), 1, 1_000).expect("old");
        let later = 1_000 + RETENTION_DAYS * DAY_MS + 1;
        let other = AppIdentity { id: "other".into(), display_name: "Other".into(), pid: 8, is_protected: false };
        capture(&db, &Probe { caption: None }, &other, 2, later).expect("new prunes the old one");
        assert_eq!(rows(&db).iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["other"]);
        {
            let conn = db.lock().expect("db");
            let kept = status(&conn, &Probe { caption: None }, later + RETENTION_DAYS * DAY_MS + 1).expect("status");
            assert_eq!(kept.row_count, 0, "status prunes too");
        }
        capture(&db, &Probe { caption: None }, &other, 3, later + 1).expect("again");
        assert_eq!(delete_all(&db.lock().expect("db")).expect("delete"), 1);
        assert!(rows(&db).is_empty());
    }

    #[test]
    fn switching_off_deletes_what_was_stored_and_is_allowed_during_a_run() {
        let db = db_with(&[ContextSettingKey::Enabled, ContextSettingKey::LlmConsent]);
        capture(&db, &Probe { caption: None }, &app(), 1, 1_000).expect("capture");
        let mut conn = db.lock().expect("db");
        set_lock_for_test(&conn, true);
        let widen = update_setting(&mut conn, ContextSettingKey::WindowTitles, SettingValue::Bool(true), 5).expect("outcome");
        assert!(matches!(widen, UpdateOutcome::RejectedLocked { .. }));
        let off = update_setting(&mut conn, ContextSettingKey::Enabled, SettingValue::Bool(false), 6).expect("off");
        assert!(matches!(off, UpdateOutcome::Applied { .. }));
        assert_eq!(repo::count(&conn).expect("count"), 0);
        assert!(!ContextConfig::load(&conn).expect("config").enabled);
    }

    #[test]
    fn labels_for_the_llm_need_the_separate_consent_but_local_matching_does_not() {
        let db = db_with(&[ContextSettingKey::Enabled]);
        let conn = db.lock().expect("db");
        let (run_id, segment) = crate::testutil::insert_run_for_test(&conn, "수학");
        repo::insert(&conn, &ContextRow { segment_id: segment, at_ms: 1, app_id: "a".into(), context_label: "미분 노트.md".into() }).expect("row");
        assert_eq!(local_labels(&conn, run_id).expect("local"), ["미분 노트.md"]);
        assert!(llm_labels(&conn, run_id).expect("llm").is_empty(), "no consent, nothing for the model");
        drop(conn);
        update_setting(&mut db.lock().expect("db"), ContextSettingKey::LlmConsent, SettingValue::Bool(true), 2).expect("consent");
        assert_eq!(llm_labels(&db.lock().expect("db"), run_id).expect("llm"), ["미분 노트.md"]);
    }

    #[test]
    fn the_caption_never_reaches_the_log() {
        let db = db_with(&[ContextSettingKey::Enabled, ContextSettingKey::WindowTitles]);
        let probe = Probe { caption: Some(format!("{CANARY}.pdf — Preview")) };
        let mut seen = String::new();
        for _ in 0..10 {
            seen = capture_logs(|| {
                capture(&db, &probe, &app(), 1, 1).ok();
                db.lock().expect("db").execute("DELETE FROM study_context", []).ok();
            });
            if seen.contains("study_context_recorded") {
                break;
            }
        }
        assert!(seen.contains("study_context_recorded"), "{seen}");
        assert!(!seen.contains(CANARY), "{seen}");
    }
}
