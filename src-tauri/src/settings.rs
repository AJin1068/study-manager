//! General settings with lock-during-study enforcement and append-only history.
//!
//! Every change goes through one transaction: the lock check, the optional OS side effect,
//! the settings row, and the history row commit together. A rejected change still commits
//! its `rejected_locked` history row; only the settings value is left untouched.

use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::platform::AutostartBackend;
use crate::store;
use crate::study;

/// Boolean settings in the "general" category. The API name and the stored key are the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingKey {
    Autostart,
    ForceQuitAllowed,
}

impl SettingKey {
    pub fn name(self) -> &'static str {
        match self {
            SettingKey::Autostart => "autostart",
            SettingKey::ForceQuitAllowed => "forceQuitAllowed",
        }
    }

    /// Autostart is on by default; forced quit is off by default (plan: 강제 종료 허용 기본 불가).
    pub fn default_value(self) -> bool {
        match self {
            SettingKey::Autostart => true,
            SettingKey::ForceQuitAllowed => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    User,
    Default,
    Reconcile,
    TestToggle,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::User => "user",
            Reason::Default => "default",
            Reason::Reconcile => "reconcile",
            Reason::TestToggle => "test_toggle",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    RejectedLocked,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Applied => "applied",
            Outcome::RejectedLocked => "rejected_locked",
        }
    }
}

/// Result of a user-facing change attempt. The TS mirror lives in `src/api/commands.ts`
/// and is checked against `src/api/fixtures/update-outcome.json` by both test suites.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum UpdateOutcome {
    Applied { key: String },
    RejectedLocked { key: String },
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i64,
    pub at_ms: i64,
    pub key: String,
    pub before: Option<String>,
    pub after: String,
    pub reason: String,
    pub outcome: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeneralSettings {
    pub autostart: bool,
    pub force_quit_allowed: bool,
}

pub fn parse_bool(raw: &str) -> AppResult<bool> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(AppError::Invalid(format!("stored boolean setting is malformed: {other}"))),
    }
}

fn read_bool(conn: &Connection, key: SettingKey) -> AppResult<bool> {
    match store::get_setting(conn, key.name())? {
        Some(raw) => parse_bool(&raw),
        None => Ok(key.default_value()),
    }
}

pub fn general_settings(conn: &Connection) -> AppResult<GeneralSettings> {
    Ok(GeneralSettings {
        autostart: read_bool(conn, SettingKey::Autostart)?,
        force_quit_allowed: read_bool(conn, SettingKey::ForceQuitAllowed)?,
    })
}

pub fn read_history(conn: &Connection, limit: i64) -> AppResult<Vec<HistoryEntry>> {
    store::list_history(conn, limit)
}

/// Applies a user change. Locked during study: the attempt is recorded and nothing else changes.
pub fn update_setting(
    conn: &mut Connection,
    autostart: &dyn AutostartBackend,
    key: SettingKey,
    value: bool,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    let name = key.name();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let before = store::get_setting(&tx, name)?;
    let after = value.to_string();

    if study::is_studying(&tx)? {
        store::append_history(
            &tx,
            at_ms,
            name,
            before.as_deref(),
            &after,
            Reason::User,
            Outcome::RejectedLocked,
        )?;
        tx.commit()?;
        return Ok(UpdateOutcome::RejectedLocked { key: name.to_string() });
    }

    // OS side effect first: if it fails, the transaction is dropped and the DB stays unchanged.
    if key == SettingKey::Autostart {
        autostart.set_enabled(value)?;
    }
    store::put_setting(&tx, name, "general", &after, at_ms)?;
    store::append_history(
        &tx,
        at_ms,
        name,
        before.as_deref(),
        &after,
        Reason::User,
        Outcome::Applied,
    )?;
    tx.commit()?;
    Ok(UpdateOutcome::Applied { key: name.to_string() })
}

/// Startup reconcile for autostart. First run inserts the default (reason=default).
/// If the OS entry disagrees with the DB, the DB wins and the change is logged (reason=reconcile).
pub fn bootstrap_autostart(conn: &mut Connection, autostart: &dyn AutostartBackend, at_ms: i64) -> AppResult<()> {
    let name = SettingKey::Autostart.name();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match store::get_setting(&tx, name)? {
        None => {
            let value = SettingKey::Autostart.default_value();
            autostart.set_enabled(value)?;
            let after = value.to_string();
            store::put_setting(&tx, name, "general", &after, at_ms)?;
            store::append_history(&tx, at_ms, name, None, &after, Reason::Default, Outcome::Applied)?;
        }
        Some(raw) => {
            let db_value = parse_bool(&raw)?;
            let os_value = autostart.is_enabled()?;
            if os_value != db_value {
                autostart.set_enabled(db_value)?;
                store::append_history(
                    &tx,
                    at_ms,
                    name,
                    Some(&os_value.to_string()),
                    &db_value.to_string(),
                    Reason::Reconcile,
                    Outcome::Applied,
                )?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::db::open_in_memory;
    use crate::platform::PlatformError;
    use crate::study::set_study_state_test;

    pub(crate) struct FakeAutostart {
        pub enabled: Mutex<bool>,
        pub fail: bool,
    }

    impl FakeAutostart {
        pub fn new(enabled: bool) -> Self {
            Self { enabled: Mutex::new(enabled), fail: false }
        }
    }

    impl AutostartBackend for FakeAutostart {
        fn is_enabled(&self) -> Result<bool, PlatformError> {
            Ok(*self.enabled.lock().expect("lock"))
        }
        fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError> {
            if self.fail {
                return Err(PlatformError::Autostart("injected failure".into()));
            }
            *self.enabled.lock().expect("lock") = enabled;
            Ok(())
        }
    }

    fn history_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM setting_history", [], |r| r.get(0)).expect("count")
    }

    #[test]
    fn first_run_enables_autostart_by_default_and_logs_default_reason() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(false);
        bootstrap_autostart(&mut conn, &os, 10).expect("bootstrap");
        assert!(general_settings(&conn).expect("read").autostart);
        assert!(*os.enabled.lock().expect("lock"));
        let history = read_history(&conn, 10).expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].reason, "default");
        assert_eq!(history[0].outcome, "applied");
    }

    #[test]
    fn os_mismatch_is_reconciled_to_db_with_reconcile_reason() {
        let mut conn = open_in_memory().expect("db");
        bootstrap_autostart(&mut conn, &FakeAutostart::new(true), 10).expect("first");
        let os = FakeAutostart::new(false);
        bootstrap_autostart(&mut conn, &os, 20).expect("second");
        assert!(*os.enabled.lock().expect("lock"));
        let latest = &read_history(&conn, 1).expect("history")[0];
        assert_eq!(latest.reason, "reconcile");
        assert_eq!(latest.before.as_deref(), Some("false"));
    }

    #[test]
    fn applied_change_writes_setting_and_history_together() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(true);
        bootstrap_autostart(&mut conn, &os, 10).expect("bootstrap");
        let outcome = update_setting(&mut conn, &os, SettingKey::Autostart, false, 20).expect("update");
        assert_eq!(outcome, UpdateOutcome::Applied { key: "autostart".into() });
        assert!(!general_settings(&conn).expect("read").autostart);
        assert!(!*os.enabled.lock().expect("lock"));
        assert_eq!(read_history(&conn, 1).expect("h")[0].outcome, "applied");
    }

    #[test]
    fn locked_study_rejects_change_and_records_attempt() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(true);
        bootstrap_autostart(&mut conn, &os, 10).expect("bootstrap");
        set_study_state_test(&mut conn, true, 15).expect("toggle on");
        let before_count = history_count(&conn);

        let outcome = update_setting(&mut conn, &os, SettingKey::Autostart, false, 20).expect("update");
        assert_eq!(outcome, UpdateOutcome::RejectedLocked { key: "autostart".into() });
        // Value and OS state unchanged; exactly one rejected_locked row added.
        assert!(general_settings(&conn).expect("read").autostart);
        assert!(*os.enabled.lock().expect("lock"));
        assert_eq!(history_count(&conn), before_count + 1);
        let latest = &read_history(&conn, 1).expect("h")[0];
        assert_eq!(latest.outcome, "rejected_locked");
    }

    #[test]
    fn locked_force_quit_toggle_is_rejected_and_recorded() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(true);
        set_study_state_test(&mut conn, true, 5).expect("toggle on");
        let outcome = update_setting(&mut conn, &os, SettingKey::ForceQuitAllowed, true, 6).expect("update");
        assert_eq!(outcome, UpdateOutcome::RejectedLocked { key: "forceQuitAllowed".into() });
        assert!(!general_settings(&conn).expect("read").force_quit_allowed);
        assert_eq!(read_history(&conn, 1).expect("h")[0].outcome, "rejected_locked");
    }

    #[test]
    fn update_setting_refuses_when_lock_is_unknown() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(true);
        bootstrap_autostart(&mut conn, &os, 10).expect("bootstrap");
        conn.execute("DELETE FROM study_state", []).expect("delete");
        let before = read_history(&conn, 100).expect("h").len();
        assert!(update_setting(&mut conn, &os, SettingKey::Autostart, false, 20).is_err());
        assert!(general_settings(&conn).expect("read").autostart, "value must not change");
        assert!(*os.enabled.lock().expect("lock"), "OS must not change");
        assert_eq!(read_history(&conn, 100).expect("h").len(), before);
    }

    #[test]
    fn force_quit_defaults_to_disallowed() {
        let conn = open_in_memory().expect("db");
        assert!(!general_settings(&conn).expect("read").force_quit_allowed);
    }

    #[test]
    fn os_failure_leaves_db_unchanged() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(true);
        bootstrap_autostart(&mut conn, &os, 10).expect("bootstrap");
        let failing = FakeAutostart { enabled: Mutex::new(true), fail: true };
        assert!(update_setting(&mut conn, &failing, SettingKey::Autostart, false, 20).is_err());
        assert!(general_settings(&conn).expect("read").autostart);
        assert_eq!(read_history(&conn, 1).expect("h")[0].outcome, "applied");
    }

    /// Contract with the TS side: serialized shapes must equal the shared fixture.
    #[test]
    fn update_outcome_serialization_matches_shared_fixture() {
        let fixture_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/update-outcome.json");
        let raw = std::fs::read_to_string(fixture_path).expect("fixture");
        let expected: serde_json::Value = serde_json::from_str(&raw).expect("json");
        let actual = serde_json::to_value(vec![
            UpdateOutcome::Applied { key: "autostart".into() },
            UpdateOutcome::RejectedLocked { key: "forceQuitAllowed".into() },
        ])
        .expect("serialize");
        assert_eq!(actual, expected);
    }
}
