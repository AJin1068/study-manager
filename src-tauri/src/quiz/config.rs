//! `quiz.*` settings, stored through the same locked, history-recorded path as every other
//! setting. Changing them is locked during a planned run like any setting, so a limit cannot be
//! loosened while the gate is the thing being tested. Switching a pop-up off only removes
//! interruptions, so those stay allowed during a run (privacy/attention tightening).

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::settings::{self, SettingSpec, SettingValue, UpdateOutcome, ValueKind};

const CATEGORY: &str = "quiz";

/// Wrong early-release attempt, then wait this long (minutes).
pub const RELEASE_COOLDOWN_MIN: SettingSpec = SettingSpec {
    name: "quiz.release_cooldown_min",
    category: CATEGORY,
    kind: ValueKind::Int { min: 1, max: 60 },
    default: "3",
    tighten_to: None,
};
/// Early-release quiz attempts per local day.
pub const RELEASE_MAX_ATTEMPTS: SettingSpec = SettingSpec {
    name: "quiz.release_max_attempts_per_day",
    category: CATEGORY,
    kind: ValueKind::Int { min: 1, max: 20 },
    default: "4",
    tighten_to: None,
};
/// One review question when a blocked app is opened (never lifts the block).
pub const DISTRACTION_ENABLED: SettingSpec =
    SettingSpec { name: "quiz.distraction_enabled", category: CATEGORY, kind: ValueKind::Bool, default: "true", tighten_to: Some("false") };
/// A wiki card added to the random focus check-in (off by default).
pub const CHECKIN_ENABLED: SettingSpec =
    SettingSpec { name: "quiz.checkin_enabled", category: CATEGORY, kind: ValueKind::Bool, default: "false", tighten_to: Some("false") };
/// Three questions after a run ends (can be skipped).
pub const SESSION_END_ENABLED: SettingSpec =
    SettingSpec { name: "quiz.session_end_enabled", category: CATEGORY, kind: ValueKind::Bool, default: "true", tighten_to: Some("false") };

/// The settings the quiz card edits. API name = camelCase of the variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuizSettingKey {
    ReleaseCooldownMin,
    ReleaseMaxAttemptsPerDay,
    DistractionEnabled,
    CheckinEnabled,
    SessionEndEnabled,
}

impl QuizSettingKey {
    pub fn spec(self) -> SettingSpec {
        match self {
            QuizSettingKey::ReleaseCooldownMin => RELEASE_COOLDOWN_MIN,
            QuizSettingKey::ReleaseMaxAttemptsPerDay => RELEASE_MAX_ATTEMPTS,
            QuizSettingKey::DistractionEnabled => DISTRACTION_ENABLED,
            QuizSettingKey::CheckinEnabled => CHECKIN_ENABLED,
            QuizSettingKey::SessionEndEnabled => SESSION_END_ENABLED,
        }
    }
}

pub fn update_setting(conn: &mut Connection, key: QuizSettingKey, value: SettingValue, at_ms: i64) -> AppResult<UpdateOutcome> {
    settings::apply_setting(conn, &key.spec(), &value, at_ms, || Ok(()))
}

fn wrong_type(spec: &SettingSpec, value: &SettingValue) -> AppError {
    AppError::Internal(format!("{} has the wrong type: {value:?}", spec.name))
}

fn read_bool(conn: &Connection, spec: &SettingSpec) -> AppResult<bool> {
    match settings::read_value(conn, spec)? {
        SettingValue::Bool(v) => Ok(v),
        other => Err(wrong_type(spec, &other)),
    }
}

fn read_int(conn: &Connection, spec: &SettingSpec) -> AppResult<i64> {
    match settings::read_value(conn, spec)? {
        SettingValue::Int(v) => Ok(v),
        other => Err(wrong_type(spec, &other)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuizConfig {
    pub release_cooldown_min: i64,
    pub release_max_attempts_per_day: i64,
    pub distraction_enabled: bool,
    pub checkin_enabled: bool,
    pub session_end_enabled: bool,
}

impl QuizConfig {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        Ok(Self {
            release_cooldown_min: read_int(conn, &RELEASE_COOLDOWN_MIN)?,
            release_max_attempts_per_day: read_int(conn, &RELEASE_MAX_ATTEMPTS)?,
            distraction_enabled: read_bool(conn, &DISTRACTION_ENABLED)?,
            checkin_enabled: read_bool(conn, &CHECKIN_ENABLED)?,
            session_end_enabled: read_bool(conn, &SESSION_END_ENABLED)?,
        })
    }

    pub fn cooldown_ms(&self) -> i64 {
        self.release_cooldown_min.saturating_mul(60_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::testutil::set_lock_for_test;

    #[test]
    fn defaults_match_the_plan() {
        let conn = open_in_memory().expect("db");
        let config = QuizConfig::load(&conn).expect("load");
        assert_eq!(
            config,
            QuizConfig {
                release_cooldown_min: 3,
                release_max_attempts_per_day: 4,
                distraction_enabled: true,
                checkin_enabled: false,
                session_end_enabled: true
            }
        );
    }

    #[test]
    fn the_release_limits_cannot_be_loosened_while_the_lock_is_held() {
        let mut conn = open_in_memory().expect("db");
        set_lock_for_test(&conn, true);
        let outcome = update_setting(&mut conn, QuizSettingKey::ReleaseMaxAttemptsPerDay, SettingValue::Int(20), 5).expect("apply");
        assert_eq!(outcome, UpdateOutcome::RejectedLocked { key: "quiz.release_max_attempts_per_day".into() });
        assert_eq!(QuizConfig::load(&conn).expect("load").release_max_attempts_per_day, 4);
        update_setting(&mut conn, QuizSettingKey::CheckinEnabled, SettingValue::Bool(false), 6).expect("tighten stays allowed");
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        let mut conn = open_in_memory().expect("db");
        assert!(update_setting(&mut conn, QuizSettingKey::ReleaseCooldownMin, SettingValue::Int(0), 1).is_err());
        assert!(update_setting(&mut conn, QuizSettingKey::ReleaseMaxAttemptsPerDay, SettingValue::Int(21), 1).is_err());
    }
}
