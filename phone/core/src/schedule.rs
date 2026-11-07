//! What the schedule, timer and diagnostics screens show, as pure functions of values the caller
//! passes in. Nothing here reads a clock: the phone supplies its own monotonic readings and its
//! local day bounds. The phone only displays; it has no way to write a plan.
//!
//! The rows are `plan_occurrences` rows (`occurrence_key`, `start_at_utc_ms`, `plan_name`,
//! `subject`, `sets`, `status`). The phone's RLS grants (`fixtures/phone/rls-matrix.json`) cover
//! only the `phone_*` tables, so there is NO server path that hands a paired phone these rows
//! yet: the shell shows an empty schedule with that reason until one exists.

use link_protocol::{GRACE_MS, LeaseHolder, PhoneLock};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct OccurrenceRow {
    pub occurrence_key: String,
    pub start_at_utc_ms: i64,
    pub plan_name: String,
    pub subject: String,
    pub sets: i64,
    pub status: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Planned,
    /// Skipped by "skip today": shown struck through.
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleItem {
    pub occurrence_key: String,
    pub start_ms: i64,
    pub plan_name: String,
    pub subject: String,
    pub sets: i64,
    pub status: ItemStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhoneSchedule {
    pub items: Vec<ScheduleItem>,
}

impl PhoneSchedule {
    /// The rows that start in `[day_start_ms, day_end_ms)`, earliest first. A cancelled slot and
    /// the origin of a moved one say "nothing is due here"; a status this code does not know is
    /// dropped rather than guessed.
    pub fn from_rows(rows: &[OccurrenceRow], day_start_ms: i64, day_end_ms: i64) -> Self {
        let mut items: Vec<ScheduleItem> = rows
            .iter()
            .filter(|row| row.start_at_utc_ms >= day_start_ms && row.start_at_utc_ms < day_end_ms)
            .filter_map(|row| {
                let status = match row.status.as_str() {
                    "planned" => ItemStatus::Planned,
                    "skipped" => ItemStatus::Skipped,
                    _ => return None,
                };
                Some(ScheduleItem {
                    occurrence_key: row.occurrence_key.clone(),
                    start_ms: row.start_at_utc_ms,
                    plan_name: row.plan_name.clone(),
                    subject: row.subject.clone(),
                    sets: row.sets,
                    status,
                })
            })
            .collect();
        items.sort_by(|a, b| (a.start_ms, &a.occurrence_key).cmp(&(b.start_ms, &b.occurrence_key)));
        Self { items }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TimerDisplay {
    /// Locked: this long until the study segment ends.
    Studying {
        remaining_ms: i64,
    },
    Free,
}

/// The lease runs [`GRACE_MS`] past the end of the segment; the countdown shows the segment.
pub fn timer_display(lock_remaining_ms: Option<i64>) -> TimerDisplay {
    match lock_remaining_ms {
        Some(remaining) => TimerDisplay::Studying {
            remaining_ms: (remaining - GRACE_MS).max(0),
        },
        None => TimerDisplay::Free,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseDiagnostics {
    pub lock: PhoneLock,
    pub seq: Option<i64>,
    /// Until the phone frees itself if nothing renews the lease; `None` when not locked.
    pub lock_remaining_ms: Option<i64>,
    pub timer: TimerDisplay,
    /// How long ago the server last answered; `None` when it never did.
    pub last_server_contact_ago_ms: Option<i64>,
    /// Leases refused so far (stale, repeated or invalid).
    pub rejected_leases: usize,
}

pub fn diagnose(
    holder: &LeaseHolder,
    mono_now_ms: i64,
    last_contact_mono_ms: Option<i64>,
) -> LeaseDiagnostics {
    let lock_remaining_ms = holder.remaining_ms(mono_now_ms);
    LeaseDiagnostics {
        lock: holder.decide(mono_now_ms),
        seq: holder.current_seq(),
        lock_remaining_ms,
        timer: timer_display(lock_remaining_ms),
        last_server_contact_ago_ms: last_contact_mono_ms
            .map(|at| mono_now_ms.saturating_sub(at).max(0)),
        rejected_leases: usize::try_from(holder.rejected_total()).unwrap_or(usize::MAX),
    }
}

#[cfg(test)]
mod tests {
    use link_protocol::{FreeReason, LockLease, ReleaseCause};

    use super::*;

    const DAY: i64 = 86_400_000;
    const H: i64 = 3_600_000;
    const TODAY: i64 = 1_793_923_200_000;

    fn row(key: &str, start: i64, status: &str) -> OccurrenceRow {
        OccurrenceRow {
            occurrence_key: key.into(),
            start_at_utc_ms: start,
            plan_name: format!("plan {key}"),
            subject: "math".into(),
            sets: 3,
            status: status.into(),
        }
    }

    #[test]
    fn the_schedule_is_the_days_slice_in_start_order_with_half_open_bounds() {
        let rows = [
            row("late", TODAY + 20 * H, "planned"),
            row("yesterday", TODAY - H, "planned"),
            row("early", TODAY + 9 * H, "planned"),
            row("tomorrow", TODAY + DAY, "planned"),
            row("done-for-today", TODAY + 12 * H, "skipped"),
            row("gone", TODAY + 13 * H, "canceled"),
            row("moved-away", TODAY + 14 * H, "moved"),
            row("start", TODAY, "planned"),
        ];
        let schedule = PhoneSchedule::from_rows(&rows, TODAY, TODAY + DAY);
        let keys: Vec<&str> = schedule
            .items
            .iter()
            .map(|i| i.occurrence_key.as_str())
            .collect();
        assert_eq!(keys, ["start", "early", "done-for-today", "late"]);
        assert_eq!(schedule.items[2].status, ItemStatus::Skipped);
        assert!(
            PhoneSchedule::from_rows(&[], TODAY, TODAY + DAY)
                .items
                .is_empty()
        );
    }

    #[test]
    fn the_timer_counts_down_to_the_end_of_the_segment_not_the_grace() {
        assert_eq!(
            timer_display(Some(25 * 60_000 + GRACE_MS)),
            TimerDisplay::Studying {
                remaining_ms: 25 * 60_000
            }
        );
        assert_eq!(
            timer_display(Some(GRACE_MS - 1)),
            TimerDisplay::Studying { remaining_ms: 0 }
        );
        assert_eq!(timer_display(None), TimerDisplay::Free);
    }

    #[test]
    fn diagnostics_say_when_the_lock_frees_itself_and_when_the_server_last_answered() {
        let mut holder = LeaseHolder::new();
        let now = 10 * H;
        holder
            .offer(
                LockLease::locked("12:34", 5, now, now + 30 * 60_000).expect("lease"),
                now,
                1_000,
            )
            .expect("offer");
        let d = diagnose(&holder, 61_000, Some(41_000));
        assert_eq!(d.lock, PhoneLock::Locked);
        assert_eq!((d.seq, d.rejected_leases), (Some(5), 0));
        assert_eq!(d.lock_remaining_ms, Some(30 * 60_000 + GRACE_MS - 60_000));
        assert_eq!(
            d.timer,
            TimerDisplay::Studying {
                remaining_ms: 30 * 60_000 - 60_000
            }
        );
        assert_eq!(d.last_server_contact_ago_ms, Some(20_000));

        assert!(
            holder
                .offer(
                    LockLease::locked("12:34", 5, now, now + 60_000).expect("lease"),
                    now,
                    2_000
                )
                .is_err()
        );
        assert_eq!(diagnose(&holder, 61_000, None).rejected_leases, 1);
        assert_eq!(
            diagnose(&holder, 61_000, None).last_server_contact_ago_ms,
            None
        );

        holder
            .offer(
                LockLease::free("12:34", 6, now, ReleaseCause::EmergencyPass).expect("lease"),
                now,
                3_000,
            )
            .expect("offer");
        let free = diagnose(&holder, 4_000, Some(3_000));
        assert_eq!(
            free.lock,
            PhoneLock::Free {
                reason: FreeReason::EmergencyPass
            }
        );
        assert_eq!(
            (free.lock_remaining_ms, free.timer),
            (None, TimerDisplay::Free)
        );
    }
}
