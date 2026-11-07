//! Shared-wiki semantics of the in-memory backend (S4.7): the same rules as
//! `supabase/migrations/*_shared_wiki.sql`, written a second time in Rust so many users can be
//! exercised against them. It mirrors the SQL; it does not prove the SQL (V4-4, V4-1).
//!
//! Nothing here is reachable through the generic table store: clients only get the four RPCs.

use serde_json::{Map, Value, json};

use super::rooms::{is_member, member_nickname};
use super::{Caller, DataError, FakeBackend, State};

pub(super) const SHARED_WIKI_RPCS: &[&str] = &[
    "publish_shared_page",
    "withdraw_shared_page",
    "list_shared_pages",
    "save_merged_page",
];

const MAX_BODY_CHARS: usize = 20_000;
const MAX_PAGES_PER_ROOM: usize = 100;
const MAX_PAGES_PER_MEMBER: usize = 20;
const MAX_LIVE_REVISIONS_PER_MEMBER: usize = 50;
const MAX_REVISIONS_PER_PAGE: usize = 500;

struct PageRow {
    id: String,
    room_id: String,
    title: String,
}

struct RevRow {
    id: String,
    page_id: String,
    user_id: String,
    merge: bool,
    body: String,
    base: Option<String>,
    branched: bool,
    allow_llm_merge: bool,
    contributors: Vec<String>,
    created_ms: i64,
    seq: u64,
    withdrawn: bool,
}

#[derive(Default)]
pub(super) struct SharedWikiState {
    seq: u64,
    pages: Vec<PageRow>,
    revs: Vec<RevRow>,
}

fn code(c: &str) -> Value {
    json!({ "code": c })
}

fn text<'a>(args: &'a Map<String, Value>, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn page_id(state: &State, room: &str, title: &str) -> Option<String> {
    state
        .shared_wiki
        .pages
        .iter()
        .find(|p| p.room_id == room && p.title == title)
        .map(|p| p.id.clone())
}

impl FakeBackend {
    pub(super) fn shared_wiki_rpc(
        &self,
        caller: &Caller,
        name: &str,
        args: &Map<String, Value>,
    ) -> Result<Value, DataError> {
        let user = Self::user_of(caller)?.to_string();
        let now = self.now();
        let mut state = self.lock();
        let state = &mut *state;
        Ok(match name {
            "publish_shared_page" => publish(state, &user, now, args),
            "withdraw_shared_page" => withdraw(state, &user, args),
            "list_shared_pages" => list(state, &user, args),
            "save_merged_page" => save_merged(state, &user, now, args),
            _ => return Err(DataError::NotFound),
        })
    }

    /// Test helper: every body text the server still holds for a room (tombstoned ones are empty).
    pub fn shared_wiki_bodies(&self, room_id: &str) -> Vec<String> {
        let state = self.lock();
        let pages: Vec<&str> = state
            .shared_wiki
            .pages
            .iter()
            .filter(|p| p.room_id == room_id)
            .map(|p| p.id.as_str())
            .collect();
        state
            .shared_wiki
            .revs
            .iter()
            .filter(|r| pages.contains(&r.page_id.as_str()))
            .map(|r| r.body.clone())
            .collect()
    }

    /// Test helper: number of revision rows (live or tombstoned) of a room.
    pub fn shared_wiki_revision_count(&self, room_id: &str) -> usize {
        let state = self.lock();
        let pages: Vec<&str> = state
            .shared_wiki
            .pages
            .iter()
            .filter(|p| p.room_id == room_id)
            .map(|p| p.id.as_str())
            .collect();
        state
            .shared_wiki
            .revs
            .iter()
            .filter(|r| pages.contains(&r.page_id.as_str()))
            .count()
    }
}

fn publish(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let room = text(args, "p_room_id").to_string();
    if !is_member(state, &room, user) {
        return code("not_member");
    }
    let title = text(args, "p_title").trim().to_string();
    let body = text(args, "p_body").to_string();
    let base = args
        .get("p_base_revision")
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(allow) = args.get("p_allow_llm_merge").and_then(Value::as_bool) else {
        return code("invalid_input");
    };
    if !(1..=80).contains(&chars(&title))
        || title.chars().any(char::is_control)
        || body.trim().is_empty()
    {
        return code("invalid_input");
    }
    if chars(&body) > MAX_BODY_CHARS {
        return code("too_large");
    }
    let existing = page_id(state, &room, &title);
    match &existing {
        None => {
            if state
                .shared_wiki
                .pages
                .iter()
                .filter(|p| p.room_id == room)
                .count()
                >= MAX_PAGES_PER_ROOM
            {
                return code("too_many");
            }
        }
        Some(page) => {
            if base.as_ref().is_some_and(|b| {
                !state
                    .shared_wiki
                    .revs
                    .iter()
                    .any(|r| r.id == *b && r.page_id == *page)
            }) {
                return code("invalid_input");
            }
            if state
                .shared_wiki
                .revs
                .iter()
                .filter(|r| r.page_id == *page)
                .count()
                >= MAX_REVISIONS_PER_PAGE
            {
                return code("too_many");
            }
        }
    }
    let mut mine: Vec<&str> = state
        .shared_wiki
        .revs
        .iter()
        .filter(|r| r.user_id == user && !r.merge && !r.withdrawn)
        .filter_map(|r| {
            state
                .shared_wiki
                .pages
                .iter()
                .find(|p| p.id == r.page_id && p.room_id == room)
        })
        .filter(|p| existing.as_deref() != Some(p.id.as_str()))
        .map(|p| p.id.as_str())
        .collect();
    mine.sort_unstable();
    mine.dedup();
    if mine.len() >= MAX_PAGES_PER_MEMBER {
        return code("too_many");
    }

    state.shared_wiki.seq += 1;
    let page = match existing {
        Some(page) => page,
        None => {
            let id = format!("30000000-0000-4000-8000-{:012}", state.shared_wiki.seq);
            state.shared_wiki.pages.push(PageRow {
                id: id.clone(),
                room_id: room.clone(),
                title,
            });
            id
        }
    };
    let live_mine = state
        .shared_wiki
        .revs
        .iter()
        .filter(|r| r.page_id == page && r.user_id == user && !r.merge && !r.withdrawn)
        .count();
    if live_mine >= MAX_LIVE_REVISIONS_PER_MEMBER {
        return code("too_many");
    }
    let head = state
        .shared_wiki
        .revs
        .iter()
        .filter(|r| r.page_id == page && !r.withdrawn)
        .max_by_key(|r| (r.created_ms, r.seq))
        .map(|r| r.id.clone());
    let has_mine = state
        .shared_wiki
        .revs
        .iter()
        .any(|r| r.page_id == page && r.user_id == user && !r.merge);
    let branched = head.is_some() && base != head && (base.is_some() || has_mine);

    state.shared_wiki.seq += 1;
    let seq = state.shared_wiki.seq;
    let id = format!("31000000-0000-4000-8000-{seq:012}");
    state.shared_wiki.revs.push(RevRow {
        id: id.clone(),
        page_id: page.clone(),
        user_id: user.to_string(),
        merge: false,
        body,
        base,
        branched,
        allow_llm_merge: allow,
        contributors: vec![user.to_string()],
        created_ms: now,
        seq,
        withdrawn: false,
    });
    json!({"code": "ok", "page_id": page, "revision_id": id, "branched": branched})
}

fn withdraw(state: &mut State, user: &str, args: &Map<String, Value>) -> Value {
    let room = text(args, "p_room_id").to_string();
    if !is_member(state, &room, user) {
        return code("not_member");
    }
    let Some(page) = page_id(state, &room, text(args, "p_title").trim()) else {
        return code("not_found");
    };
    let mut count = 0;
    for rev in
        state.shared_wiki.revs.iter_mut().filter(|r| {
            r.page_id == page && !r.withdrawn && r.contributors.iter().any(|c| c == user)
        })
    {
        rev.body.clear();
        rev.withdrawn = true;
        count += 1;
    }
    if count == 0 {
        return code("not_shared");
    }
    json!({"code": "ok", "withdrawn": count})
}

fn list(state: &State, user: &str, args: &Map<String, Value>) -> Value {
    let room = text(args, "p_room_id");
    if !is_member(state, room, user) {
        return code("not_member");
    }
    let mut pages: Vec<&PageRow> = state
        .shared_wiki
        .pages
        .iter()
        .filter(|p| p.room_id == room)
        .collect();
    pages.sort_by(|a, b| a.title.cmp(&b.title));
    let mut out = Vec::new();
    for page in pages {
        let live: Vec<&RevRow> = state
            .shared_wiki
            .revs
            .iter()
            .filter(|r| r.page_id == page.id && !r.withdrawn)
            .collect();
        let mut heads: Vec<&RevRow> = Vec::new();
        let mut authors: Vec<&str> = live
            .iter()
            .filter(|r| !r.merge)
            .map(|r| r.user_id.as_str())
            .collect();
        authors.sort_unstable();
        authors.dedup();
        for author in authors {
            if !is_member(state, room, author) {
                continue;
            }
            if let Some(head) = live
                .iter()
                .filter(|r| !r.merge && r.user_id == author)
                .max_by_key(|r| (r.created_ms, r.seq))
            {
                heads.push(head);
            }
        }
        let latest_merge = live
            .iter()
            .filter(|r| r.merge && r.contributors.iter().all(|c| is_member(state, room, c)))
            .max_by_key(|r| (r.created_ms, r.seq));
        if let Some(merge) = latest_merge {
            heads.push(merge);
        }
        if heads.is_empty() {
            continue;
        }
        heads.sort_by_key(|r| (r.created_ms, r.seq));
        let revisions: Vec<Value> = heads
            .iter()
            .map(|r| {
                let mut names: Vec<(i64, &str, String)> = r
                    .contributors
                    .iter()
                    .filter_map(|c| member_nickname(state, room, c).map(|(nick, joined)| (joined, c.as_str(), nick)))
                    .collect();
                names.sort();
                json!({
                    "revision_id": r.id,
                    "kind": if r.merge { "merge" } else { "edit" },
                    "author": member_nickname(state, room, &r.user_id).map(|(n, _)| n).unwrap_or_default(),
                    "is_mine": r.user_id == user,
                    "body": r.body,
                    "allow_llm_merge": r.allow_llm_merge,
                    "branched": r.branched,
                    "base_revision_id": r.base,
                    "contributors": names.into_iter().map(|(_, _, n)| n).collect::<Vec<_>>(),
                    "created_at_ms": r.created_ms,
                })
            })
            .collect();
        out.push(json!({"page_id": page.id, "title": page.title, "revisions": revisions}));
    }
    json!({"code": "ok", "pages": out})
}

fn save_merged(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let room = text(args, "p_room_id").to_string();
    if !is_member(state, &room, user) {
        return code("not_member");
    }
    let Some(page) = page_id(state, &room, text(args, "p_title").trim()) else {
        return code("not_found");
    };
    let body = text(args, "p_body").to_string();
    let Some(sources) = args.get("p_source_revisions").and_then(Value::as_array) else {
        return code("invalid_input");
    };
    let mut ids: Vec<String> = sources
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if ids.len() != sources.len() || body.trim().is_empty() || !(1..=10).contains(&ids.len()) {
        return code("invalid_input");
    }
    if chars(&body) > MAX_BODY_CHARS {
        return code("too_large");
    }
    ids.sort();
    ids.dedup();
    let mut contributors: Vec<String> = Vec::new();
    for id in &ids {
        let ok = state.shared_wiki.revs.iter().find(|r| {
            r.id == *id
                && r.page_id == page
                && !r.merge
                && !r.withdrawn
                && r.allow_llm_merge
                && is_member(state, &room, &r.user_id)
        });
        match ok {
            Some(rev) => {
                if !contributors.contains(&rev.user_id) {
                    contributors.push(rev.user_id.clone());
                }
            }
            None => return code("not_consented"),
        }
    }
    if state
        .shared_wiki
        .revs
        .iter()
        .filter(|r| r.page_id == page)
        .count()
        >= MAX_REVISIONS_PER_PAGE
    {
        return code("too_many");
    }
    state.shared_wiki.seq += 1;
    let seq = state.shared_wiki.seq;
    let id = format!("31000000-0000-4000-8000-{seq:012}");
    state.shared_wiki.revs.push(RevRow {
        id: id.clone(),
        page_id: page,
        user_id: user.to_string(),
        merge: true,
        body,
        base: None,
        branched: false,
        allow_llm_merge: false,
        contributors,
        created_ms: now,
        seq,
        withdrawn: false,
    });
    json!({"code": "ok", "revision_id": id})
}
