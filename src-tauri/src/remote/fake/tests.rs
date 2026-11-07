use std::sync::Arc;

use serde_json::{Value, json};

use super::*;
use crate::remote::ManualClock;

const SQL: &str = include_str!("../../../../supabase/migrations/20260601000000_s2_core.sql");
const S4_SQL: &str = include_str!("../../../../supabase/migrations/20260601000200_s4_rooms.sql");

fn backend() -> (Arc<FakeBackend>, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(5_000_000));
    (Arc::new(FakeBackend::new(clock.clone())), clock)
}

/// Signs `email` in and returns (caller, access token).
fn user(backend: &Arc<FakeBackend>, email: &str) -> (Caller, String) {
    let auth = backend.auth();
    auth.request_otp(email).expect("otp");
    let tokens = auth.verify_otp(email, FAKE_CODE).expect("verify");
    (
        backend
            .caller_for(Some(&tokens.access_token))
            .expect("caller"),
        tokens.access_token,
    )
}

fn id_filter(id: &str) -> Vec<(String, String)> {
    vec![("id".to_string(), id.to_string())]
}

const DEVICE_A: &str = "10000000-0000-4000-8000-00000000000a";
const DEVICE_B: &str = "10000000-0000-4000-8000-00000000000b";

fn two_users() -> (Arc<FakeBackend>, Caller, Caller) {
    let (backend, _) = backend();
    let (a, token_a) = user(&backend, "a@example.test");
    let (b, token_b) = user(&backend, "b@example.test");
    backend.register_device_for(&token_a, DEVICE_A);
    backend.register_device_for(&token_b, DEVICE_B);
    (backend, a, b)
}

fn occurrence(key: &str) -> Value {
    json!({"occurrence_key": key, "start_at_utc_ms": 1_000, "plan_name": "Math", "subject": "math", "sets": 2, "status": "planned"})
}

fn snapshot_args(device: &str, hash: &str, occurrences: Vec<Value>) -> Value {
    json!({"p_device_id": device, "p_snapshot_hash": hash, "p_generated_at_ms": 1, "p_horizon_days": 14, "p_occurrences": occurrences})
}

#[test]
fn matrix_is_enforced_for_owner_other_and_anonymous_on_every_table() {
    let (backend, a, b) = two_users();
    let matrix: Value = serde_json::from_str(RLS_MATRIX).expect("matrix");
    for (table, ops) in matrix["tables"].as_object().expect("tables") {
        for (op_name, rule) in ops.as_object().expect("ops") {
            let owner_allowed = rule == "owner";
            let attempt = |caller: &Caller| -> Result<(), DataError> {
                match op_name.as_str() {
                    "select" => backend.select(caller, table, &[]).map(|_| ()),
                    "insert" => {
                        let device = if caller == &a { DEVICE_A } else { DEVICE_B };
                        backend.insert(caller, table, &json!({"id": "20000000-0000-4000-8000-0000000000aa", "device_id": device, "local_id": 1}), true)
                    }
                    "update" => backend
                        .update(caller, table, &[], &json!({"studying": true}))
                        .map(|_| ()),
                    "delete" => backend
                        .delete(caller, table, &id_filter("none"))
                        .map(|_| ()),
                    other => panic!("unknown op {other}"),
                }
            };
            assert_eq!(
                attempt(&Caller::Anon),
                Err(DataError::Forbidden),
                "{table}.{op_name}: anon must be denied"
            );
            let owner = attempt(&a);
            let other = attempt(&b);
            if owner_allowed {
                assert!(
                    !matches!(owner, Err(DataError::Forbidden)),
                    "{table}.{op_name}: owner denied: {owner:?}"
                );
                assert!(
                    !matches!(other, Err(DataError::Forbidden)),
                    "{table}.{op_name}: other user acts on their own rows: {other:?}"
                );
            } else {
                assert_eq!(
                    owner,
                    Err(DataError::Forbidden),
                    "{table}.{op_name}: no client policy"
                );
                assert_eq!(
                    other,
                    Err(DataError::Forbidden),
                    "{table}.{op_name}: no client policy"
                );
            }
        }
    }
}

#[test]
fn users_never_see_each_others_rows() {
    let (backend, a, b) = two_users();
    assert_eq!(backend.select(&a, "devices", &[]).expect("a").len(), 1);
    assert_eq!(backend.select(&b, "devices", &[]).expect("b").len(), 1);
    // Direct id lookup of the other user's device finds nothing.
    assert!(
        backend
            .select(&a, "devices", &id_filter(DEVICE_B))
            .expect("lookup")
            .is_empty()
    );
    // Deleting someone else's device removes nothing.
    assert_eq!(backend.delete(&a, "devices", &id_filter(DEVICE_B)), Ok(0));
    assert_eq!(backend.device_count(), 2);
    // Inserting a row claiming another user is refused.
    let foreign = json!({"id": "30000000-0000-4000-8000-000000000001", "user_id": match &b { Caller::User(id) => id.clone(), _ => String::new() }, "app_version": "1", "platform": "macos"});
    assert_eq!(
        backend.insert(&a, "devices", &foreign, false),
        Err(DataError::Forbidden)
    );
    // Re-pointing a row at another user is refused.
    assert_eq!(
        backend.update(&a, "devices", &[], &json!({"user_id": "x"})),
        Err(DataError::Forbidden)
    );
}

#[test]
fn history_mirror_requires_an_owned_device_and_ignores_duplicates() {
    let (backend, a, _) = two_users();
    let row = |device: &str, local: i64| json!({"device_id": device, "local_id": local, "at_ms": 1, "action": "create", "reason_code": "created", "outcome": "applied"});
    assert_eq!(
        backend.insert(&a, "plan_history_mirror", &row(DEVICE_B, 1), true),
        Err(DataError::Forbidden)
    );
    assert_eq!(
        backend.insert(&a, "plan_history_mirror", &row(DEVICE_A, 1), true),
        Ok(())
    );
    assert_eq!(
        backend.insert(&a, "plan_history_mirror", &row(DEVICE_A, 1), true),
        Ok(()),
        "retry of the same batch"
    );
    assert_eq!(
        backend.insert(&a, "plan_history_mirror", &row(DEVICE_A, 1), false),
        Err(DataError::Conflict)
    );
    assert_eq!(
        backend
            .select(&a, "plan_history_mirror", &[])
            .expect("rows")
            .len(),
        1
    );
}

#[test]
fn replace_plan_snapshot_is_idempotent_and_owner_checked() {
    let (backend, a, b) = two_users();
    let args = snapshot_args(DEVICE_A, "h1", vec![occurrence("p1:1"), occurrence("p1:2")]);
    assert_eq!(
        backend.rpc(&a, "replace_plan_snapshot", &args),
        Ok(json!("ok"))
    );
    assert_eq!(
        backend.rpc(&a, "replace_plan_snapshot", &args),
        Ok(json!("unchanged"))
    );
    assert_eq!(
        backend
            .select(&a, "plan_occurrences", &[])
            .expect("rows")
            .len(),
        2
    );

    let smaller = snapshot_args(DEVICE_A, "h2", vec![occurrence("p1:1")]);
    assert_eq!(
        backend.rpc(&a, "replace_plan_snapshot", &smaller),
        Ok(json!("ok"))
    );
    assert_eq!(
        backend
            .select(&a, "plan_occurrences", &[])
            .expect("rows")
            .len(),
        1,
        "replace, not append"
    );

    assert_eq!(
        backend.rpc(
            &b,
            "replace_plan_snapshot",
            &snapshot_args(DEVICE_A, "h9", vec![])
        ),
        Ok(json!("device_not_owned"))
    );
    assert_eq!(
        backend
            .select(&a, "plan_occurrences", &[])
            .expect("rows")
            .len(),
        1,
        "B's attempt changed nothing"
    );
}

#[test]
fn replace_plan_snapshot_reports_invalid_input_as_a_code() {
    let (backend, a, _) = two_users();
    let mut bad_status = occurrence("k");
    bad_status["status"] = json!("done");
    let mut zero_sets = occurrence("k");
    zero_sets["sets"] = json!(0);
    for bad in [
        vec![bad_status],
        vec![zero_sets],
        vec![occurrence("dup"), occurrence("dup")],
        vec![json!({"occurrence_key": "k"})],
    ] {
        assert_eq!(
            backend.rpc(
                &a,
                "replace_plan_snapshot",
                &snapshot_args(DEVICE_A, "h", bad)
            ),
            Ok(json!("invalid_input"))
        );
    }
    assert!(
        backend
            .select(&a, "plan_snapshots", &[])
            .expect("rows")
            .is_empty(),
        "nothing stored on invalid input"
    );
}

#[test]
fn heartbeat_stamps_server_time_and_only_for_owned_devices() {
    let (backend, clock) = backend();
    let (a, token) = user(&backend, "a@example.test");
    let (b, _) = user(&backend, "b@example.test");
    backend.register_device_for(&token, DEVICE_A);
    let beat =
        |device: &str| json!({"p_device_id": device, "p_app_version": "0.1.0", "p_studying": true});
    clock.set(9_000_000);
    assert_eq!(
        backend.rpc(&a, "heartbeat", &beat(DEVICE_A)),
        Ok(json!("ok"))
    );
    let rows = backend.select(&a, "presence", &[]).expect("rows");
    assert_eq!(
        (rows.len(), rows[0]["last_seen_at_ms"].clone()),
        (1, json!(9_000_000))
    );
    assert_eq!(
        backend.rpc(&b, "heartbeat", &beat(DEVICE_A)),
        Ok(json!("device_not_owned"))
    );
    clock.set(9_060_000);
    backend
        .rpc(&a, "heartbeat", &beat(DEVICE_A))
        .expect("second beat");
    assert_eq!(
        backend.select(&a, "presence", &[]).expect("rows").len(),
        1,
        "one presence row per device"
    );
}

#[test]
fn claim_alert_is_service_only_and_claims_once() {
    let (backend, a, _) = two_users();
    let args = json!({"p_user_id": "u1", "p_occurrence_key": "p1:1", "p_kind": "missed_plan"});
    assert_eq!(
        backend.rpc(&a, "claim_alert", &args),
        Err(DataError::Forbidden)
    );
    assert_eq!(
        backend.rpc(&Caller::Anon, "claim_alert", &args),
        Err(DataError::Forbidden)
    );
    assert_eq!(
        backend.rpc(&Caller::Service, "claim_alert", &args),
        Ok(json!("claimed"))
    );
    assert_eq!(
        backend.rpc(&Caller::Service, "claim_alert", &args),
        Ok(json!("duplicate"))
    );
    assert_eq!(
        backend.rpc(&Caller::Anon, "heartbeat", &json!({})),
        Err(DataError::Forbidden)
    );
}

#[test]
fn rpc_contract_names_and_roles_are_implemented_and_present_in_the_sql() {
    let (backend, a, _) = two_users();
    let contract: Value = serde_json::from_str(RPC_CONTRACT).expect("contract");
    let rpcs = contract["rpcs"].as_array().expect("rpcs");
    assert_eq!(rpcs.len(), 3);
    for rpc in rpcs {
        let name = rpc["name"].as_str().expect("name");
        // Implemented: a call with an empty argument object is a bad request or a code, never "unknown".
        let outcome = backend.rpc(&Caller::Service, name, &json!({}));
        assert_ne!(
            outcome,
            Err(DataError::NotFound),
            "{name} is not implemented by the fake"
        );
        let signature = format!("function public.{name}(");
        assert!(
            SQL.contains(&signature),
            "{name} missing from the migration"
        );
        for param in rpc["params"].as_array().expect("params") {
            assert!(
                SQL.contains(param.as_str().expect("param")),
                "{name}: parameter {param} missing in SQL"
            );
        }
        let _ = &a;
    }
}

#[test]
fn every_matrix_table_exists_in_the_sql_with_rls_enabled() {
    let (backend, _, _) = two_users();
    let sql = format!("{SQL}\n{S4_SQL}");
    for table in backend.table_names() {
        assert!(
            sql.contains(&format!("create table public.{table} (")),
            "{table} not created"
        );
        assert!(
            sql.contains(&format!("alter table public.{table}"))
                && sql.contains("enable row level security"),
            "{table} RLS"
        );
    }
}

#[test]
fn sessions_expire_rotate_and_revoke() {
    let (backend, clock) = backend();
    let auth = backend.auth();
    auth.request_otp("a@example.test").expect("otp");
    assert_eq!(
        auth.verify_otp("a@example.test", "123456"),
        Err(AuthError::InvalidCode)
    );
    assert_eq!(
        auth.verify_otp("nobody@example.test", FAKE_CODE),
        Err(AuthError::InvalidCode)
    );
    let first = auth
        .verify_otp("a@example.test", FAKE_CODE)
        .expect("verify");
    assert!(backend.caller_for(Some(&first.access_token)).is_ok());

    clock.advance(3_600_000);
    assert_eq!(
        backend.caller_for(Some(&first.access_token)),
        Err(DataError::Unauthorized),
        "expired access token"
    );

    let second = auth.refresh(&first.refresh_token).expect("refresh");
    assert_eq!(
        auth.refresh(&first.refresh_token),
        Err(AuthError::Expired),
        "a spent refresh token cannot be replayed"
    );
    auth.sign_out(&second.access_token).expect("sign out");
    assert_eq!(
        backend.caller_for(Some(&second.access_token)),
        Err(DataError::Unauthorized)
    );
    assert_eq!(auth.refresh(&second.refresh_token), Err(AuthError::Expired));
}

#[test]
fn http_surface_speaks_the_routes_the_real_adapters_use() {
    use crate::remote::http::{Method, RequestTag};
    use crate::remote::payload::{Body, PresenceBeat};

    let (backend, _) = backend();
    let (_, token) = user(&backend, "a@example.test");
    backend.register_device_for(&token, DEVICE_A);
    let tag = RequestTag {
        port: "supabase",
        kind: "rpc_heartbeat",
    };
    let beat = Body::from_payload(&PresenceBeat {
        device_id: DEVICE_A.into(),
        app_version: "0.1.0".into(),
        studying: false,
    })
    .expect("body");
    let request = HttpRequest::new(
        tag,
        Method::Post,
        "http://127.0.0.1:1/rest/v1/rpc/heartbeat".into(),
    )
    .header("apikey", FAKE_ANON_KEY)
    .header("authorization", &format!("Bearer {token}"))
    .with_body(beat);
    let response = backend.send(request).expect("response");
    assert_eq!((response.status, response.body), (200, b"\"ok\"".to_vec()));

    let no_key = HttpRequest::new(
        tag,
        Method::Get,
        "http://127.0.0.1:1/rest/v1/devices".into(),
    );
    assert_eq!(backend.send(no_key).map(|r| r.status), Ok(401));
    let anon = HttpRequest::new(
        tag,
        Method::Get,
        "http://127.0.0.1:1/rest/v1/devices".into(),
    )
    .header("apikey", FAKE_ANON_KEY);
    assert_eq!(backend.send(anon).map(|r| r.status), Ok(403));

    backend.set_outage(true);
    let down = HttpRequest::new(
        tag,
        Method::Get,
        "http://127.0.0.1:1/rest/v1/devices".into(),
    )
    .header("apikey", FAKE_ANON_KEY);
    assert_eq!(backend.send(down), Err(TransportError::Connect));
}
