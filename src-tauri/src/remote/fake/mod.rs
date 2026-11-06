//! In-memory stand-in for the Supabase project: sign-in (fixed code `000000`), the S2 tables with
//! the row-level-security matrix, and the S2 RPCs. Several users can exist at once.
//!
//! It is the mock adapter behind mock mode and the counterpart that real-adapter tests talk to
//! (it also implements `HttpTransport`, speaking the GoTrue/PostgREST routes the real adapters
//! use). It enforces the same matrix as the SQL policies, but it is NOT proof that the SQL works:
//! only a real Postgres run can show that (V2-8).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Map, Value};

use super::auth::{AuthError, AuthProvider, AuthTokens, DeviceDirectory};
use super::http::{HttpRequest, HttpResponse, HttpTransport, Method, TransportError};
use super::Clock;

/// The only code the fake accepts. Mock mode only: the real adapter never sees this value.
pub const FAKE_CODE: &str = "000000";
/// Public client key the mock project expects in `apikey`.
pub const FAKE_ANON_KEY: &str = "fake-anon-key";
/// Host the mock project answers as (mock mode never opens a socket).
pub const FAKE_ENDPOINT: &str = "http://127.0.0.1:1";
const ACCESS_TTL_SEC: i64 = 3600;

const RLS_MATRIX: &str = include_str!("../../../../fixtures/sync/rls-matrix.json");
const RPC_CONTRACT: &str = include_str!("../../../../fixtures/sync/rpc-contract.json");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    Anon,
    User(String),
    /// Server-side code (Edge Functions). Not reachable through the HTTP surface.
    Service,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Select,
    Insert,
    Update,
    Delete,
}

impl Op {
    fn key(self) -> &'static str {
        match self {
            Op::Select => "select",
            Op::Insert => "insert",
            Op::Update => "update",
            Op::Delete => "delete",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DataError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("permission denied")]
    Forbidden,
    #[error("duplicate key")]
    Conflict,
    #[error("bad request")]
    BadRequest,
    #[error("not found")]
    NotFound,
}

impl DataError {
    fn status(self) -> u16 {
        match self {
            DataError::Unauthorized => 401,
            DataError::Forbidden => 403,
            DataError::Conflict => 409,
            DataError::BadRequest => 400,
            DataError::NotFound => 404,
        }
    }
}

struct Session {
    user_id: String,
    expires_at_ms: i64,
    revoked: bool,
}

struct RefreshEntry {
    user_id: String,
    live: bool,
}

#[derive(Default)]
struct State {
    users: HashMap<String, String>,
    next_id: u64,
    access: HashMap<String, Session>,
    refresh: HashMap<String, RefreshEntry>,
    tables: HashMap<String, Vec<Map<String, Value>>>,
    outage: bool,
    refresh_calls: u32,
    verify_calls: u32,
    next_row_id: i64,
}

pub struct FakeBackend {
    clock: Arc<dyn Clock>,
    matrix: HashMap<String, HashMap<String, String>>,
    state: Mutex<State>,
}

/// Table name -> unique key columns (devices.id is the primary key, etc.).
fn unique_keys(table: &str) -> &'static [&'static str] {
    match table {
        "devices" => &["id"],
        "plan_history_mirror" => &["device_id", "local_id"],
        "notify_channels" => &["user_id"],
        _ => &[],
    }
}

impl FakeBackend {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        let parsed: Value = serde_json::from_str(RLS_MATRIX).unwrap_or(Value::Null);
        let mut matrix = HashMap::new();
        if let Some(tables) = parsed.get("tables").and_then(Value::as_object) {
            for (table, ops) in tables {
                let ops = ops
                    .as_object()
                    .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
                    .unwrap_or_default();
                matrix.insert(table.clone(), ops);
            }
        }
        Self { clock, matrix, state: Mutex::new(State::default()) }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn now(&self) -> i64 {
        self.clock.now_ms().unwrap_or(0)
    }

    /// The `AuthProvider` face of the backend.
    pub fn auth(self: &Arc<Self>) -> Arc<dyn AuthProvider> {
        Arc::new(FakeAuth { backend: Arc::clone(self) })
    }

    pub fn table_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.matrix.keys().cloned().collect();
        names.sort();
        names
    }

    // ---- auth ----

    fn issue(&self, state: &mut State, user_id: &str) -> AuthTokens {
        state.next_id += 1;
        let n = state.next_id;
        let access = format!("fake-access-{n}");
        let refresh = format!("fake-refresh-{n}");
        state.access.insert(
            access.clone(),
            Session { user_id: user_id.to_string(), expires_at_ms: self.now() + ACCESS_TTL_SEC * 1000, revoked: false },
        );
        state.refresh.insert(refresh.clone(), RefreshEntry { user_id: user_id.to_string(), live: true });
        AuthTokens { access_token: access, refresh_token: refresh, expires_in_sec: ACCESS_TTL_SEC, user_id: user_id.to_string() }
    }

    fn request_otp(&self, email: &str) -> Result<(), AuthError> {
        let mut state = self.lock();
        if state.outage {
            return Err(AuthError::Network(TransportError::Connect));
        }
        if !state.users.contains_key(email) {
            let id = format!("00000000-0000-4000-8000-{:012}", state.users.len() + 1);
            state.users.insert(email.to_string(), id);
        }
        Ok(())
    }

    fn verify_otp(&self, email: &str, code: &str) -> Result<AuthTokens, AuthError> {
        let mut state = self.lock();
        if state.outage {
            return Err(AuthError::Network(TransportError::Connect));
        }
        state.verify_calls += 1;
        let Some(user_id) = state.users.get(email).cloned() else { return Err(AuthError::InvalidCode) };
        if code != FAKE_CODE {
            return Err(AuthError::InvalidCode);
        }
        Ok(self.issue(&mut state, &user_id))
    }

    fn refresh_session(&self, token: &str) -> Result<AuthTokens, AuthError> {
        let mut state = self.lock();
        if state.outage {
            return Err(AuthError::Network(TransportError::Connect));
        }
        state.refresh_calls += 1;
        let user_id = match state.refresh.get_mut(token) {
            Some(entry) if entry.live => {
                // Rotation: the presented token is spent.
                entry.live = false;
                entry.user_id.clone()
            }
            _ => return Err(AuthError::Expired),
        };
        Ok(self.issue(&mut state, &user_id))
    }

    fn end_session(&self, access_token: &str) -> Result<(), AuthError> {
        let mut state = self.lock();
        if state.outage {
            return Err(AuthError::Network(TransportError::Connect));
        }
        let user = state.access.get_mut(access_token).map(|s| {
            s.revoked = true;
            s.user_id.clone()
        });
        if let Some(user_id) = user {
            for entry in state.refresh.values_mut().filter(|e| e.user_id == user_id) {
                entry.live = false;
            }
        }
        Ok(())
    }

    /// Who is calling, from the bearer token. An expired or revoked token is `Unauthorized` (the
    /// real server answers 401 and the client refreshes).
    pub fn caller_for(&self, bearer: Option<&str>) -> Result<Caller, DataError> {
        let Some(token) = bearer else { return Ok(Caller::Anon) };
        if token == FAKE_ANON_KEY {
            return Ok(Caller::Anon);
        }
        let state = self.lock();
        match state.access.get(token) {
            Some(session) if !session.revoked && session.expires_at_ms > self.now() => Ok(Caller::User(session.user_id.clone())),
            _ => Err(DataError::Unauthorized),
        }
    }

    // ---- data API with the RLS matrix ----

    fn permitted(&self, caller: &Caller, table: &str, op: Op) -> Result<Option<String>, DataError> {
        let rule = self.matrix.get(table).ok_or(DataError::NotFound)?;
        match caller {
            Caller::Service => Ok(None),
            Caller::Anon => Err(DataError::Forbidden),
            Caller::User(id) => match rule.get(op.key()).map(String::as_str) {
                Some("owner") => Ok(Some(id.clone())),
                _ => Err(DataError::Forbidden),
            },
        }
    }

    fn matches(row: &Map<String, Value>, filters: &[(String, String)]) -> bool {
        filters.iter().all(|(col, wanted)| match row.get(col) {
            Some(Value::String(s)) => s == wanted,
            Some(other) => *other.to_string() == **wanted,
            None => false,
        })
    }

    pub fn select(&self, caller: &Caller, table: &str, filters: &[(String, String)]) -> Result<Vec<Value>, DataError> {
        let owner = self.permitted(caller, table, Op::Select)?;
        let state = self.lock();
        let rows = state.tables.get(table).map(Vec::as_slice).unwrap_or_default();
        Ok(rows
            .iter()
            .filter(|row| owner.as_ref().is_none_or(|id| row.get("user_id").and_then(Value::as_str) == Some(id)))
            .filter(|row| Self::matches(row, filters))
            .map(|row| Value::Object(row.clone()))
            .collect())
    }

    /// `ignore_duplicates` mirrors PostgREST `Prefer: resolution=ignore-duplicates` (insert is a
    /// no-op on a key clash instead of an error); it is how history batches stay idempotent.
    pub fn insert(&self, caller: &Caller, table: &str, row: &Value, ignore_duplicates: bool) -> Result<(), DataError> {
        let Some(items) = row.as_array() else { return self.insert_one(caller, table, row, ignore_duplicates) };
        // A bulk insert is one transaction: every row or none.
        let before = self.lock().tables.get(table).cloned();
        for item in items {
            if let Err(err) = self.insert_one(caller, table, item, ignore_duplicates) {
                let mut state = self.lock();
                match before {
                    Some(rows) => {
                        state.tables.insert(table.to_string(), rows);
                    }
                    None => {
                        state.tables.remove(table);
                    }
                }
                return Err(err);
            }
        }
        Ok(())
    }

    /// `Prefer: resolution=merge-duplicates`: insert, or overwrite the row with the same key.
    /// Needs both the insert and the update permission, like the real upsert.
    pub fn upsert(&self, caller: &Caller, table: &str, row: &Value) -> Result<(), DataError> {
        let owner = self.permitted(caller, table, Op::Insert)?;
        self.permitted(caller, table, Op::Update)?;
        let Some(object) = row.as_object() else { return Err(DataError::BadRequest) };
        let keys = unique_keys(table);
        if keys.is_empty() {
            return Err(DataError::BadRequest);
        }
        let mut object = object.clone();
        if let Some(id) = &owner {
            match object.get("user_id").and_then(Value::as_str) {
                Some(given) if given != id => return Err(DataError::Forbidden),
                _ => {
                    object.insert("user_id".into(), Value::String(id.clone()));
                }
            }
        }
        {
            let mut state = self.lock();
            let rows = state.tables.entry(table.to_string()).or_default();
            if let Some(existing) = rows.iter_mut().find(|e| keys.iter().all(|k| e.get(*k) == object.get(*k))) {
                for (key, value) in object {
                    if key != "user_id" {
                        existing.insert(key, value);
                    }
                }
                return Ok(());
            }
        }
        self.insert_one(caller, table, &Value::Object(object), false)
    }

    fn insert_one(&self, caller: &Caller, table: &str, row: &Value, ignore_duplicates: bool) -> Result<(), DataError> {
        let owner = self.permitted(caller, table, Op::Insert)?;
        let Some(object) = row.as_object() else { return Err(DataError::BadRequest) };
        let mut object = object.clone();
        if let Some(id) = &owner {
            // WITH CHECK (user_id = auth.uid()): a row for someone else is refused.
            match object.get("user_id").and_then(Value::as_str) {
                Some(given) if given != id => return Err(DataError::Forbidden),
                _ => {
                    object.insert("user_id".into(), Value::String(id.clone()));
                }
            }
            if table == "plan_history_mirror" {
                let device = object.get("device_id").and_then(Value::as_str).unwrap_or_default().to_string();
                if !self.device_owned_by(id, &device) {
                    return Err(DataError::Forbidden);
                }
            }
        }
        let mut state = self.lock();
        let keys = unique_keys(table);
        let clash = !keys.is_empty()
            && state
                .tables
                .get(table)
                .is_some_and(|rows| rows.iter().any(|existing| keys.iter().all(|k| existing.get(*k) == object.get(*k))));
        if clash {
            return if ignore_duplicates { Ok(()) } else { Err(DataError::Conflict) };
        }
        state.next_row_id += 1;
        let id = state.next_row_id;
        object.entry("id".to_string()).or_insert(json!(id));
        state.tables.entry(table.to_string()).or_default().push(object);
        Ok(())
    }

    pub fn update(&self, caller: &Caller, table: &str, filters: &[(String, String)], patch: &Value) -> Result<usize, DataError> {
        let owner = self.permitted(caller, table, Op::Update)?;
        let Some(patch) = patch.as_object() else { return Err(DataError::BadRequest) };
        if patch.contains_key("user_id") {
            return Err(DataError::Forbidden);
        }
        let mut state = self.lock();
        let mut changed = 0;
        for row in state.tables.entry(table.to_string()).or_default().iter_mut() {
            let mine = owner.as_ref().is_none_or(|id| row.get("user_id").and_then(Value::as_str) == Some(id));
            if mine && Self::matches(row, filters) {
                for (k, v) in patch {
                    row.insert(k.clone(), v.clone());
                }
                changed += 1;
            }
        }
        Ok(changed)
    }

    pub fn delete(&self, caller: &Caller, table: &str, filters: &[(String, String)]) -> Result<usize, DataError> {
        let owner = self.permitted(caller, table, Op::Delete)?;
        let mut state = self.lock();
        let rows = state.tables.entry(table.to_string()).or_default();
        let before = rows.len();
        let mut removed_devices = Vec::new();
        rows.retain(|row| {
            let mine = owner.as_ref().is_none_or(|id| row.get("user_id").and_then(Value::as_str) == Some(id));
            let hit = mine && Self::matches(row, filters);
            if hit && table == "devices" {
                if let Some(id) = row.get("id").and_then(Value::as_str) {
                    removed_devices.push(id.to_string());
                }
            }
            !hit
        });
        let removed = before - rows.len();
        // ON DELETE CASCADE from devices.
        for device in removed_devices {
            for name in ["plan_snapshots", "plan_occurrences", "plan_history_mirror", "presence"] {
                if let Some(rows) = state.tables.get_mut(name) {
                    rows.retain(|r| r.get("device_id").and_then(Value::as_str) != Some(device.as_str()));
                }
            }
        }
        Ok(removed)
    }

    fn device_owned_by(&self, user_id: &str, device_id: &str) -> bool {
        self.lock().tables.get("devices").is_some_and(|rows| {
            rows.iter().any(|r| {
                r.get("id").and_then(Value::as_str) == Some(device_id) && r.get("user_id").and_then(Value::as_str) == Some(user_id)
            })
        })
    }

    // ---- RPCs ----

    pub fn rpc(&self, caller: &Caller, name: &str, args: &Value) -> Result<Value, DataError> {
        let contract: Value = serde_json::from_str(RPC_CONTRACT).map_err(|_| DataError::BadRequest)?;
        let entry = contract
            .get("rpcs")
            .and_then(Value::as_array)
            .and_then(|rpcs| rpcs.iter().find(|r| r.get("name").and_then(Value::as_str) == Some(name)))
            .ok_or(DataError::NotFound)?;
        let role_ok = match caller {
            Caller::Service => true,
            Caller::User(_) => has_role(entry, "authenticated"),
            Caller::Anon => false,
        };
        if !role_ok {
            return Err(DataError::Forbidden);
        }
        let args = args.as_object().ok_or(DataError::BadRequest)?;
        let code = match name {
            "replace_plan_snapshot" => self.replace_plan_snapshot(caller, args),
            "heartbeat" => self.heartbeat(caller, args),
            "claim_alert" => self.claim_alert(args),
            _ => return Err(DataError::NotFound),
        };
        Ok(Value::String(code?.to_string()))
    }

    fn user_of(caller: &Caller) -> Result<&str, DataError> {
        match caller {
            Caller::User(id) => Ok(id),
            _ => Err(DataError::Forbidden),
        }
    }

    fn replace_plan_snapshot(&self, caller: &Caller, args: &Map<String, Value>) -> Result<&'static str, DataError> {
        let user = Self::user_of(caller)?;
        let device = args.get("p_device_id").and_then(Value::as_str).unwrap_or_default();
        if !self.device_owned_by(user, device) {
            return Ok("device_not_owned");
        }
        let (Some(hash), Some(generated), Some(horizon), Some(occurrences)) = (
            args.get("p_snapshot_hash").and_then(Value::as_str),
            args.get("p_generated_at_ms").and_then(Value::as_i64),
            args.get("p_horizon_days").and_then(Value::as_i64),
            args.get("p_occurrences").and_then(Value::as_array),
        ) else {
            return Ok("invalid_input");
        };
        let mut rows = Vec::with_capacity(occurrences.len());
        let mut seen = HashSet::new();
        for occurrence in occurrences {
            let Some(o) = occurrence.as_object() else { return Ok("invalid_input") };
            let key = o.get("occurrence_key").and_then(Value::as_str);
            let status_ok = matches!(o.get("status").and_then(Value::as_str), Some("planned" | "skipped" | "canceled" | "moved"));
            let (Some(key), Some(start), Some(plan_name), Some(subject), Some(sets)) = (
                key,
                o.get("start_at_utc_ms").and_then(Value::as_i64),
                o.get("plan_name").and_then(Value::as_str),
                o.get("subject").and_then(Value::as_str),
                o.get("sets").and_then(Value::as_i64),
            ) else {
                return Ok("invalid_input");
            };
            if !status_ok || sets < 1 || !seen.insert(key.to_string()) {
                return Ok("invalid_input");
            }
            rows.push(json!({
                "user_id": user, "device_id": device, "occurrence_key": key, "start_at_utc_ms": start,
                "plan_name": plan_name, "subject": subject, "sets": sets, "status": o["status"],
            }));
        }
        let mut state = self.lock();
        let snapshots = state.tables.entry("plan_snapshots".into()).or_default();
        if snapshots.iter().any(|s| {
            s.get("device_id").and_then(Value::as_str) == Some(device) && s.get("snapshot_hash").and_then(Value::as_str) == Some(hash)
        }) {
            return Ok("unchanged");
        }
        snapshots.retain(|s| s.get("device_id").and_then(Value::as_str) != Some(device));
        if let Value::Object(snapshot) = json!({
            "user_id": user, "device_id": device, "snapshot_hash": hash, "horizon_days": horizon, "generated_at_ms": generated,
        }) {
            snapshots.push(snapshot);
        }
        let table = state.tables.entry("plan_occurrences".into()).or_default();
        table.retain(|r| r.get("device_id").and_then(Value::as_str) != Some(device));
        for row in rows {
            if let Value::Object(object) = row {
                table.push(object);
            }
        }
        Ok("ok")
    }

    fn heartbeat(&self, caller: &Caller, args: &Map<String, Value>) -> Result<&'static str, DataError> {
        let user = Self::user_of(caller)?;
        let device = args.get("p_device_id").and_then(Value::as_str).unwrap_or_default();
        if !self.device_owned_by(user, device) {
            return Ok("device_not_owned");
        }
        let seen_at = self.now();
        let mut state = self.lock();
        let presence = state.tables.entry("presence".into()).or_default();
        presence.retain(|r| r.get("device_id").and_then(Value::as_str) != Some(device));
        if let Value::Object(row) = json!({
            "user_id": user, "device_id": device, "last_seen_at_ms": seen_at,
            "app_version": args.get("p_app_version").cloned().unwrap_or(Value::Null),
            "studying": args.get("p_studying").cloned().unwrap_or(Value::Bool(false)),
        }) {
            presence.push(row);
        }
        Ok("ok")
    }

    fn claim_alert(&self, args: &Map<String, Value>) -> Result<&'static str, DataError> {
        let (Some(user), Some(key), Some(kind)) = (
            args.get("p_user_id").and_then(Value::as_str),
            args.get("p_occurrence_key").and_then(Value::as_str),
            args.get("p_kind").and_then(Value::as_str),
        ) else {
            return Err(DataError::BadRequest);
        };
        let claimed_at = self.now();
        let mut state = self.lock();
        let log = state.tables.entry("alert_log".into()).or_default();
        let exists = log.iter().any(|r| {
            r.get("user_id").and_then(Value::as_str) == Some(user)
                && r.get("occurrence_key").and_then(Value::as_str) == Some(key)
                && r.get("kind").and_then(Value::as_str) == Some(kind)
        });
        if exists {
            return Ok("duplicate");
        }
        if let Value::Object(row) = json!({"user_id": user, "occurrence_key": key, "kind": kind, "claimed_at_ms": claimed_at}) {
            log.push(row);
        }
        Ok("claimed")
    }

    // ---- test hooks ----

    pub fn set_outage(&self, down: bool) {
        self.lock().outage = down;
    }

    /// Every row of a table as the server holds it, bypassing the permission matrix. For tests
    /// that assert what reached the server.
    pub fn table_rows(&self, table: &str) -> Vec<Value> {
        self.lock().tables.get(table).map(|rows| rows.iter().cloned().map(Value::Object).collect()).unwrap_or_default()
    }

    pub fn refresh_calls(&self) -> u32 {
        self.lock().refresh_calls
    }

    pub fn verify_calls(&self) -> u32 {
        self.lock().verify_calls
    }

    /// Server-side revocation of every refresh token (a password reset, an admin kick).
    pub fn revoke_all_sessions(&self) {
        for entry in self.lock().refresh.values_mut() {
            entry.live = false;
        }
    }

    pub fn is_revoked(&self, access_token: &str) -> bool {
        self.lock().access.get(access_token).is_some_and(|s| s.revoked)
    }

    pub fn register_device_for(&self, access_token: &str, device_id: &str) {
        let caller = self.caller_for(Some(access_token));
        if let Ok(caller) = caller {
            let row = json!({"id": device_id, "app_version": "0.1.0", "platform": "macos"});
            let _ = self.insert(&caller, "devices", &row, false);
        }
    }

    pub fn device_count(&self) -> usize {
        self.lock().tables.get("devices").map_or(0, Vec::len)
    }
}

fn has_role(entry: &Value, role: &str) -> bool {
    entry.get("roles").and_then(Value::as_array).is_some_and(|roles| roles.iter().any(|r| r.as_str() == Some(role)))
}

impl DeviceDirectory for FakeBackend {
    fn remove_device(&self, access_token: &str, device_id: &str) -> Result<(), AuthError> {
        if self.lock().outage {
            return Err(AuthError::Network(TransportError::Connect));
        }
        let caller = self.caller_for(Some(access_token)).map_err(|_| AuthError::Expired)?;
        self.delete(&caller, "devices", &[("id".to_string(), device_id.to_string())])
            .map(|_| ())
            .map_err(|_| AuthError::Server { status: 403 })
    }
}

struct FakeAuth {
    backend: Arc<FakeBackend>,
}

impl AuthProvider for FakeAuth {
    fn request_otp(&self, email: &str) -> Result<(), AuthError> {
        self.backend.request_otp(email)
    }
    fn verify_otp(&self, email: &str, code: &str) -> Result<AuthTokens, AuthError> {
        self.backend.verify_otp(email, code)
    }
    fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, AuthError> {
        self.backend.refresh_session(refresh_token)
    }
    fn sign_out(&self, access_token: &str) -> Result<(), AuthError> {
        self.backend.end_session(access_token)
    }
}

// ---- HTTP surface (PostgREST / GoTrue routes) ----

fn json_response(status: u16, value: &Value) -> HttpResponse {
    HttpResponse { status, headers: Vec::new(), body: value.to_string().into_bytes() }
}

fn error_response(err: DataError) -> HttpResponse {
    json_response(err.status(), &json!({"message": err.to_string()}))
}

fn auth_failure(err: AuthError) -> Result<HttpResponse, TransportError> {
    match err {
        AuthError::Network(t) => Err(t),
        AuthError::Expired | AuthError::InvalidCode => Ok(json_response(400, &json!({"error_code": err.code()}))),
        _ => Ok(json_response(500, &json!({"error_code": err.code()}))),
    }
}

fn token_json(tokens: &AuthTokens) -> Value {
    json!({
        "access_token": tokens.access_token, "token_type": "bearer", "expires_in": tokens.expires_in_sec,
        "refresh_token": tokens.refresh_token, "user": {"id": tokens.user_id},
    })
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

impl HttpTransport for FakeBackend {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        if self.lock().outage {
            return Err(TransportError::Connect);
        }
        if header(&request, "apikey") != Some(FAKE_ANON_KEY) {
            return Ok(json_response(401, &json!({"message": "No API key found in request"})));
        }
        let url = url::Url::parse(&request.url).map_err(|_| TransportError::InvalidUrl)?;
        let body: Value = request
            .body
            .as_ref()
            .and_then(|b| serde_json::from_slice(b.as_bytes()).ok())
            .unwrap_or(Value::Null);
        let text = |key: &str| body.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let path = url.path();
        let bearer = header(&request, "authorization").and_then(|v| v.strip_prefix("Bearer "));

        match (request.method, path) {
            (Method::Get, "/auth/v1/health") => Ok(json_response(200, &json!({"name": "GoTrue"}))),
            (Method::Post, "/auth/v1/otp") => match self.request_otp(&text("email")) {
                Ok(()) => Ok(json_response(200, &json!({}))),
                Err(e) => auth_failure(e),
            },
            (Method::Post, "/auth/v1/verify") => match self.verify_otp(&text("email"), &text("token")) {
                Ok(tokens) => Ok(json_response(200, &token_json(&tokens))),
                Err(e) => auth_failure(e),
            },
            (Method::Post, "/auth/v1/token") => match self.refresh_session(&text("refresh_token")) {
                Ok(tokens) => Ok(json_response(200, &token_json(&tokens))),
                Err(e) => auth_failure(e),
            },
            (Method::Post, "/auth/v1/logout") => match self.end_session(bearer.unwrap_or_default()) {
                Ok(()) => Ok(HttpResponse { status: 204, headers: Vec::new(), body: Vec::new() }),
                Err(e) => auth_failure(e),
            },
            (method, rest) if rest.starts_with("/rest/v1/") => {
                let caller = match self.caller_for(bearer) {
                    Ok(c) => c,
                    Err(e) => return Ok(error_response(e)),
                };
                let target = &rest["/rest/v1/".len()..];
                let filters: Vec<(String, String)> = url
                    .query_pairs()
                    .filter_map(|(k, v)| v.strip_prefix("eq.").map(|val| (k.to_string(), val.to_string())))
                    .collect();
                if let Some(name) = target.strip_prefix("rpc/") {
                    if method != Method::Post {
                        return Ok(error_response(DataError::BadRequest));
                    }
                    return Ok(match self.rpc(&caller, name, &body) {
                        Ok(value) => json_response(200, &value),
                        Err(e) => error_response(e),
                    });
                }
                let ignore = header(&request, "prefer").is_some_and(|p| p.contains("ignore-duplicates"));
                Ok(match method {
                    Method::Get => match self.select(&caller, target, &filters) {
                        Ok(rows) => json_response(200, &Value::Array(rows)),
                        Err(e) => error_response(e),
                    },
                    Method::Post => {
                        let merge = header(&request, "prefer").is_some_and(|p| p.contains("merge-duplicates"));
                        let done = if merge { self.upsert(&caller, target, &body) } else { self.insert(&caller, target, &body, ignore) };
                        match done {
                            Ok(()) => HttpResponse { status: 201, headers: Vec::new(), body: Vec::new() },
                            Err(e) => error_response(e),
                        }
                    }
                    Method::Delete => match self.delete(&caller, target, &filters) {
                        Ok(_) => HttpResponse { status: 204, headers: Vec::new(), body: Vec::new() },
                        Err(e) => error_response(e),
                    },
                })
            }
            _ => Ok(error_response(DataError::NotFound)),
        }
    }
}

#[cfg(test)]
mod tests;
