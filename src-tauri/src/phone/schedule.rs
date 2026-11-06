//! What the phone's common screen shows, derived by pure functions: today's schedule (from the plan
//! snapshot the desktop app mirrors), the countdown of the running segment and the lease
//! diagnostics (time until the lock frees itself, last contact with the server). Nothing here
//! reads a clock: the phone passes in its own monotonic readings and its local day bounds.
//!
//! The phone only receives what the snapshot already holds (plan name, subject, start, sets,
//! status). It has no way to write a plan.

use link_protocol::{LeaseHolder, PhoneLock, GRACE_MS};
use serde::{Deserialize, Serialize};

use crate::sync::snapshot::Snapshot;

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

/// Today's schedule as the phone shows it, earliest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhoneSchedule {
    pub generated_at_ms: i64,
    pub items: Vec<ScheduleItem>,
}

impl PhoneSchedule {
    /// The items of `snapshot` that start in `[day_start_ms, day_end_ms)` (the phone's local day).
    /// A cancelled slot and the origin of a moved one say "nothing is due here", so they are not
    /// shown; a status this code does not know is dropped rather than guessed.
    pub fn from_snapshot(snapshot: &Snapshot, day_start_ms: i64, day_end_ms: i64) -> Self {
        let mut items: Vec<ScheduleItem> = snapshot
            .occurrences
            .iter()
            .filter(|row| row.start_at_utc_ms >= day_start_ms && row.start_at_utc_ms < day_end_ms)
            .filter_map(|row| {
                let status = match row.status {
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
        Self { generated_at_ms: snapshot.generated_at_ms, items }
    }

    /// The first planned item that starts after `now_ms`.
    pub fn next_after(&self, now_ms: i64) -> Option<&ScheduleItem> {
        self.items.iter().find(|item| item.status == ItemStatus::Planned && item.start_ms > now_ms)
    }
}

/// The big timer of the phone's screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TimerDisplay {
    /// Locked: this long until the study segment ends.
    Studying { remaining_ms: i64 },
    Free,
}

/// The lease runs [`GRACE_MS`] past the end of the segment; the countdown shows the segment.
pub fn timer_display(lock_remaining_ms: Option<i64>) -> TimerDisplay {
    match lock_remaining_ms {
        Some(remaining) => TimerDisplay::Studying { remaining_ms: (remaining - GRACE_MS).max(0) },
        None => TimerDisplay::Free,
    }
}

/// The phone's diagnostics screen.
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

pub fn diagnose(holder: &LeaseHolder, mono_now_ms: i64, last_contact_mono_ms: Option<i64>) -> LeaseDiagnostics {
    let lock_remaining_ms = holder.remaining_ms(mono_now_ms);
    LeaseDiagnostics {
        lock: holder.decide(mono_now_ms),
        seq: holder.current_seq(),
        lock_remaining_ms,
        timer: timer_display(lock_remaining_ms),
        last_server_contact_ago_ms: last_contact_mono_ms.map(|at| mono_now_ms.saturating_sub(at).max(0)),
        rejected_leases: usize::try_from(holder.rejected_total()).unwrap_or(usize::MAX),
    }
}

#[cfg(test)]
mod tests {
    use link_protocol::{FreeReason, LockLease, ReleaseCause};

    use super::*;
    use crate::remote::payload::OccurrenceRow;
    use crate::sync::snapshot::hash_occurrences;

    const DAY: i64 = 86_400_000;
    const H: i64 = 3_600_000;
    const TODAY: i64 = 1_793_923_200_000; // a day start

    fn row(key: &str, start: i64, status: &'static str) -> OccurrenceRow {
        OccurrenceRow { occurrence_key: key.into(), start_at_utc_ms: start, plan_name: format!("plan {key}"), subject: "수학".into(), sets: 3, status }
    }

    fn snapshot(rows: Vec<OccurrenceRow>) -> Snapshot {
        Snapshot { hash: hash_occurrences(&rows).expect("hash"), generated_at_ms: TODAY + H, horizon_days: 14, occurrences: rows }
    }

    #[test]
    fn the_schedule_is_the_days_slice_of_the_snapshot_in_start_order() {
        let snap = snapshot(vec![
            row("late", TODAY + 20 * H, "planned"),
            row("yesterday", TODAY - H, "planned"),
            row("early", TODAY + 9 * H, "planned"),
            row("tomorrow", TODAY + DAY, "planned"),
            row("done-for-today", TODAY + 12 * H, "skipped"),
            row("gone", TODAY + 13 * H, "canceled"),
            row("moved-away", TODAY + 14 * H, "moved"),
        ]);
        let schedule = PhoneSchedule::from_snapshot(&snap, TODAY, TODAY + DAY);
        let keys: Vec<&str> = schedule.items.iter().map(|i| i.occurrence_key.as_str()).collect();
        assert_eq!(keys, ["early", "done-for-today", "late"]);
        assert_eq!(schedule.items[1].status, ItemStatus::Skipped);
        assert_eq!(schedule.generated_at_ms, snap.generated_at_ms);
        // The day bounds are half-open: a slot exactly at the end belongs to the next day.
        let edge = snapshot(vec![row("midnight", TODAY + DAY, "planned"), row("start", TODAY, "planned")]);
        let day = PhoneSchedule::from_snapshot(&edge, TODAY, TODAY + DAY);
        assert_eq!(day.items.iter().map(|i| i.occurrence_key.as_str()).collect::<Vec<_>>(), ["start"]);
    }

    #[test]
    fn the_next_item_is_the_first_planned_one_after_now() {
        let snap = snapshot(vec![row("a", TODAY + 9 * H, "planned"), row("b", TODAY + 12 * H, "skipped"), row("c", TODAY + 15 * H, "planned")]);
        let schedule = PhoneSchedule::from_snapshot(&snap, TODAY, TODAY + DAY);
        assert_eq!(schedule.next_after(TODAY).map(|i| i.occurrence_key.as_str()), Some("a"));
        assert_eq!(schedule.next_after(TODAY + 9 * H).map(|i| i.occurrence_key.as_str()), Some("c"), "a skipped slot is never next");
        assert_eq!(schedule.next_after(TODAY + 15 * H), None);
    }

    #[test]
    fn the_schedule_round_trips_and_holds_nothing_but_the_plan_fields() {
        let schedule = PhoneSchedule::from_snapshot(&snapshot(vec![row("a", TODAY + H, "planned")]), TODAY, TODAY + DAY);
        let json = serde_json::to_value(&schedule).expect("json");
        let item = json["items"][0].as_object().expect("item");
        let mut keys: Vec<&str> = item.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["occurrence_key", "plan_name", "sets", "start_ms", "status", "subject"]);
        assert_eq!(serde_json::from_value::<PhoneSchedule>(json).expect("back"), schedule);
    }

    #[test]
    fn the_timer_counts_down_to_the_end_of_the_segment_not_the_grace() {
        assert_eq!(timer_display(Some(25 * 60_000 + GRACE_MS)), TimerDisplay::Studying { remaining_ms: 25 * 60_000 });
        assert_eq!(timer_display(Some(GRACE_MS - 1)), TimerDisplay::Studying { remaining_ms: 0 }, "the grace is not shown");
        assert_eq!(timer_display(None), TimerDisplay::Free);
    }

    #[test]
    fn diagnostics_say_when_the_lock_frees_itself_and_when_the_server_last_answered() {
        let mut holder = LeaseHolder::new();
        let now = 10 * H;
        holder.offer(LockLease::locked("12:34", 5, now, now + 30 * 60_000).expect("lease"), now, 1_000).expect("offer");
        let d = diagnose(&holder, 61_000, Some(41_000));
        assert_eq!(d.lock, PhoneLock::Locked);
        assert_eq!((d.seq, d.rejected_leases), (Some(5), 0));
        assert_eq!(d.lock_remaining_ms, Some(30 * 60_000 + GRACE_MS - 60_000));
        assert_eq!(d.timer, TimerDisplay::Studying { remaining_ms: 30 * 60_000 - 60_000 });
        assert_eq!(d.last_server_contact_ago_ms, Some(20_000));

        // A replayed lease is counted for the screen and changes nothing.
        assert!(holder.offer(LockLease::locked("12:34", 5, now, now + 60_000).expect("lease"), now, 2_000).is_err());
        assert_eq!(diagnose(&holder, 61_000, None).rejected_leases, 1);
        assert_eq!(diagnose(&holder, 61_000, None).last_server_contact_ago_ms, None);

        holder.offer(LockLease::free("12:34", 6, now, ReleaseCause::EmergencyPass).expect("lease"), now, 3_000).expect("offer");
        let free = diagnose(&holder, 4_000, Some(3_000));
        assert_eq!(free.lock, PhoneLock::Free { reason: FreeReason::EmergencyPass });
        assert_eq!((free.lock_remaining_ms, free.timer), (None, TimerDisplay::Free));
    }
}
