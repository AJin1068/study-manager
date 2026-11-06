//! Editing site rules. Every edit is checked for the study lock inside the same transaction that
//! writes it; a locked attempt writes only its `rejected_locked` history row. Lists are validated
//! up front (the loader is lenient so a bad stored entry cannot disable blocking, but a bad typed
//! entry is an error the user sees). History rows carry counts, not the entries.

use rusqlite::{Connection, TransactionBehavior};

use super::{normalize_pattern, plain_id, StudyYoutube, SiteRulesConfig, SITE_RULES_KEY};
use crate::enforce::model::{EntryKind, EntrySource};
use crate::enforce::BLOCK_LIST_KEY;
use crate::error::{AppError, AppResult};
use crate::settings::{Outcome, Reason, UpdateOutcome};
use crate::store;

const SETTING_CATEGORY: &str = "browser";
const MAX_LIST_ENTRIES: usize = 500;

/// The saved configuration, or the default (normal mode, built-in strict set, nothing registered).
pub fn read_config(conn: &Connection) -> AppResult<SiteRulesConfig> {
    match store::get_setting(conn, SITE_RULES_KEY)? {
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|err| AppError::Internal(format!("{SITE_RULES_KEY} setting is invalid: {err}"))),
        None => Ok(SiteRulesConfig::default()),
    }
}

fn dedup_keep_order(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items.into_iter().filter(|item| seen.insert(item.clone())).collect()
}

fn check_count(len: usize, what: &str) -> AppResult<()> {
    if len > MAX_LIST_ENTRIES {
        return Err(AppError::Invalid(format!("{what}: at most {MAX_LIST_ENTRIES} entries")));
    }
    Ok(())
}

/// Normalizes and validates a configuration; the first unusable entry is an error.
pub fn validate(config: SiteRulesConfig) -> AppResult<SiteRulesConfig> {
    let allow = match config.allow {
        Some(entries) => {
            check_count(entries.len(), "allow list")?;
            let normalized = entries
                .iter()
                .map(|entry| {
                    normalize_pattern(entry).ok_or_else(|| AppError::Invalid("an allow-list entry is not a site".into()))
                })
                .collect::<AppResult<Vec<_>>>()?;
            Some(dedup_keep_order(normalized))
        }
        None => None,
    };
    check_count(config.youtube_playlists.len(), "YouTube playlists")?;
    let playlists = config
        .youtube_playlists
        .iter()
        .map(|id| {
            plain_id(id).map(str::to_owned).ok_or_else(|| AppError::Invalid("a YouTube playlist ID is not usable".into()))
        })
        .collect::<AppResult<Vec<_>>>()?;
    check_count(config.youtube_channels.len(), "YouTube channels")?;
    let mut probe = StudyYoutube::default();
    let channels = config
        .youtube_channels
        .iter()
        .map(|id| {
            if !probe.add_channel(id) {
                return Err(AppError::Invalid("a YouTube channel ID or handle is not usable".into()));
            }
            Ok(id.trim().to_owned())
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(SiteRulesConfig {
        strict: config.strict,
        allow,
        youtube_playlists: dedup_keep_order(playlists),
        youtube_channels: dedup_keep_order(channels),
    })
}

fn summary(config: &SiteRulesConfig) -> String {
    format!(
        "strict={} allow={} playlists={} channels={}",
        config.strict,
        config.allow.as_ref().map_or_else(|| "default".to_owned(), |list| list.len().to_string()),
        config.youtube_playlists.len(),
        config.youtube_channels.len()
    )
}

/// Saves strict mode, the allow list, and the YouTube study entries.
pub fn update_config(conn: &mut Connection, config: SiteRulesConfig, at_ms: i64) -> AppResult<UpdateOutcome> {
    let config = validate(config)?;
    let json = serde_json::to_string(&config).map_err(|err| AppError::Internal(err.to_string()))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let before = summary(&read_config(&tx)?);
    let after = summary(&config);
    if crate::study::lock_held(&tx)? {
        store::append_history(&tx, at_ms, SITE_RULES_KEY, Some(&before), &after, Reason::User, Outcome::RejectedLocked)?;
        tx.commit()?;
        return Ok(UpdateOutcome::RejectedLocked { key: SITE_RULES_KEY.into() });
    }
    store::put_setting(&tx, SITE_RULES_KEY, SETTING_CATEGORY, &json, at_ms)?;
    store::append_history(&tx, at_ms, SITE_RULES_KEY, Some(&before), &after, Reason::User, Outcome::Applied)?;
    tx.commit()?;
    Ok(UpdateOutcome::Applied { key: SITE_RULES_KEY.into() })
}

/// Adds a site to the block list (`domain` or `domain/path`). Locked during study.
pub fn add_site(conn: &mut Connection, raw: &str, at_ms: i64) -> AppResult<UpdateOutcome> {
    let pattern = normalize_pattern(raw).ok_or_else(|| AppError::Invalid("not a usable domain or domain/path".into()))?;
    let after = format!("add:{pattern}");
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if crate::study::lock_held(&tx)? {
        store::append_history(&tx, at_ms, BLOCK_LIST_KEY, None, &after, Reason::User, Outcome::RejectedLocked)?;
        tx.commit()?;
        return Ok(UpdateOutcome::RejectedLocked { key: BLOCK_LIST_KEY.into() });
    }
    if !store::insert_block_entry(&tx, EntryKind::Site, &pattern, EntrySource::User, None, at_ms)? {
        return Err(AppError::Invalid("this site is already on the block list".into()));
    }
    store::append_history(&tx, at_ms, BLOCK_LIST_KEY, None, &after, Reason::User, Outcome::Applied)?;
    tx.commit()?;
    Ok(UpdateOutcome::Applied { key: BLOCK_LIST_KEY.into() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::settings::read_history;
    use crate::testutil::set_lock_for_test;

    fn config(strict: bool, allow: Option<&[&str]>, lists: &[&str], channels: &[&str]) -> SiteRulesConfig {
        SiteRulesConfig {
            strict,
            allow: allow.map(|a| a.iter().map(|s| (*s).to_owned()).collect()),
            youtube_playlists: lists.iter().map(|s| (*s).to_owned()).collect(),
            youtube_channels: channels.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn saved_rules_round_trip_normalized_and_deduplicated() {
        let mut conn = open_in_memory().expect("db");
        let input = config(
            true,
            Some(&["GitHub.com", "https://docs.rs/", "github.com"]),
            &["PLstudy", " PLstudy "],
            &["@StudyChannel", "UC123"],
        );
        assert!(matches!(update_config(&mut conn, input, 5).expect("save"), UpdateOutcome::Applied { .. }));
        let saved = read_config(&conn).expect("read");
        assert!(saved.strict);
        assert_eq!(saved.allow, Some(vec!["github.com".to_owned(), "docs.rs".to_owned()]));
        assert_eq!(saved.youtube_playlists, ["PLstudy"]);
        assert_eq!(saved.youtube_channels, ["@StudyChannel", "UC123"]);
        // The engine reads exactly what was saved: strict mode blocks the unlisted, allows the listed.
        let rules = super::super::load(&conn).expect("load");
        let mut cache = super::super::DecisionCache::default();
        let study = super::super::Situation { in_study: true, pass_active: false };
        assert!(super::super::judge("https://example.org/", &rules, study, &mut cache).block);
        assert!(!super::super::judge("https://docs.rs/serde", &rules, study, &mut cache).block);
        assert!(!super::super::judge("https://www.youtube.com/watch?v=x&list=PLstudy", &rules, study, &mut cache).block);
    }

    #[test]
    fn a_locked_study_block_refuses_edits_and_keeps_the_rules() {
        let mut conn = open_in_memory().expect("db");
        update_config(&mut conn, config(true, None, &[], &[]), 1).expect("first");
        set_lock_for_test(&conn, true);
        let outcome = update_config(&mut conn, config(false, Some(&["evil.example"]), &["PL1"], &[]), 2).expect("attempt");
        assert_eq!(outcome, UpdateOutcome::RejectedLocked { key: SITE_RULES_KEY.into() });
        assert_eq!(read_config(&conn).expect("read"), config(true, None, &[], &[]));
        let latest = &read_history(&conn, 1).expect("history")[0];
        assert_eq!((latest.key.as_str(), latest.outcome.as_str()), (SITE_RULES_KEY, "rejected_locked"));
        assert_eq!(latest.after, "strict=false allow=1 playlists=1 channels=0", "history keeps counts, not entries");

        let added = add_site(&mut conn, "reddit.com", 3).expect("attempt");
        assert!(matches!(added, UpdateOutcome::RejectedLocked { .. }));
        assert!(store::list_block_entries(&conn).expect("entries").is_empty());
        assert_eq!(read_history(&conn, 1).expect("history")[0].outcome, "rejected_locked");
    }

    #[test]
    fn unusable_entries_are_errors_that_save_nothing() {
        let mut conn = open_in_memory().expect("db");
        for bad in [
            config(false, Some(&["not a site"]), &[], &[]),
            config(false, Some(&["a.com?x=1"]), &[], &[]),
            config(false, None, &["bad/list"], &[]),
            config(false, None, &[], &["@"]),
            config(false, None, &[], &["has space"]),
        ] {
            assert!(update_config(&mut conn, bad, 1).is_err());
        }
        assert_eq!(store::get_setting(&conn, SITE_RULES_KEY).expect("get"), None);
        assert!(read_history(&conn, 5).expect("history").is_empty());
        let many: Vec<String> = (0..=MAX_LIST_ENTRIES).map(|i| format!("site{i}.example")).collect();
        let over = SiteRulesConfig { allow: Some(many), ..SiteRulesConfig::default() };
        assert!(update_config(&mut conn, over, 1).is_err());
        let empty = config(true, Some(&[]), &[], &[]);
        assert!(update_config(&mut conn, empty, 1).is_ok(), "an empty allow list is allowed: strict mode then allows only music");
    }

    #[test]
    fn add_site_normalizes_rejects_duplicates_and_bad_input() {
        let mut conn = open_in_memory().expect("db");
        add_site(&mut conn, "https://WWW.Example.com/Learn/", 1).expect("add");
        let entries = store::list_block_entries(&conn).expect("entries");
        assert_eq!((entries[0].kind.as_str(), entries[0].pattern.as_str(), entries[0].source.as_str()), ("site", "www.example.com/learn", "user"));
        assert!(add_site(&mut conn, "www.example.com/learn", 2).is_err());
        assert!(add_site(&mut conn, "   ", 3).is_err());
        assert!(add_site(&mut conn, "a b", 3).is_err());
        assert_eq!(store::list_block_entries(&conn).expect("entries").len(), 1);
    }
}
