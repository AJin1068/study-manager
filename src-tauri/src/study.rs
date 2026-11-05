//! Study state (manual test toggle for now), daily study mode, and mode configuration.
//! Study time locks every setting, including the mode. Changes during study are rejected
//! and recorded, never silently dropped.

use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::AppResult;
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

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
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
pub struct StudyState {
    pub studying: bool,
    pub changed_at_ms: i64,
}

pub fn is_studying(conn: &Connection) -> AppResult<bool> {
    Ok(store::read_study_state(conn)?.0)
}

pub fn read_state(conn: &Connection) -> AppResult<StudyState> {
    let (studying, changed_at_ms) = store::read_study_state(conn)?;
    Ok(StudyState { studying, changed_at_ms })
}

/// Manual "공부 중" toggle used to exercise the lock until 1-2 adds automatic timers.
/// Toggling is always allowed (it is the switch that creates the lock) and is recorded
/// with reason=test_toggle.
pub fn set_study_state_test(conn: &mut Connection, studying: bool, at_ms: i64) -> AppResult<UpdateOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (previous, _) = store::read_study_state(&tx)?;
    store::put_study_state(&tx, studying, at_ms)?;
    store::append_history(
        &tx,
        at_ms,
        "studyState",
        Some(&previous.to_string()),
        &studying.to_string(),
        Reason::TestToggle,
        Outcome::Applied,
    )?;
    tx.commit()?;
    Ok(UpdateOutcome::Applied { key: "studyState".into() })
}

fn effective_level(conn: &Connection, date: &str) -> AppResult<(StudyLevel, bool)> {
    Ok(match store::read_daily_mode(conn, date)? {
        Some(level) => (level, true),
        None => (StudyLevel::DEFAULT, false),
    })
}

/// Chooses today's mode. Rejected while studying, with a rejected_locked history row.
pub fn set_daily_mode(
    conn: &mut Connection,
    date: &str,
    level: StudyLevel,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (current, _) = effective_level(&tx, date)?;
    if is_studying(&tx)? {
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
        return Ok(UpdateOutcome::RejectedLocked { key: "studyMode".into() });
    }
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
    tx.commit()?;
    Ok(UpdateOutcome::Applied { key: "studyMode".into() })
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
        set_study_state_test(&mut conn, true, 1).expect("on");
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
    fn studying_flag_is_restored_after_reopening_the_database() {
        let dir = TempDir::new("restart");
        let path = dir.0.join("restart.sqlite");
        {
            let mut conn = crate::db::open(&path).expect("open");
            set_study_state_test(&mut conn, true, 2).expect("on");
        }
        let reopened = crate::db::open(&path).expect("reopen");
        assert!(is_studying(&reopened).expect("read"));
    }

    /// Restart fail-open guard: deleting the lock row between launches must not reset the lock
    /// to "not studying". The next launch seeds only brand-new databases, so the read errors.
    #[test]
    fn deleted_lock_row_is_not_reseeded_on_restart() {
        let dir = TempDir::new("deleted");
        let path = dir.0.join("deleted.sqlite");
        {
            let conn = crate::db::open(&path).expect("open");
            conn.execute("DELETE FROM study_state", []).expect("delete");
        }
        let reopened = crate::db::open(&path).expect("reopen");
        assert!(is_studying(&reopened).is_err(), "missing row must error, not read as free");
        assert!(
            set_study_state_test(&mut crate::db::open(&path).expect("reopen again"), true, 3).is_err(),
            "a write must not recreate the deleted row"
        );
    }

    #[test]
    fn toggle_records_test_toggle_reason() {
        let mut conn = open_in_memory().expect("db");
        set_study_state_test(&mut conn, true, 7).expect("on");
        assert!(read_state(&conn).expect("state").studying);
        let latest = &crate::settings::read_history(&conn, 1).expect("h")[0];
        assert_eq!(latest.reason, "test_toggle");
        assert_eq!(latest.before.as_deref(), Some("false"));
        assert_eq!(latest.after, "true");
    }

    /// Fail-closed: a missing lock row must surface as an error, never as "not studying".
    #[test]
    fn missing_lock_row_is_an_error_not_free() {
        let conn = open_in_memory().expect("db");
        conn.execute("DELETE FROM study_state", []).expect("delete setup row");
        assert!(is_studying(&conn).is_err());
        assert!(read_state(&conn).is_err());
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
    fn set_study_state_test_refuses_when_lock_is_unknown() {
        let mut conn = open_in_memory().expect("db");
        conn.execute("DELETE FROM study_state", []).expect("delete");
        assert!(set_study_state_test(&mut conn, true, 1).is_err());
        assert!(crate::settings::read_history(&conn, 10).expect("h").is_empty());
    }
}
