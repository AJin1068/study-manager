use std::collections::BTreeMap;

use rusqlite::Connection;

use super::*;
use crate::db::open_in_memory;
use crate::enforce::model::{BlockEntry, EntrySource};
use crate::enforce::{self};
use crate::platform::is_protected_id;
use crate::settings::{UpdateOutcome, read_history};
use crate::testutil::set_lock_for_test;

fn entries(conn: &Connection) -> Vec<BlockEntry> {
    store::list_block_entries(conn).expect("entries")
}

fn enabled_patterns(conn: &Connection, kind: EntryKind) -> Vec<String> {
    let mut list = store::list_enabled_patterns(conn, kind).expect("patterns");
    list.sort();
    list
}

fn choose(pairs: &[(&str, bool)]) -> BTreeMap<String, bool> {
    pairs
        .iter()
        .map(|(id, on)| ((*id).to_owned(), *on))
        .collect()
}

#[test]
fn catalog_matches_the_plan_table() {
    let defaults: Vec<(&str, bool)> = CATALOG.iter().map(|c| (c.id, c.default_on)).collect();
    assert_eq!(
        defaults,
        [
            ("messenger", true),
            ("video", true),
            ("sns", true),
            ("community", true),
            ("game", true),
            ("shopping_webtoon", false),
        ]
    );
    let sites = |id: &str| category(id).expect("category").sites.to_vec();
    assert_eq!(sites("messenger"), ["web.kakao.com", "discord.com"]);
    assert_eq!(
        sites("video"),
        ["youtube.com", "netflix.com", "chzzk.naver.com", "twitch.tv"]
    );
    assert_eq!(
        sites("sns"),
        ["instagram.com", "x.com", "threads.net", "facebook.com"]
    );
    assert_eq!(
        sites("community"),
        ["dcinside.com", "fmkorea.com", "theqoo.net", "reddit.com"]
    );
    assert!(sites("game").is_empty());
    assert_eq!(
        sites("shopping_webtoon"),
        ["coupang.com", "29cm.co.kr", "comic.naver.com"]
    );
    // Apps: messenger has three (KakaoTalk, Discord, Telegram), games four (Steam, Riot, Battle.net, Epic).
    assert_eq!(category("messenger").expect("c").apps.len(), 3);
    assert_eq!(category("game").expect("c").apps.len(), 4);
    assert!(
        category("sns").expect("c").apps.is_empty()
            && category("community").expect("c").apps.is_empty()
    );
}

#[test]
fn catalog_entries_are_usable_unique_and_never_protected() {
    let mut seen = std::collections::HashSet::new();
    for cat in CATALOG {
        for app in cat.apps {
            let id = normalize_app_id(app);
            assert!(!is_protected_id(&id), "{id} is protected");
            assert!(seen.insert(("app", id.clone())), "duplicate {id}");
        }
        for site in cat.sites {
            let pattern =
                normalize_pattern(site).unwrap_or_else(|| panic!("{site} does not parse"));
            assert_eq!(&pattern, site, "{site} is not stored in canonical form");
            assert!(seen.insert(("site", pattern)), "duplicate {site}");
        }
    }
}

#[test]
fn nothing_is_on_the_list_before_the_first_choice() {
    let conn = open_in_memory().expect("db");
    let status = status(&conn).expect("status");
    assert!(!status.onboarded);
    let on: Vec<_> = status
        .categories
        .iter()
        .filter(|c| c.enabled)
        .map(|c| c.id)
        .collect();
    assert_eq!(on, ["messenger", "video", "sns", "community", "game"]);
    assert!(entries(&conn).is_empty());
}

#[test]
fn accepting_the_defaults_enables_every_category_but_shopping_webtoon() {
    let mut conn = open_in_memory().expect("db");
    let outcome = set_categories(&mut conn, &BTreeMap::new(), 10).expect("choose");
    assert_eq!(
        outcome,
        UpdateOutcome::Applied {
            key: HISTORY_KEY.into()
        }
    );
    assert!(status(&conn).expect("status").onboarded);
    let sites = enabled_patterns(&conn, EntryKind::Site);
    assert!(
        sites.contains(&"reddit.com".to_owned()) && sites.contains(&"web.kakao.com".to_owned())
    );
    assert!(
        !sites.contains(&"coupang.com".to_owned()),
        "shopping/webtoon is off by default"
    );
    let disabled: Vec<_> = entries(&conn)
        .into_iter()
        .filter(|e| !e.enabled)
        .map(|e| e.pattern)
        .collect();
    assert_eq!(disabled, ["coupang.com", "29cm.co.kr", "comic.naver.com"]);
    assert!(
        entries(&conn)
            .iter()
            .all(|e| e.source == "preset" && e.category.is_some())
    );
    assert!(
        enabled_patterns(&conn, EntryKind::App).contains(&"com.valvesoftware.steam".to_owned())
    );
}

#[test]
fn switching_a_category_off_disables_its_entries_and_on_restores_them() {
    let mut conn = open_in_memory().expect("db");
    set_categories(&mut conn, &BTreeMap::new(), 10).expect("first");
    set_categories(
        &mut conn,
        &choose(&[("sns", false), ("shopping_webtoon", true)]),
        20,
    )
    .expect("toggle");
    let sites = enabled_patterns(&conn, EntryKind::Site);
    assert!(!sites.contains(&"instagram.com".to_owned()));
    assert!(sites.contains(&"coupang.com".to_owned()) && sites.contains(&"reddit.com".to_owned()));
    // Categories left out of a later call keep their state.
    set_categories(&mut conn, &choose(&[("sns", true)]), 30).expect("on");
    let sites = enabled_patterns(&conn, EntryKind::Site);
    assert!(
        sites.contains(&"instagram.com".to_owned()) && sites.contains(&"coupang.com".to_owned())
    );
    // The rows were never duplicated.
    let all = entries(&conn);
    let unique: std::collections::HashSet<_> = all
        .iter()
        .map(|e| (e.kind.clone(), e.pattern.clone()))
        .collect();
    assert_eq!(all.len(), unique.len());
}

#[test]
fn unknown_category_is_an_error_and_changes_nothing() {
    let mut conn = open_in_memory().expect("db");
    assert!(set_categories(&mut conn, &choose(&[("nope", true)]), 1).is_err());
    assert!(entries(&conn).is_empty());
    assert!(!status(&conn).expect("status").onboarded);
}

#[test]
fn a_locked_study_block_rejects_the_choice_and_records_it() {
    let mut conn = open_in_memory().expect("db");
    set_categories(&mut conn, &BTreeMap::new(), 10).expect("first");
    let before = entries(&conn);
    set_lock_for_test(&conn, true);
    let outcome = set_categories(&mut conn, &choose(&[("sns", false)]), 20).expect("attempt");
    assert_eq!(
        outcome,
        UpdateOutcome::RejectedLocked {
            key: HISTORY_KEY.into()
        }
    );
    assert_eq!(entries(&conn), before, "nothing may change under the lock");
    assert!(
        status(&conn)
            .expect("status")
            .categories
            .iter()
            .any(|c| c.id == "sns" && c.enabled)
    );
    let latest = &read_history(&conn, 1).expect("history")[0];
    assert_eq!(
        (
            latest.key.as_str(),
            latest.after.as_str(),
            latest.outcome.as_str()
        ),
        (HISTORY_KEY, "sns:off", "rejected_locked")
    );
}

#[test]
fn the_first_choice_is_also_refused_under_the_lock() {
    let mut conn = open_in_memory().expect("db");
    set_lock_for_test(&conn, true);
    let outcome = set_categories(&mut conn, &BTreeMap::new(), 1).expect("attempt");
    assert!(matches!(outcome, UpdateOutcome::RejectedLocked { .. }));
    assert!(entries(&conn).is_empty() && !status(&conn).expect("status").onboarded);
}

#[test]
fn user_entries_survive_category_changes_and_refreshes() {
    let mut conn = open_in_memory().expect("db");
    // The user blocks reddit.com themselves before choosing presets, and adds another site.
    crate::site_rules::edit::add_site(&mut conn, "Reddit.com", 1).expect("user reddit");
    crate::site_rules::edit::add_site(&mut conn, "https://news.example.com/feed", 2)
        .expect("user news");
    set_categories(&mut conn, &BTreeMap::new(), 10).expect("choose");
    set_categories(&mut conn, &choose(&[("community", false)]), 20).expect("off");
    let by_pattern = |pattern: &str| {
        entries(&conn)
            .into_iter()
            .find(|e| e.pattern == pattern)
            .expect("entry")
    };
    // The pre-existing user row stays a user row, still enabled, even with its category off.
    let reddit = by_pattern("reddit.com");
    assert_eq!(
        (
            reddit.source.as_str(),
            reddit.category.clone(),
            reddit.enabled
        ),
        ("user", None, true)
    );
    assert!(by_pattern("news.example.com/feed").enabled);
    assert!(!by_pattern("dcinside.com").enabled);
    // An older stored version triggers a refresh; nothing the user has is changed.
    conn.execute("UPDATE settings SET value = '{\"version\":0,\"enabled\":{\"community\":false}}' WHERE key = ?1", [SELECTION_KEY])
        .expect("old version");
    let before = entries(&conn);
    refresh(&mut conn, 30).expect("refresh");
    assert_eq!(entries(&conn), before);
    assert_eq!(
        read_selection(&conn).expect("sel").expect("some").version,
        PRESET_VERSION
    );
}

#[test]
fn refresh_adds_missing_preset_rows_with_the_stored_category_state() {
    let mut conn = open_in_memory().expect("db");
    set_categories(&mut conn, &choose(&[("sns", false)]), 10).expect("choose");
    conn.execute(
        "DELETE FROM block_entries WHERE pattern IN ('instagram.com', 'twitch.tv')",
        [],
    )
    .expect("simulate older catalog");
    conn.execute("UPDATE settings SET value = replace(value, '\"version\":1', '\"version\":0') WHERE key = ?1", [SELECTION_KEY])
        .expect("old version");
    refresh(&mut conn, 20).expect("refresh");
    let find = |pattern: &str| {
        entries(&conn)
            .into_iter()
            .find(|e| e.pattern == pattern)
            .expect("entry")
    };
    assert!(!find("instagram.com").enabled, "an off category stays off");
    assert!(find("twitch.tv").enabled);
    // Current version: refresh is a no-op.
    let before = entries(&conn);
    refresh(&mut conn, 30).expect("again");
    assert_eq!(entries(&conn), before);
}

#[test]
fn refresh_before_the_first_choice_does_nothing() {
    let mut conn = open_in_memory().expect("db");
    refresh(&mut conn, 1).expect("refresh");
    assert!(entries(&conn).is_empty());
}

#[test]
fn preset_entries_cannot_be_deleted_only_user_entries_can() {
    let mut conn = open_in_memory().expect("db");
    set_categories(&mut conn, &BTreeMap::new(), 10).expect("choose");
    let preset = entries(&conn)
        .into_iter()
        .find(|e| e.source == EntrySource::Preset.as_str())
        .expect("preset row");
    assert!(enforce::remove_entry(&mut conn, preset.id, 20).is_err());
    assert!(
        store::get_block_entry(&conn, preset.id)
            .expect("get")
            .is_some()
    );
    crate::site_rules::edit::add_site(&mut conn, "mine.example", 30).expect("user");
    let mine = entries(&conn)
        .into_iter()
        .find(|e| e.pattern == "mine.example")
        .expect("mine");
    assert!(matches!(
        enforce::remove_entry(&mut conn, mine.id, 40).expect("remove"),
        UpdateOutcome::Applied { .. }
    ));
}
