//! Multi-person scenarios on the in-memory server. They exercise the same adapter the app uses and
//! pin the behavior that the SQL in `supabase/migrations/*_s4_rooms.sql` must also have (the SQL
//! itself is only checked by pgTAP, which has not been run here).

use super::model::{RoomCode, RoomError};
use super::port::{RoomFeed, RoomReporter, RoomService};
use super::prefs::FriendSettings;
use super::testkit::{Person, World};
use crate::remote::fake::Caller;
use crate::remote::payload::{DailyReportArgs, StatusReportArgs};

fn rejected(code: RoomCode) -> RoomError {
    RoomError::Rejected(code)
}

fn invite(owner: &Person, room: &str) -> String {
    owner.rooms.create_invite(room, 24, 10).expect("invite").invite_code
}

fn open_all() -> FriendSettings {
    FriendSettings {
        share_level: true,
        share_status: true,
        share_week_time: true,
        share_attempts: true,
        distraction_publish: false,
        distraction_receive: false,
    }
}

fn status(level: i64, attempts: Option<i64>) -> StatusReportArgs {
    StatusReportArgs { state: "studying", remaining_sec: 600, level, attempts_today: attempts }
}

#[test]
fn a_user_may_own_five_rooms_and_the_sixth_is_refused() {
    let world = World::new();
    let a = world.person("a@example.test");
    for i in 0..5 {
        a.rooms.create_room(&format!("방{i}"), "가").expect("room");
    }
    assert_eq!(a.rooms.create_room("여섯", "가"), Err(rejected(RoomCode::RoomLimit)));
}

#[test]
fn the_server_keeps_only_a_hash_of_the_invite_and_a_wrong_code_reveals_nothing() {
    let world = World::new();
    let (a, b) = (world.person("a@example.test"), world.person("b@example.test"));
    let room = a.rooms.create_room("방", "가").expect("room");
    let code = invite(&a, &room);
    assert_eq!(world.backend.stored_invite_hashes(), 1);
    assert_eq!(b.rooms.join_room("ZZZZZZZZZZ", "나"), Err(rejected(RoomCode::InvalidCode)));
    // The plaintext never appears in anything the server stored or answered to a list call.
    let listed = format!("{:?}", a.rooms.list_rooms().expect("list"));
    assert!(!listed.contains(&code));
    assert_eq!(b.rooms.join_room(&code, "나"), Ok(room));
}

#[test]
fn join_attempts_are_limited_to_five_a_minute_and_failed_ones_count() {
    let world = World::new();
    let (a, b) = (world.person("a@example.test"), world.person("b@example.test"));
    let room = a.rooms.create_room("방", "가").expect("room");
    let code = invite(&a, &room);
    for _ in 0..5 {
        assert_eq!(b.rooms.join_room("AAAAAAAAAA", "나"), Err(rejected(RoomCode::InvalidCode)));
    }
    assert_eq!(b.rooms.join_room(&code, "나"), Err(rejected(RoomCode::RateLimited)), "even the right code waits");
    world.clock.advance(61_000);
    assert_eq!(b.rooms.join_room(&code, "나"), Ok(room));
}

#[test]
fn a_revoked_expired_or_used_up_invite_does_not_admit() {
    let world = World::new();
    let (a, b, c, d) = (world.person("a@example.test"), world.person("b@example.test"), world.person("c@example.test"), world.person("d@example.test"));
    let room = a.rooms.create_room("방", "가").expect("room");

    let revoked = a.rooms.create_invite(&room, 24, 10).expect("invite");
    a.rooms.revoke_invite(&room, &revoked.invite_id).expect("revoke");
    assert_eq!(b.rooms.join_room(&revoked.invite_code, "나"), Err(rejected(RoomCode::Revoked)));

    let short = a.rooms.create_invite(&room, 1, 10).expect("invite");
    world.clock.advance(2 * 3_600_000);
    assert_eq!(b.rooms.join_room(&short.invite_code, "나"), Err(rejected(RoomCode::Expired)));

    let single = a.rooms.create_invite(&room, 24, 1).expect("invite");
    assert_eq!(c.rooms.join_room(&single.invite_code, "다"), Ok(room.clone()));
    assert_eq!(d.rooms.join_room(&single.invite_code, "라"), Err(rejected(RoomCode::Exhausted)));
}

#[test]
fn a_room_holds_eight_members_and_only_the_owner_manages_it() {
    let world = World::new();
    let owner = world.person("o@example.test");
    let room = owner.rooms.create_room("방", "주인").expect("room");
    let code = invite(&owner, &room);
    let mut members = Vec::new();
    for i in 0..7 {
        let p = world.person(&format!("m{i}@example.test"));
        world.clock.advance(61_000);
        p.rooms.join_room(&code, &format!("멤버{i}")).expect("join");
        members.push(p);
    }
    let ninth = world.person("late@example.test");
    assert_eq!(ninth.rooms.join_room(&code, "늦음"), Err(rejected(RoomCode::RoomFull)));
    assert!(matches!(members[0].rooms.create_invite(&room, 24, 10), Err(RoomError::Rejected(RoomCode::NotOwner))));
    assert_eq!(members[0].rooms.kick_member(&room, &members[1].user_id), Err(rejected(RoomCode::NotOwner)));
}

#[test]
fn a_kicked_member_is_banned_from_rejoining_and_loses_access_at_once() {
    let world = World::new();
    let (a, b) = (world.person("a@example.test"), world.person("b@example.test"));
    let room = a.rooms.create_room("방", "가").expect("room");
    let code = invite(&a, &room);
    b.rooms.join_room(&code, "나").expect("join");
    assert!(b.rooms.snapshot(&room).is_ok());

    assert_eq!(a.rooms.kick_member(&room, &a.user_id), Err(rejected(RoomCode::CannotKickSelf)));
    a.rooms.kick_member(&room, &b.user_id).expect("kick");
    assert_eq!(b.rooms.snapshot(&room), Err(rejected(RoomCode::NotMember)));
    world.clock.advance(61_000);
    assert_eq!(b.rooms.join_room(&code, "나"), Err(rejected(RoomCode::Banned)));
    assert!(b.rooms.list_rooms().expect("list").is_empty());
}

#[test]
fn a_stranger_cannot_read_a_room_and_nobody_reads_other_peoples_rows_through_tables() {
    let world = World::new();
    let (a, b, x) = (world.person("a@example.test"), world.person("b@example.test"), world.person("x@example.test"));
    let room = a.rooms.create_room("방", "가").expect("room");
    b.rooms.join_room(&invite(&a, &room), "나").expect("join");
    a.rooms.report_status(&status(4, Some(1))).expect("status");

    assert_eq!(x.rooms.snapshot(&room), Err(rejected(RoomCode::NotMember)));
    assert_eq!(x.rooms.share_schedule(&room, &[]), Err(rejected(RoomCode::NotOwner)));

    let matrix: serde_json::Value = serde_json::from_str(include_str!("../../../fixtures/rooms/rls-matrix.json")).expect("matrix");
    for (table, rule) in matrix["tables"].as_object().expect("tables") {
        let as_x = world.backend.select(&Caller::User(x.user_id.clone()), table, &[]);
        let as_b = world.backend.select(&Caller::User(b.user_id.clone()), table, &[]);
        let anon = world.backend.select(&Caller::Anon, table, &[]);
        assert!(anon.is_err(), "{table}: anonymous callers are denied");
        match rule["select"].as_str() {
            Some("owner") => {
                assert!(as_x.expect("owner select").is_empty(), "{table}: a stranger sees nothing");
                assert!(as_b.expect("owner select").iter().all(|r| r["user_id"] == b.user_id.as_str()), "{table}: only own rows");
            }
            _ => {
                assert!(as_x.is_err() && as_b.is_err(), "{table}: no direct read");
            }
        }
    }
}

#[test]
fn a_friend_sees_exactly_what_the_member_shares_and_turning_a_share_off_removes_the_data() {
    let world = World::new();
    let (a, b) = (world.person("a@example.test"), world.person("b@example.test"));
    let room = a.rooms.create_room("방", "가").expect("room");
    b.rooms.join_room(&invite(&a, &room), "나").expect("join");
    let day = "2026-11-06".to_string();

    let view = |viewer: &Person, of: &Person| {
        let snapshot = viewer.rooms.snapshot(&room).expect("snapshot");
        snapshot.members.into_iter().find(|m| m.user_id == of.user_id).expect("member")
    };

    // Defaults: level and state are shared; time and attempts are not.
    a.rooms.set_prefs(&FriendSettings { share_week_time: false, share_attempts: false, ..open_all() }.as_args()).expect("prefs");
    a.rooms.report_status(&status(7, Some(3))).expect("status");
    assert_eq!(a.rooms.report_daily(&DailyReportArgs { day: day.clone(), study_min: 30, plan_min: 60 }), Err(rejected(RoomCode::NotShared)));
    let seen = view(&b, &a);
    assert_eq!((seen.level, seen.attempts_today, seen.days.is_none()), (Some(7), None, true));
    assert!(seen.status.is_some());

    a.rooms.set_prefs(&open_all().as_args()).expect("prefs");
    a.rooms.report_status(&status(7, Some(3))).expect("status");
    a.rooms.report_daily(&DailyReportArgs { day, study_min: 30, plan_min: 60 }).expect("daily");
    let seen = view(&b, &a);
    assert_eq!((seen.attempts_today, seen.days.as_ref().map(Vec::len)), (Some(3), Some(1)));

    a.rooms.set_prefs(&FriendSettings { share_level: false, share_status: false, share_week_time: false, share_attempts: false, ..open_all() }.as_args()).expect("prefs");
    let seen = view(&b, &a);
    assert_eq!((seen.level, seen.attempts_today, seen.days.is_none(), seen.status.is_none()), (None, None, true, true));
    assert!(view(&a, &a).is_self);
}

#[test]
fn reported_minutes_are_bounded_and_a_sudden_jump_is_rejected() {
    let world = World::new();
    let a = world.person("a@example.test");
    a.rooms.create_room("방", "가").expect("room");
    a.rooms.set_prefs(&open_all().as_args()).expect("prefs");
    let report = |min: i64, day: &str| a.rooms.report_daily(&DailyReportArgs { day: day.into(), study_min: min, plan_min: 0 });

    assert_eq!(report(1441, "2026-11-06"), Err(rejected(RoomCode::InvalidInput)));
    assert_eq!(report(10, "2026-10-01"), Err(rejected(RoomCode::InvalidInput)), "too old");
    assert_eq!(report(10, "2026-12-30"), Err(rejected(RoomCode::InvalidInput)), "too far ahead");
    report(10, "2026-11-06").expect("first report");
    world.clock.advance(60_000);
    assert_eq!(report(500, "2026-11-06"), Err(rejected(RoomCode::SurgeRejected)));
    report(14, "2026-11-06").expect("a believable rise");
}

#[test]
fn only_the_owner_shares_a_timetable_and_members_read_it() {
    let world = World::new();
    let (a, b) = (world.person("a@example.test"), world.person("b@example.test"));
    let room = a.rooms.create_room("방", "가").expect("room");
    b.rooms.join_room(&invite(&a, &room), "나").expect("join");
    let item = crate::remote::payload::ScheduleItemRow { start_at_utc_ms: 1_794_013_200_000, name: "수학".into(), subject: "수학".into(), sets: 2 };

    assert_eq!(b.rooms.share_schedule(&room, std::slice::from_ref(&item)), Err(rejected(RoomCode::NotOwner)));
    a.rooms.share_schedule(&room, &[item]).expect("share");
    let schedule = b.rooms.snapshot(&room).expect("snapshot").schedule.expect("schedule");
    assert_eq!(schedule.items.len(), 1);
    a.rooms.share_schedule(&room, &[]).expect("clear");
    assert!(b.rooms.snapshot(&room).expect("snapshot").schedule.is_none_or(|s| s.items.is_empty()));
}

#[test]
fn leaving_as_the_owner_hands_the_room_to_the_longest_member_and_the_last_leaver_closes_it() {
    let world = World::new();
    let (a, b) = (world.person("a@example.test"), world.person("b@example.test"));
    let room = a.rooms.create_room("방", "가").expect("room");
    b.rooms.join_room(&invite(&a, &room), "나").expect("join");
    a.rooms.leave_room(&room).expect("leave");
    let rooms = b.rooms.list_rooms().expect("list");
    assert_eq!(rooms[0].role, super::model::RoomRole::Owner);
    b.rooms.leave_room(&room).expect("leave");
    assert!(!world.backend.room_exists(&room));
}
