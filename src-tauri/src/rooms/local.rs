//! Reads the local facts a status report is made of: lock phase and time left, level, today's
//! distraction-attempt count, and this week's study and planned minutes per local date.
//! Read-only; everything comes from the PC's own database.

use chrono::{Duration, TimeZone};
use rusqlite::Connection;

use super::prefs::{LocalDay, LocalPhase, LocalSnapshot};
use crate::dashboard;
use crate::error::{AppError, AppResult};
use crate::plan;
use crate::points;
use crate::store;
use crate::timer::{self, model::Phase};

const MINUTE_MS: i64 = 60_000;

pub fn collect<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<LocalSnapshot> {
    let state = timer::current_state(conn, tz, now_ms)?;
    let phase = match state.phase {
        Phase::Study => LocalPhase::Studying,
        Phase::Break | Phase::LongBreak => LocalPhase::Break,
        Phase::Idle => LocalPhase::Idle,
    };
    let remaining_sec = match (phase, state.planned_end_ms) {
        (LocalPhase::Idle, _) | (_, None) => 0,
        (_, Some(end)) => ((end - now_ms) / 1000).max(0),
    };

    let today = tz
        .timestamp_millis_opt(now_ms)
        .single()
        .ok_or_else(|| AppError::Internal("timestamp has no local date".into()))?
        .date_naive();
    let day_start = |date: chrono::NaiveDate| plan::local_to_ms(tz, date, chrono::NaiveTime::MIN);
    let attempts_today = store::rooms::count_attempts_between(conn, day_start(today)?, day_start(today + Duration::days(1))?)?;

    let week = dashboard::week(conn, tz, now_ms)?;
    let today_text = today.format("%Y-%m-%d").to_string();
    let days = week
        .days
        .into_iter()
        .filter(|d| d.date <= today_text)
        .map(|d| LocalDay { day: d.date, study_min: d.actual_ms / MINUTE_MS, plan_min: d.planned_ms / MINUTE_MS })
        .collect();

    Ok(LocalSnapshot { phase, remaining_sec, level: points::status(conn)?.level, attempts_today, days })
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn a_fresh_database_is_idle_level_one_with_nothing_studied() {
        let conn = open_in_memory().expect("db");
        let utc = FixedOffset::east_opt(0).expect("offset");
        let snapshot = collect(&conn, &utc, 1_793_975_400_000).expect("collect");
        assert_eq!((snapshot.phase, snapshot.remaining_sec, snapshot.attempts_today), (LocalPhase::Idle, 0, 0));
        assert!(snapshot.level >= 1);
        assert!(snapshot.days.iter().all(|d| d.study_min == 0), "{:?}", snapshot.days);
        assert!(snapshot.days.last().is_some_and(|d| d.day == "2026-11-06"), "the week ends at today, not at Sunday");
    }

    #[test]
    fn todays_attempts_are_counted_in_the_local_day_not_the_utc_day() {
        let conn = open_in_memory().expect("db");
        let seoul = FixedOffset::east_opt(9 * 3600).expect("offset");
        // 2026-11-06T15:30:00Z is already 2026-11-07 00:30 in Seoul.
        let now = 1_793_979_000_000;
        let seoul_midnight = 1_793_977_200_000; // 2026-11-06T15:00:00Z = 2026-11-07 00:00 KST
        for at in [seoul_midnight - 1, seoul_midnight, now] {
            conn.execute(
                "INSERT INTO block_events (at_ms, target_kind, target_id, action, mode) VALUES (?1, 'app', 'x', 'would_terminate', 'dryRun')",
                [at],
            )
            .expect("event");
        }
        assert_eq!(collect(&conn, &seoul, now).expect("collect").attempts_today, 2);
    }
}
