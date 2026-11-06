//! Ranking arithmetic and the viewer's local dates. Pure functions over what the server returned.
//!
//! Trust (O-21): minutes are what each PC reported; the server only bounds them (1440 a day, no
//! sudden jumps). A "day" is the reporting member's own local calendar date, so the viewer's
//! "today" is the viewer's local date and a member in another time zone is ranked by the date
//! label they reported. The week is the viewer's local Monday-to-Sunday week up to today.

use chrono::{Datelike, Duration, TimeZone};

use super::model::{RankRow, WireDay};
use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalDates {
    /// `YYYY-MM-DD`.
    pub today: String,
    /// The Monday of the week that holds `today`.
    pub week_start: String,
}

pub fn local_dates<Tz: TimeZone>(tz: &Tz, now_ms: i64) -> AppResult<LocalDates> {
    let today = tz
        .timestamp_millis_opt(now_ms)
        .single()
        .ok_or_else(|| AppError::Internal("timestamp has no local date".into()))?
        .date_naive();
    let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
    Ok(LocalDates { today: today.format("%Y-%m-%d").to_string(), week_start: monday.format("%Y-%m-%d").to_string() })
}

/// Minutes reported for exactly `day` (0 when the member reported nothing that day).
pub fn minutes_on(days: &[WireDay], day: &str) -> i64 {
    days.iter().filter(|d| d.day == day).map(|d| d.study_min).sum()
}

/// Minutes on days from `week_start` through `today`, both inclusive. ISO dates order as text.
pub fn minutes_in_week(days: &[WireDay], week_start: &str, today: &str) -> i64 {
    days.iter().filter(|d| d.day.as_str() >= week_start && d.day.as_str() <= today).map(|d| d.study_min).sum()
}

/// `(user id, nickname, is self, minutes or None when not shared)` -> rows ordered by minutes
/// descending, then nickname, then id. Ties share a rank by competition ranking (1, 1, 3), and
/// members who do not share come last without a rank.
pub fn rank_rows(entries: Vec<(String, String, bool, Option<i64>)>) -> Vec<RankRow> {
    let mut entries = entries;
    entries.sort_by(|a, b| {
        b.3.is_some()
            .cmp(&a.3.is_some())
            .then_with(|| b.3.cmp(&a.3))
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut rows: Vec<RankRow> = Vec::with_capacity(entries.len());
    let mut previous: Option<(i64, u32)> = None;
    for (index, (user_id, nickname, is_self, minutes)) in entries.into_iter().enumerate() {
        let rank = minutes.map(|m| {
            let position = u32::try_from(index + 1).unwrap_or(u32::MAX);
            match previous {
                Some((prev_minutes, prev_rank)) if prev_minutes == m => prev_rank,
                _ => position,
            }
        });
        if let (Some(m), Some(r)) = (minutes, rank) {
            previous = Some((m, r));
        }
        rows.push(RankRow { user_id, nickname, is_self, minutes, rank });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn day(day: &str, study_min: i64) -> WireDay {
        WireDay { day: day.into(), study_min, plan_min: 0 }
    }

    fn entry(id: &str, minutes: Option<i64>) -> (String, String, bool, Option<i64>) {
        (id.into(), id.to_uppercase(), false, minutes)
    }

    #[test]
    fn local_dates_follow_the_viewers_time_zone_across_midnight() {
        // 2026-11-06 (Friday) 23:30 in Seoul is 14:30 UTC; one hour later it is already Saturday there.
        let seoul = FixedOffset::east_opt(9 * 3600).expect("offset");
        let utc = FixedOffset::east_opt(0).expect("offset");
        let before = 1_793_975_400_000; // 2026-11-06T14:30:00Z
        assert_eq!(local_dates(&seoul, before).expect("dates").today, "2026-11-06");
        assert_eq!(local_dates(&seoul, before + 3_600_000).expect("dates").today, "2026-11-07");
        assert_eq!(local_dates(&utc, before + 3_600_000).expect("dates").today, "2026-11-06");
        assert_eq!(local_dates(&seoul, before).expect("dates").week_start, "2026-11-02", "Monday");
    }

    #[test]
    fn the_week_starts_on_monday_and_a_sunday_still_belongs_to_the_week_that_began_six_days_before() {
        let utc = FixedOffset::east_opt(0).expect("offset");
        let sunday = 1_794_139_200_000; // 2026-11-08T12:00:00Z
        let dates = local_dates(&utc, sunday).expect("dates");
        assert_eq!((dates.today.as_str(), dates.week_start.as_str()), ("2026-11-08", "2026-11-02"));
        let monday = sunday + 86_400_000; // 2026-11-09T12:00:00Z
        assert_eq!(local_dates(&utc, monday).expect("dates").week_start, "2026-11-09");
    }

    #[test]
    fn minutes_are_summed_inside_the_inclusive_week_window_only() {
        let days = [day("2026-11-01", 100), day("2026-11-02", 30), day("2026-11-04", 45), day("2026-11-06", 20), day("2026-11-07", 99)];
        assert_eq!(minutes_on(&days, "2026-11-06"), 20);
        assert_eq!(minutes_on(&days, "2026-11-05"), 0);
        assert_eq!(minutes_in_week(&days, "2026-11-02", "2026-11-06"), 95, "Sunday before and Saturday after are outside");
    }

    #[test]
    fn ties_share_a_rank_and_the_next_rank_skips() {
        let rows = rank_rows(vec![entry("c", Some(30)), entry("a", Some(60)), entry("b", Some(60)), entry("d", Some(10))]);
        let got: Vec<(&str, Option<u32>)> = rows.iter().map(|r| (r.user_id.as_str(), r.rank)).collect();
        assert_eq!(got, [("a", Some(1)), ("b", Some(1)), ("c", Some(3)), ("d", Some(4))]);
    }

    #[test]
    fn members_who_do_not_share_are_listed_last_without_a_rank() {
        let rows = rank_rows(vec![entry("h", None), entry("a", Some(0)), entry("b", Some(5))]);
        let got: Vec<(&str, Option<u32>)> = rows.iter().map(|r| (r.user_id.as_str(), r.rank)).collect();
        assert_eq!(got, [("b", Some(1)), ("a", Some(2)), ("h", None)]);
    }

    #[test]
    fn zero_minutes_still_rank_and_tie_at_zero() {
        let rows = rank_rows(vec![entry("a", Some(0)), entry("b", Some(0))]);
        assert_eq!(rows.iter().map(|r| r.rank).collect::<Vec<_>>(), [Some(1), Some(1)]);
    }
}
