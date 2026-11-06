//! General settings with lock-during-study enforcement and append-only history.
//!
//! Every change goes through one transaction: the lock check, the optional OS side effect,
//! the settings row, and the history row commit together. A rejected change still commits
//! its `rejected_locked` history row; only the settings value is left untouched.

use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::platform::{AutostartBackend, PlatformError};
use crate::store;
use crate::study;

/// A setting value as it crosses the Tauri boundary. Integers and text exist so later
/// stories (grace time, enforcement mode) share one validated path; the stored form is the
/// plain string, which is also what `setting_history` keeps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Text(String),
}

impl SettingValue {
    pub fn to_stored(&self) -> String {
        match self {
            SettingValue::Bool(v) => v.to_string(),
            SettingValue::Int(v) => v.to_string(),
            SettingValue::Text(v) => v.clone(),
        }
    }
}

/// Per-key validation rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Bool,
    /// Inclusive range.
    Int { min: i64, max: i64 },
    /// Closed set of accepted strings.
    Text { allowed: &'static [&'static str] },
    /// Open text of a known shape, empty meaning "unset".
    Free { max_len: usize, shape: FreeShape },
}

/// What an open-text setting must look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreeShape {
    /// `https://host` (or `http://` to a loopback host), no credentials, query, or fragment.
    Endpoint,
    /// One token without whitespace or control characters (keys, ids).
    Token,
    /// An absolute folder path without `..` components or control characters.
    Path,
}

impl FreeShape {
    fn accepts(self, text: &str) -> bool {
        if text.is_empty() {
            return true;
        }
        match self {
            FreeShape::Token => !text.chars().any(|c| c.is_whitespace() || c.is_control()),
            FreeShape::Endpoint => crate::remote::http::is_valid_endpoint(text),
            FreeShape::Path => {
                let path = std::path::Path::new(text);
                path.is_absolute()
                    && !text.chars().any(char::is_control)
                    && !path.components().any(|c| matches!(c, std::path::Component::ParentDir))
            }
        }
    }
}

impl ValueKind {
    /// Rejects a value of the wrong type or outside the allowed range/set.
    pub fn validate(self, value: &SettingValue) -> AppResult<()> {
        match (self, value) {
            (ValueKind::Bool, SettingValue::Bool(_)) => Ok(()),
            (ValueKind::Int { min, max }, SettingValue::Int(v)) if (min..=max).contains(v) => Ok(()),
            (ValueKind::Int { min, max }, SettingValue::Int(v)) => {
                Err(AppError::Invalid(format!("{v} is outside {min}..={max}")))
            }
            (ValueKind::Text { allowed }, SettingValue::Text(v)) if allowed.contains(&v.as_str()) => Ok(()),
            (ValueKind::Text { .. }, SettingValue::Text(v)) => {
                Err(AppError::Invalid(format!("{v:?} is not an allowed value")))
            }
            (ValueKind::Free { max_len, shape }, SettingValue::Text(v)) if v.len() <= max_len && shape.accepts(v) => {
                Ok(())
            }
            // The rejected text may be a key, so it is never echoed into the error.
            (ValueKind::Free { .. }, SettingValue::Text(_)) => {
                Err(AppError::Invalid("the text is too long or has the wrong shape".into()))
            }
            _ => Err(AppError::Invalid("setting value has the wrong type".into())),
        }
    }

    /// Parses a stored string back into a value of this kind.
    pub fn parse_stored(self, raw: &str) -> AppResult<SettingValue> {
        let value = match self {
            ValueKind::Bool => SettingValue::Bool(parse_bool(raw)?),
            ValueKind::Int { .. } => SettingValue::Int(
                raw.parse::<i64>()
                    .map_err(|_| AppError::Invalid(format!("stored integer setting is malformed: {raw}")))?,
            ),
            ValueKind::Text { .. } | ValueKind::Free { .. } => SettingValue::Text(raw.to_string()),
        };
        self.validate(&value)?;
        Ok(value)
    }
}

/// Storage name, category, validation kind, and default of one setting.
#[derive(Clone, Copy, Debug)]
pub struct SettingSpec {
    pub name: &'static str,
    pub category: &'static str,
    pub kind: ValueKind,
    /// Default in stored form; parsed by `kind`, so a bad default fails the first read loudly.
    pub default: &'static str,
    /// A change to exactly this stored value narrows what leaves the device, so it stays allowed
    /// during a planned run (settings lock exception, recorded as `applied_privacy_tighten`).
    /// Only items that cannot weaken blocking may set it.
    pub tighten_to: Option<&'static str>,
}

/// Settings the UI may change. The API name and the stored key are the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingKey {
    Autostart,
    ForceQuitAllowed,
    RecheckPlanOnWake,
    /// Relaunch after a kill/crash (macOS LaunchAgent KeepAlive). Needs autostart to be on.
    RelaunchOnKill,
    GraceWaitSec,
    EmergencyPassDailyLimit,
    /// Calendar (.ics) import and export. Off by default; read by `plan_import`.
    CalendarIoEnabled,
}

impl SettingKey {
    pub fn name(self) -> &'static str {
        self.spec().name
    }

    /// Autostart is on by default; forced quit is off by default (plan: 강제 종료 허용 기본 불가).
    /// `recheckPlanOnWake` only controls whether the UI shows the re-check; reconcile itself
    /// always runs, because a switch that disabled it would bypass the study lock.
    pub fn spec(self) -> SettingSpec {
        match self {
            SettingKey::Autostart => SettingSpec {
                name: "autostart",
                category: "general",
                kind: ValueKind::Bool,
                default: "true",
                tighten_to: None,
            },
            SettingKey::ForceQuitAllowed => SettingSpec {
                name: "forceQuitAllowed",
                category: "general",
                kind: ValueKind::Bool,
                default: "false",
                tighten_to: None,
            },
            SettingKey::RecheckPlanOnWake => SettingSpec {
                name: "recheckPlanOnWake",
                category: "general",
                kind: ValueKind::Bool,
                default: "true",
                tighten_to: None,
            },
            // On by default (D-17): the plan lists "relaunch when force-quit" as a decision.
            SettingKey::RelaunchOnKill => SettingSpec {
                name: "relaunchOnKill",
                category: "general",
                kind: ValueKind::Bool,
                default: "true",
                tighten_to: None,
            },
            // Seconds to wait after the quit request before a forced kill is even considered.
            SettingKey::GraceWaitSec => SettingSpec {
                name: "graceWaitSec",
                category: "enforcement",
                kind: ValueKind::Int { min: 1, max: 60 },
                default: "10",
                tighten_to: None,
            },
            // Emergency passes the user may spend per local day (plan: 기본 1, 설정 1~2).
            SettingKey::EmergencyPassDailyLimit => SettingSpec {
                name: "emergencyPassDailyLimit",
                category: "release",
                kind: ValueKind::Int { min: 1, max: 2 },
                default: "1",
                tighten_to: None,
            },
            SettingKey::CalendarIoEnabled => SettingSpec {
                name: "calendarIoEnabled",
                category: "plan",
                kind: ValueKind::Bool,
                default: "false",
                tighten_to: None,
            },
        }
    }

    pub fn default_value(self) -> bool {
        self.spec().default == "true"
    }
}

/// `dryRun` (default) only logs what would be ended; `enforce` really ends apps. Switching to
/// `enforce` has its own command with a typed confirmation (`enforce::set_mode`), so it is not a
/// `SettingKey`.
pub const ENFORCEMENT_MODE: SettingSpec = SettingSpec {
    name: "enforcementMode",
    category: "enforcement",
    kind: ValueKind::Text { allowed: &["dryRun", "enforce"] },
    default: "dryRun",
    tighten_to: None,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    User,
    Default,
    Reconcile,
    /// A secret was stored. History keeps only the fact, never the value or a hash.
    SecretSet,
    SecretCleared,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::User => "user",
            Reason::Default => "default",
            Reason::Reconcile => "reconcile",
            Reason::SecretSet => "secret_set",
            Reason::SecretCleared => "secret_cleared",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    RejectedLocked,
    /// Allowed during a run because it only narrows what leaves the device (section 4.6).
    AppliedPrivacyTighten,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Applied => "applied",
            Outcome::RejectedLocked => "rejected_locked",
            Outcome::AppliedPrivacyTighten => "applied_privacy_tighten",
        }
    }
}

/// Result of a user-facing change attempt. The TS mirror lives in `src/api/settings.ts`
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
    pub recheck_plan_on_wake: bool,
    pub relaunch_on_kill: bool,
}

pub fn parse_bool(raw: &str) -> AppResult<bool> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(AppError::Invalid(format!("stored boolean setting is malformed: {other}"))),
    }
}

fn read_bool(conn: &Connection, key: SettingKey) -> AppResult<bool> {
    match read_value(conn, &key.spec())? {
        SettingValue::Bool(v) => Ok(v),
        other => Err(AppError::Internal(format!("{} is not a boolean setting: {other:?}", key.name()))),
    }
}

pub fn general_settings(conn: &Connection) -> AppResult<GeneralSettings> {
    Ok(GeneralSettings {
        autostart: read_bool(conn, SettingKey::Autostart)?,
        force_quit_allowed: read_bool(conn, SettingKey::ForceQuitAllowed)?,
        recheck_plan_on_wake: read_bool(conn, SettingKey::RecheckPlanOnWake)?,
        relaunch_on_kill: read_bool(conn, SettingKey::RelaunchOnKill)?,
    })
}

pub fn read_history(conn: &Connection, limit: i64) -> AppResult<Vec<HistoryEntry>> {
    store::list_history(conn, limit)
}

/// Reads a setting as a typed value, falling back to the spec default when unset.
pub fn read_value(conn: &Connection, spec: &SettingSpec) -> AppResult<SettingValue> {
    match store::get_setting(conn, spec.name)? {
        Some(raw) => spec.kind.parse_stored(&raw),
        None => spec.kind.parse_stored(spec.default),
    }
}

/// Applies a user change to a known key. Locked during study: the attempt is recorded and
/// nothing else changes.
pub fn update_setting(
    conn: &mut Connection,
    autostart: &dyn AutostartBackend,
    key: SettingKey,
    value: SettingValue,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    let spec = key.spec();
    apply_setting(conn, &spec, &value, at_ms, || {
        // OS side effect first: if it fails, the transaction is dropped and the DB stays unchanged.
        if let SettingValue::Bool(enabled) = value {
            match key {
                SettingKey::Autostart => autostart.set_enabled(enabled)?,
                SettingKey::RelaunchOnKill => autostart.set_relaunch(enabled)?,
                _ => {}
            }
        }
        Ok(())
    })
}

/// Validation, lock check, side effect, value row and history row in one Immediate transaction.
/// A value that fails validation is an error and records nothing (it never reached the setting).
pub(crate) fn apply_setting(
    conn: &mut Connection,
    spec: &SettingSpec,
    value: &SettingValue,
    at_ms: i64,
    side_effect: impl FnOnce() -> AppResult<()>,
) -> AppResult<UpdateOutcome> {
    spec.kind.validate(value)?;
    let name = spec.name;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let before = store::get_setting(&tx, name)?;
    let after = value.to_stored();

    let mut outcome = Outcome::Applied;
    if study::lock_held(&tx)? {
        if spec.tighten_to != Some(after.as_str()) {
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
        outcome = Outcome::AppliedPrivacyTighten;
    }

    side_effect()?;
    store::put_setting(&tx, name, spec.category, &after, at_ms)?;
    store::append_history(&tx, at_ms, name, before.as_deref(), &after, Reason::User, outcome)?;
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

/// Startup: applies the stored relaunch choice to the OS (first run stores the default, true).
/// Call before `bootstrap_autostart` so the login item is written with the right supervision.
/// A platform without relaunch supervision is logged and skipped; any other failure is an error.
pub fn bootstrap_relaunch(conn: &mut Connection, autostart: &dyn AutostartBackend, at_ms: i64) -> AppResult<()> {
    let key = SettingKey::RelaunchOnKill;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let enabled = match store::get_setting(&tx, key.name())? {
        Some(raw) => parse_bool(&raw)?,
        None => {
            let value = key.default_value();
            let after = value.to_string();
            store::put_setting(&tx, key.name(), key.spec().category, &after, at_ms)?;
            store::append_history(&tx, at_ms, key.name(), None, &after, Reason::Default, Outcome::Applied)?;
            value
        }
    };
    match autostart.set_relaunch(enabled) {
        Ok(()) => {}
        Err(PlatformError::Unsupported(what)) => tracing::warn!(what, "relaunch supervision unavailable"),
        Err(other) => return Err(other.into()),
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::db::open_in_memory;
    use crate::testutil::set_lock_for_test;

    pub(crate) struct FakeAutostart {
        pub enabled: Mutex<bool>,
        pub fail: bool,
        /// `None` = the backend reports Unsupported for relaunch supervision.
        pub relaunch: Mutex<Option<bool>>,
    }

    impl FakeAutostart {
        pub fn new(enabled: bool) -> Self {
            Self { enabled: Mutex::new(enabled), fail: false, relaunch: Mutex::new(Some(false)) }
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
        fn set_relaunch(&self, enabled: bool) -> Result<(), PlatformError> {
            let mut state = self.relaunch.lock().expect("lock");
            if state.is_none() {
                return Err(PlatformError::Unsupported("relaunch supervision"));
            }
            *state = Some(enabled);
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
        let outcome = update_setting(&mut conn, &os, SettingKey::Autostart, SettingValue::Bool(false), 20).expect("update");
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
        set_lock_for_test(&conn, true);
        let before_count = history_count(&conn);

        let outcome = update_setting(&mut conn, &os, SettingKey::Autostart, SettingValue::Bool(false), 20).expect("update");
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
        set_lock_for_test(&conn, true);
        let outcome = update_setting(&mut conn, &os, SettingKey::ForceQuitAllowed, SettingValue::Bool(true), 6).expect("update");
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
        assert!(update_setting(&mut conn, &os, SettingKey::Autostart, SettingValue::Bool(false), 20).is_err());
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
        let failing = FakeAutostart { enabled: Mutex::new(true), fail: true, relaunch: Mutex::new(Some(false)) };
        assert!(update_setting(&mut conn, &failing, SettingKey::Autostart, SettingValue::Bool(false), 20).is_err());
        assert!(general_settings(&conn).expect("read").autostart);
        assert_eq!(read_history(&conn, 1).expect("h")[0].outcome, "applied");
    }

    const GRACE: SettingSpec = SettingSpec {
        name: "graceWaitSec",
        category: "enforcement",
        kind: ValueKind::Int { min: 1, max: 60 },
        default: "10",
        tighten_to: None,
    };
    const MODE: SettingSpec = ENFORCEMENT_MODE;

    #[test]
    fn int_and_text_settings_round_trip_with_history() {
        let mut conn = open_in_memory().expect("db");
        assert_eq!(read_value(&conn, &GRACE).expect("default"), SettingValue::Int(10));
        apply_setting(&mut conn, &GRACE, &SettingValue::Int(30), 1, || Ok(())).expect("int");
        apply_setting(&mut conn, &MODE, &SettingValue::Text("enforce".into()), 2, || Ok(())).expect("text");
        assert_eq!(read_value(&conn, &GRACE).expect("read"), SettingValue::Int(30));
        assert_eq!(read_value(&conn, &MODE).expect("read"), SettingValue::Text("enforce".into()));
        let history = read_history(&conn, 2).expect("h");
        assert_eq!((history[0].key.as_str(), history[0].after.as_str()), ("enforcementMode", "enforce"));
        assert_eq!(history[1].before, None);
        assert_eq!(history[1].after, "30");
    }

    #[test]
    fn invalid_values_are_errors_that_change_and_record_nothing() {
        let mut conn = open_in_memory().expect("db");
        for (spec, value) in [
            (&GRACE, SettingValue::Int(0)),
            (&GRACE, SettingValue::Int(61)),
            (&GRACE, SettingValue::Bool(true)),
            (&GRACE, SettingValue::Text("10".into())),
            (&MODE, SettingValue::Text("loud".into())),
            (&MODE, SettingValue::Int(1)),
        ] {
            assert!(apply_setting(&mut conn, spec, &value, 1, || Ok(())).is_err(), "{value:?}");
        }
        assert_eq!(store::get_setting(&conn, "graceWaitSec").expect("get"), None);
        assert_eq!(history_count(&conn), 0);
    }

    #[test]
    fn locked_study_rejects_int_change_and_keeps_value() {
        let mut conn = open_in_memory().expect("db");
        apply_setting(&mut conn, &GRACE, &SettingValue::Int(30), 1, || Ok(())).expect("set");
        set_lock_for_test(&conn, true);
        let outcome = apply_setting(&mut conn, &GRACE, &SettingValue::Int(5), 3, || Ok(())).expect("attempt");
        assert_eq!(outcome, UpdateOutcome::RejectedLocked { key: "graceWaitSec".into() });
        assert_eq!(read_value(&conn, &GRACE).expect("read"), SettingValue::Int(30));
        let latest = &read_history(&conn, 1).expect("h")[0];
        assert_eq!((latest.outcome.as_str(), latest.after.as_str()), ("rejected_locked", "5"));
    }

    #[test]
    fn malformed_stored_values_error_instead_of_defaulting() {
        let conn = open_in_memory().expect("db");
        store::put_setting(&conn, "graceWaitSec", "enforcement", "abc", 1).expect("put");
        assert!(read_value(&conn, &GRACE).is_err());
        store::put_setting(&conn, "graceWaitSec", "enforcement", "999", 2).expect("put");
        assert!(read_value(&conn, &GRACE).is_err());
    }

    #[test]
    fn setting_value_json_shapes_are_bool_number_string() {
        assert_eq!(serde_json::from_str::<SettingValue>("true").expect("b"), SettingValue::Bool(true));
        assert_eq!(serde_json::from_str::<SettingValue>("12").expect("i"), SettingValue::Int(12));
        assert_eq!(serde_json::from_str::<SettingValue>("\"x\"").expect("t"), SettingValue::Text("x".into()));
        assert!(serde_json::from_str::<SettingValue>("1.5").is_err());
    }

    #[test]
    fn recheck_plan_on_wake_defaults_on_and_is_lock_protected() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(true);
        assert!(general_settings(&conn).expect("read").recheck_plan_on_wake);
        update_setting(&mut conn, &os, SettingKey::RecheckPlanOnWake, SettingValue::Bool(false), 1).expect("off");
        assert!(!general_settings(&conn).expect("read").recheck_plan_on_wake);
        set_lock_for_test(&conn, true);
        let outcome =
            update_setting(&mut conn, &os, SettingKey::RecheckPlanOnWake, SettingValue::Bool(true), 3).expect("try");
        assert_eq!(outcome, UpdateOutcome::RejectedLocked { key: "recheckPlanOnWake".into() });
        assert!(!general_settings(&conn).expect("read").recheck_plan_on_wake);
    }

    #[test]
    fn relaunch_defaults_on_and_bootstrap_applies_it_to_the_os() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(false);
        bootstrap_relaunch(&mut conn, &os, 10).expect("bootstrap");
        assert!(general_settings(&conn).expect("read").relaunch_on_kill);
        assert_eq!(*os.relaunch.lock().expect("lock"), Some(true));
        let history = read_history(&conn, 10).expect("h");
        assert_eq!((history[0].key.as_str(), history[0].reason.as_str()), ("relaunchOnKill", "default"));
        // A stored "off" wins over the default and is re-applied on the next launch.
        update_setting(&mut conn, &os, SettingKey::RelaunchOnKill, SettingValue::Bool(false), 20).expect("off");
        let fresh = FakeAutostart::new(false);
        bootstrap_relaunch(&mut conn, &fresh, 30).expect("again");
        assert_eq!(*fresh.relaunch.lock().expect("lock"), Some(false));
        assert_eq!(read_history(&conn, 10).expect("h").len(), 2, "no second default row");
    }

    #[test]
    fn relaunch_toggle_is_lock_protected_and_leaves_the_os_alone() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(true);
        bootstrap_relaunch(&mut conn, &os, 1).expect("bootstrap");
        set_lock_for_test(&conn, true);
        let outcome = update_setting(&mut conn, &os, SettingKey::RelaunchOnKill, SettingValue::Bool(false), 3).expect("try");
        assert_eq!(outcome, UpdateOutcome::RejectedLocked { key: "relaunchOnKill".into() });
        assert!(general_settings(&conn).expect("read").relaunch_on_kill);
        assert_eq!(*os.relaunch.lock().expect("lock"), Some(true));
        assert_eq!(read_history(&conn, 1).expect("h")[0].outcome, "rejected_locked");
    }

    #[test]
    fn unsupported_relaunch_is_skipped_at_startup_but_an_explicit_toggle_errors() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(true);
        *os.relaunch.lock().expect("lock") = None;
        bootstrap_relaunch(&mut conn, &os, 1).expect("startup tolerates Unsupported");
        assert!(update_setting(&mut conn, &os, SettingKey::RelaunchOnKill, SettingValue::Bool(false), 2).is_err());
        assert!(general_settings(&conn).expect("read").relaunch_on_kill, "failed toggle changes nothing");
    }

    #[test]
    fn autostart_reconcile_is_unchanged_by_relaunch_support() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(false);
        bootstrap_relaunch(&mut conn, &os, 1).expect("relaunch");
        bootstrap_autostart(&mut conn, &os, 2).expect("autostart");
        assert!(*os.enabled.lock().expect("lock"));
        os.set_enabled(false).expect("drift");
        bootstrap_autostart(&mut conn, &os, 3).expect("reconcile");
        assert!(*os.enabled.lock().expect("lock"));
        assert_eq!(read_history(&conn, 1).expect("h")[0].reason, "reconcile");
    }

    #[test]
    fn known_key_rejects_wrong_value_type() {
        let mut conn = open_in_memory().expect("db");
        let os = FakeAutostart::new(true);
        assert!(update_setting(&mut conn, &os, SettingKey::Autostart, SettingValue::Int(1), 1).is_err());
        assert_eq!(history_count(&conn), 0);
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

    #[test]
    fn general_settings_serialization_matches_shared_fixture() {
        let fixture_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/general-settings.json");
        let raw = std::fs::read_to_string(fixture_path).expect("fixture");
        let expected: serde_json::Value = serde_json::from_str(&raw).expect("json");
        let conn = open_in_memory().expect("db");
        let actual = serde_json::to_value(general_settings(&conn).expect("read")).expect("serialize");
        assert_eq!(actual, expected);
    }
}
