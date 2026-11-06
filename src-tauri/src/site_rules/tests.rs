use std::sync::{Arc, Mutex};

use super::*;
use crate::db::open_in_memory;
use crate::enforce::model::EntrySource;
use crate::testutil::capture_logs;

const STUDY: Situation = Situation { in_study: true, pass_active: false };

fn rules(block: &[&str], stored_json: &str) -> SiteRules {
    let block: Vec<String> = block.iter().map(|p| (*p).to_owned()).collect();
    SiteRules::build(&block, serde_json::from_str(stored_json).expect("stored json"))
}

fn judge_fresh(url: &str, rules: &SiteRules, situation: Situation) -> (bool, &'static str) {
    let decision = judge(url, rules, situation, &mut DecisionCache::default());
    (decision.block, decision.reason)
}

fn normal_rules() -> SiteRules {
    rules(
        &[
            "youtube.com",
            "youtu.be",
            "netflix.com",
            "reddit.com",
            "kakao.com/talk",
            "dcinside.com",
            "한글.kr",
            "github.com",
            "10.0.0.7",
        ],
        r#"{"allow":["github.com/myorg"],"youtubePlaylists":["PLstudy"],"youtubeChannels":["@StudyChannel","UCstudy"]}"#,
    )
}

fn strict_rules() -> SiteRules {
    rules(
        &["youtube.com", "youtu.be"],
        r#"{"strict":true,"allow":["github.com","docs.rs","wiki.example.com/notes"],"youtubePlaylists":["PLstudy"]}"#,
    )
}

const BLOCKED: bool = true;
const ALLOWED: bool = false;

#[test]
fn normal_mode_table() {
    let cases: &[(&str, bool, &str)] = &[
        // domain and subdomain matching
        ("https://youtube.com/", BLOCKED, REASON_BLOCKED_SITE),
        ("https://www.youtube.com/feed/trending", BLOCKED, REASON_BLOCKED_SITE),
        ("https://m.youtube.com/", BLOCKED, REASON_BLOCKED_SITE),
        ("https://a.b.c.reddit.com/r/x", BLOCKED, REASON_BLOCKED_SITE),
        ("https://old.reddit.com/", BLOCKED, REASON_BLOCKED_SITE),
        ("https://netflix.com/browse", BLOCKED, REASON_BLOCKED_SITE),
        // case, port, trailing dot, query and fragment
        ("HTTPS://YOUTUBE.COM/", BLOCKED, REASON_BLOCKED_SITE),
        ("https://YouTube.Com:8443/watch", BLOCKED, REASON_BLOCKED_SITE),
        ("https://youtube.com./", BLOCKED, REASON_BLOCKED_SITE),
        ("https://youtube.com?x=1#frag", BLOCKED, REASON_BLOCKED_SITE),
        ("  https://youtube.com/  ", BLOCKED, REASON_BLOCKED_SITE),
        ("http://youtube.com/", BLOCKED, REASON_BLOCKED_SITE),
        // look-alikes and attacker-controlled suffixes are not the blocked site
        ("https://evil-youtube.com/", ALLOWED, REASON_UNLISTED_ALLOWED),
        ("https://notyoutube.com/", ALLOWED, REASON_UNLISTED_ALLOWED),
        ("https://youtube.com.attacker.com/", ALLOWED, REASON_UNLISTED_ALLOWED),
        ("https://evil-youtube.com.attacker.com/watch?list=PLstudy", ALLOWED, REASON_UNLISTED_ALLOWED),
        ("https://youtubexcom/", ALLOWED, REASON_UNLISTED_ALLOWED),
        // userinfo and backslash tricks resolve to the host a browser would contact
        ("https://google.com@youtube.com/", BLOCKED, REASON_BLOCKED_SITE),
        ("https://youtube.com@evil.example/", ALLOWED, REASON_UNLISTED_ALLOWED),
        ("https://youtube.com:pass@evil.example/", ALLOWED, REASON_UNLISTED_ALLOWED),
        ("https://youtube.com\\@evil.example/", BLOCKED, REASON_BLOCKED_SITE),
        // path rules (web messenger style) match at segment boundaries
        ("https://kakao.com/talk", BLOCKED, REASON_BLOCKED_SITE),
        ("https://web.kakao.com/talk/room/1", BLOCKED, REASON_BLOCKED_SITE),
        ("https://kakao.com/TALK", BLOCKED, REASON_BLOCKED_SITE),
        ("https://kakao.com/a/../talk", BLOCKED, REASON_BLOCKED_SITE),
        ("https://kakao.com/talkative", ALLOWED, REASON_UNLISTED_ALLOWED),
        ("https://kakao.com/", ALLOWED, REASON_UNLISTED_ALLOWED),
        ("https://kakao.com/map?next=/talk", ALLOWED, REASON_UNLISTED_ALLOWED),
        // Unicode domains compare in punycode
        ("https://한글.kr/", BLOCKED, REASON_BLOCKED_SITE),
        ("https://www.한글.kr/x", BLOCKED, REASON_BLOCKED_SITE),
        ("https://한글.kr.evil.com/", ALLOWED, REASON_UNLISTED_ALLOWED),
        // IP addresses match exactly, never by suffix
        ("http://10.0.0.7:3000/", BLOCKED, REASON_BLOCKED_SITE),
        ("http://110.0.0.7/", ALLOWED, REASON_UNLISTED_ALLOWED),
        // YouTube study playlists and channels
        ("https://www.youtube.com/playlist?list=PLstudy", ALLOWED, REASON_ALLOWED_STUDY_VIDEO),
        ("https://www.youtube.com/watch?v=abc&list=PLstudy&index=2", ALLOWED, REASON_ALLOWED_STUDY_VIDEO),
        ("https://youtu.be/abc?list=PLstudy", ALLOWED, REASON_ALLOWED_STUDY_VIDEO),
        ("https://www.youtube.com/watch?list=PLother", BLOCKED, REASON_BLOCKED_SITE),
        ("https://www.youtube.com/watch?list=plstudy", BLOCKED, REASON_BLOCKED_SITE),
        ("https://www.youtube.com/watch?playlist=PLstudy", BLOCKED, REASON_BLOCKED_SITE),
        ("https://www.youtube.com/results?search_query=list%3DPLstudy", BLOCKED, REASON_BLOCKED_SITE),
        ("https://www.youtube.com/@StudyChannel", ALLOWED, REASON_ALLOWED_STUDY_VIDEO),
        ("https://www.youtube.com/@studychannel/videos", ALLOWED, REASON_ALLOWED_STUDY_VIDEO),
        ("https://www.youtube.com/@OtherChannel", BLOCKED, REASON_BLOCKED_SITE),
        ("https://www.youtube.com/channel/UCstudy", ALLOWED, REASON_ALLOWED_STUDY_VIDEO),
        ("https://www.youtube.com/channel/UCother", BLOCKED, REASON_BLOCKED_SITE),
        // the channel of a plain watch page is not in the URL: blocked (stage 3 can judge by title)
        ("https://www.youtube.com/watch?v=abc", BLOCKED, REASON_BLOCKED_SITE),
        ("https://youtu.be/abc", BLOCKED, REASON_BLOCKED_SITE),
        // user allow entries win over a block entry for the same site
        ("https://github.com/myorg/repo", ALLOWED, REASON_ALLOWED_LISTED),
        ("https://github.com/other/repo", BLOCKED, REASON_BLOCKED_SITE),
        // music defaults
        ("https://open.spotify.com/track/1", ALLOWED, REASON_ALLOWED_MUSIC),
        ("https://www.melon.com/", ALLOWED, REASON_ALLOWED_MUSIC),
        ("https://evil-spotify.com/", ALLOWED, REASON_UNLISTED_ALLOWED),
        // LLM-off fallback in normal mode
        ("https://example.org/anything?q=1", ALLOWED, REASON_UNLISTED_ALLOWED),
        // not web pages
        ("chrome://settings", ALLOWED, REASON_NOT_WEB),
        ("about:blank", ALLOWED, REASON_NOT_WEB),
        ("file:///etc/hosts", ALLOWED, REASON_NOT_WEB),
        ("ftp://youtube.com/", ALLOWED, REASON_NOT_WEB),
        ("javascript:alert(1)", ALLOWED, REASON_NOT_WEB),
        ("http://[::1]/", ALLOWED, REASON_NOT_WEB),
        ("not a url", ALLOWED, REASON_NOT_WEB),
        ("", ALLOWED, REASON_NOT_WEB),
    ];
    assert!(cases.len() >= 40);
    let rules = normal_rules();
    for (url, blocked, reason) in cases {
        assert_eq!(judge_fresh(url, &rules, STUDY), (*blocked, *reason), "url: {url}");
    }
}

#[test]
fn strict_mode_table() {
    let cases: &[(&str, bool, &str)] = &[
        ("https://github.com/rust-lang/rust", ALLOWED, REASON_ALLOWED_LISTED),
        ("https://gist.github.com/x", ALLOWED, REASON_ALLOWED_LISTED),
        ("https://docs.rs/serde", ALLOWED, REASON_ALLOWED_LISTED),
        ("https://evil-github.com/", BLOCKED, REASON_STRICT_UNLISTED),
        ("https://github.com.evil.com/", BLOCKED, REASON_STRICT_UNLISTED),
        ("https://wiki.example.com/notes/tcp", ALLOWED, REASON_ALLOWED_LISTED),
        ("https://wiki.example.com/other", BLOCKED, REASON_STRICT_UNLISTED),
        ("https://example.org/", BLOCKED, REASON_STRICT_UNLISTED),
        // an explicit block reason wins over the generic strict reason
        ("https://www.youtube.com/", BLOCKED, REASON_BLOCKED_SITE),
        ("https://www.youtube.com/watch?list=PLstudy", ALLOWED, REASON_ALLOWED_STUDY_VIDEO),
        ("https://open.spotify.com/", ALLOWED, REASON_ALLOWED_MUSIC),
        // the user's list replaces the built-in set
        ("https://developer.mozilla.org/", BLOCKED, REASON_STRICT_UNLISTED),
        ("chrome://extensions", ALLOWED, REASON_NOT_WEB),
    ];
    let rules = strict_rules();
    for (url, blocked, reason) in cases {
        assert_eq!(judge_fresh(url, &rules, STUDY), (*blocked, *reason), "url: {url}");
    }
}

#[test]
fn strict_mode_without_a_user_list_uses_the_built_in_docs_set() {
    let rules = rules(&[], r#"{"strict":true}"#);
    assert_eq!(judge_fresh("https://github.com/x", &rules, STUDY), (ALLOWED, REASON_ALLOWED_LISTED));
    assert_eq!(judge_fresh("https://developer.mozilla.org/", &rules, STUDY), (ALLOWED, REASON_ALLOWED_LISTED));
    assert_eq!(judge_fresh("https://example.org/", &rules, STUDY), (BLOCKED, REASON_STRICT_UNLISTED));
}

#[test]
fn strict_and_normal_differ_only_for_unlisted_sites() {
    let normal = rules(&["netflix.com"], "{}");
    let strict = rules(&["netflix.com"], r#"{"strict":true}"#);
    let unlisted = "https://news.example.com/";
    assert_eq!(judge_fresh(unlisted, &normal, STUDY), (ALLOWED, REASON_UNLISTED_ALLOWED));
    assert_eq!(judge_fresh(unlisted, &strict, STUDY), (BLOCKED, REASON_STRICT_UNLISTED));
    let listed = "https://netflix.com/";
    assert_eq!(judge_fresh(listed, &normal, STUDY), judge_fresh(listed, &strict, STUDY));
}

#[test]
fn outside_study_everything_is_allowed_and_a_pass_is_recorded_as_such() {
    let rules = strict_rules();
    let outside = Situation { in_study: false, pass_active: false };
    let outside_with_pass = Situation { in_study: false, pass_active: true };
    let pass = Situation { in_study: true, pass_active: true };
    let cases: &[(&str, Situation, bool, &str)] = &[
        ("https://www.youtube.com/", outside, ALLOWED, REASON_OUTSIDE_STUDY),
        ("https://example.org/", outside, ALLOWED, REASON_OUTSIDE_STUDY),
        ("https://www.youtube.com/", outside_with_pass, ALLOWED, REASON_OUTSIDE_STUDY),
        ("chrome://settings", outside, ALLOWED, REASON_OUTSIDE_STUDY),
        // a pass turns a block into a recorded allow...
        ("https://www.youtube.com/", pass, ALLOWED, REASON_SITE_ALLOWED_PASS),
        ("https://example.org/", pass, ALLOWED, REASON_SITE_ALLOWED_PASS),
        // ...but does not relabel what was allowed anyway
        ("https://github.com/", pass, ALLOWED, REASON_ALLOWED_LISTED),
        ("https://open.spotify.com/", pass, ALLOWED, REASON_ALLOWED_MUSIC),
        ("chrome://settings", pass, ALLOWED, REASON_NOT_WEB),
    ];
    for (url, situation, blocked, reason) in cases {
        assert_eq!(judge_fresh(url, &rules, *situation), (*blocked, *reason), "url: {url} {situation:?}");
    }
}

#[test]
fn cache_is_keyed_by_domain_and_skips_path_sensitive_hosts() {
    let rules = normal_rules();
    let mut cache = DecisionCache::default();
    for url in [
        "https://reddit.com/r/secret-subreddit?token=SECRETQ",
        "https://reddit.com/another/path",
        "https://example.org/private/path#frag",
        "https://www.youtube.com/watch?v=abc&list=PLstudy",
        "https://www.youtube.com/watch?v=abc",
        "https://kakao.com/talk",
        "https://kakao.com/",
        "https://github.com/myorg/x",
        "https://github.com/other",
    ] {
        judge(url, &rules, STUDY, &mut cache);
    }
    let mut keys: Vec<_> = cache.by_domain.keys().cloned().collect();
    keys.sort();
    // youtube.com, kakao.com, github.com have path or query rules: never cached by domain.
    assert_eq!(keys, vec!["example.org".to_owned(), "reddit.com".to_owned()]);
    let dump = format!("{cache:?}");
    for forbidden in ["secret-subreddit", "SECRETQ", "private", "frag", "another", "PLstudy"] {
        assert!(!dump.contains(forbidden), "cache leaked `{forbidden}`: {dump}");
    }
    // The same page gives the same decision from the cache.
    let first = judge("https://reddit.com/x", &rules, STUDY, &mut cache);
    let second = judge("https://reddit.com/x", &rules, STUDY, &mut DecisionCache::default());
    assert_eq!(first, second);
}

#[test]
fn cache_decision_does_not_hide_a_pass() {
    let rules = rules(&["reddit.com"], "{}");
    let mut cache = DecisionCache::default();
    let blocked = judge("https://reddit.com/", &rules, STUDY, &mut cache);
    assert_eq!((blocked.block, blocked.reason), (BLOCKED, REASON_BLOCKED_SITE));
    let pass = judge("https://reddit.com/", &rules, Situation { in_study: true, pass_active: true }, &mut cache);
    assert_eq!((pass.block, pass.reason), (ALLOWED, REASON_SITE_ALLOWED_PASS));
    let after = judge("https://reddit.com/", &rules, STUDY, &mut cache);
    assert!(after.block);
}

#[test]
fn cache_is_bounded() {
    let rules = rules(&[], "{}");
    let mut cache = DecisionCache::default();
    for i in 0..(MAX_CACHED_DOMAINS + 10) {
        judge(&format!("https://site{i}.example/"), &rules, STUDY, &mut cache);
    }
    assert!(cache.by_domain.len() <= MAX_CACHED_DOMAINS);
}

#[test]
fn pattern_parsing_normalizes_and_rejects() {
    let ok: &[(&str, &str, Option<&str>)] = &[
        ("YouTube.com", "youtube.com", None),
        ("https://www.Reddit.com/", "www.reddit.com", None),
        ("*.discord.com", "discord.com", None),
        ("kakao.com/talk/", "kakao.com", Some("/talk")),
        ("kakao.com/a/../Talk", "kakao.com", Some("/talk")),
        ("10.0.0.7", "10.0.0.7", None),
    ];
    for (raw, host, path) in ok {
        let parsed = SitePattern::parse(raw).unwrap_or_else(|| panic!("`{raw}` should parse"));
        assert_eq!((parsed.host.as_str(), parsed.path.as_deref()), (*host, *path), "{raw}");
    }
    let unicode = SitePattern::parse("한글.kr").expect("unicode");
    assert!(unicode.host.starts_with("xn--"), "{}", unicode.host);
    for bad in ["", "   ", "/talk", "a@b.com", "youtube.com:8080", "you tube.com", "a.com/x?y=1", "a.com/x#f", "[::1]", "a%2eb.com"] {
        assert!(SitePattern::parse(bad).is_none(), "`{bad}` should be rejected");
    }
}

#[test]
fn unusable_entries_are_counted_not_applied() {
    let rules = rules(&["youtube.com", "", "a b.com"], r#"{"allow":["??"],"youtubePlaylists":["bad id", "PLok"],"youtubeChannels":["@", "UC/x"]}"#);
    assert_eq!(rules.skipped_entries, 6);
    assert_eq!(judge_fresh("https://youtube.com/", &rules, STUDY), (BLOCKED, REASON_BLOCKED_SITE));
    assert_eq!(judge_fresh("https://youtube.com/watch?list=PLok", &rules, STUDY), (ALLOWED, REASON_ALLOWED_STUDY_VIDEO));
}

fn db_with(entries: &[&str], stored: Option<&str>) -> Arc<Mutex<Connection>> {
    let conn = open_in_memory().expect("db");
    for entry in entries {
        assert!(store::insert_block_entry(&conn, EntryKind::Site, entry, EntrySource::User, None, 1).expect("insert"));
    }
    if let Some(json) = stored {
        store::put_setting(&conn, SITE_RULES_KEY, "browser", json, 1).expect("setting");
    }
    Arc::new(Mutex::new(conn))
}

#[test]
fn load_reads_block_entries_and_the_setting() {
    let db = db_with(&["youtube.com", "kakao.com/talk"], Some(r#"{"strict":true,"allow":["docs.rs"]}"#));
    let rules = load(&db.lock().expect("db")).expect("load");
    assert_eq!(judge_fresh("https://youtube.com/", &rules, STUDY), (BLOCKED, REASON_BLOCKED_SITE));
    assert_eq!(judge_fresh("https://docs.rs/", &rules, STUDY), (ALLOWED, REASON_ALLOWED_LISTED));
    assert_eq!(judge_fresh("https://example.org/", &rules, STUDY), (BLOCKED, REASON_STRICT_UNLISTED));
}

#[test]
fn load_defaults_without_a_setting_and_rejects_a_corrupt_one() {
    let empty = db_with(&[], None);
    let rules = load(&empty.lock().expect("db")).expect("load");
    assert_eq!(judge_fresh("https://example.org/", &rules, STUDY), (ALLOWED, REASON_UNLISTED_ALLOWED));
    for corrupt in ["not json", r#"{"strict":"yes"}"#, r#"{"unknownField":1}"#] {
        let db = db_with(&[], Some(corrupt));
        assert!(load(&db.lock().expect("db")).is_err(), "{corrupt}");
    }
}

#[test]
fn disabled_entries_do_not_block() {
    let db = db_with(&["youtube.com"], None);
    db.lock().expect("db").execute("UPDATE block_entries SET enabled = 0", []).expect("disable");
    let rules = load(&db.lock().expect("db")).expect("load");
    assert_eq!(judge_fresh("https://youtube.com/", &rules, STUDY), (ALLOWED, REASON_UNLISTED_ALLOWED));
}

#[test]
fn engine_reloads_per_segment_and_fails_open_on_bad_rules() {
    let db = db_with(&["youtube.com"], None);
    let engine = SiteRuleEngine::new(Arc::clone(&db));
    let url = "https://reddit.com/";
    let first = engine.decide(url, (Some(1), Some(1)), STUDY);
    assert_eq!((first.block, first.reason), (ALLOWED, REASON_UNLISTED_ALLOWED));
    // Rules cannot change in a locked segment, so a changed table is not seen until the next one.
    store::insert_block_entry(&db.lock().expect("db"), EntryKind::Site, "reddit.com", EntrySource::User, None, 2).expect("insert");
    assert!(!engine.decide(url, (Some(1), Some(1)), STUDY).block);
    assert!(engine.decide(url, (Some(1), Some(2)), STUDY).block);

    store::put_setting(&db.lock().expect("db"), SITE_RULES_KEY, "browser", "garbage", 3).expect("setting");
    let failed = engine.decide(url, (Some(1), Some(3)), STUDY);
    assert_eq!((failed.block, failed.reason), (ALLOWED, REASON_APP_ERROR));
}

#[test]
fn judging_and_loading_log_and_store_no_url_part() {
    let secret = "https://reddit.com/r/SECRET-PATH?token=SECRETQ#SECRET-FRAG";
    let db = db_with(&["reddit.com", "bad entry with spaces"], None);
    let engine = SiteRuleEngine::new(Arc::clone(&db));
    let logs = capture_logs(|| {
        assert!(engine.decide(secret, (None, None), STUDY).block);
        assert!(!engine.decide("not a url SECRET-PATH", (None, None), STUDY).block);
    });
    for forbidden in ["SECRET-PATH", "SECRETQ", "SECRET-FRAG", "token", "r/SECRET"] {
        assert!(!logs.contains(forbidden), "log leaked `{forbidden}`: {logs}");
    }
    let state = engine.state.lock().expect("state");
    let dump = format!("{:?}", state.cache);
    assert!(dump.contains("reddit.com"), "{dump}");
    assert!(!dump.contains("SECRET"), "{dump}");
}
