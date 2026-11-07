//! Phone pairing and lease semantics of the in-memory backend (S5.2): the same rules as
//! `supabase/migrations/*_s5_phone.sql`, written a second time in Rust so the app and its tests
//! can run an owner, anonymous phone users and strangers against them. It mirrors the SQL; it does
//! not prove the SQL (V5-2).
//!
//! Clients reach this state through the RPCs of `fixtures/phone/rpc-contract.json` and through the
//! SELECT policies of `fixtures/phone/rls-matrix.json` (`select`, below). Anonymous sign-in
//! (`sign_in_anonymously`) gives a phone its own user id with the same `authenticated` access as
//! any other signed-in user; what separates it is only the pairing row.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use link_protocol::{MAX_LEASE_MS, normalize_code};
use serde_json::{Map, Value, json};

use super::super::auth::{AuthError, AuthTokens};
use super::super::http::TransportError;
use super::{Caller, DataError, FakeBackend};

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_TTL_MS: i64 = 5 * 60_000;
const ATTEMPT_WINDOW_MS: i64 = 60_000;
const ATTEMPT_LIMIT: usize = 5;
const DAY_MS: i64 = 86_400_000;
const CAUSES: [&str; 5] = [
    "run_ended",
    "emergency_pass",
    "skip_today",
    "early_release",
    "pair_revoked",
];

pub(super) const PHONE_RPCS: &[&str] = &[
    "create_pair_code",
    "claim_pair",
    "revoke_pair",
    "phone_status",
    "publish_phone_lock",
    "phone_lease",
    "phone_beat",
    "report_phone_pickups",
];

/// Every table of the phone migration; none has a client write path.
pub(super) const ALL_TABLES: &[&str] = &[
    "phone_pairs",
    "pair_codes",
    "pair_attempts",
    "phone_lock_state",
    "phone_beats",
    "phone_pickups",
];

/// Tables whose rows the data API serves through the SELECT policies.
const PHONE_TABLES: &[&str] = &[
    "phone_pairs",
    "phone_lock_state",
    "phone_beats",
    "phone_pickups",
];

struct PairRow {
    id: String,
    owner: String,
    phone: String,
    paired_ms: i64,
    revoked_ms: Option<i64>,
}

struct CodeRow {
    owner: String,
    hash: u64,
    expires_ms: i64,
    used: bool,
}

struct LeaseRow {
    owner: String,
    seq: i64,
    run_id: String,
    state: String,
    cause: Option<String>,
    issued_ms: i64,
    valid_until_ms: i64,
}

struct BeatRow {
    owner: String,
    phone: String,
    lease_seq: i64,
    locked: bool,
    service_enabled: bool,
    at_ms: i64,
}

struct PickupRow {
    owner: String,
    phone: String,
    segment_id: String,
    count: i64,
    at_ms: i64,
}

#[derive(Default)]
pub(super) struct PhoneState {
    seq: u64,
    rng: u64,
    anonymous: HashSet<String>,
    pairs: Vec<PairRow>,
    codes: Vec<CodeRow>,
    attempts: Vec<(String, i64)>,
    leases: Vec<LeaseRow>,
    beats: Vec<BeatRow>,
    pickups: Vec<PickupRow>,
}

impl PhoneState {
    /// Deterministic stream for the mock only; the real server uses its CSPRNG.
    fn next_random(&mut self) -> u64 {
        if self.rng == 0 {
            self.rng = 0xD1B5_4A32_D192_ED03;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn active_pair_of_owner(&self, owner: &str) -> Option<&PairRow> {
        self.pairs
            .iter()
            .find(|p| p.owner == owner && p.revoked_ms.is_none())
    }

    fn active_pair_of_phone(&self, phone: &str) -> Option<&PairRow> {
        self.pairs
            .iter()
            .find(|p| p.phone == phone && p.revoked_ms.is_none())
    }

    /// The owner a phone is bound to, if the pairing is not revoked.
    fn owner_of(&self, phone: &str) -> Option<String> {
        self.active_pair_of_phone(phone).map(|p| p.owner.clone())
    }

    /// Whether `user` may read `owner`'s phone rows: the owner, or the unrevoked paired phone.
    fn may_read(&self, user: &str, owner: &str) -> bool {
        user == owner
            || self
                .active_pair_of_owner(owner)
                .is_some_and(|p| p.phone == user)
    }
}

fn code(c: &str) -> Value {
    json!({ "code": c })
}

fn text<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn hash_of(code: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    code.hash(&mut hasher);
    hasher.finish()
}

fn lease_json(row: &LeaseRow) -> Value {
    json!({
        "run_id": row.run_id, "seq": row.seq, "state": row.state, "release_cause": row.cause,
        "issued_at_ms": row.issued_ms, "valid_until_ms": row.valid_until_ms,
    })
}

impl FakeBackend {
    /// Supabase anonymous sign-in: a brand-new user with no email, flagged anonymous.
    pub fn sign_in_anonymously(&self) -> Result<AuthTokens, AuthError> {
        let mut state = self.lock();
        if state.outage {
            return Err(AuthError::Network(TransportError::Connect));
        }
        state.next_id += 1;
        let id = format!(
            "00000000-0000-4000-8000-{:012}",
            0xa000_0000_u64 + state.next_id
        );
        state.phone.anonymous.insert(id.clone());
        Ok(self.issue(&mut state, &id))
    }

    pub(super) fn phone_rpc(
        &self,
        caller: &Caller,
        name: &str,
        args: &Map<String, Value>,
    ) -> Result<Value, DataError> {
        let user = Self::user_of(caller)?.to_string();
        let now = self.now();
        let mut state = self.lock();
        let phone = &mut state.phone;
        Ok(match name {
            "create_pair_code" => create_pair_code(phone, &user, now),
            "claim_pair" => claim_pair(phone, &user, now, args),
            "revoke_pair" => revoke_pair(phone, &user, now),
            "phone_status" => phone_status(phone, &user, now),
            "publish_phone_lock" => publish_phone_lock(phone, &user, now, args),
            "phone_lease" => phone_lease(phone, &user, now),
            "phone_beat" => phone_beat(phone, &user, now, args),
            "report_phone_pickups" => report_phone_pickups(phone, &user, now, args),
            _ => return Err(DataError::NotFound),
        })
    }

    /// The SELECT policies of the phone tables. `None` means the table is not one of them (the
    /// caller continues with the generic matrix, which refuses `none` tables).
    pub(super) fn phone_select(
        &self,
        caller: &Caller,
        table: &str,
        filters: &[(String, String)],
    ) -> Option<Result<Vec<Value>, DataError>> {
        if !PHONE_TABLES.contains(&table) {
            return None;
        }
        let user = match caller {
            Caller::User(id) => Some(id.as_str()),
            Caller::Service => None,
            Caller::Anon => return Some(Err(DataError::Forbidden)),
        };
        let state = self.lock();
        let phone = &state.phone;
        let visible = |owner: &str| user.is_none_or(|u| phone.may_read(u, owner));
        let rows: Vec<Value> = match table {
            "phone_pairs" => phone
                .pairs
                .iter()
                .filter(|p| user.is_none_or(|u| p.owner == u || p.phone == u))
                .map(|p| {
                    json!({
                        "id": p.id, "owner_user_id": p.owner, "phone_user_id": p.phone,
                        "paired_at": p.paired_ms, "revoked_at": p.revoked_ms,
                    })
                })
                .collect(),
            "phone_lock_state" => phone.leases.iter().filter(|l| visible(&l.owner)).map(|l| {
                let mut row = lease_json(l);
                row["owner_user_id"] = json!(l.owner);
                row
            }).collect(),
            "phone_beats" => phone
                .beats
                .iter()
                .filter(|b| visible(&b.owner))
                .map(|b| {
                    json!({
                        "owner_user_id": b.owner, "phone_user_id": b.phone, "lease_seq": b.lease_seq,
                        "locked": b.locked, "service_enabled": b.service_enabled, "at": b.at_ms,
                    })
                })
                .collect(),
            _ => phone
                .pickups
                .iter()
                .filter(|p| visible(&p.owner))
                .map(|p| {
                    json!({
                        "owner_user_id": p.owner, "phone_user_id": p.phone, "segment_id": p.segment_id,
                        "count": p.count, "reported_at": p.at_ms,
                    })
                })
                .collect(),
        };
        Some(Ok(rows
            .into_iter()
            .filter(|row| {
                filters.iter().all(|(col, wanted)| match row.get(col) {
                    Some(Value::String(s)) => s == wanted,
                    Some(other) => *other.to_string() == **wanted,
                    None => false,
                })
            })
            .collect()))
    }

    /// Test hook: how many pairing rows exist (revoked ones included).
    pub fn phone_pair_rows(&self) -> usize {
        self.lock().phone.pairs.len()
    }

    /// Test hook: stored code hashes. The plain code is never kept.
    pub fn stored_pair_code_hashes(&self) -> usize {
        self.lock().phone.codes.len()
    }

    /// Test hook: claim attempts the server remembers for a user.
    pub fn pair_attempts_of(&self, user_id: &str) -> usize {
        self.lock()
            .phone
            .attempts
            .iter()
            .filter(|(u, _)| u == user_id)
            .count()
    }

    /// Test hook: forget the attempt counter (the real one ages out after a minute).
    pub fn clear_pair_attempts(&self) {
        self.lock().phone.attempts.clear();
    }
}

fn create_pair_code(phone: &mut PhoneState, user: &str, now: i64) -> Value {
    if phone.anonymous.contains(user) {
        return code("anonymous_not_allowed");
    }
    if phone.active_pair_of_owner(user).is_some() {
        return code("already_paired");
    }
    for old in phone
        .codes
        .iter_mut()
        .filter(|c| c.owner == user && !c.used && c.expires_ms > now)
    {
        old.expires_ms = now;
    }
    for _ in 0..5 {
        let mut text = String::with_capacity(10);
        for _ in 0..10 {
            let r = phone.next_random();
            text.push(char::from(
                ALPHABET[usize::try_from(r >> 59).unwrap_or(0) % ALPHABET.len()],
            ));
        }
        let hash = hash_of(&text);
        if phone.codes.iter().any(|c| c.hash == hash) {
            continue;
        }
        let expires = now + CODE_TTL_MS;
        phone.codes.push(CodeRow {
            owner: user.to_string(),
            hash,
            expires_ms: expires,
            used: false,
        });
        return json!({ "code": "ok", "pair_code": text, "expires_at_ms": expires });
    }
    code("invalid_input")
}

fn claim_pair(phone: &mut PhoneState, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    phone.attempts.retain(|(_, at)| now - at < DAY_MS);
    let recent = phone
        .attempts
        .iter()
        .filter(|(u, at)| u == user && now - at < ATTEMPT_WINDOW_MS)
        .count();
    if recent >= ATTEMPT_LIMIT {
        return code("rate_limited");
    }
    // Counted before anything is checked, so every refusal below leaves it in place.
    phone.attempts.push((user.to_string(), now));

    let Some(normalized) = text(args, "p_code").and_then(normalize_code) else {
        return code("invalid_code");
    };
    let hash = hash_of(&normalized);
    let Some(index) = phone.codes.iter().position(|c| c.hash == hash) else {
        return code("invalid_code");
    };
    let (owner, used, expires) = {
        let row = &phone.codes[index];
        (row.owner.clone(), row.used, row.expires_ms)
    };
    if used {
        return code("used");
    }
    if expires <= now {
        return code("expired");
    }
    if owner == user {
        return code("cannot_self");
    }
    if phone.active_pair_of_owner(&owner).is_some() {
        return code("already_paired");
    }
    if phone.active_pair_of_phone(user).is_some() {
        return code("phone_already_paired");
    }
    phone.codes[index].used = true;
    phone.seq += 1;
    let id = format!("40000000-0000-4000-8000-{:012}", phone.seq);
    phone.pairs.push(PairRow {
        id: id.clone(),
        owner: owner.clone(),
        phone: user.to_string(),
        paired_ms: now,
        revoked_ms: None,
    });
    json!({ "code": "ok", "pair_id": id, "owner_user_id": owner, "server_now_ms": now })
}

fn revoke_pair(phone: &mut PhoneState, user: &str, now: i64) -> Value {
    let mut changed = 0;
    for pair in phone
        .pairs
        .iter_mut()
        .filter(|p| p.revoked_ms.is_none() && (p.owner == user || p.phone == user))
    {
        pair.revoked_ms = Some(now);
        changed += 1;
    }
    code(if changed >= 1 { "ok" } else { "not_found" })
}

fn phone_status(phone: &PhoneState, user: &str, now: i64) -> Value {
    let seq = phone
        .leases
        .iter()
        .find(|l| l.owner == user)
        .map_or(0, |l| l.seq);
    let Some(pair) = phone.active_pair_of_owner(user) else {
        return json!({ "code": "ok", "paired": false, "lease_seq": seq });
    };
    let beat = phone.beats.iter().rfind(|b| b.owner == user);
    json!({
        "code": "ok", "paired": true, "paired_at_ms": pair.paired_ms, "lease_seq": seq,
        "last_beat_ms": beat.map(|b| b.at_ms), "service_enabled": beat.map(|b| b.service_enabled), "server_now_ms": now,
    })
}

fn publish_phone_lock(
    phone: &mut PhoneState,
    user: &str,
    now: i64,
    args: &Map<String, Value>,
) -> Value {
    let seq = args.get("p_seq").and_then(Value::as_i64);
    let run_id = text(args, "p_run_id");
    let state = text(args, "p_state");
    let cause = text(args, "p_release_cause");
    let until = args.get("p_valid_until_ms").and_then(Value::as_i64);
    let (Some(seq), Some(run_id), Some(state)) = (seq, run_id, state) else {
        return code("invalid_input");
    };
    if seq < 1 || !(1..=64).contains(&run_id.len()) || !matches!(state, "locked" | "free") {
        return code("invalid_input");
    }
    let valid = match state {
        "locked" => {
            let Some(until) = until.filter(|&u| cause.is_none() && u > now) else {
                return code("invalid_input");
            };
            if until - now > MAX_LEASE_MS {
                return code("lease_too_long");
            }
            until
        }
        _ => {
            if !cause.is_some_and(|c| CAUSES.contains(&c)) {
                return code("invalid_input");
            }
            now
        }
    };
    if let Some(current) = phone.leases.iter_mut().find(|l| l.owner == user) {
        if seq < current.seq {
            return json!({ "code": "stale_seq", "current_seq": current.seq });
        }
        if seq == current.seq {
            let same = current.run_id == run_id
                && current.state == state
                && current.cause.as_deref() == cause
                && (state == "free" || current.valid_until_ms == valid);
            return if same {
                json!({ "code": "ok", "seq": seq, "replayed": true, "server_now_ms": now })
            } else {
                json!({ "code": "stale_seq", "current_seq": current.seq })
            };
        }
        *current = LeaseRow {
            owner: user.to_string(),
            seq,
            run_id: run_id.to_string(),
            state: state.to_string(),
            cause: cause.map(str::to_string),
            issued_ms: now,
            valid_until_ms: valid,
        };
    } else {
        phone.leases.push(LeaseRow {
            owner: user.to_string(),
            seq,
            run_id: run_id.to_string(),
            state: state.to_string(),
            cause: cause.map(str::to_string),
            issued_ms: now,
            valid_until_ms: valid,
        });
    }
    json!({ "code": "ok", "seq": seq, "replayed": false, "server_now_ms": now })
}

fn phone_lease(phone: &PhoneState, user: &str, now: i64) -> Value {
    let Some(owner) = phone.owner_of(user) else {
        return code("not_paired");
    };
    let lease = phone
        .leases
        .iter()
        .find(|l| l.owner == owner)
        .map(lease_json);
    json!({ "code": "ok", "lease": lease, "server_now_ms": now })
}

fn phone_beat(phone: &mut PhoneState, user: &str, now: i64, args: &Map<String, Value>) -> Value {
    let Some(owner) = phone.owner_of(user) else {
        return code("not_paired");
    };
    let (Some(seq), Some(locked), Some(service_enabled)) = (
        args.get("p_lease_seq").and_then(Value::as_i64),
        args.get("p_locked").and_then(Value::as_bool),
        args.get("p_service_enabled").and_then(Value::as_bool),
    ) else {
        return code("invalid_input");
    };
    if seq < 0 {
        return code("invalid_input");
    }
    phone
        .beats
        .retain(|b| b.owner != owner || now - b.at_ms < 7 * DAY_MS);
    phone.beats.push(BeatRow {
        owner,
        phone: user.to_string(),
        lease_seq: seq,
        locked,
        service_enabled,
        at_ms: now,
    });
    json!({ "code": "ok", "server_now_ms": now })
}

fn report_phone_pickups(
    phone: &mut PhoneState,
    user: &str,
    now: i64,
    args: &Map<String, Value>,
) -> Value {
    let Some(owner) = phone.owner_of(user) else {
        return code("not_paired");
    };
    let (Some(segment), Some(count)) = (
        text(args, "p_segment_id"),
        args.get("p_count").and_then(Value::as_i64),
    ) else {
        return code("invalid_input");
    };
    if !(1..=64).contains(&segment.len()) || !(0..=100_000).contains(&count) {
        return code("invalid_input");
    }
    if !phone
        .pickups
        .iter()
        .any(|p| p.owner == owner && p.segment_id == segment && p.count == count)
    {
        phone.pickups.push(PickupRow {
            owner,
            phone: user.to_string(),
            segment_id: segment.to_string(),
            count,
            at_ms: now,
        });
    }
    code("ok")
}

/// The anonymous sign-in face of the backend, for the phone session.
pub struct FakeAnonymousAuth {
    backend: std::sync::Arc<FakeBackend>,
}

impl crate::phone::session::AnonymousAuth for FakeAnonymousAuth {
    fn sign_in_anonymously(&self) -> Result<AuthTokens, AuthError> {
        self.backend.sign_in_anonymously()
    }

    fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, AuthError> {
        self.backend.refresh_session(refresh_token)
    }
}

impl FakeBackend {
    pub fn anonymous_auth(
        self: &std::sync::Arc<Self>,
    ) -> std::sync::Arc<dyn crate::phone::session::AnonymousAuth> {
        std::sync::Arc::new(FakeAnonymousAuth {
            backend: std::sync::Arc::clone(self),
        })
    }
}
