//! Turns the server's room snapshot into what the screen draws: presence (with the 90-second
//! offline rule), the two rankings, invites, and the shared timetable in local time.
//! Pure: the clock, the time zone and the "already copied" set are inputs.

use std::collections::HashSet;

use chrono::TimeZone;

use super::model::{
    CharacterView, InviteView, MemberView, PresenceKind, PresenceView, RoomView, TogetherItem, WireSnapshot, WireStatus,
};
use super::ranking::{minutes_in_week, minutes_on, rank_rows, LocalDates};

/// A member whose last report is older than this is shown as offline.
pub const OFFLINE_AFTER_SEC: i64 = 90;

/// `elapsed_ms` is the time since the snapshot was fetched, so a value fetched a while ago ages
/// correctly while the screen waits for the next poll.
pub fn presence(status: Option<&WireStatus>, elapsed_ms: i64) -> PresenceView {
    let Some(status) = status else {
        return PresenceView { kind: PresenceKind::Hidden, remaining_sec: None };
    };
    let age = status.age_sec.max(0).saturating_add(elapsed_ms.max(0) / 1000);
    if age >= OFFLINE_AFTER_SEC {
        return PresenceView { kind: PresenceKind::Offline, remaining_sec: None };
    }
    let left = (status.remaining_sec - age).max(0);
    match status.state {
        super::model::MemberState::Studying => PresenceView { kind: PresenceKind::Studying, remaining_sec: Some(left) },
        super::model::MemberState::Break => PresenceView { kind: PresenceKind::Break, remaining_sec: Some(left) },
        super::model::MemberState::Idle => PresenceView { kind: PresenceKind::Idle, remaining_sec: None },
    }
}

pub struct ViewContext<'a, Tz: TimeZone> {
    pub tz: &'a Tz,
    pub dates: &'a LocalDates,
    pub fetched_at_ms: i64,
    pub now_ms: i64,
    /// Keys (UTC start as text) of timetable blocks already copied into this PC's plan.
    pub imported: &'a HashSet<String>,
}

pub fn schedule_key(start_at_utc_ms: i64) -> String {
    start_at_utc_ms.to_string()
}

pub fn build_room_view<Tz: TimeZone>(wire: &WireSnapshot, ctx: &ViewContext<'_, Tz>) -> RoomView {
    let elapsed = ctx.now_ms - ctx.fetched_at_ms;
    let members = wire
        .members
        .iter()
        .map(|m| MemberView {
            user_id: m.user_id.clone(),
            nickname: m.nickname.clone(),
            role: m.role,
            is_self: m.is_self,
            level: m.level,
            presence: presence(m.status.as_ref(), elapsed),
            attempts_today: m.attempts_today,
            character: m.character.clone().map(CharacterView::from),
        })
        .collect();

    let ranking = |minutes_of: &dyn Fn(&[super::model::WireDay]) -> i64| {
        rank_rows(
            wire.members
                .iter()
                .map(|m| (m.user_id.clone(), m.nickname.clone(), m.is_self, m.days.as_deref().map(minutes_of)))
                .collect(),
        )
    };
    let today_ranking = ranking(&|days| minutes_on(days, &ctx.dates.today));
    let week_ranking = ranking(&|days| minutes_in_week(days, &ctx.dates.week_start, &ctx.dates.today));

    let invites = wire
        .invites
        .iter()
        .map(|i| InviteView {
            invite_id: i.invite_id.clone(),
            expires_at_ms: i.expires_at_ms,
            max_uses: i.max_uses,
            uses: i.uses,
            revoked: i.revoked,
            expired: i.expires_at_ms <= wire.server_now_ms,
        })
        .collect();

    let schedule = wire
        .schedule
        .as_ref()
        .map(|s| {
            s.items
                .iter()
                .map(|item| {
                    let local = ctx.tz.timestamp_millis_opt(item.start_at_utc_ms).single();
                    let key = schedule_key(item.start_at_utc_ms);
                    TogetherItem {
                        already_imported: ctx.imported.contains(&key),
                        key,
                        name: item.name.clone(),
                        subject: item.subject.clone(),
                        sets: item.sets,
                        start_at_utc_ms: item.start_at_utc_ms,
                        local_date: local.as_ref().map(|d| d.date_naive().format("%Y-%m-%d").to_string()).unwrap_or_default(),
                        local_time: local.as_ref().map(|d| d.time().format("%H:%M").to_string()).unwrap_or_default(),
                        in_past: item.start_at_utc_ms < ctx.now_ms,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    RoomView {
        room_id: wire.room.room_id.clone(),
        name: wire.room.name.clone(),
        role: wire.room.role,
        capacity: wire.room.capacity,
        members,
        today_ranking,
        week_ranking,
        invites,
        schedule_updated_at_ms: wire.schedule.as_ref().map(|s| s.updated_at_ms),
        schedule,
        fetched_at_ms: ctx.fetched_at_ms,
    }
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;
    use crate::rooms::model::{MemberState, RoomRole, WireDay, WireMember, WireRoomInfo, WireSchedule, WireScheduleItem};

    fn status(state: MemberState, remaining_sec: i64, age_sec: i64) -> WireStatus {
        WireStatus { state, remaining_sec, age_sec }
    }

    #[test]
    fn a_report_older_than_ninety_seconds_is_offline_counting_the_time_since_the_fetch() {
        let fresh = status(MemberState::Studying, 1_920, 10);
        assert_eq!(presence(Some(&fresh), 0), PresenceView { kind: PresenceKind::Studying, remaining_sec: Some(1_910) });
        // 10 s old at the fetch + 79 s on the screen = 89 s: still online; one more second: offline.
        assert_eq!(presence(Some(&fresh), 79_000).kind, PresenceKind::Studying);
        assert_eq!(presence(Some(&fresh), 80_000).kind, PresenceKind::Offline);
        assert_eq!(presence(Some(&status(MemberState::Studying, 60, 90)), 0).kind, PresenceKind::Offline);
    }

    #[test]
    fn remaining_time_counts_down_and_never_goes_negative() {
        let s = status(MemberState::Break, 30, 0);
        assert_eq!(presence(Some(&s), 20_000).remaining_sec, Some(10));
        assert_eq!(presence(Some(&s), 40_000).remaining_sec, Some(0));
    }

    #[test]
    fn idle_has_no_remaining_time_and_a_private_state_is_hidden() {
        assert_eq!(presence(Some(&status(MemberState::Idle, 0, 5)), 0), PresenceView { kind: PresenceKind::Idle, remaining_sec: None });
        assert_eq!(presence(None, 0), PresenceView { kind: PresenceKind::Hidden, remaining_sec: None });
    }

    fn member(id: &str, is_self: bool, days: Option<Vec<WireDay>>) -> WireMember {
        WireMember {
            user_id: id.into(),
            nickname: id.to_uppercase(),
            role: RoomRole::Member,
            is_self,
            level: Some(2),
            status: Some(status(MemberState::Idle, 0, 0)),
            attempts_today: None,
            days,
            character: None,
        }
    }

    fn day(d: &str, m: i64) -> WireDay {
        WireDay { day: d.into(), study_min: m, plan_min: 60 }
    }

    #[test]
    fn rankings_use_the_viewers_dates_and_leave_out_members_who_do_not_share() {
        let wire = WireSnapshot {
            server_now_ms: 1_000,
            room: WireRoomInfo { room_id: "r".into(), name: "room".into(), capacity: 8, role: RoomRole::Member },
            members: vec![
                member("a", true, Some(vec![day("2026-11-05", 50), day("2026-11-06", 20)])),
                member("b", false, Some(vec![day("2026-11-06", 40)])),
                member("c", false, None),
            ],
            invites: vec![],
            schedule: Some(WireSchedule {
                items: vec![WireScheduleItem { start_at_utc_ms: 1_793_975_400_000, name: "수학".into(), subject: "math".into(), sets: 2 }],
                updated_at_ms: 5,
            }),
        };
        let dates = LocalDates { today: "2026-11-06".into(), week_start: "2026-11-02".into() };
        let imported = HashSet::from(["1793975400000".to_string()]);
        let seoul = FixedOffset::east_opt(9 * 3600).expect("offset");
        let ctx = ViewContext { tz: &seoul, dates: &dates, fetched_at_ms: 0, now_ms: 0, imported: &imported };
        let view = build_room_view(&wire, &ctx);

        let today: Vec<(&str, Option<i64>, Option<u32>)> = view.today_ranking.iter().map(|r| (r.user_id.as_str(), r.minutes, r.rank)).collect();
        assert_eq!(today, [("b", Some(40), Some(1)), ("a", Some(20), Some(2)), ("c", None, None)]);
        let week: Vec<(&str, Option<i64>)> = view.week_ranking.iter().map(|r| (r.user_id.as_str(), r.minutes)).collect();
        assert_eq!(week, [("a", Some(70)), ("b", Some(40)), ("c", None)]);

        let item = &view.schedule[0];
        assert_eq!((item.local_date.as_str(), item.local_time.as_str()), ("2026-11-06", "23:30"));
        assert!(item.already_imported);
        assert!(!item.in_past);
    }
}
