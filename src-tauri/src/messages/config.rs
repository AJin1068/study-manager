//! `messages.*` settings, through the same locked, history-recorded path as every other setting.
//! Switching a source of interruptions off only removes interruptions, so those stay allowed
//! during a planned run; changing the tone or the rate is locked like any setting.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::Tone;
use crate::error::{AppError, AppResult};
use crate::settings::{self, SettingSpec, SettingValue, UpdateOutcome, ValueKind};

const CATEGORY: &str = "messages";

pub const ENABLED: SettingSpec =
    SettingSpec { name: "messages.enabled", category: CATEGORY, kind: ValueKind::Bool, default: "true", tighten_to: Some("false") };
pub const TONE: SettingSpec = SettingSpec {
    name: "messages.tone",
    category: CATEGORY,
    kind: ValueKind::Text { allowed: &["gentle", "warm", "normal", "firm", "harsh"] },
    default: "normal",
    tighten_to: None,
};
/// Messages per rolling hour, all kinds together.
pub const MAX_PER_HOUR: SettingSpec =
    SettingSpec { name: "messages.max_per_hour", category: CATEGORY, kind: ValueKind::Int { min: 1, max: 12 }, default: "3", tighten_to: None };
/// A message on a blocked-app attempt, a finished set, or a streak.
pub const EVENTS_ENABLED: SettingSpec =
    SettingSpec { name: "messages.events_enabled", category: CATEGORY, kind: ValueKind::Bool, default: "true", tighten_to: Some("false") };
/// A message now and then while studying, without an event.
pub const RANDOM_ENABLED: SettingSpec =
    SettingSpec { name: "messages.random_enabled", category: CATEGORY, kind: ValueKind::Bool, default: "false", tighten_to: Some("false") };
/// Minutes without any message before a random one may come.
pub const RANDOM_INTERVAL_MIN: SettingSpec = SettingSpec {
    name: "messages.random_interval_min",
    category: CATEGORY,
    kind: ValueKind::Int { min: 10, max: 120 },
    default: "30",
    tighten_to: None,
};
/// Also send the general phrase as a system notification (never with a subject or page name).
pub const NOTIFY_ENABLED: SettingSpec =
    SettingSpec { name: "messages.notify_enabled", category: CATEGORY, kind: ValueKind::Bool, default: "true", tighten_to: Some("false") };

/// The settings the messages card edits. API name = camelCase of the variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageSettingKey {
    Enabled,
    Tone,
    MaxPerHour,
    EventsEnabled,
    RandomEnabled,
    RandomIntervalMin,
    NotifyEnabled,
}

impl MessageSettingKey {
    pub fn spec(self) -> SettingSpec {
        match self {
            MessageSettingKey::Enabled => ENABLED,
            MessageSettingKey::Tone => TONE,
            MessageSettingKey::MaxPerHour => MAX_PER_HOUR,
            MessageSettingKey::EventsEnabled => EVENTS_ENABLED,
            MessageSettingKey::RandomEnabled => RANDOM_ENABLED,
            MessageSettingKey::RandomIntervalMin => RANDOM_INTERVAL_MIN,
            MessageSettingKey::NotifyEnabled => NOTIFY_ENABLED,
        }
    }
}

pub fn update_setting(conn: &mut Connection, key: MessageSettingKey, value: SettingValue, at_ms: i64) -> AppResult<UpdateOutcome> {
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
pub struct MessageConfig {
    pub enabled: bool,
    pub tone: Tone,
    pub max_per_hour: i64,
    pub events_enabled: bool,
    pub random_enabled: bool,
    pub random_interval_min: i64,
    pub notify_enabled: bool,
}

impl MessageConfig {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        let tone = match settings::read_value(conn, &TONE)? {
            SettingValue::Text(raw) => Tone::parse(&raw).ok_or_else(|| AppError::Internal(format!("unknown tone {raw}")))?,
            other => return Err(wrong_type(&TONE, &other)),
        };
        Ok(Self {
            enabled: read_bool(conn, &ENABLED)?,
            tone,
            max_per_hour: read_int(conn, &MAX_PER_HOUR)?,
            events_enabled: read_bool(conn, &EVENTS_ENABLED)?,
            random_enabled: read_bool(conn, &RANDOM_ENABLED)?,
            random_interval_min: read_int(conn, &RANDOM_INTERVAL_MIN)?,
            notify_enabled: read_bool(conn, &NOTIFY_ENABLED)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::testutil::set_lock_for_test;

    #[test]
    fn defaults_are_normal_tone_with_events_and_a_small_hourly_cap() {
        let conn = open_in_memory().expect("db");
        assert_eq!(
            MessageConfig::load(&conn).expect("load"),
            MessageConfig {
                enabled: true,
                tone: Tone::Normal,
                max_per_hour: 3,
                events_enabled: true,
                random_enabled: false,
                random_interval_min: 30,
                notify_enabled: true
            }
        );
    }

    #[test]
    fn the_tone_is_stored_and_unknown_tones_and_ranges_are_rejected() {
        let mut conn = open_in_memory().expect("db");
        update_setting(&mut conn, MessageSettingKey::Tone, SettingValue::Text("harsh".into()), 1).expect("tone");
        assert_eq!(MessageConfig::load(&conn).expect("load").tone, Tone::Harsh);
        assert!(update_setting(&mut conn, MessageSettingKey::Tone, SettingValue::Text("rude".into()), 2).is_err());
        assert!(update_setting(&mut conn, MessageSettingKey::MaxPerHour, SettingValue::Int(0), 3).is_err());
        assert!(update_setting(&mut conn, MessageSettingKey::RandomIntervalMin, SettingValue::Int(5), 4).is_err());
    }

    #[test]
    fn the_rate_cannot_be_raised_during_a_run_but_messages_can_be_switched_off() {
        let mut conn = open_in_memory().expect("db");
        set_lock_for_test(&conn, true);
        let raised = update_setting(&mut conn, MessageSettingKey::MaxPerHour, SettingValue::Int(12), 5).expect("apply");
        assert_eq!(raised, UpdateOutcome::RejectedLocked { key: "messages.max_per_hour".into() });
        update_setting(&mut conn, MessageSettingKey::Enabled, SettingValue::Bool(false), 6).expect("off stays allowed");
        assert!(!MessageConfig::load(&conn).expect("load").enabled);
    }
}
