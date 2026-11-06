//! Room semantics of the in-memory backend (S4.1): the same rules as
//! `supabase/migrations/*_s4_rooms.sql`, written a second time in Rust so the app and its tests
//! can run many users against them. It mirrors the SQL; it does not prove the SQL (V4-1).
//!
//! The four owner-readable tables (`member_status`, `daily_summaries`, `friend_prefs`,
//! `inventory_public`) live in the generic table store, so the RLS matrix applies to them exactly
//! as it does for the stage-2 tables. Everything else is typed state that only the RPCs reach.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, NaiveDate};
use serde_json::{json, Map, Value};

use super::{Caller, DataError, FakeBackend, State};

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const DAY_MS: i64 = 86_400_000;

pub(super) struct RoomRow {
    id: String,
    name: String,
    capacity: usize,
    created_seq: u64,
}

pub(super) struct MemberRow {
    room_id: String,
    user_id: String,
    owner: bool,
    nickname: String,
    joined_ms: i64,
}

pub(super) struct InviteRow {
    id: String,
    room_id: String,
    hash: u64,
    expires_ms: i64,
    max_uses: i64,
    uses: i64,
    revoked: bool,
    created_seq: u64,
}

#[derive(Default)]
pub(super) struct RoomsState {
    seq: u64,
    rng: u64,
    rooms: Vec<RoomRow>,
    members: Vec<MemberRow>,
    bans: Vec<(String, String)>,
    invites: Vec<InviteRow>,
    attempts: Vec<(String, i64)>,
    schedules: Vec<(String, Value, i64)>,
    events: Vec<(i64, String, i64)>,
}

impl RoomsState {
    fn next_id(&mut self) -> String {
        self.seq += 1;
        format!("20000000-0000-4000-8000-{:012}", self.seq)
    }

    /// Deterministic stream for the mock only; the real server uses its CSPRNG.
    fn next_random(&mut self) -> u64 {
        if self.rng == 0 {
            self.rng = 0x9E37_79B9_7F4A_7C15;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }
}

pub(super) const ROOM_RPCS: &[&str] = &[
    "create_room",
    "create_invite",
    "revoke_invite",
    "join_room",
    "leave_room",
    "kick_member",
    "list_my_rooms",
    "room_snapshot",
    "report_status",
    "report_daily",
    "set_friend_prefs",
    "set_inventory_public",
    "report_distraction",
    "poll_distraction_events",
    "share_schedule",
];

fn code(c: &str) -> Value {
    json!({ "code": c })
}

fn hash_code(code: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    code.hash(&mut hasher);
    hasher.finish()
}

fn normalize(raw: &str) -> Option<String> {
    let mut out = String::new();
    for ch in raw.chars().filter(|c| c.is_ascii_alphanumeric()) {
        let ch = match ch.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        if !ALPHABET.contains(&(ch as u8)) {
            return None;
        }
        out.push(ch);
    }
    (out.len() == 10).then_some(out)
}

fn text_len(s: &str) -> usize {
    s.chars().count()
}

fn trimmed<'a>(args: &'a Map<String, Value>, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or_default().trim()
}

fn valid_item_id(s: &str) -> bool {
    (1..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn date_of(ms: i64) -> Option<NaiveDate> {
    DateTime::from_timestamp_millis(ms).map(|d| d.date_naive())
}

#[derive(Clone, Copy)]
struct Prefs {
    level: bool,
    status: bool,
    week: bool,
    attempts: bool,
    publish: bool,
    receive: bool,
}

fn prefs_of(state: &State, user: &str) -> Prefs {
    let row = find(state, "friend_prefs", user);
    let flag = |key: &str, default: bool| row.and_then(|r| r.get(key)).and_then(Value::as_bool).unwrap_or(default);
    Prefs {
        level: flag("share_level", true),
        status: flag("share_status", true),
        week: flag("share_week_time", false),
        attempts: flag("share_attempts", false),
        publish: flag("distraction_publish", false),
        receive: flag("distraction_receive", false),
    }
}

pub(super) fn find<'a>(state: &'a State, table: &str, user: &str) -> Option<&'a Map<String, Value>> {
    state.tables.get(table)?.iter().find(|r| r.get("user_id").and_then(Value::as_str) == Some(user))
}

fn put_row(state: &mut State, table: &str, keys: &[&str], row: Map<String, Value>) {
    let rows = state.tables.entry(table.to_string()).or_default();
    match rows.iter_mut().find(|e| keys.iter().all(|k| e.get(*k) == row.get(*k))) {
        Some(existing) => *existing = row,
        None => rows.push(row),
    }
}

pub(super) fn is_member(state: &State, room: &str, user: &str) -> bool {
    state.rooms.members.iter().any(|m| m.room_id == room && m.user_id == user)
}

/// The nickname a member uses in a room, in join order position `(joined_ms, user)` for sorting.
pub(super) fn member_nickname(state: &State, room: &str, user: &str) -> Option<(String, i64)> {
    state.rooms.members.iter().find(|m| m.room_id == room && m.user_id == user).map(|m| (m.nickname.clone(), m.joined_ms))
}

fn is_owner(state: &State, room: &str, user: &str) -> bool {
    state.rooms.members.iter().any(|m| m.room_id == room && m.user_id == user && m.owner)
}

fn ensure_owner(state: &mut State, room: &str) {
    if state.rooms.members.iter().any(|m| m.room_id == room && m.owner) {
        return;
    }
    let first = state
        .rooms
        .members
        .iter_mut()
        .filter(|m| m.room_id == room)
        .min_by(|a, b| a.joined_ms.cmp(&b.joined_ms).then_with(|| a.user_id.cmp(&b.user_id)));
    if let Some(member) = first {
        member.owner = true;
    }
}

pub(super) fn shares_room(state: &State, a: &str, b: &str) -> bool {
    state.rooms.members.iter().any(|m| m.user_id == a && is_member(state, &m.room_id, b))
}

/// The nickname `user` uses in the earliest room they share with `other` (profiles and gifts show
/// a room name, never an account name).
pub(super) fn shared_nickname(state: &State, user: &str, other: &str) -> Option<String> {
    state
        .rooms
        .members
        .iter()
        .filter(|m| m.user_id == user && is_member(state, &m.room_id, other))
        .min_by_key(|m| m.joined_ms)
        .map(|m| m.nickname.clone())
}

impl FakeBackend {
    pub(super) fn room_rpc(&self, caller: &Caller, name: &str, args: &Map<String, Value>) -> Result<Value, DataError> {
        let user = Self::user_of(caller)?.to_string();
        let now = self.now();
        let mut state = self.lock();
        let state = &mut *state;
        Ok(match name {
            "create_room" => create_room(state, &user, now, args),
            "create_invite" => create_invite(state, &user, now, args),
            "revoke_invite" => revoke_invite(state, &user, args),
            "join_room" => join_room(state, &user, now, args),
            "leave_room" => leave_room(state, &user, args),
            "kick_member" => kick_member(state, &user, args),
            "list_my_rooms" => list_my_rooms(state, &user),
            "room_snapshot" => room_snapshot(state, &user, now, args),
            "report_status" => report_status(state, &user, now, args),
            "report_daily" => report_daily(state, &user, now, args),
            "set_friend_prefs" => set_friend_prefs(state, &user, now, args),
            "set_inventory_public" => set_inventory_public(state, &user, now, args),
            "report_distraction" => report_distraction(state, &user, now),
            "poll_distraction_events" => poll_distraction_events(state, &user, args),
            "share_schedule" => share_schedule(state, &user, now, args),
            _ => return Err(DataError::NotFound),
        })
    }

    /// Test and diagnostics helper: how many members a room has.
    pub fn room_member_count(&self, room_id: &str) -> usize {
        self.lock().rooms.members.iter().filter(|m| m.room_id == room_id).count()
    }

    /// Test helper: number of stored invites whose plain code could be recovered (always zero:
    /// only a hash is kept).
    pub fn stored_invite_hashes(&self) -> usize {
        self.lock().rooms.invites.len()
    }

    /// Test helper: the room (if any) exists on the server.
    pub fn room_exists(&self, room_id: &str) -> bool {
        self.lock().rooms.rooms.iter().any(|r| r.id == room_id)
    }
}

fn create_room(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let name = trimmed(args, "p_name").to_string();
    let nick = trimmed(args, "p_nickname").to_string();
    if !(1..=40).contains(&text_len(&name)) || !(1..=20).contains(&text_len(&nick)) {
        return code("invalid_input");
    }
    if state.rooms.members.iter().filter(|m| m.user_id == user && m.owner).count() >= 5 {
        return code("room_limit");
    }
    let id = state.rooms.next_id();
    let created_seq = state.rooms.seq;
    state.rooms.rooms.push(RoomRow { id: id.clone(), name, capacity: 8, created_seq });
    state.rooms.members.push(MemberRow { room_id: id.clone(), user_id: user.to_string(), owner: true, nickname: nick, joined_ms: now });
    json!({"code": "ok", "room_id": id})
}

fn create_invite(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let room = args.get("p_room_id").and_then(Value::as_str).unwrap_or_default().to_string();
    let (Some(ttl), Some(max_uses)) = (args.get("p_ttl_hours").and_then(Value::as_i64), args.get("p_max_uses").and_then(Value::as_i64)) else {
        return code("invalid_input");
    };
    if !(1..=168).contains(&ttl) || !(1..=50).contains(&max_uses) {
        return code("invalid_input");
    }
    if !is_owner(state, &room, user) {
        return code("not_owner");
    }
    let active = state.rooms.invites.iter().filter(|i| i.room_id == room && !i.revoked && i.expires_ms > now && i.uses < i.max_uses).count();
    if active >= 10 {
        return code("too_many_invites");
    }
    let mut plain = String::new();
    for _ in 0..10 {
        let r = state.rooms.next_random();
        plain.push(char::from(ALPHABET[(r % 32) as usize]));
    }
    let id = state.rooms.next_id();
    let created_seq = state.rooms.seq;
    let expires_ms = now + ttl * 3_600_000;
    state.rooms.invites.push(InviteRow { id: id.clone(), room_id: room, hash: hash_code(&plain), expires_ms, max_uses, uses: 0, revoked: false, created_seq });
    json!({"code": "ok", "invite_id": id, "invite_code": plain, "expires_at_ms": expires_ms})
}

fn revoke_invite(state: &mut State, user: &str, args: &Map<String, Value>) -> Value {
    let room = args.get("p_room_id").and_then(Value::as_str).unwrap_or_default();
    let invite = args.get("p_invite_id").and_then(Value::as_str).unwrap_or_default();
    if !is_owner(state, room, user) {
        return code("not_owner");
    }
    match state.rooms.invites.iter_mut().find(|i| i.id == invite && i.room_id == room && !i.revoked) {
        Some(row) => {
            row.revoked = true;
            code("ok")
        }
        None => code("not_found"),
    }
}

fn join_room(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    state.rooms.attempts.retain(|(_, at)| *at > now - DAY_MS);
    let recent = state.rooms.attempts.iter().filter(|(u, at)| u == user && *at > now - 60_000).count();
    if recent >= 5 {
        return code("rate_limited");
    }
    state.rooms.attempts.push((user.to_string(), now));

    let nick = trimmed(args, "p_nickname").to_string();
    if !(1..=20).contains(&text_len(&nick)) {
        return code("invalid_input");
    }
    let Some(plain) = normalize(args.get("p_code").and_then(Value::as_str).unwrap_or_default()) else {
        return code("invalid_code");
    };
    let hash = hash_code(&plain);
    let Some(invite) = state.rooms.invites.iter().find(|i| i.hash == hash) else { return code("invalid_code") };
    let (invite_id, room) = (invite.id.clone(), invite.room_id.clone());
    if invite.revoked {
        return code("revoked");
    }
    if invite.expires_ms <= now {
        return code("expired");
    }
    if invite.uses >= invite.max_uses {
        return code("exhausted");
    }
    if state.rooms.bans.iter().any(|(r, u)| *r == room && u == user) {
        return code("banned");
    }
    if is_member(state, &room, user) {
        return json!({"code": "already_member", "room_id": room});
    }
    let capacity = state.rooms.rooms.iter().find(|r| r.id == room).map_or(0, |r| r.capacity);
    if state.rooms.members.iter().filter(|m| m.room_id == room).count() >= capacity {
        return code("room_full");
    }
    state.rooms.members.push(MemberRow { room_id: room.clone(), user_id: user.to_string(), owner: false, nickname: nick, joined_ms: now });
    if let Some(row) = state.rooms.invites.iter_mut().find(|i| i.id == invite_id) {
        row.uses += 1;
    }
    json!({"code": "ok", "room_id": room})
}

fn leave_room(state: &mut State, user: &str, args: &Map<String, Value>) -> Value {
    let room = args.get("p_room_id").and_then(Value::as_str).unwrap_or_default().to_string();
    let Some(position) = state.rooms.members.iter().position(|m| m.room_id == room && m.user_id == user) else {
        return code("not_member");
    };
    let was_owner = state.rooms.members.remove(position).owner;
    if !state.rooms.members.iter().any(|m| m.room_id == room) {
        state.rooms.rooms.retain(|r| r.id != room);
        state.rooms.bans.retain(|(r, _)| *r != room);
        state.rooms.invites.retain(|i| i.room_id != room);
        state.rooms.schedules.retain(|(r, _, _)| *r != room);
    } else if was_owner {
        ensure_owner(state, &room);
    }
    code("ok")
}

fn kick_member(state: &mut State, user: &str, args: &Map<String, Value>) -> Value {
    let room = args.get("p_room_id").and_then(Value::as_str).unwrap_or_default().to_string();
    let target = args.get("p_target_user").and_then(Value::as_str).unwrap_or_default().to_string();
    if !is_owner(state, &room, user) {
        return code("not_owner");
    }
    if target.is_empty() || target == user {
        return code("cannot_kick_self");
    }
    if !is_member(state, &room, &target) {
        return code("not_member");
    }
    state.rooms.members.retain(|m| !(m.room_id == room && m.user_id == target));
    if !state.rooms.bans.iter().any(|(r, u)| *r == room && *u == target) {
        state.rooms.bans.push((room, target));
    }
    code("ok")
}

fn list_my_rooms(state: &mut State, user: &str) -> Value {
    let mine: Vec<String> = state.rooms.members.iter().filter(|m| m.user_id == user).map(|m| m.room_id.clone()).collect();
    for room in &mine {
        ensure_owner(state, room);
    }
    let mut rooms: Vec<&RoomRow> = state.rooms.rooms.iter().filter(|r| mine.contains(&r.id)).collect();
    rooms.sort_by_key(|r| r.created_seq);
    let list: Vec<Value> = rooms
        .iter()
        .map(|r| {
            let role = if is_owner(state, &r.id, user) { "owner" } else { "member" };
            let count = state.rooms.members.iter().filter(|m| m.room_id == r.id).count();
            json!({"room_id": r.id, "name": r.name, "role": role, "capacity": r.capacity, "member_count": count})
        })
        .collect();
    json!({"code": "ok", "rooms": list})
}

fn room_snapshot(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let room_id = args.get("p_room_id").and_then(Value::as_str).unwrap_or_default().to_string();
    if !is_member(state, &room_id, user) {
        return code("not_member");
    }
    ensure_owner(state, &room_id);
    let Some(room) = state.rooms.rooms.iter().find(|r| r.id == room_id) else { return code("not_member") };
    let role = if is_owner(state, &room_id, user) { "owner" } else { "member" };
    let first_day = date_of(now - 16 * DAY_MS).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();

    let mut members: Vec<&MemberRow> = state.rooms.members.iter().filter(|m| m.room_id == room_id).collect();
    members.sort_by(|a, b| a.joined_ms.cmp(&b.joined_ms).then_with(|| a.user_id.cmp(&b.user_id)));
    let member_json: Vec<Value> = members
        .iter()
        .map(|m| {
            let me = m.user_id == user;
            let prefs = prefs_of(state, &m.user_id);
            let status = find(state, "member_status", &m.user_id);
            let level_visible = prefs.level || me;
            let days = if prefs.week || me {
                let mut rows: Vec<&Map<String, Value>> = state
                    .tables
                    .get("daily_summaries")
                    .map(|rows| {
                        rows.iter()
                            .filter(|r| r.get("user_id").and_then(Value::as_str) == Some(m.user_id.as_str()))
                            .filter(|r| r.get("day").and_then(Value::as_str).unwrap_or_default() >= first_day.as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                rows.sort_by(|a, b| a.get("day").and_then(Value::as_str).cmp(&b.get("day").and_then(Value::as_str)));
                Value::Array(rows.iter().map(|r| json!({"day": r["day"], "study_min": r["study_min"], "plan_min": r["plan_min"]})).collect())
            } else {
                Value::Null
            };
            let status_json = match status {
                Some(s) if prefs.status || me => {
                    let updated = s.get("updated_at_ms").and_then(Value::as_i64).unwrap_or(now);
                    json!({"state": s["state"], "remaining_sec": s["remaining_sec"], "age_sec": ((now - updated) / 1000).max(0)})
                }
                _ => Value::Null,
            };
            let character = match find(state, "inventory_public", &m.user_id) {
                Some(inv) if prefs.level || me => json!({
                    "species": inv["species"], "hat": inv["slot_hat"], "top": inv["slot_top"],
                    "accessory": inv["slot_accessory"], "hand": inv["slot_hand"],
                }),
                _ => Value::Null,
            };
            json!({
                "user_id": m.user_id,
                "nickname": m.nickname,
                "role": if m.owner { "owner" } else { "member" },
                "is_self": me,
                "level": if level_visible { status.map_or(Value::Null, |s| s["level"].clone()) } else { Value::Null },
                "status": status_json,
                "attempts_today": if prefs.attempts || me { status.map_or(Value::Null, |s| s.get("attempts_today").cloned().unwrap_or(Value::Null)) } else { Value::Null },
                "days": days,
                "character": character,
            })
        })
        .collect();

    let invites: Vec<Value> = if role == "owner" {
        let mut rows: Vec<&InviteRow> = state.rooms.invites.iter().filter(|i| i.room_id == room_id).collect();
        rows.sort_by_key(|i| std::cmp::Reverse(i.created_seq));
        rows.iter()
            .map(|i| json!({"invite_id": i.id, "expires_at_ms": i.expires_ms, "max_uses": i.max_uses, "uses": i.uses, "revoked": i.revoked}))
            .collect()
    } else {
        Vec::new()
    };
    let schedule = state
        .rooms
        .schedules
        .iter()
        .find(|(r, _, _)| *r == room_id)
        .map_or(Value::Null, |(_, items, at)| json!({"items": items, "updated_at_ms": at}));

    json!({
        "code": "ok",
        "server_now_ms": now,
        "room": {"room_id": room.id, "name": room.name, "capacity": room.capacity, "role": role},
        "members": member_json,
        "invites": invites,
        "schedule": schedule,
    })
}

fn report_status(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let status = args.get("p_state").and_then(Value::as_str);
    let (Some(remaining), Some(level)) = (args.get("p_remaining_sec").and_then(Value::as_i64), args.get("p_level").and_then(Value::as_i64)) else {
        return code("invalid_input");
    };
    let attempts = args.get("p_attempts_today").filter(|v| !v.is_null());
    let attempts_ok = attempts.is_none_or(|v| v.as_i64().is_some_and(|n| n >= 0));
    if !matches!(status, Some("studying" | "break" | "idle")) || !(0..=86_400).contains(&remaining) || !(1..=999).contains(&level) || !attempts_ok {
        return code("invalid_input");
    }
    let share_attempts = prefs_of(state, user).attempts;
    let mut row = Map::new();
    row.insert("user_id".into(), json!(user));
    row.insert("state".into(), json!(status));
    row.insert("remaining_sec".into(), json!(remaining));
    row.insert("level".into(), json!(level));
    row.insert("attempts_today".into(), if share_attempts { attempts.cloned().unwrap_or(Value::Null) } else { Value::Null });
    row.insert("updated_at_ms".into(), json!(now));
    put_row(state, "member_status", &["user_id"], row);
    code("ok")
}

fn report_daily(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let day = args.get("p_day").and_then(Value::as_str).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    let (Some(day), Some(study), Some(plan)) =
        (day, args.get("p_study_min").and_then(Value::as_i64), args.get("p_plan_min").and_then(Value::as_i64))
    else {
        return code("invalid_input");
    };
    let (Some(earliest), Some(latest)) = (date_of(now - 8 * DAY_MS), date_of(now + DAY_MS)) else { return code("invalid_input") };
    if !(0..=1440).contains(&study) || !(0..=1440).contains(&plan) || day < earliest || day > latest {
        return code("invalid_input");
    }
    if !prefs_of(state, user).week {
        return code("not_shared");
    }
    let day_text = day.format("%Y-%m-%d").to_string();
    let old = state
        .tables
        .get("daily_summaries")
        .and_then(|rows| {
            rows.iter().find(|r| r.get("user_id").and_then(Value::as_str) == Some(user) && r.get("day").and_then(Value::as_str) == Some(day_text.as_str()))
        })
        .map(|r| (r.get("study_min").and_then(Value::as_i64).unwrap_or(0), r.get("updated_at_ms").and_then(Value::as_i64).unwrap_or(now)));
    if let Some((old_study, updated)) = old {
        let elapsed_min = ((now - updated) / 1000 / 60).max(0);
        if study - old_study > elapsed_min + 5 {
            return code("surge_rejected");
        }
    }
    let mut row = Map::new();
    row.insert("user_id".into(), json!(user));
    row.insert("day".into(), json!(day_text));
    row.insert("study_min".into(), json!(study));
    row.insert("plan_min".into(), json!(plan));
    row.insert("updated_at_ms".into(), json!(now));
    put_row(state, "daily_summaries", &["user_id", "day"], row);
    if let (Some(cutoff), Some(rows)) = (date_of(now - 60 * DAY_MS), state.tables.get_mut("daily_summaries")) {
        let cutoff = cutoff.format("%Y-%m-%d").to_string();
        rows.retain(|r| {
            r.get("user_id").and_then(Value::as_str) != Some(user) || r.get("day").and_then(Value::as_str).unwrap_or_default() >= cutoff.as_str()
        });
    }
    code("ok")
}

fn set_friend_prefs(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let flags = ["p_share_level", "p_share_status", "p_share_week_time", "p_share_attempts", "p_distraction_publish", "p_distraction_receive"];
    let values: Vec<Option<bool>> = flags.iter().map(|k| args.get(*k).and_then(Value::as_bool)).collect();
    if values.iter().any(Option::is_none) {
        return code("invalid_input");
    }
    let [level, status, week, attempts, publish, receive] = [0, 1, 2, 3, 4, 5].map(|i| values[i].unwrap_or(false));
    let mut row = Map::new();
    row.insert("user_id".into(), json!(user));
    row.insert("share_level".into(), json!(level));
    row.insert("share_status".into(), json!(status));
    row.insert("share_week_time".into(), json!(week));
    row.insert("share_attempts".into(), json!(attempts));
    row.insert("distraction_publish".into(), json!(publish));
    row.insert("distraction_receive".into(), json!(receive));
    row.insert("updated_at_ms".into(), json!(now));
    put_row(state, "friend_prefs", &["user_id"], row);

    let mine = |r: &Map<String, Value>| r.get("user_id").and_then(Value::as_str) == Some(user);
    if !attempts {
        if let Some(rows) = state.tables.get_mut("member_status") {
            for r in rows.iter_mut().filter(|r| mine(r)) {
                r.insert("attempts_today".into(), Value::Null);
            }
        }
    }
    if !week {
        if let Some(rows) = state.tables.get_mut("daily_summaries") {
            rows.retain(|r| !mine(r));
        }
    }
    if !level && !status {
        if let Some(rows) = state.tables.get_mut("member_status") {
            rows.retain(|r| !mine(r));
        }
    }
    if !level {
        if let Some(rows) = state.tables.get_mut("inventory_public") {
            rows.retain(|r| !mine(r));
        }
    }
    if !publish {
        state.rooms.events.retain(|(_, u, _)| u != user);
    }
    code("ok")
}

fn set_inventory_public(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let species = args.get("p_species").and_then(Value::as_str);
    let slot = |key: &str| -> Result<Option<String>, ()> {
        match args.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if valid_item_id(s) => Ok(Some(s.clone())),
            _ => Err(()),
        }
    };
    let (Some(species), Ok(hat), Ok(top), Ok(accessory), Ok(hand)) =
        (species.filter(|s| valid_item_id(s)), slot("p_hat"), slot("p_top"), slot("p_accessory"), slot("p_hand"))
    else {
        return code("invalid_input");
    };
    let mut row = Map::new();
    row.insert("user_id".into(), json!(user));
    row.insert("species".into(), json!(species));
    row.insert("slot_hat".into(), json!(hat));
    row.insert("slot_top".into(), json!(top));
    row.insert("slot_accessory".into(), json!(accessory));
    row.insert("slot_hand".into(), json!(hand));
    row.insert("updated_at_ms".into(), json!(now));
    put_row(state, "inventory_public", &["user_id"], row);
    code("ok")
}

fn report_distraction(state: &mut State, user: &str, now: i64) -> Value {
    if !prefs_of(state, user).publish {
        return code("disabled");
    }
    state.rooms.events.retain(|(_, _, at)| *at > now - DAY_MS);
    if state.rooms.events.iter().any(|(_, u, at)| u == user && *at > now - 10_000) {
        return code("throttled");
    }
    let id = i64::try_from(state.rooms.events.len()).unwrap_or(0) + i64::try_from(state.rooms.seq).unwrap_or(0) + 1;
    state.rooms.seq += 1;
    state.rooms.events.push((id, user.to_string(), now));
    code("ok")
}

fn poll_distraction_events(state: &mut State, user: &str, args: &Map<String, Value>) -> Value {
    let Some(since) = args.get("p_since_ms").and_then(Value::as_i64).filter(|s| *s >= 0) else { return code("invalid_input") };
    if !prefs_of(state, user).receive {
        return json!({"code": "disabled", "events": []});
    }
    let mut events: Vec<&(i64, String, i64)> = state
        .rooms
        .events
        .iter()
        .filter(|(_, sender, at)| sender != user && *at > since && prefs_of(state, sender).publish && shares_room(state, user, sender))
        .collect();
    events.sort_by_key(|(id, _, at)| (*at, *id));
    let list: Vec<Value> = events
        .iter()
        .take(50)
        .map(|(id, sender, at)| {
            let nickname = state
                .rooms
                .members
                .iter()
                .filter(|m| m.user_id == *sender && is_member(state, &m.room_id, user))
                .min_by_key(|m| m.joined_ms)
                .map(|m| m.nickname.clone());
            json!({"event_id": id, "user_id": sender, "nickname": nickname, "at_ms": at})
        })
        .collect();
    json!({"code": "ok", "events": list})
}

fn share_schedule(state: &mut State, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let room = args.get("p_room_id").and_then(Value::as_str).unwrap_or_default().to_string();
    if !is_owner(state, &room, user) {
        return code("not_owner");
    }
    let Some(items) = args.get("p_items").and_then(Value::as_array).filter(|a| a.len() <= 20) else { return code("invalid_input") };
    for item in items {
        let ok = item.as_object().is_some_and(|o| {
            o.get("start_at_utc_ms").and_then(Value::as_i64).is_some()
                && o.get("name").and_then(Value::as_str).is_some_and(|s| (1..=60).contains(&text_len(s)))
                && o.get("subject").and_then(Value::as_str).is_some_and(|s| (1..=40).contains(&text_len(s)))
                && o.get("sets").and_then(Value::as_i64).is_some_and(|n| (1..=12).contains(&n))
        });
        if !ok {
            return code("invalid_input");
        }
    }
    let value = Value::Array(items.clone());
    match state.rooms.schedules.iter_mut().find(|(r, _, _)| *r == room) {
        Some(entry) => {
            entry.1 = value;
            entry.2 = now;
        }
        None => state.rooms.schedules.push((room, value, now)),
    }
    code("ok")
}
