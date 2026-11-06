//! What this user shows to a room (`friends.*` settings) and the pure functions that turn local
//! data into outbound reports under those settings.
//!
//! Defaults are conservative: level and state are shown (a room member is a friend who sees how you
//! are doing), study minutes and the distraction count are private, and distraction notices
//! neither leave nor arrive. Switching a share off removes that data from the next report and
//! the server deletes what it already held (`set_friend_prefs`). Switching off is allowed during a
//! planned run (privacy tightening); switching on is locked like every setting. What a distraction
//! was (app, site, window title) is never part of any report, under any setting.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::model::MemberState;
use crate::error::{AppError, AppResult};
use crate::remote::payload::{DailyReportArgs, FriendPrefsArgs, StatusReportArgs};
use crate::settings::{self, SettingSpec, SettingValue, UpdateOutcome, ValueKind};

const CATEGORY: &str = "friends";

pub const SHARE_LEVEL: SettingSpec =
    SettingSpec { name: "friends.share_level", category: CATEGORY, kind: ValueKind::Bool, default: "true", tighten_to: Some("false") };
pub const SHARE_STATUS: SettingSpec =
    SettingSpec { name: "friends.share_status", category: CATEGORY, kind: ValueKind::Bool, default: "true", tighten_to: Some("false") };
pub const SHARE_WEEK_TIME: SettingSpec =
    SettingSpec { name: "friends.share_week_time", category: CATEGORY, kind: ValueKind::Bool, default: "false", tighten_to: Some("false") };
pub const SHARE_ATTEMPTS: SettingSpec =
    SettingSpec { name: "friends.share_attempts", category: CATEGORY, kind: ValueKind::Bool, default: "false", tighten_to: Some("false") };
pub const DISTRACTION_PUBLISH: SettingSpec =
    SettingSpec { name: "friends.distraction_publish", category: CATEGORY, kind: ValueKind::Bool, default: "false", tighten_to: Some("false") };
pub const DISTRACTION_RECEIVE: SettingSpec =
    SettingSpec { name: "friends.distraction_receive", category: CATEGORY, kind: ValueKind::Bool, default: "false", tighten_to: Some("false") };

/// The settings the friends card edits. API name = camelCase of the variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FriendSettingKey {
    ShareLevel,
    ShareStatus,
    ShareWeekTime,
    ShareAttempts,
    DistractionPublish,
    DistractionReceive,
}

impl FriendSettingKey {
    pub fn spec(self) -> SettingSpec {
        match self {
            FriendSettingKey::ShareLevel => SHARE_LEVEL,
            FriendSettingKey::ShareStatus => SHARE_STATUS,
            FriendSettingKey::ShareWeekTime => SHARE_WEEK_TIME,
            FriendSettingKey::ShareAttempts => SHARE_ATTEMPTS,
            FriendSettingKey::DistractionPublish => DISTRACTION_PUBLISH,
            FriendSettingKey::DistractionReceive => DISTRACTION_RECEIVE,
        }
    }
}

pub fn update_setting(conn: &mut Connection, key: FriendSettingKey, value: SettingValue, at_ms: i64) -> AppResult<UpdateOutcome> {
    settings::apply_setting(conn, &key.spec(), &value, at_ms, || Ok(()))
}

fn read_bool(conn: &Connection, spec: &SettingSpec) -> AppResult<bool> {
    match settings::read_value(conn, spec)? {
        SettingValue::Bool(v) => Ok(v),
        other => Err(AppError::Internal(format!("{} has the wrong type: {other:?}", spec.name))),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendSettings {
    pub share_level: bool,
    pub share_status: bool,
    pub share_week_time: bool,
    pub share_attempts: bool,
    pub distraction_publish: bool,
    pub distraction_receive: bool,
}

impl FriendSettings {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        Ok(Self {
            share_level: read_bool(conn, &SHARE_LEVEL)?,
            share_status: read_bool(conn, &SHARE_STATUS)?,
            share_week_time: read_bool(conn, &SHARE_WEEK_TIME)?,
            share_attempts: read_bool(conn, &SHARE_ATTEMPTS)?,
            distraction_publish: read_bool(conn, &DISTRACTION_PUBLISH)?,
            distraction_receive: read_bool(conn, &DISTRACTION_RECEIVE)?,
        })
    }

    pub fn as_args(&self) -> FriendPrefsArgs {
        FriendPrefsArgs {
            share_level: self.share_level,
            share_status: self.share_status,
            share_week_time: self.share_week_time,
            share_attempts: self.share_attempts,
            distraction_publish: self.distraction_publish,
            distraction_receive: self.distraction_receive,
        }
    }
}

/// The lock phase as a room sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalPhase {
    Studying,
    Break,
    Idle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalDay {
    /// Local date, `YYYY-MM-DD`.
    pub day: String,
    pub study_min: i64,
    pub plan_min: i64,
}

/// Everything the reports are built from, read once per tick from the local database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSnapshot {
    pub phase: LocalPhase,
    pub remaining_sec: i64,
    pub level: i64,
    pub attempts_today: i64,
    /// This week's days up to today.
    pub days: Vec<LocalDay>,
}

/// The status report under the user's settings, or `None` when nothing about level or state may
/// be shown (then nothing is sent). A field the user keeps private is replaced by a neutral value
/// the server hides anyway (`idle`, 0 seconds, level 1), so the payload never carries it.
pub fn status_report(settings: &FriendSettings, local: &LocalSnapshot) -> Option<StatusReportArgs> {
    if !settings.share_level && !settings.share_status {
        return None;
    }
    let (state, remaining_sec) = if settings.share_status {
        match local.phase {
            LocalPhase::Studying => (MemberState::Studying, local.remaining_sec.clamp(0, 86_400)),
            LocalPhase::Break => (MemberState::Break, local.remaining_sec.clamp(0, 86_400)),
            LocalPhase::Idle => (MemberState::Idle, 0),
        }
    } else {
        (MemberState::Idle, 0)
    };
    Some(StatusReportArgs {
        state: state.as_str(),
        remaining_sec,
        level: if settings.share_level { local.level.clamp(1, 999) } else { 1 },
        attempts_today: settings.share_attempts.then_some(local.attempts_today.max(0)),
    })
}

/// Daily minutes, only when the user shares study time.
pub fn daily_reports(settings: &FriendSettings, local: &LocalSnapshot) -> Vec<DailyReportArgs> {
    if !settings.share_week_time {
        return Vec::new();
    }
    local
        .days
        .iter()
        .map(|d| DailyReportArgs { day: d.day.clone(), study_min: d.study_min.clamp(0, 1440), plan_min: d.plan_min.clamp(0, 1440) })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::testutil::set_lock_for_test;

    fn local() -> LocalSnapshot {
        LocalSnapshot {
            phase: LocalPhase::Studying,
            remaining_sec: 1_920,
            level: 7,
            attempts_today: 4,
            days: vec![LocalDay { day: "2026-11-06".into(), study_min: 95, plan_min: 120 }],
        }
    }

    fn settings(level: bool, status: bool, week: bool, attempts: bool) -> FriendSettings {
        FriendSettings {
            share_level: level,
            share_status: status,
            share_week_time: week,
            share_attempts: attempts,
            distraction_publish: false,
            distraction_receive: false,
        }
    }

    #[test]
    fn every_default_is_conservative() {
        let conn = open_in_memory().expect("db");
        let defaults = FriendSettings::load(&conn).expect("load");
        assert_eq!(defaults, settings(true, true, false, false));
        assert!(!defaults.distraction_publish && !defaults.distraction_receive);
    }

    #[test]
    fn the_default_payload_carries_level_and_state_only() {
        let report = status_report(&settings(true, true, false, false), &local()).expect("report");
        assert_eq!((report.state, report.remaining_sec, report.level, report.attempts_today), ("studying", 1_920, 7, None));
        assert!(daily_reports(&settings(true, true, false, false), &local()).is_empty(), "study minutes stay home by default");
    }

    #[test]
    fn a_scope_is_added_to_the_payload_when_switched_on_and_removed_when_switched_off() {
        let on = status_report(&settings(true, true, true, true), &local()).expect("report");
        assert_eq!(on.attempts_today, Some(4));
        assert_eq!(daily_reports(&settings(true, true, true, true), &local()).len(), 1);

        let off = status_report(&settings(true, true, false, false), &local()).expect("report");
        assert_eq!(off.attempts_today, None);
        assert!(daily_reports(&settings(true, true, false, false), &local()).is_empty());
    }

    #[test]
    fn a_private_level_or_state_is_replaced_by_a_neutral_value_not_sent() {
        let no_state = status_report(&settings(true, false, false, false), &local()).expect("report");
        assert_eq!((no_state.state, no_state.remaining_sec, no_state.level), ("idle", 0, 7));
        let no_level = status_report(&settings(false, true, false, false), &local()).expect("report");
        assert_eq!((no_level.state, no_level.level), ("studying", 1));
        assert_eq!(status_report(&settings(false, false, true, true), &local()), None, "nothing to show, nothing sent");
    }

    #[test]
    fn the_wire_form_of_a_report_has_no_field_for_what_the_distraction_was() {
        let json = serde_json::to_value(status_report(&settings(true, true, true, true), &local()).expect("report")).expect("json");
        let mut keys: Vec<&str> = json.as_object().expect("object").keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["p_attempts_today", "p_level", "p_remaining_sec", "p_state"]);
    }

    #[test]
    fn during_a_run_switching_off_is_allowed_and_recorded_but_switching_on_is_refused() {
        let mut conn = open_in_memory().expect("db");
        update_setting(&mut conn, FriendSettingKey::ShareAttempts, SettingValue::Bool(true), 1).expect("on, before the run");
        set_lock_for_test(&conn, true);

        let widen = update_setting(&mut conn, FriendSettingKey::ShareWeekTime, SettingValue::Bool(true), 2).expect("outcome");
        assert_eq!(widen, UpdateOutcome::RejectedLocked { key: "friends.share_week_time".into() });
        let widen_distraction = update_setting(&mut conn, FriendSettingKey::DistractionPublish, SettingValue::Bool(true), 3).expect("outcome");
        assert!(matches!(widen_distraction, UpdateOutcome::RejectedLocked { .. }));

        let tighten = update_setting(&mut conn, FriendSettingKey::ShareAttempts, SettingValue::Bool(false), 4).expect("outcome");
        assert!(matches!(tighten, UpdateOutcome::Applied { .. }));
        assert!(!FriendSettings::load(&conn).expect("load").share_attempts);
        assert!(!FriendSettings::load(&conn).expect("load").share_week_time);

        let outcomes: Vec<(String, String)> = settings::read_history(&conn, 20)
            .expect("history")
            .into_iter()
            .map(|h| (h.key, h.outcome))
            .collect();
        assert!(outcomes.contains(&("friends.share_attempts".to_string(), "applied_privacy_tighten".to_string())));
        assert!(outcomes.contains(&("friends.share_week_time".to_string(), "rejected_locked".to_string())));
    }
}
