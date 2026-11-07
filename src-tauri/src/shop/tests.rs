use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use super::items::{self, Achievement, ItemKind, Slot, Source};
use super::*;
use crate::character::{ChooseRequest, Gender, SetupInput, choose};
use crate::db::open_in_memory;
use crate::store::{insert_ledger_entry, ledger_sums};

const NOW: i64 = 1_800_000_000_000;
const YEAR: i32 = 2027;

fn db_with_points(points: i64) -> Connection {
    let conn = open_in_memory().expect("db");
    if points > 0 {
        insert_ledger_entry(&conn, 1, points, "test", "seed").expect("seed points");
    }
    conn
}

fn pick_character(conn: &mut Connection) {
    let req = ChooseRequest {
        look: character::Look {
            species: "fox".into(),
            palette: "honey".into(),
            expression: "smile".into(),
            prop: "book".into(),
        },
        starter_top: "top_vest".into(),
        name: "도토리".into(),
        blurb: "함께해요.".into(),
        input: SetupInput {
            mbti: Some("INTJ".into()),
            birth_year: Some(1990),
            gender: Some(Gender::Other),
        },
    };
    choose(conn, &req, NOW, YEAR).expect("choose");
}

fn ledger(conn: &Connection) -> Vec<(i64, String, String)> {
    let mut stmt = conn
        .prepare("SELECT delta, reason_code, ref_key FROM point_ledger ORDER BY id")
        .expect("prepare");
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

#[test]
fn a_purchase_is_a_negative_ledger_row_and_an_inventory_row_together() {
    let mut conn = db_with_points(100);
    assert_eq!(
        purchase(&mut conn, "hat_beanie", NOW).expect("buy"),
        PurchaseOutcome::Purchased { balance: 60 }
    );
    assert_eq!(
        ledger(&conn).last().expect("row"),
        &(
            -40,
            "shop_purchase".to_string(),
            "buy:hat_beanie:1".to_string()
        )
    );
    let linked: String = conn
        .query_row("SELECT ledger_ref_key FROM inventory_local WHERE item_id = 'hat_beanie' AND source = 'shop'", [], |r| r.get(0))
        .expect("inventory row names its ledger row");
    assert_eq!(linked, "buy:hat_beanie:1");
    // Spending never lowers the level: it counts earned points.
    let status = crate::points::status(&conn).expect("status");
    assert_eq!((status.balance, status.earned), (60, 100));
}

#[test]
fn the_balance_is_always_the_sum_of_the_ledger() {
    let mut conn = db_with_points(300);
    for id in [
        "hat_beanie",
        "top_stripe",
        "acc_bowtie",
        "furn_lamp",
        "hat_beanie",
    ] {
        purchase(&mut conn, id, NOW).expect("buy or refuse");
    }
    let sum: i64 = ledger(&conn).iter().map(|r| r.0).sum();
    assert_eq!(ledger_sums(&conn).expect("sums").0, sum);
    assert_eq!(sum, 300 - 40 - 50 - 30 - 70);
    assert_eq!(shop_view(&conn).expect("view").balance, sum);
}

#[test]
fn too_few_points_refuse_and_write_nothing() {
    let mut conn = db_with_points(39);
    assert_eq!(
        purchase(&mut conn, "hat_beanie", NOW).expect("refused"),
        PurchaseOutcome::InsufficientPoints {
            balance: 39,
            price: 40
        }
    );
    assert_eq!(ledger(&conn).len(), 1);
    assert!(!repo::is_owned(&conn, "hat_beanie").expect("owned"));
    // Exactly the price is enough.
    insert_ledger_entry(&conn, 2, 1, "test", "more").expect("point");
    assert_eq!(
        purchase(&mut conn, "hat_beanie", NOW).expect("buy"),
        PurchaseOutcome::Purchased { balance: 0 }
    );
}

#[test]
fn buying_twice_is_idempotent_and_charges_once() {
    let mut conn = db_with_points(200);
    assert!(matches!(
        purchase(&mut conn, "hat_beanie", NOW).expect("first"),
        PurchaseOutcome::Purchased { .. }
    ));
    for _ in 0..3 {
        assert_eq!(
            purchase(&mut conn, "hat_beanie", NOW + 5).expect("again"),
            PurchaseOutcome::AlreadyOwned
        );
    }
    assert_eq!(
        ledger(&conn)
            .iter()
            .filter(|r| r.2.starts_with("buy:hat_beanie:"))
            .count(),
        1
    );
    assert_eq!(ledger_sums(&conn).expect("sums").0, 160);
}

#[test]
fn concurrent_purchases_charge_once_and_never_overspend() {
    let db = Arc::new(Mutex::new(db_with_points(200)));
    let results: Vec<PurchaseOutcome> = (0..8)
        .map(|_| {
            let db = Arc::clone(&db);
            std::thread::spawn(move || {
                purchase(&mut db.lock().expect("db"), "furn_desk", NOW).expect("purchase")
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().expect("thread"))
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, PurchaseOutcome::Purchased { .. }))
            .count(),
        1,
        "{results:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| **r == PurchaseOutcome::AlreadyOwned)
            .count(),
        7
    );
    assert_eq!(ledger_sums(&db.lock().expect("db")).expect("sums").0, 100);

    // Points for one of two different items: exactly one sale, never a negative balance.
    let db = Arc::new(Mutex::new(db_with_points(50)));
    let handles: Vec<_> = ["hat_beanie", "top_stripe"]
        .into_iter()
        .map(|id| {
            let db = Arc::clone(&db);
            std::thread::spawn(move || {
                purchase(&mut db.lock().expect("db"), id, NOW).expect("purchase")
            })
        })
        .collect();
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().expect("thread"))
        .collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, PurchaseOutcome::Purchased { .. }))
            .count(),
        1,
        "{outcomes:?}"
    );
    assert!(ledger_sums(&db.lock().expect("db")).expect("sums").0 >= 0);
}

#[test]
fn starter_and_achievement_items_are_not_for_sale_and_unknown_items_are_errors() {
    let mut conn = db_with_points(1000);
    for id in ["acc_knit_scarf", "hat_mortarboard", "top_vest"] {
        assert_eq!(
            purchase(&mut conn, id, NOW).expect("refused"),
            PurchaseOutcome::NotForSale,
            "{id}"
        );
    }
    assert!(purchase(&mut conn, "no_such_item", NOW).is_err());
    assert_eq!(ledger(&conn).len(), 1);
}

#[test]
fn achievement_conditions_hold_at_n_minus_1_and_n() {
    assert_eq!(earned_achievements(6, 99), []);
    assert_eq!(earned_achievements(7, 99), [Achievement::Streak7]);
    assert_eq!(earned_achievements(6, 100), [Achievement::Quiz100]);
    assert_eq!(
        earned_achievements(30, 1000),
        [Achievement::Streak7, Achievement::Quiz100]
    );
    assert_eq!(earned_achievements(0, 0), []);
}

#[test]
fn achievement_items_are_granted_once_and_kept_when_the_streak_breaks() {
    let conn = db_with_points(0);
    assert!(
        grant_achievements(&conn, 6, 99, NOW)
            .expect("none")
            .is_empty()
    );
    assert_eq!(
        grant_achievements(&conn, 7, 99, NOW).expect("scarf"),
        ["acc_knit_scarf"]
    );
    assert!(
        grant_achievements(&conn, 7, 99, NOW + 1)
            .expect("again")
            .is_empty()
    );
    assert_eq!(
        grant_achievements(&conn, 0, 100, NOW + 2).expect("cap"),
        ["hat_mortarboard"]
    );
    assert!(
        repo::is_owned(&conn, "acc_knit_scarf").expect("kept"),
        "a broken streak does not take the scarf back"
    );
    let source: String = conn
        .query_row(
            "SELECT source FROM inventory_local WHERE item_id = 'acc_knit_scarf'",
            [],
            |r| r.get(0),
        )
        .expect("source");
    assert_eq!(source, "achievement");
    assert_eq!(
        ledger(&conn).len(),
        0,
        "an achievement costs nothing and pays nothing"
    );
}

#[test]
fn granting_from_the_records_works_on_an_empty_database() {
    let mut conn = db_with_points(0);
    assert!(
        grant_from_records(&mut conn, NOW)
            .expect("empty records")
            .is_empty()
    );
}

#[test]
fn the_flags_say_which_items_can_be_given_away() {
    let catalog = items::catalog().expect("items");
    for item in &catalog.items {
        match item.source {
            Source::Shop => assert!(item.giftable() && item.price > 0, "{}", item.id),
            Source::Achievement => assert!(
                !item.giftable() && item.price == 0 && item.achievement.is_some(),
                "{}",
                item.id
            ),
            Source::Starter => assert!(!item.giftable() && item.price == 0, "{}", item.id),
        }
        assert_eq!(
            item.achievement.is_some(),
            item.source == Source::Achievement,
            "{}",
            item.id
        );
        match item.kind {
            ItemKind::Clothing => {
                assert!(item.slot.is_some() && !item.svg.is_empty(), "{}", item.id)
            }
            ItemKind::Furniture => assert!(item.slot.is_none(), "{}", item.id),
            ItemKind::Character => {
                let species = item.species.as_deref().expect("species");
                assert!(
                    crate::character::catalog::catalog()
                        .expect("parts")
                        .species(species)
                        .is_some_and(|s| s.shop_only),
                    "{}",
                    item.id
                );
            }
        }
    }
    // Every achievement unlocks an item, and every slot has something to buy.
    for a in [Achievement::Streak7, Achievement::Quiz100] {
        assert!(
            catalog.items.iter().any(|i| i.achievement == Some(a)),
            "{a:?}"
        );
    }
    for slot in Slot::ALL {
        assert!(
            catalog
                .items
                .iter()
                .any(|i| i.slot == Some(slot) && i.source == Source::Shop),
            "{slot:?}"
        );
    }
    assert!(
        catalog.items.iter().any(|i| i.kind == ItemKind::Furniture)
            && catalog.items.iter().any(|i| i.kind == ItemKind::Character)
    );
    let mut ids = HashSet::new();
    assert!(catalog.items.iter().all(|i| ids.insert(&i.id)));
    let view = shop_view(&open_in_memory().expect("db")).expect("view");
    assert!(
        view.items
            .iter()
            .filter(|i| i.achievement.is_some())
            .all(|i| !i.giftable)
    );
}

#[test]
fn wearing_needs_a_character_and_an_owned_item_and_replaces_the_slot() {
    let mut conn = db_with_points(300);
    assert!(equip(&mut conn, "top_vest").is_err(), "no character yet");
    pick_character(&mut conn);
    assert!(equip(&mut conn, "hat_beanie").is_err(), "not owned");
    purchase(&mut conn, "hat_beanie", NOW).expect("buy");
    purchase(&mut conn, "hat_ribbon", NOW).expect("buy");
    let worn = equip(&mut conn, "hat_beanie").expect("wear");
    assert_eq!(worn.equipped.hat.as_deref(), Some("hat_beanie"));
    assert_eq!(
        worn.equipped.top.as_deref(),
        Some("top_vest"),
        "the starter top stays"
    );
    assert!(worn.svg.contains("#c25b5b"), "the beanie is drawn");
    let swapped = equip(&mut conn, "hat_ribbon").expect("swap");
    assert_eq!(swapped.equipped.hat.as_deref(), Some("hat_ribbon"));
    assert!(!swapped.svg.contains("#c25b5b"));
    let bare = unequip(&mut conn, Slot::Hat).expect("take off");
    assert_eq!(bare.equipped.hat, None);
    assert!(
        equip(&mut conn, "hat_beanie").is_ok(),
        "still owned after taking it off"
    );
    purchase(&mut conn, "furn_lamp", NOW).expect("buy");
    assert!(
        equip(&mut conn, "furn_lamp").is_err(),
        "furniture is placed, not worn"
    );
    assert!(equip(&mut conn, "no_such_item").is_err());
}

#[test]
fn clothes_are_shared_by_every_character() {
    let mut conn = db_with_points(500);
    pick_character(&mut conn);
    purchase(&mut conn, "acc_bowtie", NOW).expect("buy");
    equip(&mut conn, "acc_bowtie").expect("wear");
    purchase(&mut conn, "char_raccoon", NOW).expect("buy the raccoon");
    let raccoon = equip(&mut conn, "char_raccoon").expect("switch species");
    assert_eq!(raccoon.look.species, "raccoon");
    assert_eq!(
        raccoon.equipped.accessory.as_deref(),
        Some("acc_bowtie"),
        "the wardrobe stays with the new character"
    );
    assert!(raccoon.svg.contains("#b03a48"));
    // A shop-only species can be picked at setup only after buying it.
    let mut other = db_with_points(0);
    let req = ChooseRequest {
        look: character::Look {
            species: "hedgehog".into(),
            palette: "honey".into(),
            expression: "smile".into(),
            prop: "book".into(),
        },
        starter_top: "top_vest".into(),
        name: "솔이".into(),
        blurb: "함께해요.".into(),
        input: SetupInput::default(),
    };
    assert!(choose(&mut other, &req, NOW, YEAR).is_err());
    insert_ledger_entry(&other, 1, 200, "test", "p").expect("points");
    purchase(&mut other, "char_hedgehog", NOW).expect("buy");
    assert!(choose(&mut other, &req, NOW, YEAR).is_ok());
}

#[test]
fn the_shop_view_marks_owned_worn_and_affordable() {
    let mut conn = db_with_points(45);
    pick_character(&mut conn);
    purchase(&mut conn, "hat_beanie", NOW).expect("buy");
    equip(&mut conn, "hat_beanie").expect("wear");
    let view = shop_view(&conn).expect("view");
    assert_eq!(view.balance, 5);
    let by = |id: &str| {
        view.items
            .iter()
            .find(|i| i.id == id)
            .expect("item")
            .clone()
    };
    assert!(by("hat_beanie").owned && by("hat_beanie").equipped);
    assert!(by("top_vest").owned && by("top_vest").equipped && !by("top_vest").affordable);
    assert!(!by("hat_ribbon").owned && !by("hat_ribbon").affordable);
    assert_eq!(
        by("acc_knit_scarf").achievement.as_deref(),
        Some("7일 연속 달성")
    );
}

#[test]
fn the_server_mirror_carries_species_and_worn_item_ids_only() {
    let mut conn = db_with_points(100);
    assert!(mirror::inventory_public(&conn).expect("none").is_none());
    pick_character(&mut conn);
    purchase(&mut conn, "hat_beanie", NOW).expect("buy");
    purchase(&mut conn, "hand_mug", NOW).expect("owned but not worn");
    equip(&mut conn, "hat_beanie").expect("wear");
    let args = mirror::inventory_public(&conn)
        .expect("mirror")
        .expect("character");
    let json = serde_json::to_value(&args).expect("json");
    assert_eq!(
        json,
        serde_json::json!({"p_species": "fox", "p_hat": "hat_beanie", "p_top": "top_vest", "p_accessory": null, "p_hand": null})
    );
    let text = json.to_string();
    for secret in ["INTJ", "1990", "other", "도토리", "hand_mug"] {
        assert!(!text.contains(secret), "{secret}");
    }
    // The change shows at once after wearing something else.
    equip(&mut conn, "hand_mug").expect("wear");
    assert_eq!(
        mirror::inventory_public(&conn)
            .expect("mirror")
            .expect("character")
            .hand
            .as_deref(),
        Some("hand_mug")
    );
}

#[test]
fn the_migration_checks_match_the_rust_enums() {
    let conn = open_in_memory().expect("db");
    let ins = |source: &str, ledger: Option<&str>| {
        conn.execute("INSERT INTO inventory_local (item_id, source, acquired_at_ms, ledger_ref_key) VALUES (?1, ?2, 0, ?3)", rusqlite::params![format!("i_{source}"), source, ledger])
    };
    for source in [Source::Starter, Source::Achievement] {
        ins(source.as_str(), None).expect("accepted");
    }
    assert!(ins("gift", None).is_ok(), "gift is reserved");
    assert!(ins("other", None).is_err());
    assert!(
        ins("shop", None).is_err(),
        "a shop item must name its ledger row"
    );
    assert!(ins("shop", Some("nope")).is_err(), "and the row must exist");
}
