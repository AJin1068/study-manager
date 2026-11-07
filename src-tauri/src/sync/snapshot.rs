//! The plan snapshot the server mirrors: every occurrence in a window around today, repeats and
//! exceptions already applied by the desktop (the server never interprets a repeat rule).
//!
//! `build_snapshot` is a pure function of its inputs. The snapshot hash covers the occurrences
//! only (not the generation time), so an unchanged plan produces the same hash and is not sent
//! again.

use std::collections::HashSet;

use chrono::{Duration, TimeZone};

use crate::error::{AppError, AppResult};
use crate::plan::{self, ExceptionKind, ModeLookup, Plan, PlanException};
use crate::remote::payload::OccurrenceRow;

/// Days ahead of today that the mirror covers.
pub const HORIZON_DAYS: i64 = 14;
/// Days behind today that stay in the mirror: a plan that started shortly before midnight is
/// still inside the server's 30-minute alert window after midnight.
const LOOKBACK_DAYS: i64 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub hash: String,
    pub generated_at_ms: i64,
    pub horizon_days: i64,
    pub occurrences: Vec<OccurrenceRow>,
}

/// A "skip today" release: occurrences on `date` that end after `at_ms` were skipped by it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkipMark {
    pub date: String,
    pub at_ms: i64,
}

/// FNV-1a, 64 bit: small, stable across Rust versions, and enough for change detection (the
/// server compares the string for equality, nothing trusts it).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn hash_occurrences(rows: &[OccurrenceRow]) -> AppResult<String> {
    let canonical = serde_json::to_string(rows)
        .map_err(|e| AppError::Internal(format!("snapshot encode: {e}")))?;
    Ok(format!(
        "v1-{:016x}-{}",
        fnv1a(canonical.as_bytes()),
        rows.len()
    ))
}

struct Collector {
    rows: Vec<OccurrenceRow>,
    keys: HashSet<String>,
}

impl Collector {
    /// Keys are `s<series>:<original date>[:from]`: stable when a series is split into several
    /// plan rows, which is what keeps one alert per occurrence. A clash (should not happen) gets
    /// the plan id appended instead of being sent, because the server rejects a snapshot with a
    /// duplicate key as a whole.
    fn push(
        &mut self,
        plan: &Plan,
        key: String,
        start_at_utc_ms: i64,
        sets: i64,
        status: &'static str,
    ) {
        let mut key = key;
        if !self.keys.insert(key.clone()) {
            key = format!("{key}:p{}", plan.id);
            if !self.keys.insert(key.clone()) {
                return;
            }
        }
        self.rows.push(OccurrenceRow {
            occurrence_key: key,
            start_at_utc_ms,
            plan_name: plan.name.clone(),
            subject: plan.subject.clone(),
            sets,
            status,
        });
    }
}

pub fn build_snapshot<Tz: TimeZone>(
    tz: &Tz,
    plans: &[Plan],
    exceptions: &[PlanException],
    modes: &ModeLookup,
    skips: &[SkipMark],
    now_ms: i64,
    horizon_days: i64,
) -> AppResult<Snapshot> {
    let today = tz
        .timestamp_millis_opt(now_ms)
        .single()
        .map(|dt| dt.date_naive())
        .ok_or_else(|| AppError::Internal("timestamp has no local date".into()))?;
    let first = today - Duration::days(LOOKBACK_DAYS);
    let last = today + Duration::days(horizon_days.max(1) - 1);
    let mut out = Collector {
        rows: Vec::new(),
        keys: HashSet::new(),
    };

    let mut day = first;
    while day <= last {
        for occurrence in plan::occurrences_on(tz, plans, exceptions, modes, day)? {
            let skipped = skips
                .iter()
                .any(|s| s.date == occurrence.date && occurrence.planned_end_ms > s.at_ms);
            let Some(owner) = plans.iter().find(|p| p.id == occurrence.plan_id) else {
                continue;
            };
            out.push(
                owner,
                format!("s{}:{}", occurrence.series_id, occurrence.source_date),
                occurrence.planned_start_ms,
                occurrence.sets,
                if skipped { "skipped" } else { "planned" },
            );
        }
        day += Duration::days(1);
    }

    // A cancelled or moved-away slot is sent too, as itself: it tells the server "nothing is due
    // here", which is what keeps it from alerting about a slot the user gave up.
    for exception in exceptions {
        let Some(owner) = plans
            .iter()
            .find(|p| p.id == exception.plan_id && p.deleted_at_ms.is_none())
        else {
            continue;
        };
        let date = plan::parse_date(&exception.date)?;
        if date < first || date > last || !plan::is_base_date(owner, date)? {
            continue;
        }
        let start = plan::local_to_ms(tz, date, plan::parse_time(&owner.start_time)?)?;
        match exception.kind {
            ExceptionKind::Cancelled => out.push(
                owner,
                format!("s{}:{}", owner.series_id, exception.date),
                start,
                owner.sets,
                "canceled",
            ),
            ExceptionKind::Moved => out.push(
                owner,
                format!("s{}:{}:from", owner.series_id, exception.date),
                start,
                owner.sets,
                "moved",
            ),
        }
    }

    out.rows.sort_by(|a, b| {
        (a.start_at_utc_ms, &a.occurrence_key).cmp(&(b.start_at_utc_ms, &b.occurrence_key))
    });
    let hash = hash_occurrences(&out.rows)?;
    Ok(Snapshot {
        hash,
        generated_at_ms: now_ms,
        horizon_days,
        occurrences: out.rows,
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use chrono_tz::{Asia::Seoul, Tz};

    use super::*;
    use crate::db::open_in_memory;

    fn utc_ms(y: i32, m: u32, d: u32, h: u32, min: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .expect("valid time")
            .timestamp_millis()
    }

    fn weekly(id: i64, series: i64, name: &str) -> Plan {
        Plan {
            id,
            series_id: series,
            name: name.into(),
            subject: "math".into(),
            start_date: "2026-06-01".into(),
            start_time: "20:00".into(),
            // Monday and Wednesday.
            sets: 2,
            repeat_weekdays: Some(0b000_0101),
            repeat_until: None,
            effective_from_date: "2026-06-01".into(),
            created_at_ms: 0,
            deleted_at_ms: None,
        }
    }

    fn modes() -> ModeLookup {
        ModeLookup::load(&open_in_memory().expect("db")).expect("modes")
    }

    fn build(
        tz: &Tz,
        plans: &[Plan],
        exceptions: &[PlanException],
        skips: &[SkipMark],
        now: i64,
    ) -> Snapshot {
        build_snapshot(tz, plans, exceptions, &modes(), skips, now, HORIZON_DAYS).expect("snapshot")
    }

    fn keys(snapshot: &Snapshot) -> Vec<(&str, &str)> {
        snapshot
            .occurrences
            .iter()
            .map(|o| (o.occurrence_key.as_str(), o.status))
            .collect()
    }

    // Monday 2026-06-01 12:00 KST. Window: 2026-05-31 ..= 2026-06-13 (today - 1 day, + 14 days).
    fn now() -> i64 {
        utc_ms(2026, 6, 1, 3, 0)
    }

    #[test]
    fn weekly_plan_is_expanded_over_the_horizon_with_utc_starts() {
        let snapshot = build(&Seoul, &[weekly(1, 1, "Math")], &[], &[], now());
        assert_eq!(
            keys(&snapshot),
            [
                ("s1:2026-06-01", "planned"),
                ("s1:2026-06-03", "planned"),
                ("s1:2026-06-08", "planned"),
                ("s1:2026-06-10", "planned")
            ]
        );
        // 20:00 Asia/Seoul is 11:00 UTC.
        assert_eq!(
            snapshot.occurrences[0].start_at_utc_ms,
            utc_ms(2026, 6, 1, 11, 0)
        );
        assert_eq!(
            snapshot.occurrences[3].start_at_utc_ms,
            utc_ms(2026, 6, 10, 11, 0)
        );
        assert_eq!(
            (snapshot.horizon_days, snapshot.generated_at_ms),
            (14, now())
        );
        assert!(
            snapshot
                .occurrences
                .iter()
                .all(|o| o.sets == 2 && o.plan_name == "Math")
        );
    }

    #[test]
    fn moved_cancelled_and_skipped_occurrences_carry_their_own_status() {
        let exceptions = [
            PlanException {
                plan_id: 1,
                date: "2026-06-03".into(),
                kind: ExceptionKind::Moved,
                new_date: Some("2026-06-04".into()),
                new_time: Some("21:30".into()),
                sets_override: Some(3),
            },
            PlanException {
                plan_id: 1,
                date: "2026-06-08".into(),
                kind: ExceptionKind::Cancelled,
                new_date: None,
                new_time: None,
                sets_override: None,
            },
        ];
        // "Skip today" on 2026-06-10, granted before the plan starts.
        let skips = [SkipMark {
            date: "2026-06-10".into(),
            at_ms: utc_ms(2026, 6, 10, 2, 0),
        }];
        let snapshot = build(&Seoul, &[weekly(1, 1, "Math")], &exceptions, &skips, now());
        assert_eq!(
            keys(&snapshot),
            [
                ("s1:2026-06-01", "planned"),
                ("s1:2026-06-03:from", "moved"),
                ("s1:2026-06-03", "planned"),
                ("s1:2026-06-08", "canceled"),
                ("s1:2026-06-10", "skipped"),
            ]
        );
        let live = snapshot
            .occurrences
            .iter()
            .find(|o| o.occurrence_key == "s1:2026-06-03")
            .expect("moved-to row");
        assert_eq!(
            (live.start_at_utc_ms, live.sets),
            (utc_ms(2026, 6, 4, 12, 30), 3),
            "the new slot is the live one"
        );
        let vacated = snapshot
            .occurrences
            .iter()
            .find(|o| o.occurrence_key == "s1:2026-06-03:from")
            .expect("vacated");
        assert_eq!(vacated.start_at_utc_ms, utc_ms(2026, 6, 3, 11, 0));
    }

    #[test]
    fn a_skip_only_covers_occurrences_that_end_after_it_and_only_that_day() {
        // Skip granted after the Monday plan ended: it must not retroactively skip it.
        let late = [SkipMark {
            date: "2026-06-01".into(),
            at_ms: utc_ms(2026, 6, 1, 23, 59),
        }];
        let snapshot = build(&Seoul, &[weekly(1, 1, "Math")], &[], &late, now());
        assert_eq!(snapshot.occurrences[0].status, "planned");
        assert_eq!(
            snapshot.occurrences[1].status, "planned",
            "Wednesday is another day"
        );
    }

    #[test]
    fn deleted_plans_and_exceptions_of_missing_plans_are_not_sent() {
        let mut gone = weekly(2, 2, "Old");
        gone.deleted_at_ms = Some(5);
        let orphan = PlanException {
            plan_id: 99,
            date: "2026-06-03".into(),
            kind: ExceptionKind::Cancelled,
            new_date: None,
            new_time: None,
            sets_override: None,
        };
        let snapshot = build(&Seoul, &[gone], &[orphan], &[], now());
        assert!(snapshot.occurrences.is_empty());
    }

    #[test]
    fn hash_ignores_generation_time_and_changes_with_content_and_timezone() {
        let plans = [weekly(1, 1, "Math")];
        let a = build(&Seoul, &plans, &[], &[], now());
        let later = build(&Seoul, &plans, &[], &[], now() + 60_000);
        assert_eq!(
            a.hash, later.hash,
            "same plan, same hash, even a minute later"
        );
        assert_ne!(a.generated_at_ms, later.generated_at_ms);

        let mut edited = plans.clone();
        edited[0].start_time = "20:30".into();
        assert_ne!(a.hash, build(&Seoul, &edited, &[], &[], now()).hash);

        // The same wall-clock plan read in UTC starts at a different instant, so the hash differs.
        let utc: Tz = chrono_tz::UTC;
        let in_utc = build(&utc, &plans, &[], &[], now());
        assert_ne!(a.hash, in_utc.hash);
        assert_eq!(
            in_utc.occurrences[0].start_at_utc_ms,
            utc_ms(2026, 6, 1, 20, 0)
        );
    }

    #[test]
    fn a_split_series_keeps_one_key_per_original_date() {
        // Series 1 was split on 2026-06-08: the old row ends before it, the new row (id 2) starts there.
        let mut old = weekly(1, 1, "Math");
        old.repeat_until = Some("2026-06-07".into());
        let mut new = weekly(2, 1, "Math");
        new.start_date = "2026-06-08".into();
        new.effective_from_date = "2026-06-08".into();
        let snapshot = build(&Seoul, &[old, new], &[], &[], now());
        let all: Vec<&str> = snapshot
            .occurrences
            .iter()
            .map(|o| o.occurrence_key.as_str())
            .collect();
        assert_eq!(
            all,
            [
                "s1:2026-06-01",
                "s1:2026-06-03",
                "s1:2026-06-08",
                "s1:2026-06-10"
            ]
        );
    }

    #[test]
    fn midnight_lookback_keeps_yesterdays_late_plan() {
        let mut late = weekly(1, 1, "Night");
        late.start_time = "23:50".into();
        // Tuesday 00:10 KST: Monday's 23:50 plan is 20 minutes old and must still be in the mirror.
        let snapshot = build(&Seoul, &[late], &[], &[], utc_ms(2026, 6, 1, 15, 10));
        assert_eq!(snapshot.occurrences[0].occurrence_key, "s1:2026-06-01");
    }

    #[test]
    fn serialized_rows_carry_no_forbidden_fields() {
        let snapshot = build(&Seoul, &[weekly(1, 1, "Math")], &[], &[], now());
        let text = serde_json::to_string(&snapshot.occurrences).expect("json");
        for forbidden in ["window_title", "\"url\"", "\"title\"", "typed"] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
    }
}
