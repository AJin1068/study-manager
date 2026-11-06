//! Lock state (written only by `timer`), daily study mode, and mode configuration.
//! Study time locks every setting, including the mode. Changes during study are rejected
//! and recorded, never silently dropped.

use chrono::TimeZone;
use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::plan::ModeLookup;
use crate::plan_shift::{shift_after_mode_change, ModeChange, ModeTrigger};
use crate::settings::{Outcome, Reason, UpdateOutcome};
use crate::store;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StudyLevel {
    Easy,
    Medium,
    Hard,
}

impl StudyLevel {
    /// Medium is the default shown when no row exists for the day.
    pub const DEFAULT: StudyLevel = StudyLevel::Medium;

    pub fn as_str(self) -> &'static str {
        match self {
            StudyLevel::Easy => "easy",
            StudyLevel::Medium => "medium",
            StudyLevel::Hard => "hard",
        }
    }

    pub fn parse(raw: &str) -> Option<StudyLevel> {
        match raw {
            "easy" => Some(StudyLevel::Easy),
            "medium" => Some(StudyLevel::Medium),
            "hard" => Some(StudyLevel::Hard),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModeConfig {
    pub level: StudyLevel,
    pub study_min: i64,
    pub break_min: i64,
    /// Only easy defines a long break; `None` means "미정" for the other levels.
    pub long_break_min: Option<i64>,
    pub sets_before_long_break: Option<i64>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TodayMode {
    pub date: String,
    pub level: StudyLevel,
    /// False when the day has no stored choice and the default is displayed.
    pub stored: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModeSnapshot {
    pub today: TodayMode,
    pub configs: Vec<ModeConfig>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LockState {
    /// True while a planned study run holds the settings/exit/plan lock (study and the breaks
    /// between its sets). Stored in the `study_state.studying` column.
    pub lock_held: bool,
    pub changed_at_ms: i64,
}

pub fn lock_held(conn: &Connection) -> AppResult<bool> {
    Ok(store::read_study_state(conn)?.0)
}

pub fn read_lock_state(conn: &Connection) -> AppResult<LockState> {
    let (lock_held, changed_at_ms) = store::read_study_state(conn)?;
    Ok(LockState { lock_held, changed_at_ms })
}

fn effective_level(conn: &Connection, date: &str) -> AppResult<(StudyLevel, bool)> {
    Ok(match store::read_daily_mode(conn, date)? {
        Some(level) => (level, true),
        None => (StudyLevel::DEFAULT, false),
    })
}

/// Chooses today's mode and, in the same transaction, shifts today's not-yet-started plans by
/// the length difference. Rejected while the lock is held, with a rejected_locked history row.
pub fn set_daily_mode<Tz: TimeZone>(
    conn: &mut Connection,
    tz: &Tz,
    date: &str,
    level: StudyLevel,
    at_ms: i64,
) -> AppResult<ModeChange> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (current, _) = effective_level(&tx, date)?;
    if lock_held(&tx)? {
        store::append_history(
            &tx,
            at_ms,
            "studyMode",
            Some(current.as_str()),
            level.as_str(),
            Reason::User,
            Outcome::RejectedLocked,
        )?;
        tx.commit()?;
        return Ok(ModeChange::rejected("studyMode".into()));
    }
    let old = ModeLookup::load(&tx)?;
    store::put_daily_mode(&tx, date, level, at_ms)?;
    store::append_history(
        &tx,
        at_ms,
        "studyMode",
        Some(current.as_str()),
        level.as_str(),
        Reason::User,
        Outcome::Applied,
    )?;
    let trigger = ModeTrigger::Daily { date: date.to_string(), before: current, after: level };
    let shift = shift_after_mode_change(&tx, tz, date, &old, &trigger, at_ms)?;
    tx.commit()?;
    Ok(ModeChange { outcome: UpdateOutcome::Applied { key: "studyMode".into() }, shift: Some(shift) })
}

/// Allowed minutes for any mode length (D-9).
pub const MODE_MINUTES_RANGE: std::ops::RangeInclusive<i64> = 1..=240;
/// Sets before a long break; below 2 there would be no short break between long ones.
pub const SETS_BEFORE_LONG_BREAK_RANGE: std::ops::RangeInclusive<i64> = 2..=12;

fn check_minutes(label: &str, value: i64) -> AppResult<()> {
    if MODE_MINUTES_RANGE.contains(&value) {
        return Ok(());
    }
    Err(AppError::Invalid(format!(
        "{label} must be {}..={} minutes, got {value}",
        MODE_MINUTES_RANGE.start(),
        MODE_MINUTES_RANGE.end()
    )))
}

/// Only easy defines a long break (PLAN: 4세트마다 긴 휴식); the other levels must leave it unset.
fn validate_mode_config(config: &ModeConfig) -> AppResult<()> {
    check_minutes("studyMin", config.study_min)?;
    check_minutes("breakMin", config.break_min)?;
    match (config.level, config.long_break_min, config.sets_before_long_break) {
        (StudyLevel::Easy, Some(long), Some(sets)) => {
            check_minutes("longBreakMin", long)?;
            if SETS_BEFORE_LONG_BREAK_RANGE.contains(&sets) {
                Ok(())
            } else {
                Err(AppError::Invalid(format!(
                    "setsBeforeLongBreak must be {}..={}, got {sets}",
                    SETS_BEFORE_LONG_BREAK_RANGE.start(),
                    SETS_BEFORE_LONG_BREAK_RANGE.end()
                )))
            }
        }
        (StudyLevel::Easy, _, _) => Err(AppError::Invalid(
            "easy needs both longBreakMin and setsBeforeLongBreak".into(),
        )),
        (_, None, None) => Ok(()),
        _ => Err(AppError::Invalid(
            "only easy has a long break; leave longBreakMin and setsBeforeLongBreak unset".into(),
        )),
    }
}

/// Edits one level's lengths and shifts today's not-yet-started plans by the resulting length
/// difference (nothing moves unless `today` uses this level). Rejected while the lock is held,
/// with a rejected_locked row (key `modeConfig.<level>`, before/after as JSON). Invalid input is
/// an error and records nothing.
pub fn update_mode_config<Tz: TimeZone>(
    conn: &mut Connection,
    tz: &Tz,
    today: &str,
    config: ModeConfig,
    at_ms: i64,
) -> AppResult<ModeChange> {
    validate_mode_config(&config)?;
    let key = format!("modeConfig.{}", config.level.as_str());
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let before = store::list_mode_configs(&tx)?
        .into_iter()
        .find(|c| c.level == config.level)
        .ok_or_else(|| AppError::Internal(format!("mode config row missing for {}", config.level.as_str())))?;
    let before_json = serde_json::to_string(&before)
        .map_err(|e| AppError::Internal(format!("mode config serialization failed: {e}")))?;
    let after_json = serde_json::to_string(&config)
        .map_err(|e| AppError::Internal(format!("mode config serialization failed: {e}")))?;

    if lock_held(&tx)? {
        store::append_history(&tx, at_ms, &key, Some(&before_json), &after_json, Reason::User, Outcome::RejectedLocked)?;
        tx.commit()?;
        return Ok(ModeChange::rejected(key));
    }
    let old = ModeLookup::load(&tx)?;
    store::put_mode_config(&tx, &config)?;
    store::append_history(&tx, at_ms, &key, Some(&before_json), &after_json, Reason::User, Outcome::Applied)?;
    let trigger = ModeTrigger::Config { before, after: config };
    let shift = shift_after_mode_change(&tx, tz, today, &old, &trigger, at_ms)?;
    tx.commit()?;
    Ok(ModeChange { outcome: UpdateOutcome::Applied { key }, shift: Some(shift) })
}

pub fn mode_snapshot(conn: &Connection, date: &str) -> AppResult<ModeSnapshot> {
    let (level, stored) = effective_level(conn, date)?;
    Ok(ModeSnapshot {
        today: TodayMode { date: date.to_string(), level, stored },
        configs: store::list_mode_configs(conn)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use chrono::Utc;

    fn set_daily_mode(conn: &mut Connection, date: &str, level: StudyLevel, at_ms: i64) -> AppResult<UpdateOutcome> {
        super::set_daily_mode(conn, &Utc, date, level, at_ms).map(|change| change.outcome)
    }

    fn update_mode_config(conn: &mut Connection, config: ModeConfig, at_ms: i64) -> AppResult<UpdateOutcome> {
        super::update_mode_config(conn, &Utc, "2026-11-05", config, at_ms).map(|change| change.outcome)
    }

    #[test]
    fn mode_config_matches_spec_values() {
        let conn = open_in_memory().expect("db");
        let configs = store::list_mode_configs(&conn).expect("configs");
        let by_level = |level| configs.iter().find(|c| c.level == level).expect("row").clone();
        assert_eq!(
            by_level(StudyLevel::Easy),
            ModeConfig {
                level: StudyLevel::Easy,
                study_min: 25,
                break_min: 5,
                long_break_min: Some(20),
                sets_before_long_break: Some(4),
            }
        );
        assert_eq!(by_level(StudyLevel::Medium).study_min, 50);
        assert_eq!(by_level(StudyLevel::Medium).break_min, 15);
        assert_eq!(by_level(StudyLevel::Hard).study_min, 90);
        assert_eq!(by_level(StudyLevel::Hard).break_min, 10);
        assert_eq!(by_level(StudyLevel::Hard).long_break_min, None);
    }

    fn easy(study: i64, brk: i64, long: Option<i64>, sets: Option<i64>) -> ModeConfig {
        ModeConfig {
            level: StudyLevel::Easy,
            study_min: study,
            break_min: brk,
            long_break_min: long,
            sets_before_long_break: sets,
        }
    }

    fn medium(study: i64, brk: i64) -> ModeConfig {
        ModeConfig {
            level: StudyLevel::Medium,
            study_min: study,
            break_min: brk,
            long_break_min: None,
            sets_before_long_break: None,
        }
    }

    fn config_of(conn: &Connection, level: StudyLevel) -> ModeConfig {
        store::list_mode_configs(conn).expect("configs").into_iter().find(|c| c.level == level).expect("row")
    }

    #[test]
    fn mode_config_serialization_matches_shared_fixture() {
        let fixture_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/mode-config.json");
        let raw = std::fs::read_to_string(fixture_path).expect("fixture");
        let expected: serde_json::Value = serde_json::from_str(&raw).expect("json");
        let conn = open_in_memory().expect("db");
        let configs = store::list_mode_configs(&conn).expect("configs");
        let actual = serde_json::to_value(&configs[..2]).expect("serialize");
        assert_eq!(actual, expected);
    }

    #[test]
    fn mode_config_edit_is_stored_and_logged_as_json() {
        let mut conn = open_in_memory().expect("db");
        let outcome = update_mode_config(&mut conn, easy(30, 10, Some(25), Some(3)), 5).expect("edit");
        assert_eq!(outcome, UpdateOutcome::Applied { key: "modeConfig.easy".into() });
        assert_eq!(config_of(&conn, StudyLevel::Easy), easy(30, 10, Some(25), Some(3)));
        assert_eq!(config_of(&conn, StudyLevel::Medium).study_min, 50, "other levels stay untouched");
        let latest = &crate::settings::read_history(&conn, 1).expect("h")[0];
        assert_eq!((latest.key.as_str(), latest.reason.as_str(), latest.outcome.as_str()), ("modeConfig.easy", "user", "applied"));
        let before: serde_json::Value = serde_json::from_str(latest.before.as_deref().expect("before")).expect("json");
        let after: serde_json::Value = serde_json::from_str(&latest.after).expect("json");
        assert_eq!(before["studyMin"], 25);
        assert_eq!(after["studyMin"], 30);
        assert_eq!(after["setsBeforeLongBreak"], 3);
    }

    #[test]
    fn mode_minutes_outside_one_to_240_are_rejected_without_a_record() {
        let mut conn = open_in_memory().expect("db");
        for bad in [medium(0, 15), medium(241, 15), medium(50, 0), medium(50, 241), medium(-5, 15)] {
            assert!(update_mode_config(&mut conn, bad.clone(), 1).is_err(), "{bad:?}");
        }
        assert!(update_mode_config(&mut conn, easy(25, 5, Some(0), Some(4)), 1).is_err());
        assert!(update_mode_config(&mut conn, easy(25, 5, Some(241), Some(4)), 1).is_err());
        assert_eq!(config_of(&conn, StudyLevel::Medium).study_min, 50);
        assert!(crate::settings::read_history(&conn, 10).expect("h").is_empty());
        // Boundaries are inclusive.
        update_mode_config(&mut conn, medium(1, 240), 2).expect("1/240");
        update_mode_config(&mut conn, medium(240, 1), 3).expect("240/1");
    }

    #[test]
    fn long_break_rules_apply_per_level() {
        let mut conn = open_in_memory().expect("db");
        assert!(update_mode_config(&mut conn, easy(25, 5, None, None), 1).is_err());
        assert!(update_mode_config(&mut conn, easy(25, 5, Some(20), None), 1).is_err());
        assert!(update_mode_config(&mut conn, easy(25, 5, Some(20), Some(1)), 1).is_err());
        assert!(update_mode_config(&mut conn, easy(25, 5, Some(20), Some(13)), 1).is_err());
        let mut hard = medium(90, 10);
        hard.level = StudyLevel::Hard;
        hard.long_break_min = Some(20);
        assert!(update_mode_config(&mut conn, hard.clone(), 1).is_err());
        hard.long_break_min = None;
        hard.sets_before_long_break = Some(4);
        assert!(update_mode_config(&mut conn, hard, 1).is_err());
        assert!(crate::settings::read_history(&conn, 10).expect("h").is_empty());
    }

    #[test]
    fn mode_config_edit_during_study_is_rejected_recorded_and_value_unchanged() {
        let mut conn = open_in_memory().expect("db");
        crate::testutil::set_lock_for_test(&conn, true);
        let outcome = update_mode_config(&mut conn, medium(60, 20), 2).expect("attempt");
        assert_eq!(outcome, UpdateOutcome::RejectedLocked { key: "modeConfig.medium".into() });
        assert_eq!(config_of(&conn, StudyLevel::Medium), medium(50, 15));
        let latest = &crate::settings::read_history(&conn, 1).expect("h")[0];
        assert_eq!((latest.key.as_str(), latest.outcome.as_str()), ("modeConfig.medium", "rejected_locked"));
        assert!(latest.after.contains("\"studyMin\":60"));
    }

    #[test]
    fn mode_config_edit_refuses_when_lock_is_unknown() {
        let mut conn = open_in_memory().expect("db");
        conn.execute("DELETE FROM study_state", []).expect("delete");
        assert!(update_mode_config(&mut conn, medium(60, 20), 1).is_err());
        assert_eq!(config_of(&conn, StudyLevel::Medium), medium(50, 15));
        assert!(crate::settings::read_history(&conn, 10).expect("h").is_empty());
    }

    #[test]
    fn missing_daily_row_shows_medium_unstored() {
        let conn = open_in_memory().expect("db");
        let snap = mode_snapshot(&conn, "2026-11-05").expect("snap");
        assert_eq!(snap.today.level, StudyLevel::Medium);
        assert!(!snap.today.stored);
        // Reading must not write a row.
        assert_eq!(store::read_daily_mode(&conn, "2026-11-05").expect("read"), None);
    }

    #[test]
    fn daily_mode_is_stored_per_date() {
        let mut conn = open_in_memory().expect("db");
        set_daily_mode(&mut conn, "2026-11-05", StudyLevel::Hard, 1).expect("set");
        let today = mode_snapshot(&conn, "2026-11-05").expect("snap").today;
        assert_eq!((today.level, today.stored), (StudyLevel::Hard, true));
        let other = mode_snapshot(&conn, "2026-11-06").expect("snap").today;
        assert_eq!((other.level, other.stored), (StudyLevel::Medium, false));
    }

    #[test]
    fn mode_change_during_study_is_rejected_and_recorded() {
        let mut conn = open_in_memory().expect("db");
        crate::testutil::set_lock_for_test(&conn, true);
        let outcome = set_daily_mode(&mut conn, "2026-11-05", StudyLevel::Easy, 2).expect("set");
        assert_eq!(outcome, UpdateOutcome::RejectedLocked { key: "studyMode".into() });
        assert_eq!(store::read_daily_mode(&conn, "2026-11-05").expect("read"), None);
        let latest = &crate::settings::read_history(&conn, 1).expect("h")[0];
        assert_eq!(latest.key, "studyMode");
        assert_eq!(latest.outcome, "rejected_locked");
    }

    /// Removes its temp directory on drop, so a failing assertion does not leak it.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "study-manager-{tag}-{}",
                crate::clock::now_ms().expect("clock")
            ));
            std::fs::create_dir_all(&dir).expect("dir");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// AC8: the lock survives a restart because it is read from SQLite on every check.
    #[test]
    fn lock_flag_is_restored_after_reopening_the_database() {
        let dir = TempDir::new("restart");
        let path = dir.0.join("restart.sqlite");
        {
            let conn = crate::db::open(&path).expect("open");
            crate::testutil::set_lock_for_test(&conn, true);
        }
        let reopened = crate::db::open(&path).expect("reopen");
        assert!(lock_held(&reopened).expect("read"));
    }

    /// Restart fail-open guard: deleting the lock row between launches must not reset the lock
    /// to "lock free". The next launch seeds only brand-new databases, so the read errors.
    #[test]
    fn deleted_lock_row_is_not_reseeded_on_restart() {
        let dir = TempDir::new("deleted");
        let path = dir.0.join("deleted.sqlite");
        {
            let conn = crate::db::open(&path).expect("open");
            conn.execute("DELETE FROM study_state", []).expect("delete");
        }
        let reopened = crate::db::open(&path).expect("reopen");
        assert!(lock_held(&reopened).is_err(), "missing row must error, not read as free");
        assert!(
            store::put_study_state(&crate::db::open(&path).expect("reopen again"), true, 3).is_err(),
            "a write must not recreate the deleted row"
        );
    }

    /// Fail-closed: a missing lock row must surface as an error, never as "lock free".
    #[test]
    fn missing_lock_row_is_an_error_not_free() {
        let conn = open_in_memory().expect("db");
        conn.execute("DELETE FROM study_state", []).expect("delete setup row");
        assert!(lock_held(&conn).is_err());
        assert!(read_lock_state(&conn).is_err());
    }

    /// Every mutation that checks the lock must refuse to act when the lock is unknown:
    /// no daily mode row, no history row.
    #[test]
    fn set_daily_mode_refuses_when_lock_is_unknown() {
        let mut conn = open_in_memory().expect("db");
        conn.execute("DELETE FROM study_state", []).expect("delete");
        assert!(set_daily_mode(&mut conn, "2026-11-05", StudyLevel::Hard, 1).is_err());
        assert_eq!(store::read_daily_mode(&conn, "2026-11-05").expect("read"), None);
        assert!(crate::settings::read_history(&conn, 10).expect("h").is_empty());
    }

    #[test]
    fn writing_the_lock_refuses_when_the_row_is_missing() {
        let conn = open_in_memory().expect("db");
        conn.execute("DELETE FROM study_state", []).expect("delete");
        assert!(store::put_study_state(&conn, true, 1).is_err());
        assert!(crate::settings::read_history(&conn, 10).expect("h").is_empty());
    }
}
