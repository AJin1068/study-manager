//! Profile and gift semantics of the in-memory backend (S4.9/S4.10): the same rules as
//! `supabase/migrations/*_s4p_profile.sql`, written a second time in Rust so the app and its tests
//! can run many users, and an anonymous caller, against them. It mirrors the SQL; it does not
//! prove the SQL (V4-1).
//!
//! Nothing here is reachable through the generic table store: clients only get the RPCs of
//! `fixtures/profile/rpc-contract.json`.

use serde_json::{Map, Value, json};

use super::rooms::{find, shared_nickname, shares_room};
use super::{Caller, DataError, FakeBackend, State};

const DAY_MS: i64 = 86_400_000;
const ITEMS: &str = include_str!("../../../assets/character/items.json");

pub(super) const PROFILE_RPCS: &[&str] = &[
    "set_profile_settings",
    "report_profile_grass",
    "get_my_profile",
    "get_public_profile",
    "get_member_profile",
    "sign_guestbook",
    "delete_guestbook_entry",
    "report_guestbook_entry",
    "send_gift",
    "gift_status",
    "list_gift_inbox",
    "accept_gift",
    "decline_gift",
];

struct Settings {
    user_id: String,
    slug: String,
    name: String,
    vis: [String; 9],
    level: Option<i64>,
    badges: Vec<String>,
    furniture: Vec<String>,
    wiki_titles: Vec<String>,
    subject: Option<String>,
}

struct GrassRow {
    user_id: String,
    day: i64,
    study_min: i64,
}

struct EntryRow {
    id: i64,
    owner: String,
    author: String,
    author_name: String,
    body: String,
    reported: bool,
    at_ms: i64,
}

struct VisitRow {
    owner: String,
    visitor: String,
    day: i64,
}

struct GiftRow {
    id: String,
    key: String,
    sender: String,
    recipient: String,
    sender_name: String,
    item_id: String,
    message: String,
    status: &'static str,
    created_ms: i64,
    resolved_ms: Option<i64>,
}

#[derive(Default)]
pub(super) struct ProfileState {
    seq: u64,
    /// The UTC day of the call being served, set at the top of every RPC.
    seq_day: i64,
    settings: Vec<Settings>,
    grass: Vec<GrassRow>,
    entries: Vec<EntryRow>,
    visits: Vec<VisitRow>,
    gifts: Vec<GiftRow>,
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

fn item_ok(s: &str) -> bool {
    (1..=40).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn strings(args: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => None,
    }
}

/// Shop clothes and furniture of the app catalog: what the SQL `gift_catalog` holds.
fn giftable(item: &str) -> bool {
    serde_json::from_str::<Value>(ITEMS)
        .ok()
        .and_then(|v| v.get("items").and_then(Value::as_array).cloned())
        .is_some_and(|items| {
            items.iter().any(|i| {
                i.get("id").and_then(Value::as_str) == Some(item)
                    && i.get("source").and_then(Value::as_str) == Some("shop")
                    && matches!(
                        i.get("kind").and_then(Value::as_str),
                        Some("clothing" | "furniture")
                    )
            })
        })
}

fn allows(state: &State, scope: &str, owner: &str, viewer: Option<&str>) -> bool {
    match viewer {
        Some(v) if v == owner => true,
        _ if scope == "public" => true,
        Some(v) if scope == "friends" => shares_room(state, owner, v),
        _ => false,
    }
}

fn settings_of<'a>(state: &'a State, user: &str) -> Option<&'a Settings> {
    state.profile.settings.iter().find(|s| s.user_id == user)
}

fn entry_json(e: &EntryRow, viewer: Option<&str>, owner_view: bool) -> Value {
    json!({
        "id": e.id, "author_name": e.author_name, "body": e.body, "at_ms": e.at_ms,
        "mine": viewer == Some(e.author.as_str()), "reported": owner_view && e.reported,
    })
}

/// The one place an answer is assembled; a section the viewer may not see is absent.
fn view(state: &State, owner: &str, viewer: Option<&str>) -> Option<Value> {
    let s = settings_of(state, owner)?;
    let owner_view = viewer == Some(owner);
    let mut sections = Map::new();
    let ok = |i: usize| allows(state, &s.vis[i], owner, viewer);
    if ok(0) {
        let inv = find(state, "inventory_public", owner);
        let slot = |k: &str| inv.and_then(|r| r.get(k)).cloned().unwrap_or(Value::Null);
        sections.insert(
            "room".into(),
            json!({
                "species": slot("species"), "hat": slot("slot_hat"), "top": slot("slot_top"),
                "accessory": slot("slot_accessory"), "hand": slot("slot_hand"), "furniture": s.furniture,
            }),
        );
    }
    if ok(1) {
        let mut rows: Vec<&GrassRow> = state
            .profile
            .grass
            .iter()
            .filter(|g| g.user_id == owner)
            .collect();
        rows.sort_by_key(|g| g.day);
        let days: Vec<Value> = rows
            .iter()
            .map(|g| json!({ "day": day_text(g.day), "study_min": g.study_min }))
            .collect();
        sections.insert("grass".into(), Value::Array(days));
    }
    if ok(2) {
        sections.insert("level".into(), json!(s.level));
    }
    if ok(3) {
        sections.insert("badges".into(), json!(s.badges));
    }
    if ok(4) {
        let mut rows: Vec<&EntryRow> = state
            .profile
            .entries
            .iter()
            .filter(|e| e.owner == owner && (owner_view || !e.reported))
            .collect();
        rows.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| b.id.cmp(&a.id)));
        let items: Vec<Value> = rows
            .iter()
            .take(50)
            .map(|e| entry_json(e, viewer, owner_view))
            .collect();
        sections.insert("guestbook".into(), Value::Array(items));
    }
    if ok(5) {
        let total = state
            .profile
            .visits
            .iter()
            .filter(|v| v.owner == owner)
            .count();
        let today = state
            .profile
            .visits
            .iter()
            .filter(|v| v.owner == owner && v.day == now_day(state))
            .count();
        sections.insert("visitors".into(), json!({ "total": total, "today": today }));
    }
    if ok(6) {
        sections.insert("wiki".into(), json!(s.wiki_titles));
    }
    if ok(7) {
        sections.insert("subject".into(), json!(s.subject));
    }
    if ok(8) {
        let mut rows: Vec<&GiftRow> = state
            .profile
            .gifts
            .iter()
            .filter(|g| g.recipient == owner && g.status == "accepted")
            .collect();
        rows.sort_by_key(|g| std::cmp::Reverse(g.resolved_ms));
        let items: Vec<Value> = rows
            .iter()
            .take(20)
            .map(|g| json!({ "item_id": g.item_id, "sender_name": g.sender_name, "message": g.message, "at_ms": g.resolved_ms }))
            .collect();
        sections.insert("gifts".into(), Value::Array(items));
    }
    Some(json!({ "name": s.name, "sections": sections }))
}

fn day_text(day: i64) -> String {
    chrono::DateTime::from_timestamp_millis(day * DAY_MS)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn parse_day(raw: &str) -> Option<i64> {
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc().timestamp_millis().div_euclid(DAY_MS))
}

/// The fake's clock day, stored by every RPC call before use.
fn now_day(state: &State) -> i64 {
    state.profile.seq_day
}

impl FakeBackend {
    pub(super) fn profile_rpc(
        &self,
        caller: &Caller,
        name: &str,
        args: &Map<String, Value>,
    ) -> Result<Value, DataError> {
        let viewer = match caller {
            Caller::User(id) => Some(id.clone()),
            Caller::Anon => None,
            Caller::Service => return Err(DataError::Forbidden),
        };
        let now = self.now();
        let mut state = self.lock();
        let state = &mut *state;
        state.profile.seq_day = now.div_euclid(DAY_MS);
        if name == "get_public_profile" {
            return Ok(get_public_profile(state, viewer.as_deref(), args));
        }
        let Some(user) = viewer else {
            return Err(DataError::Forbidden);
        };
        Ok(match name {
            "set_profile_settings" => set_profile_settings(state, &user, args),
            "report_profile_grass" => report_profile_grass(state, &user, now, args),
            "get_my_profile" => get_my_profile(state, &user),
            "get_member_profile" => get_member_profile(state, &user, args),
            "sign_guestbook" => sign_guestbook(state, &user, now, args),
            "delete_guestbook_entry" => delete_entry(state, &user, args),
            "report_guestbook_entry" => report_entry(state, &user, args),
            "send_gift" => send_gift(state, &user, now, args),
            "gift_status" => gift_status(state, &user, args),
            "list_gift_inbox" => list_gift_inbox(state, &user, now),
            "accept_gift" => resolve_gift(state, &user, now, args, true),
            "decline_gift" => resolve_gift(state, &user, now, args, false),
            _ => return Err(DataError::NotFound),
        })
    }

    /// Test helper: how many guestbook rows the server holds for an owner (any status).
    pub fn profile_entry_count(&self, owner: &str) -> usize {
        self.lock()
            .profile
            .entries
            .iter()
            .filter(|e| e.owner == owner)
            .count()
    }

    /// Test helper: gift rows the server holds from one sender.
    pub fn profile_gift_count(&self, sender: &str) -> usize {
        self.lock()
            .profile
            .gifts
            .iter()
            .filter(|g| g.sender == sender)
            .count()
    }

    /// Test helper: every text value the server holds about a user's profile (for canary greps).
    pub fn profile_dump(&self, user: &str) -> String {
        let state = self.lock();
        let Some(s) = settings_of(&state, user) else {
            return String::new();
        };
        let grass = state
            .profile
            .grass
            .iter()
            .filter(|g| g.user_id == user)
            .count();
        format!(
            "{} {} {:?} {:?} {:?} {:?} {:?} grass={grass}",
            s.name, s.slug, s.level, s.badges, s.furniture, s.wiki_titles, s.subject
        )
    }
}

fn get_public_profile(state: &mut State, viewer: Option<&str>, args: &Map<String, Value>) -> Value {
    let slug = text(args, "p_slug");
    let valid = slug.len() == 16
        && slug
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let owner = state
        .profile
        .settings
        .iter()
        .find(|s| valid && s.slug == slug)
        .map(|s| s.user_id.clone());
    let Some(owner) = owner else {
        return code("not_found");
    };
    if let Some(v) = viewer.filter(|v| *v != owner) {
        let day = now_day(state);
        let seen = state
            .profile
            .visits
            .iter()
            .any(|x| x.owner == owner && x.visitor == v && x.day == day);
        if !seen {
            state.profile.visits.push(VisitRow {
                owner: owner.clone(),
                visitor: v.to_string(),
                day,
            });
        }
    }
    answer(view(state, &owner, viewer))
}

fn answer(view: Option<Value>) -> Value {
    match view {
        Some(v) => json!({ "code": "ok", "name": v["name"], "sections": v["sections"] }),
        None => code("not_found"),
    }
}

fn get_my_profile(state: &State, user: &str) -> Value {
    let Some(v) = view(state, user, Some(user)) else {
        return code("not_found");
    };
    let slug = settings_of(state, user)
        .map(|s| s.slug.clone())
        .unwrap_or_default();
    json!({ "code": "ok", "slug": slug, "name": v["name"], "sections": v["sections"] })
}

fn get_member_profile(state: &mut State, viewer: &str, args: &Map<String, Value>) -> Value {
    let target = text(args, "p_user").to_string();
    if target.is_empty() || (target != viewer && !shares_room(state, &target, viewer)) {
        return code("not_member");
    }
    if target != viewer && settings_of(state, &target).is_some() {
        let day = now_day(state);
        let seen = state
            .profile
            .visits
            .iter()
            .any(|x| x.owner == target && x.visitor == viewer && x.day == day);
        if !seen {
            state.profile.visits.push(VisitRow {
                owner: target.clone(),
                visitor: viewer.to_string(),
                day,
            });
        }
    }
    answer(view(state, &target, Some(viewer)))
}

fn set_profile_settings(state: &mut State, user: &str, args: &Map<String, Value>) -> Value {
    let name = text(args, "p_display_name").trim().to_string();
    if !(1..=20).contains(&chars(&name)) {
        return code("invalid_input");
    }
    let keys = [
        "p_vis_room",
        "p_vis_grass",
        "p_vis_level",
        "p_vis_badges",
        "p_vis_guestbook",
        "p_vis_visitors",
        "p_vis_wiki",
        "p_vis_subject",
        "p_vis_gifts",
    ];
    let mut vis: [String; 9] = Default::default();
    for (i, key) in keys.iter().enumerate() {
        let scope = text(args, key);
        if !matches!(scope, "public" | "friends" | "private") {
            return code("invalid_input");
        }
        vis[i] = scope.to_string();
    }
    let level = args.get("p_level").and_then(Value::as_i64);
    if args.get("p_level").is_some_and(|v| !v.is_null())
        && !level.is_some_and(|l| (1..=999).contains(&l))
    {
        return code("invalid_input");
    }
    let (Some(mut badges), Some(mut furniture), Some(titles)) = (
        strings(args, "p_badges"),
        strings(args, "p_furniture"),
        strings(args, "p_wiki_titles"),
    ) else {
        return code("invalid_input");
    };
    let mut titles: Vec<String> = titles.iter().map(|t| t.trim().to_string()).collect();
    if furniture.len() > 12
        || furniture.iter().any(|f| !item_ok(f))
        || badges.iter().any(|b| b != "streak_7" && b != "quiz_100")
    {
        return code("invalid_input");
    }
    if titles.len() > 10 || titles.iter().any(|t| !(1..=60).contains(&chars(t))) {
        return code("invalid_input");
    }
    let mut subject = args
        .get("p_subject_now")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if subject.as_ref().is_some_and(|s| chars(s) > 40) {
        return code("invalid_input");
    }
    if vis[0] == "private" {
        furniture.clear();
    }
    if vis[3] == "private" {
        badges.clear();
    }
    if vis[6] == "private" {
        titles.clear();
    }
    if vis[7] == "private" {
        subject = None;
    }
    if vis[1] == "private" {
        state.profile.grass.retain(|g| g.user_id != user);
    }
    let level = if vis[2] == "private" { None } else { level };
    if let Some(existing) = state
        .profile
        .settings
        .iter_mut()
        .find(|s| s.user_id == user)
    {
        existing.name = name;
        existing.vis = vis;
        existing.level = level;
        existing.badges = badges;
        existing.furniture = furniture;
        existing.wiki_titles = titles;
        existing.subject = subject;
        return json!({ "code": "ok", "slug": existing.slug });
    }
    state.profile.seq += 1;
    let slug = format!(
        "{:016x}",
        0x51ab_0000_0000_0000_u64 + state.profile.seq * 7919
    );
    state.profile.settings.push(Settings {
        user_id: user.to_string(),
        slug: slug.clone(),
        name,
        vis,
        level,
        badges,
        furniture,
        wiki_titles: titles,
        subject,
    });
    json!({ "code": "ok", "slug": slug })
}

fn report_profile_grass(
    state: &mut State,
    user: &str,
    now: i64,
    args: &Map<String, Value>,
) -> Value {
    let Some(entries) = args.get("p_entries").and_then(Value::as_array) else {
        return code("invalid_input");
    };
    if entries.len() > 31 {
        return code("invalid_input");
    }
    if !settings_of(state, user).is_some_and(|s| s.vis[1] != "private") {
        return code("not_shared");
    }
    let today = now.div_euclid(DAY_MS);
    let mut parsed = Vec::new();
    for entry in entries {
        let day = entry.get("day").and_then(Value::as_str).and_then(parse_day);
        let minutes = entry.get("study_min").and_then(Value::as_i64);
        match (day, minutes) {
            (Some(d), Some(m)) if (0..=1440).contains(&m) && d >= today - 366 && d <= today + 1 => {
                parsed.push((d, m))
            }
            _ => return code("invalid_input"),
        }
    }
    for (day, study_min) in parsed {
        match state
            .profile
            .grass
            .iter_mut()
            .find(|g| g.user_id == user && g.day == day)
        {
            Some(row) => row.study_min = study_min,
            None => state.profile.grass.push(GrassRow {
                user_id: user.to_string(),
                day,
                study_min,
            }),
        }
    }
    state
        .profile
        .grass
        .retain(|g| g.user_id != user || g.day > today - 366);
    code("ok")
}

fn sign_guestbook(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let body = text(args, "p_body").trim().to_string();
    if !(1..=200).contains(&chars(&body)) {
        return code("invalid_input");
    }
    let owner = text(args, "p_owner").to_string();
    if owner.is_empty() || owner == user {
        return code("cannot_self");
    }
    if !shares_room(state, user, &owner) {
        return code("not_friends");
    }
    if !settings_of(state, &owner).is_some_and(|s| s.vis[4] != "private") {
        return code("closed");
    }
    let minute = state
        .profile
        .entries
        .iter()
        .filter(|e| e.author == user && e.at_ms > now - 60_000)
        .count();
    let day = state
        .profile
        .entries
        .iter()
        .filter(|e| e.author == user && e.at_ms > now - DAY_MS)
        .count();
    if minute >= 5 || day >= 30 {
        return code("rate_limited");
    }
    let author_name = shared_nickname(state, user, &owner).unwrap_or_default();
    state.profile.seq += 1;
    let id = i64::try_from(state.profile.seq).unwrap_or(i64::MAX);
    state.profile.entries.push(EntryRow {
        id,
        owner,
        author: user.to_string(),
        author_name,
        body,
        reported: false,
        at_ms: now,
    });
    json!({ "code": "ok", "id": id })
}

fn delete_entry(state: &mut State, user: &str, args: &Map<String, Value>) -> Value {
    let id = args.get("p_id").and_then(Value::as_i64).unwrap_or(-1);
    let before = state.profile.entries.len();
    state
        .profile
        .entries
        .retain(|e| !(e.id == id && (e.owner == user || e.author == user)));
    code(if state.profile.entries.len() < before {
        "ok"
    } else {
        "not_found"
    })
}

fn report_entry(state: &mut State, user: &str, args: &Map<String, Value>) -> Value {
    let id = args.get("p_id").and_then(Value::as_i64).unwrap_or(-1);
    match state
        .profile
        .entries
        .iter_mut()
        .find(|e| e.id == id && e.owner == user)
    {
        Some(entry) => {
            entry.reported = true;
            code("ok")
        }
        None => code("not_found"),
    }
}

fn send_gift(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let key = text(args, "p_idem_key").to_string();
    let recipient = text(args, "p_recipient").to_string();
    let item = text(args, "p_item_id").to_string();
    let message = text(args, "p_message").trim().to_string();
    if key.is_empty() || recipient.is_empty() || item.is_empty() || chars(&message) > 60 {
        return code("invalid_input");
    }
    if let Some(existing) = state
        .profile
        .gifts
        .iter()
        .find(|g| g.sender == user && g.key == key)
    {
        if existing.recipient == recipient && existing.item_id == item {
            return json!({ "code": "ok", "gift_id": existing.id, "replayed": true });
        }
        return code("key_conflict");
    }
    if !giftable(&item) {
        return code("not_giftable");
    }
    if recipient == user {
        return code("cannot_self");
    }
    if !shares_room(state, user, &recipient) {
        return code("not_friends");
    }
    if state
        .profile
        .gifts
        .iter()
        .filter(|g| g.sender == user && g.created_ms > now - DAY_MS)
        .count()
        >= 5
    {
        return code("daily_limit");
    }
    let sender_name = shared_nickname(state, user, &recipient).unwrap_or_default();
    state.profile.seq += 1;
    let id = format!("32000000-0000-4000-8000-{:012}", state.profile.seq);
    state.profile.gifts.push(GiftRow {
        id: id.clone(),
        key,
        sender: user.to_string(),
        recipient,
        sender_name,
        item_id: item,
        message,
        status: "pending",
        created_ms: now,
        resolved_ms: None,
    });
    json!({ "code": "ok", "gift_id": id, "replayed": false })
}

fn gift_status(state: &State, user: &str, args: &Map<String, Value>) -> Value {
    let key = text(args, "p_idem_key");
    match state
        .profile
        .gifts
        .iter()
        .find(|g| g.sender == user && g.key == key)
    {
        Some(g) => json!({ "code": "ok", "state": g.status, "gift_id": g.id }),
        None => json!({ "code": "ok", "state": "none" }),
    }
}

fn list_gift_inbox(state: &State, user: &str, now: i64) -> Value {
    let mut rows: Vec<&GiftRow> = state
        .profile
        .gifts
        .iter()
        .filter(|g| {
            g.recipient == user
                && (g.status == "pending"
                    || (g.status == "accepted"
                        && g.resolved_ms.is_some_and(|r| r > now - 30 * DAY_MS)))
        })
        .collect();
    rows.sort_by_key(|g| g.created_ms);
    let gifts: Vec<Value> = rows
        .iter()
        .map(|g| {
            json!({ "gift_id": g.id, "item_id": g.item_id, "sender_name": g.sender_name, "message": g.message,
                    "status": g.status, "at_ms": g.created_ms })
        })
        .collect();
    json!({ "code": "ok", "gifts": gifts })
}

fn resolve_gift(
    state: &mut State,
    user: &str,
    now: i64,
    args: &Map<String, Value>,
    accept: bool,
) -> Value {
    let id = text(args, "p_gift_id");
    let Some(gift) = state
        .profile
        .gifts
        .iter_mut()
        .find(|g| g.id == id && g.recipient == user)
    else {
        return code("not_found");
    };
    let (target, other) = if accept {
        ("accepted", "declined")
    } else {
        ("declined", "accepted")
    };
    if gift.status == other {
        return code("already_resolved");
    }
    if gift.status == "pending" {
        gift.status = target;
        gift.resolved_ms = Some(now);
    }
    if accept {
        json!({ "code": "ok", "item_id": gift.item_id, "sender_name": gift.sender_name })
    } else {
        code("ok")
    }
}
