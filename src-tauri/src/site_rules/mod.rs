//! Site rules: which web pages a locked study block allows. Judgement is pure (`judge`) and runs
//! on the URL in memory only; what leaves it is a domain and a fixed reason code. Nothing here
//! logs or stores a URL, path, query, or title, and the decision cache is keyed by domain.
//!
//! With the LLM stage not built, the fixed fallback is "LLM off" (docs/PLAN.md feature 2): a page
//! no rule covers is allowed in normal mode and blocked in strict mode. Order, first match wins:
//!   1. user allow list (domain or domain/path)        -> allow
//!   2. registered YouTube study playlists and channels -> allow
//!   3. block list (preset and user entries)            -> block
//!   4. music sites and apps                            -> allow
//!   5. strict mode: built-in docs set, else            -> block
//!   6. everything else                                 -> allow
//!
//! Outside a locked study block everything is allowed, and an emergency pass turns every block
//! into `site_allowed_pass` so the lenience is recorded.
//!
//! Known limit: the channel of a plain `/watch?v=` page is not in its URL, so channel-based
//! allowance only covers channel pages and playlist links. Judging those by title is stage 3.

pub mod edit;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::commands::lock_db;
use crate::enforce::model::EntryKind;
use crate::error::{AppError, AppResult};
use crate::store;

/// Fixed identifiers the extension, the log, and `block_events.detail_code` see.
pub const REASON_BLOCKED_SITE: &str = "blocked_site";
pub const REASON_STRICT_UNLISTED: &str = "strict_unlisted";
pub const REASON_ALLOWED_LISTED: &str = "allowed_listed";
pub const REASON_ALLOWED_STUDY_VIDEO: &str = "allowed_study_video";
pub const REASON_ALLOWED_MUSIC: &str = "allowed_music";
pub const REASON_UNLISTED_ALLOWED: &str = "unlisted_allowed";
pub const REASON_NOT_WEB: &str = "not_web";
pub const REASON_OUTSIDE_STUDY: &str = "outside_study";
pub const REASON_APP_ERROR: &str = "app_error";
/// A pass lets a site through that the rules would have blocked.
pub const REASON_SITE_ALLOWED_PASS: &str = "site_allowed_pass";

/// Settings key of the JSON the user edits: strict mode, allow list, YouTube study entries.
/// Block entries live in `block_entries` (kind `site`).
pub const SITE_RULES_KEY: &str = "siteRules";

const MAX_CACHED_DOMAINS: usize = 1024;
const MAX_ENTRY_CHARS: usize = 512;

/// Music apps' web players stay reachable by default (PLAN feature 2). Matching is by domain suffix.
const MUSIC_DOMAINS: &[&str] = &["spotify.com", "melon.com", "genie.co.kr", "bugs.co.kr", "music.apple.com"];
/// Strict-mode allow list used until the user registers their own. A minimal set pending the
/// user's confirmation (decision D-23).
const STRICT_DEFAULT_ALLOW: &[&str] =
    &["github.com", "developer.mozilla.org", "docs.rs", "doc.rust-lang.org", "docs.python.org", "learn.microsoft.com"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub block: bool,
    pub reason: &'static str,
}

impl Decision {
    fn allow(reason: &'static str) -> Decision {
        Decision { block: false, reason }
    }
}

/// What the app knows about the moment, besides the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Situation {
    /// A locked study block is running (phase study, lock held). Rules apply only then.
    pub in_study: bool,
    /// An emergency pass is active.
    pub pass_active: bool,
}

/// The rules' verdict before the situation is applied; this is what the cache keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Allow(&'static str),
    Block(&'static str),
}

// ---- URL as the rules see it ----

#[derive(Debug)]
struct HostKey {
    name: String,
    /// Names match by suffix; IP addresses match exactly.
    is_domain: bool,
}

fn host_key(url: &Url) -> Option<HostKey> {
    match url.host()? {
        Host::Domain(name) => Some(HostKey { name: name.trim_end_matches('.').to_owned(), is_domain: true }),
        Host::Ipv4(ip) => Some(HostKey { name: ip.to_string(), is_domain: false }),
        Host::Ipv6(_) => None,
    }
}

/// `host` is `pattern` or a subdomain of it. `evil-youtube.com` and `youtube.com.evil.com` are not.
fn host_matches(pattern: &str, host: &HostKey) -> bool {
    if host.name == pattern {
        return true;
    }
    let (name, suffix) = (host.name.as_bytes(), pattern.as_bytes());
    host.is_domain && name.len() > suffix.len() && name.ends_with(suffix) && name[name.len() - suffix.len() - 1] == b'.'
}

/// Parsed page address, alive only for one judgement.
struct Target {
    host: HostKey,
    path: String,
    /// Values of every `list=` query parameter.
    lists: Vec<String>,
}

impl Target {
    /// `None` for anything that is not an http(s) page with a host.
    fn parse(raw: &str) -> Option<Target> {
        let url = Url::parse(raw.trim()).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let host = host_key(&url)?;
        let lists = url.query_pairs().filter(|(key, _)| key == "list").map(|(_, value)| value.into_owned()).collect();
        Some(Target { host, path: url.path().to_owned(), lists })
    }
}

// ---- patterns ----

/// `domain` or `domain/path-prefix`, as written in the block list or allow list.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SitePattern {
    host: String,
    /// Lowercase, no trailing slash; matches at a segment boundary.
    path: Option<String>,
}

impl SitePattern {
    fn parse(raw: &str) -> Option<SitePattern> {
        if raw.chars().count() > MAX_ENTRY_CHARS {
            return None;
        }
        let lowered = raw.trim().to_lowercase();
        let rest = lowered.strip_prefix("https://").or_else(|| lowered.strip_prefix("http://")).unwrap_or(&lowered);
        let rest = rest.strip_prefix("*.").unwrap_or(rest);
        let (host_part, path_part) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let bad_host = host_part.is_empty() || host_part.contains(|c: char| c.is_whitespace() || "@:?#\\%".contains(c));
        if bad_host || path_part.contains(['?', '#']) {
            return None;
        }
        // Parsing resolves `..` and punycodes Unicode names the same way a tab URL is resolved.
        let url = Url::parse(&format!("https://{host_part}{path_part}")).ok()?;
        let host = host_key(&url)?.name;
        let path = url.path().trim_end_matches('/').to_lowercase();
        Some(SitePattern { host, path: (!path.is_empty()).then_some(path) })
    }

    /// `domain` or `domain/path`: the form stored in the block list and the allow list.
    fn canonical(&self) -> String {
        match &self.path {
            Some(path) => format!("{}{path}", self.host),
            None => self.host.clone(),
        }
    }

    fn matches(&self, target: &Target) -> bool {
        if !host_matches(&self.host, &target.host) {
            return false;
        }
        match &self.path {
            None => true,
            Some(prefix) => {
                let path = target.path.to_lowercase();
                path.strip_prefix(prefix.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            }
        }
    }
}

/// The stored form of a user-typed site entry, or `None` when it is not a usable domain or
/// `domain/path`. Block-list and allow-list entries pass through here before they are saved.
pub fn normalize_pattern(raw: &str) -> Option<String> {
    SitePattern::parse(raw).map(|pattern| pattern.canonical())
}

/// Sites that stay open in strict mode until the user registers an allow list of their own.
pub fn strict_default_allow() -> &'static [&'static str] {
    STRICT_DEFAULT_ALLOW
}

fn any_match(patterns: &[SitePattern], target: &Target) -> bool {
    patterns.iter().any(|p| p.matches(target))
}

// ---- YouTube study entries ----

#[derive(Default, Debug)]
struct StudyYoutube {
    /// Playlist IDs (`list=`), case-sensitive.
    playlists: HashSet<String>,
    /// First path segment of a handle page, lowercase, with the `@`.
    handles: HashSet<String>,
    /// Channel IDs of `/channel/<id>` pages, case-sensitive.
    channels: HashSet<String>,
}

fn youtube_host(host: &HostKey) -> bool {
    host.is_domain && (host_matches("youtube.com", host) || host.name == "youtu.be")
}

fn plain_id(raw: &str) -> Option<&str> {
    let id = raw.trim();
    let ok = !id.is_empty()
        && id.chars().count() <= MAX_ENTRY_CHARS
        && id.chars().all(|c| !c.is_whitespace() && !c.is_control() && !"/?#&=\\%".contains(c));
    ok.then_some(id)
}

impl StudyYoutube {
    fn is_empty(&self) -> bool {
        self.playlists.is_empty() && self.handles.is_empty() && self.channels.is_empty()
    }

    /// Returns false for an entry that is not a usable ID.
    fn add_channel(&mut self, raw: &str) -> bool {
        let Some(id) = plain_id(raw) else { return false };
        match id.strip_prefix('@') {
            // The URL parser encodes a non-ASCII handle; entries get the same encoding.
            Some(handle) if !handle.is_empty() => match Url::parse(&format!("https://youtube.com/@{handle}")) {
                Ok(url) => {
                    let segment = url.path().trim_start_matches('/').to_lowercase();
                    self.handles.insert(segment)
                }
                Err(_) => return false,
            },
            Some(_) => return false,
            None => self.channels.insert(id.to_owned()),
        };
        true
    }

    fn allows(&self, target: &Target) -> bool {
        if !youtube_host(&target.host) {
            return false;
        }
        if target.lists.iter().any(|list| self.playlists.contains(list)) {
            return true;
        }
        let mut segments = target.path.split('/').filter(|s| !s.is_empty());
        match (segments.next(), segments.next()) {
            (Some(first), _) if first.starts_with('@') => self.handles.contains(&first.to_lowercase()),
            (Some("channel"), Some(id)) => self.channels.contains(id),
            _ => false,
        }
    }
}

// ---- the rule set ----

/// What the user stores under [`SITE_RULES_KEY`]. `allow` absent means "use the built-in strict
/// set"; present (even empty) replaces it.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SiteRulesConfig {
    #[serde(default)]
    pub strict: bool,
    pub allow: Option<Vec<String>>,
    #[serde(default)]
    pub youtube_playlists: Vec<String>,
    #[serde(default)]
    pub youtube_channels: Vec<String>,
}

#[derive(Default, Debug)]
pub struct SiteRules {
    strict: bool,
    block: Vec<SitePattern>,
    allow: Vec<SitePattern>,
    strict_defaults: Vec<SitePattern>,
    youtube: StudyYoutube,
    /// Entries that were not usable and were left out (count only: no entry text is logged).
    skipped_entries: usize,
}

/// Parses every entry, counting the ones that are not usable.
fn parse_all(raw: &[String], skipped: &mut usize) -> Vec<SitePattern> {
    raw.iter()
        .filter_map(|entry| {
            let parsed = SitePattern::parse(entry);
            *skipped += usize::from(parsed.is_none());
            parsed
        })
        .collect()
}

impl SiteRules {
    fn build(block_patterns: &[String], stored: SiteRulesConfig) -> SiteRules {
        let mut rules = SiteRules { strict: stored.strict, ..SiteRules::default() };
        let mut skipped = 0;
        rules.block = parse_all(block_patterns, &mut skipped);
        match &stored.allow {
            Some(allow) => rules.allow = parse_all(allow, &mut skipped),
            None => rules.strict_defaults = STRICT_DEFAULT_ALLOW.iter().filter_map(|d| SitePattern::parse(d)).collect(),
        }
        for list in &stored.youtube_playlists {
            match plain_id(list) {
                Some(id) => {
                    rules.youtube.playlists.insert(id.to_owned());
                }
                None => skipped += 1,
            }
        }
        for channel in &stored.youtube_channels {
            skipped += usize::from(!rules.youtube.add_channel(channel));
        }
        rules.skipped_entries = skipped;
        rules
    }

    fn outcome(&self, target: &Target) -> Outcome {
        if any_match(&self.allow, target) {
            return Outcome::Allow(REASON_ALLOWED_LISTED);
        }
        if self.youtube.allows(target) {
            return Outcome::Allow(REASON_ALLOWED_STUDY_VIDEO);
        }
        if any_match(&self.block, target) {
            return Outcome::Block(REASON_BLOCKED_SITE);
        }
        if MUSIC_DOMAINS.iter().any(|domain| host_matches(domain, &target.host)) {
            return Outcome::Allow(REASON_ALLOWED_MUSIC);
        }
        if self.strict {
            return if any_match(&self.strict_defaults, target) {
                Outcome::Allow(REASON_ALLOWED_LISTED)
            } else {
                Outcome::Block(REASON_STRICT_UNLISTED)
            };
        }
        Outcome::Allow(REASON_UNLISTED_ALLOWED)
    }

    /// The outcome for this host can depend on the path or query, so it must not be cached by
    /// domain alone.
    fn path_sensitive(&self, host: &HostKey) -> bool {
        let pathful = |patterns: &[SitePattern]| patterns.iter().any(|p| p.path.is_some() && host_matches(&p.host, host));
        pathful(&self.block)
            || pathful(&self.allow)
            || pathful(&self.strict_defaults)
            || (youtube_host(host) && !self.youtube.is_empty())
    }
}

/// Reads the current rules: block entries from `block_entries`, the rest from the setting. A
/// stored value that does not parse is an error rather than silently empty rules.
pub fn load(conn: &Connection) -> AppResult<SiteRules> {
    let block = store::list_enabled_patterns(conn, EntryKind::Site)?;
    let stored = match store::get_setting(conn, SITE_RULES_KEY)? {
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|err| AppError::Internal(format!("{SITE_RULES_KEY} setting is invalid: {err}")))?,
        None => SiteRulesConfig::default(),
    };
    Ok(SiteRules::build(&block, stored))
}

// ---- judgement ----

/// Decisions per domain. Only domains whose outcome ignores path and query are kept, so a hit is
/// always correct; no URL part is ever a key.
#[derive(Default, Debug)]
pub struct DecisionCache {
    by_domain: HashMap<String, Outcome>,
}

impl DecisionCache {
    fn put(&mut self, domain: &str, outcome: Outcome) {
        if self.by_domain.len() >= MAX_CACHED_DOMAINS {
            self.by_domain.clear();
        }
        self.by_domain.insert(domain.to_owned(), outcome);
    }
}

/// Judges one page. `cache` must have been filled under the same `rules`.
pub fn judge(url: &str, rules: &SiteRules, situation: Situation, cache: &mut DecisionCache) -> Decision {
    if !situation.in_study {
        return Decision::allow(REASON_OUTSIDE_STUDY);
    }
    let Some(target) = Target::parse(url) else { return Decision::allow(REASON_NOT_WEB) };
    let outcome = match cache.by_domain.get(&target.host.name) {
        Some(outcome) => *outcome,
        None => {
            let outcome = rules.outcome(&target);
            if !rules.path_sensitive(&target.host) {
                cache.put(&target.host.name, outcome);
            }
            outcome
        }
    };
    match outcome {
        Outcome::Allow(reason) => Decision::allow(reason),
        Outcome::Block(_) if situation.pass_active => Decision::allow(REASON_SITE_ALLOWED_PASS),
        Outcome::Block(reason) => Decision { block: true, reason },
    }
}

/// (run, segment) of the study block being judged.
pub type Scope = (Option<i64>, Option<i64>);

struct EngineState {
    scope: Option<Scope>,
    rules: SiteRules,
    cache: DecisionCache,
}

/// Rules plus cache for the running app. Rules are read once per study segment: the block list and
/// the setting cannot change while the lock is held, so a segment never sees a stale list.
pub struct SiteRuleEngine {
    db: Arc<Mutex<Connection>>,
    state: Mutex<EngineState>,
}

impl SiteRuleEngine {
    pub fn new(db: Arc<Mutex<Connection>>) -> SiteRuleEngine {
        SiteRuleEngine {
            db,
            state: Mutex::new(EngineState { scope: None, rules: SiteRules::default(), cache: DecisionCache::default() }),
        }
    }

    /// Fails open: rules that cannot be read allow the page and say `app_error`.
    pub fn decide(&self, url: &str, scope: Scope, situation: Situation) -> Decision {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.scope != Some(scope) {
            let loaded = lock_db(&self.db).and_then(|conn| load(&conn));
            match loaded {
                Ok(rules) => {
                    if rules.skipped_entries > 0 {
                        tracing::warn!(event = "site_rules_entries_skipped", count = rules.skipped_entries);
                    }
                    *state = EngineState { scope: Some(scope), rules, cache: DecisionCache::default() };
                }
                Err(err) => {
                    tracing::error!(error = %err, "site rules unavailable; allowing");
                    return Decision::allow(REASON_APP_ERROR);
                }
            }
        }
        let EngineState { rules, cache, .. } = &mut *state;
        judge(url, rules, situation, cache)
    }
}
