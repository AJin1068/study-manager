//! Profile and gift scenarios on the in-memory server. They run the same adapter the app uses and
//! pin the behavior the SQL in `supabase/migrations/*_s4p_profile.sql` must also have (the SQL
//! itself is only checked by pgTAP, which has not been run here).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde_json::json;

use super::gift::{self, DAILY_LIMIT, REFUND_GRACE_MS};
use super::model::{GiftState, InboxGift, ProfileCode, ProfileError, Scope, Section, WireProfile};
use super::payload::{GrassEntry, SendGiftArgs, SetProfileArgs};
use super::port::{AcceptedGift, ProfileApi};
use super::publish::{self, LocalFacts};
use super::rest::RestProfile;
use super::service::{ApiSource, ProfileService};
use crate::db::open_in_memory;
use crate::remote::fake::Caller;
use crate::remote::http::TransportError;
use crate::remote::supabase::RemoteError;
use crate::rooms::port::RoomService;
use crate::rooms::testkit::{Person, World, T0};
use crate::store::{insert_ledger_entry, ledger_sums};

// ---- scaffolding ----

struct Member {
    person: Person,
    api: Arc<RestProfile>,
}

fn member(world: &World, email: &str) -> Member {
    let person = world.person(email);
    let api = Arc::new(RestProfile::new(Arc::clone(&person.rest)));
    Member { person, api }
}

/// Puts the given people into one room (the first owns it).
fn same_room(people: &[&Member]) {
    let owner = people[0];
    let room = owner.person.rooms.create_room("방", "가").expect("room");
    let code = owner.person.rooms.create_invite(&room, 24, 10).expect("invite").invite_code;
    for (i, other) in people.iter().enumerate().skip(1) {
        other.person.rooms.join_room(&code, &format!("친구{i}")).expect("join");
    }
}

fn scopes(list: &[(Section, Scope)]) -> BTreeMap<Section, Scope> {
    let mut map: BTreeMap<Section, Scope> = Section::ALL.into_iter().map(|s| (s, Scope::Private)).collect();
    map.extend(list.iter().copied());
    map
}

fn facts(name: &str, scopes: BTreeMap<Section, Scope>) -> LocalFacts {
    LocalFacts {
        name: name.into(),
        scopes,
        level: 7,
        badges: vec!["streak_7".into()],
        furniture: vec!["furn_lamp".into()],
        wiki_titles: vec!["SECRET-WIKI-TITLE".into()],
        subject: Some("SECRET-SUBJECT".into()),
        grass: vec![GrassEntry { day: "2026-11-05".into(), study_min: 90 }],
    }
}

fn db_with_points(points: i64) -> Arc<Mutex<Connection>> {
    let conn = open_in_memory().expect("db");
    if points > 0 {
        insert_ledger_entry(&conn, 1, points, "test", "seed").expect("seed");
    }
    Arc::new(Mutex::new(conn))
}

fn sums(db: &Mutex<Connection>) -> (i64, i64) {
    ledger_sums(&db.lock().expect("lock")).expect("sums")
}

fn ledger_rows(db: &Mutex<Connection>, reason: &str) -> usize {
    db.lock()
        .expect("lock")
        .query_row("SELECT COUNT(*) FROM point_ledger WHERE reason_code = ?1", [reason], |r| r.get::<_, i64>(0))
        .expect("count") as usize
}

fn public_view(world: &World, slug: &str) -> serde_json::Value {
    world.backend.rpc(&Caller::Anon, "get_public_profile", &json!({ "p_slug": slug })).expect("rpc")
}

fn sections_of(answer: &serde_json::Value) -> Vec<String> {
    answer["sections"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default()
}

fn timeout() -> ProfileError {
    ProfileError::Remote(RemoteError::Transport(TransportError::Timeout))
}

// ---- visibility ----

#[test]
fn every_section_is_private_until_the_user_opens_it() {
    let world = World::new();
    let a = member(&world, "a@example.test");
    let slug = publish::publish(a.api.as_ref(), &facts("민수", scopes(&[]))).expect("publish");
    assert_eq!(sections_of(&public_view(&world, &slug)), Vec::<String>::new());
    // The owner's own page lists every section, but a private one holds nothing.
    let mine = a.api.my_profile().expect("mine");
    assert_eq!(mine.sections["level"], json!(null));
    for empty in ["badges", "wiki", "grass"] {
        assert_eq!(mine.sections[empty], json!([]), "{empty}");
    }
    assert_eq!(mine.sections["subject"], json!(null));
    let dump = world.backend.profile_dump(&a.person.user_id);
    for canary in ["SECRET-WIKI-TITLE", "SECRET-SUBJECT", "streak_7", "furn_lamp"] {
        assert!(!dump.contains(canary), "{canary} was stored although its section is private");
    }
}

#[test]
fn a_private_section_never_leaves_the_pc() {
    let world = World::new();
    let a = member(&world, "a@example.test");
    publish::publish(a.api.as_ref(), &facts("민수", scopes(&[(Section::Level, Scope::Public)]))).expect("publish");
    let bodies = world.recorder.bodies_of("set_profile_settings").join("\n");
    assert!(bodies.contains("\"p_level\":7"));
    for canary in ["SECRET-WIKI-TITLE", "SECRET-SUBJECT", "streak_7", "furn_lamp"] {
        assert!(!bodies.contains(canary), "{canary} was sent although its section is private");
    }
    assert!(world.recorder.bodies_of("report_profile_grass").is_empty(), "a private grass is not even sent");
}

#[test]
fn nothing_about_attempts_or_apps_ever_travels() {
    let world = World::new();
    let (a, b) = (member(&world, "a@example.test"), member(&world, "b@example.test"));
    same_room(&[&a, &b]);
    let all = scopes(&Section::ALL.map(|s| (s, Scope::Public)));
    publish::publish(a.api.as_ref(), &facts("민수", all)).expect("publish");
    b.api.sign_guestbook(&a.person.user_id, "안녕").expect("sign");
    let everything: String = world.recorder.bodies().into_iter().map(|(name, body)| format!("{name} {body}")).collect::<Vec<_>>().join("\n").to_lowercase();
    for word in ["attempt", "deviation", "distraction", "app_name", "window", "points", "balance"] {
        assert!(!everything.contains(word), "{word}");
    }
}

#[test]
fn the_scope_matrix_holds_for_owner_member_stranger_and_anonymous() {
    let world = World::new();
    let (owner, mate, stranger) = (member(&world, "o@example.test"), member(&world, "m@example.test"), member(&world, "s@example.test"));
    same_room(&[&owner, &mate]);
    let mixed = scopes(&[
        (Section::Level, Scope::Public),
        (Section::Badges, Scope::Friends),
        (Section::Grass, Scope::Friends),
        (Section::Wiki, Scope::Private),
    ]);
    let slug = publish::publish(owner.api.as_ref(), &facts("민수", mixed)).expect("publish");

    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    // Anonymous (a link holder): only the public sections.
    assert_eq!(sections_of(&public_view(&world, &slug)), vec!["level".to_string()]);
    // A stranger signed in but not in a room with the owner: the same as anonymous through the link,
    // and refused by the member call.
    let via_link = world
        .backend
        .rpc(&Caller::User(stranger.person.user_id.clone()), "get_public_profile", &json!({ "p_slug": slug }))
        .expect("rpc");
    assert_eq!(sections_of(&via_link), vec!["level".to_string()]);
    assert_eq!(stranger.api.member_profile(&owner.person.user_id), Err(ProfileError::Rejected(ProfileCode::NotMember)));
    // A room mate: public and friends sections, never the private one.
    let mate_view = mate.api.member_profile(&owner.person.user_id).expect("mate");
    assert_eq!(sorted(mate_view.sections.keys().cloned().collect()), vec!["badges", "grass", "level"]);
    // The owner sees every section, but the private one holds nothing and the shown ones hold data.
    let mine = owner.api.my_profile().expect("mine");
    assert_eq!(mine.sections["wiki"], json!([]));
    assert_eq!((mine.sections["level"].as_i64(), mine.sections["badges"].as_array().map(Vec::len)), (Some(7), Some(1)));
    assert!(!world.backend.profile_dump(&owner.person.user_id).contains("SECRET-WIKI-TITLE"));
}

#[test]
fn narrowing_a_scope_removes_what_the_server_held() {
    let world = World::new();
    let a = member(&world, "a@example.test");
    let open = scopes(&[(Section::Wiki, Scope::Public), (Section::Subject, Scope::Public), (Section::Grass, Scope::Public)]);
    let slug = publish::publish(a.api.as_ref(), &facts("민수", open)).expect("open");
    assert!(world.backend.profile_dump(&a.person.user_id).contains("SECRET-WIKI-TITLE"));
    assert_eq!(sections_of(&public_view(&world, &slug)).len(), 3);
    assert_eq!(publish::publish(a.api.as_ref(), &facts("민수", scopes(&[]))), Ok(slug.clone()), "the link id is stable");
    let dump = world.backend.profile_dump(&a.person.user_id);
    assert!(!dump.contains("SECRET-WIKI-TITLE") && !dump.contains("SECRET-SUBJECT"));
    assert!(sections_of(&public_view(&world, &slug)).is_empty());
}

#[test]
fn an_unknown_or_malformed_link_id_reveals_nothing() {
    let world = World::new();
    for slug in ["0000000000000000", "short", "../../etc/passwd", "ZZZZZZZZZZZZZZZZ"] {
        assert_eq!(public_view(&world, slug)["code"], "not_found", "{slug}");
    }
}

#[test]
fn a_profile_needs_a_name() {
    let world = World::new();
    let a = member(&world, "a@example.test");
    assert_eq!(publish::publish(a.api.as_ref(), &facts("  ", scopes(&[]))), Err(ProfileError::Rejected(ProfileCode::NameRequired)));
    assert!(world.recorder.bodies_of("set_profile_settings").is_empty());
}

#[test]
fn the_settings_payload_empties_private_sections_whatever_the_local_data_holds() {
    let args: SetProfileArgs = publish::settings_args(&facts("민수", scopes(&[])));
    assert_eq!((args.level, args.badges.len(), args.furniture.len(), args.wiki_titles.len(), args.subject_now.clone()), (None, 0, 0, 0, None));
    assert_eq!(args.vis_room, "private");
    let open = publish::settings_args(&facts("민수", scopes(&[(Section::Badges, Scope::Friends)])));
    assert_eq!(open.badges, vec!["streak_7".to_string()]);
    assert_eq!(open.vis_badges, "friends");
}

// ---- guestbook and visitors ----

#[test]
fn only_a_room_mate_may_sign_and_the_owner_may_delete_and_report() {
    let world = World::new();
    let (owner, mate, stranger) = (member(&world, "o@example.test"), member(&world, "m@example.test"), member(&world, "s@example.test"));
    same_room(&[&owner, &mate]);
    publish::publish(owner.api.as_ref(), &facts("민수", scopes(&[(Section::Guestbook, Scope::Friends)]))).expect("publish");
    let owner_id = owner.person.user_id.clone();

    assert_eq!(stranger.api.sign_guestbook(&owner_id, "안녕"), Err(ProfileError::Rejected(ProfileCode::NotFriends)));
    assert_eq!(owner.api.sign_guestbook(&owner_id, "내가 씀"), Err(ProfileError::Rejected(ProfileCode::CannotSelf)));
    assert_eq!(mate.api.sign_guestbook(&owner_id, "  "), Err(ProfileError::Rejected(ProfileCode::InvalidInput)));
    mate.api.sign_guestbook(&owner_id, "응원해요").expect("sign");

    let seen = mate.api.member_profile(&owner_id).expect("view");
    let entries = seen.sections["guestbook"].as_array().expect("entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["mine"], true);
    let id = entries[0]["id"].as_i64().expect("id");

    // Only the owner can report; reported lines disappear for everyone else.
    assert_eq!(mate.api.report_entry(id), Err(ProfileError::Rejected(ProfileCode::NotFound)));
    owner.api.report_entry(id).expect("report");
    assert!(mate.api.member_profile(&owner_id).expect("view").sections["guestbook"].as_array().expect("entries").is_empty());
    // Neither the owner nor the author is a stranger to the line; a stranger cannot delete it.
    assert_eq!(stranger.api.delete_entry(id), Err(ProfileError::Rejected(ProfileCode::NotFound)));
    owner.api.delete_entry(id).expect("delete");
    assert_eq!(world.backend.profile_entry_count(&owner_id), 0);
}

#[test]
fn a_closed_guestbook_takes_no_lines() {
    let world = World::new();
    let (owner, mate) = (member(&world, "o@example.test"), member(&world, "m@example.test"));
    same_room(&[&owner, &mate]);
    publish::publish(owner.api.as_ref(), &facts("민수", scopes(&[]))).expect("publish");
    assert_eq!(mate.api.sign_guestbook(&owner.person.user_id, "안녕"), Err(ProfileError::Rejected(ProfileCode::Closed)));
}

#[test]
fn guestbook_flooding_is_limited_per_minute() {
    let world = World::new();
    let (owner, mate) = (member(&world, "o@example.test"), member(&world, "m@example.test"));
    same_room(&[&owner, &mate]);
    publish::publish(owner.api.as_ref(), &facts("민수", scopes(&[(Section::Guestbook, Scope::Friends)]))).expect("publish");
    for i in 0..5 {
        mate.api.sign_guestbook(&owner.person.user_id, &format!("줄{i}")).expect("within the limit");
    }
    assert_eq!(mate.api.sign_guestbook(&owner.person.user_id, "여섯째"), Err(ProfileError::Rejected(ProfileCode::RateLimited)));
    world.clock.advance(61_000);
    mate.api.sign_guestbook(&owner.person.user_id, "다시").expect("a minute later");
}

#[test]
fn visitors_count_once_per_person_per_day_and_anonymous_visits_are_not_counted() {
    let world = World::new();
    let (owner, mate) = (member(&world, "o@example.test"), member(&world, "m@example.test"));
    same_room(&[&owner, &mate]);
    let slug = publish::publish(owner.api.as_ref(), &facts("민수", scopes(&[(Section::Visitors, Scope::Friends)]))).expect("publish");
    let count = || {
        let view = owner.api.my_profile().expect("mine");
        (view.sections["visitors"]["total"].as_i64().expect("total"), view.sections["visitors"]["today"].as_i64().expect("today"))
    };
    assert_eq!(count(), (0, 0));
    for _ in 0..3 {
        mate.api.member_profile(&owner.person.user_id).expect("visit");
    }
    assert_eq!(count(), (1, 1), "three visits in a day are one");
    public_view(&world, &slug);
    assert_eq!(count(), (1, 1), "anonymous visits are not counted");
    world.clock.advance(86_400_000);
    mate.api.member_profile(&owner.person.user_id).expect("next day");
    assert_eq!(count(), (2, 1));
    owner.api.member_profile(&owner.person.user_id).expect("self");
    assert_eq!(count().0, 2, "the owner looking at their own page is not a visit");
}

#[test]
fn grass_is_shown_in_day_order_and_bad_days_are_refused() {
    let world = World::new();
    let a = member(&world, "a@example.test");
    publish::publish(a.api.as_ref(), &facts("민수", scopes(&[]))).expect("publish");
    assert_eq!(a.api.report_grass(&[GrassEntry { day: "2026-11-05".into(), study_min: 5 }]), Err(ProfileError::Rejected(ProfileCode::NotShared)));
    let mut f = facts("민수", scopes(&[(Section::Grass, Scope::Public)]));
    f.grass = vec![GrassEntry { day: "2026-11-05".into(), study_min: 30 }, GrassEntry { day: "2026-11-01".into(), study_min: 10 }];
    publish::publish(a.api.as_ref(), &f).expect("publish");
    let days = a.api.my_profile().expect("mine").sections["grass"].clone();
    assert_eq!(days, json!([{ "day": "2026-11-01", "study_min": 10 }, { "day": "2026-11-05", "study_min": 30 }]));
    for bad in [GrassEntry { day: "2026-11-05".into(), study_min: 1441 }, GrassEntry { day: "yesterday".into(), study_min: 1 }] {
        assert_eq!(a.api.report_grass(&[bad]), Err(ProfileError::Rejected(ProfileCode::InvalidInput)));
    }
}

// ---- gifts ----

struct Pair {
    world: World,
    sender: Member,
    receiver: Member,
    sender_db: Arc<Mutex<Connection>>,
    receiver_db: Arc<Mutex<Connection>>,
}

fn pair(points: i64) -> Pair {
    let world = World::new();
    let (sender, receiver) = (member(&world, "s@example.test"), member(&world, "r@example.test"));
    same_room(&[&sender, &receiver]);
    Pair { world, sender, receiver, sender_db: db_with_points(points), receiver_db: db_with_points(0) }
}

const GIFT_A: &str = "00000000-0000-4000-8000-00000000000a";
const GIFT_B: &str = "00000000-0000-4000-8000-00000000000b";

fn send(p: &Pair, api: &dyn ProfileApi, id: &str, item: &str) -> gift::SendOutcome {
    gift::send(&p.sender_db, api, id, &p.receiver.person.user_id, "친구", item, "고마워", T0).expect("send")
}

#[test]
fn a_gift_spends_the_senders_points_and_gives_the_receiver_an_item_not_points() {
    let p = pair(100);
    let out = send(&p, p.sender.api.as_ref(), GIFT_A, "hat_beanie");
    assert_eq!((out.code, out.state.as_deref(), out.balance), (ProfileCode::Ok, Some("sent"), 60));
    assert_eq!(p.world.backend.profile_gift_count(&p.sender.person.user_id), 1);
    assert_eq!(sums(&p.sender_db), (60, 100), "spending never lowers the level");

    let waiting = gift::sync_inbox(&p.receiver_db, p.receiver.api.as_ref(), T0).expect("inbox");
    assert_eq!(waiting.len(), 1);
    assert_eq!((waiting[0].item_id.as_str(), waiting[0].message.as_str()), ("hat_beanie", "고마워"));
    let got = gift::accept(&p.receiver_db, p.receiver.api.as_ref(), &waiting[0].gift_id, &waiting[0].message, T0).expect("accept");
    assert_eq!((got.code, got.item_id.as_deref(), got.already_owned), (ProfileCode::Ok, Some("hat_beanie"), false));
    assert!(crate::store::shop::is_owned(&p.receiver_db.lock().expect("lock"), "hat_beanie").expect("owned"));
    assert_eq!(sums(&p.receiver_db), (0, 0), "no points arrive");
    assert_eq!(ledger_rows(&p.receiver_db, "gift_sent") + ledger_rows(&p.receiver_db, "gift_refund"), 0);

    // Accepting again changes nothing, and the box no longer lists it.
    let again = gift::accept(&p.receiver_db, p.receiver.api.as_ref(), &waiting[0].gift_id, "", T0).expect("accept again");
    assert_eq!(again.code, ProfileCode::Ok);
    assert!(gift::sync_inbox(&p.receiver_db, p.receiver.api.as_ref(), T0).expect("inbox").is_empty());
    let count: i64 = p.receiver_db.lock().expect("lock").query_row("SELECT COUNT(*) FROM inventory_local WHERE item_id = 'hat_beanie'", [], |r| r.get(0)).expect("count");
    assert_eq!(count, 1);
}

#[test]
fn the_receiver_can_decline_and_a_resolved_gift_cannot_change_its_mind() {
    let p = pair(100);
    send(&p, p.sender.api.as_ref(), GIFT_A, "furn_lamp");
    let id = gift::sync_inbox(&p.receiver_db, p.receiver.api.as_ref(), T0).expect("inbox")[0].gift_id.clone();
    assert_eq!(gift::decline(&p.receiver_db, p.receiver.api.as_ref(), &id, T0).expect("decline"), ProfileCode::Ok);
    assert_eq!(gift::decline(&p.receiver_db, p.receiver.api.as_ref(), &id, T0).expect("again"), ProfileCode::Ok);
    let late = gift::accept(&p.receiver_db, p.receiver.api.as_ref(), &id, "", T0).expect("accept");
    assert_eq!(late.code, ProfileCode::AlreadyResolved);
    assert!(!crate::store::shop::is_owned(&p.receiver_db.lock().expect("lock"), "furn_lamp").expect("owned"));
    // A gift only its recipient can see or answer.
    assert_eq!(p.sender.api.accept_gift(&id), Err(ProfileError::Rejected(ProfileCode::NotFound)));
    assert_eq!(sums(&p.sender_db).0, 30, "a declined gift is not refunded: the points were spent on a present");
}

#[test]
fn an_item_the_receiver_already_owns_is_reported_not_duplicated() {
    let p = pair(100);
    insert_inventory_for(&p.receiver_db, "hat_beanie");
    send(&p, p.sender.api.as_ref(), GIFT_A, "hat_beanie");
    let msg = gift::sync_inbox(&p.receiver_db, p.receiver.api.as_ref(), T0).expect("inbox").remove(0);
    let got = gift::accept(&p.receiver_db, p.receiver.api.as_ref(), &msg.gift_id, &msg.message, T0).expect("accept");
    assert!(got.already_owned);
}

fn insert_inventory_for(db: &Mutex<Connection>, item: &str) {
    crate::store::shop::insert_inventory(&db.lock().expect("lock"), item, "gift", 1, None).expect("inventory");
}

#[test]
fn points_items_and_strangers_cannot_be_gifted() {
    let p = pair(1000);
    let outsider = member(&p.world, "x@example.test");
    for (item, code) in [
        ("acc_knit_scarf", ProfileCode::NotGiftable),
        ("hat_mortarboard", ProfileCode::NotGiftable),
        ("top_vest", ProfileCode::NotGiftable),
        ("char_raccoon", ProfileCode::NotGiftable),
        ("no_such_item", ProfileCode::UnknownItem),
    ] {
        let out = send(&p, p.sender.api.as_ref(), GIFT_A, item);
        assert_eq!((out.code, out.gift_id), (code, None), "{item}");
    }
    assert_eq!(sums(&p.sender_db).0, 1000, "a refused gift costs nothing");
    // A recipient outside the sender's rooms: the server refuses for certain, the points come back once.
    let out = gift::send(&p.sender_db, p.sender.api.as_ref(), GIFT_B, &outsider.person.user_id, "남", "hat_beanie", "", T0).expect("send");
    assert_eq!((out.code, out.state.as_deref(), out.balance), (ProfileCode::NotFriends, Some("refunded"), 1000));
    assert_eq!(p.world.backend.profile_gift_count(&p.sender.person.user_id), 0);
    // Oneself is refused too.
    let own = gift::send(&p.sender_db, p.sender.api.as_ref(), GIFT_A, &p.sender.person.user_id, "나", "hat_beanie", "", T0).expect("send");
    assert_eq!((own.code, own.balance), (ProfileCode::CannotSelf, 1000));
    assert_eq!(ledger_rows(&p.sender_db, "gift_refund"), 2);
}

#[test]
fn a_poor_sender_cannot_spend_below_zero_and_long_messages_are_refused() {
    let p = pair(30);
    assert_eq!(send(&p, p.sender.api.as_ref(), GIFT_A, "hat_beanie").code, ProfileCode::InsufficientPoints);
    let long = "가".repeat(gift::MESSAGE_MAX_CHARS + 1);
    let out = gift::send(&p.sender_db, p.sender.api.as_ref(), GIFT_B, &p.receiver.person.user_id, "친구", "hat_ribbon", &long, T0).expect("send");
    assert_eq!(out.code, ProfileCode::InvalidInput);
    assert_eq!(sums(&p.sender_db).0, 30);
    assert_eq!(ledger_rows(&p.sender_db, "gift_sent"), 0);
}

#[test]
fn the_daily_limit_counts_sent_gifts_but_not_refunded_ones() {
    let p = pair(10_000);
    for i in 0..DAILY_LIMIT {
        let id = format!("00000000-0000-4000-8000-{i:012}");
        assert_eq!(send(&p, p.sender.api.as_ref(), &id, "hat_ribbon").state.as_deref(), Some("sent"));
    }
    let over = send(&p, p.sender.api.as_ref(), GIFT_B, "hat_ribbon");
    assert_eq!((over.code, over.gift_id), (ProfileCode::DailyLimit, None));
    assert_eq!(p.world.backend.profile_gift_count(&p.sender.person.user_id), DAILY_LIMIT as usize);
    assert_eq!(ledger_rows(&p.sender_db, "gift_sent"), DAILY_LIMIT as usize, "the refused one cost nothing");
    // The next day the allowance is back (both here and on the server).
    p.world.clock.advance(86_400_000 + 1);
    let next = gift::send(&p.sender_db, p.sender.api.as_ref(), GIFT_B, &p.receiver.person.user_id, "친구", "hat_ribbon", "", T0 + 86_400_001).expect("send");
    assert_eq!(next.state.as_deref(), Some("sent"));
}

#[test]
fn the_server_applies_a_daily_limit_of_its_own() {
    let p = pair(0);
    for i in 0..5 {
        let key = format!("k{i}");
        p.sender.api.send_gift(&SendGiftArgs { idem_key: key, recipient: p.receiver.person.user_id.clone(), item_id: "hat_ribbon".into(), message: String::new() }).expect("within");
    }
    let over = SendGiftArgs { idem_key: "k5".into(), recipient: p.receiver.person.user_id.clone(), item_id: "hat_ribbon".into(), message: String::new() };
    assert_eq!(p.sender.api.send_gift(&over), Err(ProfileError::Rejected(ProfileCode::DailyLimit)));
}

/// Wraps the real adapter and breaks `send_gift` in the ways a network breaks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    /// The server processes the request, the answer is lost.
    LoseAnswer,
    /// The request never arrives.
    LoseRequest,
}

struct Flaky {
    inner: RestProfile,
    fault: Mutex<Fault>,
}

impl Flaky {
    fn new(inner: &Member) -> Arc<Self> {
        Arc::new(Self { inner: RestProfile::new(Arc::clone(&inner.person.rest)), fault: Mutex::new(Fault::None) })
    }

    fn set(&self, fault: Fault) {
        *self.fault.lock().expect("lock") = fault;
    }

    fn fault(&self) -> Fault {
        *self.fault.lock().expect("lock")
    }
}

impl ProfileApi for Flaky {
    fn set_settings(&self, args: &SetProfileArgs) -> Result<String, ProfileError> {
        self.inner.set_settings(args)
    }
    fn report_grass(&self, entries: &[GrassEntry]) -> Result<(), ProfileError> {
        self.inner.report_grass(entries)
    }
    fn my_profile(&self) -> Result<WireProfile, ProfileError> {
        self.inner.my_profile()
    }
    fn member_profile(&self, user: &str) -> Result<WireProfile, ProfileError> {
        self.inner.member_profile(user)
    }
    fn sign_guestbook(&self, owner: &str, body: &str) -> Result<(), ProfileError> {
        self.inner.sign_guestbook(owner, body)
    }
    fn delete_entry(&self, id: i64) -> Result<(), ProfileError> {
        self.inner.delete_entry(id)
    }
    fn report_entry(&self, id: i64) -> Result<(), ProfileError> {
        self.inner.report_entry(id)
    }
    fn send_gift(&self, args: &SendGiftArgs) -> Result<(), ProfileError> {
        match self.fault() {
            Fault::None => self.inner.send_gift(args),
            Fault::LoseAnswer => {
                self.inner.send_gift(args)?;
                Err(timeout())
            }
            Fault::LoseRequest => Err(timeout()),
        }
    }
    fn gift_status(&self, idem_key: &str) -> Result<GiftState, ProfileError> {
        self.inner.gift_status(idem_key)
    }
    fn gift_inbox(&self) -> Result<Vec<InboxGift>, ProfileError> {
        self.inner.gift_inbox()
    }
    fn accept_gift(&self, gift_id: &str) -> Result<AcceptedGift, ProfileError> {
        self.inner.accept_gift(gift_id)
    }
    fn decline_gift(&self, gift_id: &str) -> Result<(), ProfileError> {
        self.inner.decline_gift(gift_id)
    }
}

fn outbox_state(db: &Mutex<Connection>, id: &str) -> String {
    crate::store::profile::outbox(&db.lock().expect("lock"), id).expect("row").expect("exists").state
}

#[test]
fn a_lost_answer_is_not_refunded_and_the_gift_exists_exactly_once() {
    let p = pair(100);
    let flaky = Flaky::new(&p.sender);
    flaky.set(Fault::LoseAnswer);
    let out = send(&p, flaky.as_ref(), GIFT_A, "hat_beanie");
    assert_eq!((out.code, out.state.as_deref(), out.balance), (ProfileCode::Ok, Some("pending"), 60), "unclear: nothing refunded");
    assert_eq!(ledger_rows(&p.sender_db, "gift_refund"), 0);
    assert_eq!(p.world.backend.profile_gift_count(&p.sender.person.user_id), 1);

    flaky.set(Fault::None);
    // Asked long after, the server says it has the gift: marked sent, never refunded.
    for _ in 0..3 {
        gift::reconcile(&p.sender_db, flaky.as_ref(), T0 + 5 * REFUND_GRACE_MS).expect("reconcile");
    }
    assert_eq!(outbox_state(&p.sender_db, GIFT_A), "sent");
    assert_eq!(sums(&p.sender_db).0, 60);
    assert_eq!(ledger_rows(&p.sender_db, "gift_refund"), 0);
    assert_eq!(p.world.backend.profile_gift_count(&p.sender.person.user_id), 1);
}

#[test]
fn a_retry_of_the_same_gift_key_creates_no_second_gift() {
    let p = pair(100);
    let flaky = Flaky::new(&p.sender);
    flaky.set(Fault::LoseAnswer);
    send(&p, flaky.as_ref(), GIFT_A, "hat_beanie");
    flaky.set(Fault::None);
    let row = crate::store::profile::outbox(&p.sender_db.lock().expect("lock"), GIFT_A).expect("row").expect("exists");
    let retry = SendGiftArgs { idem_key: row.gift_id, recipient: row.recipient_user, item_id: row.item_id, message: row.message };
    p.sender.api.send_gift(&retry).expect("a replay is accepted");
    assert_eq!(p.world.backend.profile_gift_count(&p.sender.person.user_id), 1);
    // The same key for a different gift is a conflict, not a silent overwrite.
    let other = SendGiftArgs { item_id: "furn_lamp".into(), ..retry };
    assert_eq!(p.sender.api.send_gift(&other), Err(ProfileError::Rejected(ProfileCode::KeyConflict)));
}

#[test]
fn a_request_that_never_arrived_is_refunded_once_after_the_grace_period() {
    let p = pair(100);
    let flaky = Flaky::new(&p.sender);
    flaky.set(Fault::LoseRequest);
    let out = send(&p, flaky.as_ref(), GIFT_A, "hat_beanie");
    assert_eq!((out.state.as_deref(), out.balance), (Some("pending"), 60));
    flaky.set(Fault::None);

    // Too early: a delayed request might still arrive, so the server's "none" is not trusted yet.
    assert_eq!(gift::reconcile(&p.sender_db, flaky.as_ref(), T0 + 1000).expect("early"), 0);
    assert_eq!(outbox_state(&p.sender_db, GIFT_A), "pending");
    assert_eq!(sums(&p.sender_db).0, 60);

    let late = T0 + REFUND_GRACE_MS + 1;
    assert_eq!(gift::reconcile(&p.sender_db, flaky.as_ref(), late).expect("late"), 1);
    assert_eq!(gift::reconcile(&p.sender_db, flaky.as_ref(), late + 1000).expect("again"), 0);
    assert_eq!(outbox_state(&p.sender_db, GIFT_A), "refunded");
    assert_eq!(sums(&p.sender_db), (100, 100), "refunded in full, and a refund is not earned");
    assert_eq!(ledger_rows(&p.sender_db, "gift_refund"), 1);
    assert_eq!(p.world.backend.profile_gift_count(&p.sender.person.user_id), 0);
}

#[test]
fn an_unreachable_server_during_reconcile_changes_nothing() {
    let p = pair(100);
    let flaky = Flaky::new(&p.sender);
    flaky.set(Fault::LoseRequest);
    send(&p, flaky.as_ref(), GIFT_A, "hat_beanie");
    struct Down;
    impl ProfileApi for Down {
        fn set_settings(&self, _: &SetProfileArgs) -> Result<String, ProfileError> { Err(timeout()) }
        fn report_grass(&self, _: &[GrassEntry]) -> Result<(), ProfileError> { Err(timeout()) }
        fn my_profile(&self) -> Result<WireProfile, ProfileError> { Err(timeout()) }
        fn member_profile(&self, _: &str) -> Result<WireProfile, ProfileError> { Err(timeout()) }
        fn sign_guestbook(&self, _: &str, _: &str) -> Result<(), ProfileError> { Err(timeout()) }
        fn delete_entry(&self, _: i64) -> Result<(), ProfileError> { Err(timeout()) }
        fn report_entry(&self, _: i64) -> Result<(), ProfileError> { Err(timeout()) }
        fn send_gift(&self, _: &SendGiftArgs) -> Result<(), ProfileError> { Err(timeout()) }
        fn gift_status(&self, _: &str) -> Result<GiftState, ProfileError> { Err(timeout()) }
        fn gift_inbox(&self) -> Result<Vec<InboxGift>, ProfileError> { Err(timeout()) }
        fn accept_gift(&self, _: &str) -> Result<AcceptedGift, ProfileError> { Err(timeout()) }
        fn decline_gift(&self, _: &str) -> Result<(), ProfileError> { Err(timeout()) }
    }
    assert_eq!(gift::reconcile(&p.sender_db, &Down, T0 + 10 * REFUND_GRACE_MS).expect("down"), 0);
    assert_eq!(outbox_state(&p.sender_db, GIFT_A), "pending");
    assert_eq!(sums(&p.sender_db).0, 60);
}

#[test]
fn a_gift_accepted_on_the_server_but_not_yet_applied_here_is_applied_once() {
    let p = pair(100);
    send(&p, p.sender.api.as_ref(), GIFT_A, "hat_beanie");
    // The accept reached the server, the answer never reached this PC.
    let id = p.receiver.api.gift_inbox().expect("inbox")[0].gift_id.clone();
    p.receiver.api.accept_gift(&id).expect("server side accept");
    assert!(!crate::store::shop::is_owned(&p.receiver_db.lock().expect("lock"), "hat_beanie").expect("owned"));
    for _ in 0..2 {
        gift::sync_inbox(&p.receiver_db, p.receiver.api.as_ref(), T0).expect("sync");
    }
    assert!(crate::store::shop::is_owned(&p.receiver_db.lock().expect("lock"), "hat_beanie").expect("owned"));
    let count: i64 = p.receiver_db.lock().expect("lock").query_row("SELECT COUNT(*) FROM inventory_local WHERE item_id = 'hat_beanie'", [], |r| r.get(0)).expect("count");
    assert_eq!(count, 1);
}

#[test]
fn the_ledger_balance_is_always_the_sum_of_its_rows_through_every_gift_path() {
    let p = pair(500);
    let flaky = Flaky::new(&p.sender);
    send(&p, flaky.as_ref(), GIFT_A, "hat_beanie");
    flaky.set(Fault::LoseAnswer);
    send(&p, flaky.as_ref(), "00000000-0000-4000-8000-00000000000c", "furn_lamp");
    flaky.set(Fault::LoseRequest);
    send(&p, flaky.as_ref(), "00000000-0000-4000-8000-00000000000d", "furn_rug");
    flaky.set(Fault::None);
    gift::reconcile(&p.sender_db, flaky.as_ref(), T0 + 2 * REFUND_GRACE_MS).expect("reconcile");
    let sum: i64 = {
        let conn = p.sender_db.lock().expect("lock");
        let mut stmt = conn.prepare("SELECT delta FROM point_ledger").expect("prepare");
        stmt.query_map([], |r| r.get::<_, i64>(0)).expect("rows").map(|r| r.expect("row")).sum()
    };
    assert_eq!(sums(&p.sender_db).0, sum);
    assert_eq!(sum, 500 - 40 - 70, "the lost request was refunded, the other two were spent");
}

// ---- the service ----

struct Fixed(Option<Arc<dyn ProfileApi>>);

impl ApiSource for Fixed {
    fn api(&self) -> Option<Arc<dyn ProfileApi>> {
        self.0.clone()
    }
}

fn service(world: &World, api: Option<Arc<dyn ProfileApi>>, db: Arc<Mutex<Connection>>) -> ProfileService {
    ProfileService::with_parts(db, world.clock.clone(), Arc::new(Fixed(api)))
}

#[test]
fn the_service_starts_private_and_a_scope_change_publishes() {
    let world = World::new();
    let a = member(&world, "a@example.test");
    let db = db_with_points(0);
    let svc = service(&world, Some(a.api.clone()), Arc::clone(&db));
    let home = svc.home().expect("home");
    assert!(home.scopes.iter().all(|r| r.scope == Scope::Private));
    assert_eq!(home.scopes.len(), Section::ALL.len());
    assert_eq!((home.slug, home.link), (None, None));

    // No name yet: the server is not asked and nothing is published.
    assert_eq!(svc.set_scope(Section::Level, Scope::Public).expect("scope").code, ProfileCode::NameRequired);
    assert!(world.recorder.bodies_of("set_profile_settings").is_empty());
    assert_eq!(svc.set_name(&"가".repeat(21)).expect("long").code, ProfileCode::InvalidInput);
    assert_eq!(svc.set_name(" 민수 ").expect("name").code, ProfileCode::Ok);
    assert_eq!(svc.set_web_base("ftp://x.test").expect("bad").code, ProfileCode::InvalidInput);
    assert_eq!(svc.set_web_base("https://me.example.test/study/").expect("base").code, ProfileCode::Ok);

    let published = svc.set_scope(Section::Level, Scope::Public).expect("scope");
    assert_eq!(published.code, ProfileCode::Ok);
    let slug = published.slug.expect("slug");
    assert_eq!(published.link, Some(format!("https://me.example.test/study/#/u/{slug}")));
    assert_eq!(svc.home().expect("home").link, published.link);
    assert_eq!(sections_of(&public_view(&world, &slug)), vec!["level".to_string()]);
    let my = svc.my_view().expect("view");
    assert_eq!((my.name.as_deref(), my.level, my.badges.clone()), (Some("민수"), Some(1), Some(vec![])));
}

#[test]
fn a_scope_cannot_be_widened_during_a_run_but_can_be_narrowed_and_every_change_is_in_the_history() {
    let world = World::new();
    let a = member(&world, "a@example.test");
    let db = db_with_points(0);
    let svc = service(&world, Some(a.api.clone()), Arc::clone(&db));
    svc.set_name("민수").expect("name");
    svc.set_scope(Section::Level, Scope::Friends).expect("widen while free");

    crate::testutil::set_lock_for_test(&db.lock().expect("db"), true);
    let refused = svc.set_scope(Section::Level, Scope::Public).expect("refusal is an answer");
    assert_eq!((refused.code, refused.slug), (ProfileCode::RunLocked, None));
    let refused_new = svc.set_scope(Section::Badges, Scope::Friends).expect("private -> friends");
    assert_eq!(refused_new.code, ProfileCode::RunLocked);
    let scope_of = |section| svc.home().expect("home").scopes.iter().find(|r| r.section == section).map(|r| r.scope);
    assert_eq!((scope_of(Section::Level), scope_of(Section::Badges)), (Some(Scope::Friends), Some(Scope::Private)));

    assert_eq!(svc.set_scope(Section::Level, Scope::Private).expect("narrow").code, ProfileCode::Ok);
    assert_eq!(scope_of(Section::Level), Some(Scope::Private));
    crate::testutil::set_lock_for_test(&db.lock().expect("db"), false);

    let conn = db.lock().expect("db");
    let rows: Vec<(String, Option<String>, String, String)> = conn
        .prepare("SELECT key, before, after, outcome FROM setting_history WHERE key LIKE 'profile.scope.%' ORDER BY id")
        .expect("prepare")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    let row = |key: &str, before: &str, after: &str, outcome: &str| (key.to_string(), Some(before.to_string()), after.to_string(), outcome.to_string());
    assert_eq!(
        rows,
        vec![
            row("profile.scope.level", "private", "friends", "applied"),
            row("profile.scope.level", "friends", "public", "rejected_locked"),
            row("profile.scope.badges", "private", "friends", "rejected_locked"),
            row("profile.scope.level", "friends", "private", "applied_privacy_tighten"),
        ]
    );
}

#[test]
fn the_service_reports_a_missing_connection_as_an_error_and_keeps_local_choices() {
    let world = World::new();
    let db = db_with_points(0);
    let svc = service(&world, None, Arc::clone(&db));
    svc.set_name("민수").expect("name");
    assert!(svc.set_scope(Section::Badges, Scope::Friends).is_err(), "not signed in is an error the UI shows as the link state");
    assert_eq!(svc.home().expect("home").scopes.iter().find(|r| r.section == Section::Badges).map(|r| r.scope), Some(Scope::Friends), "the choice is kept");
    assert!(svc.send_gift("u", "친구", "hat_beanie", "").is_err());
    let overview = svc.gift_overview().expect("offline overview");
    assert!(!overview.online && overview.inbox.is_empty());
}

#[test]
fn wiki_names_are_cleaned_deduplicated_and_limited() {
    let world = World::new();
    let svc = service(&world, None, db_with_points(0));
    let titles = vec![" 미적분 ".to_string(), "미적분".into(), "".into(), "선형대수".into()];
    assert_eq!(svc.set_wiki_titles(&titles).expect("titles").code, ProfileCode::Ok);
    assert_eq!(svc.home().expect("home").wiki_titles, vec!["미적분".to_string(), "선형대수".to_string()]);
    let many: Vec<String> = (0..11).map(|i| format!("페이지{i}")).collect();
    assert_eq!(svc.set_wiki_titles(&many).expect("many").code, ProfileCode::TooMany);
    assert_eq!(svc.set_wiki_titles(&["가".repeat(61)]).expect("long").code, ProfileCode::InvalidInput);
    assert_eq!(svc.home().expect("home").wiki_titles.len(), 2, "a refused list changes nothing");
}

#[test]
fn the_service_sends_a_gift_end_to_end_and_the_receiver_accepts_it() {
    let world = World::new();
    let (s, r) = (member(&world, "s@example.test"), member(&world, "r@example.test"));
    same_room(&[&s, &r]);
    let (sdb, rdb) = (db_with_points(100), db_with_points(0));
    let (ssvc, rsvc) = (service(&world, Some(s.api.clone()), Arc::clone(&sdb)), service(&world, Some(r.api.clone()), Arc::clone(&rdb)));
    let out = ssvc.send_gift(&r.person.user_id, "친구", "hat_beanie", "화이팅").expect("send");
    assert_eq!((out.code, out.state.as_deref(), out.balance), (ProfileCode::Ok, Some("sent"), 60));
    let sent = ssvc.gift_overview().expect("overview");
    assert_eq!((sent.outbox.len(), sent.outbox[0].state.as_str(), sent.online), (1, "sent", true));
    let inbox = rsvc.gift_overview().expect("overview").inbox;
    assert_eq!(inbox.len(), 1);
    let got = rsvc.accept_gift(&inbox[0].gift_id).expect("accept");
    assert_eq!((got.code, got.item_id.as_deref()), (ProfileCode::Ok, Some("hat_beanie")));
    assert!(rsvc.gift_overview().expect("overview").inbox.is_empty());
    let received: (String, String) = rdb
        .lock()
        .expect("lock")
        .query_row("SELECT sender_name, message FROM gift_received", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("remembered");
    assert_eq!(received.1, "화이팅");
    assert!(!received.0.is_empty());
}

#[test]
fn the_service_guestbook_checks_input_before_the_network() {
    let world = World::new();
    let svc = service(&world, None, db_with_points(0));
    assert_eq!(svc.sign_guestbook("u", "   ").expect("blank").code, ProfileCode::InvalidInput);
    assert_eq!(svc.sign_guestbook("u", &"가".repeat(201)).expect("long").code, ProfileCode::InvalidInput);
}

#[test]
fn collect_reads_owned_badges_furniture_and_the_current_scopes() {
    let db = db_with_points(0);
    {
        let conn = db.lock().expect("lock");
        crate::store::shop::insert_inventory(&conn, "furn_lamp", "gift", 1, None).expect("lamp");
        crate::store::shop::insert_inventory(&conn, "acc_knit_scarf", "achievement", 1, None).expect("scarf");
        crate::store::profile::set_visibility(&conn, Section::Badges, Scope::Friends, 1).expect("scope");
    }
    let facts = publish::collect(&db.lock().expect("lock"), &chrono::Utc, T0).expect("collect");
    assert_eq!(facts.furniture, vec!["furn_lamp".to_string()]);
    assert_eq!(facts.badges, vec!["streak_7".to_string()]);
    assert_eq!(facts.scopes[&Section::Badges], Scope::Friends);
    assert_eq!(facts.scopes[&Section::Room], Scope::Private);
    assert_eq!(facts.subject, None, "no study session is running");
}
