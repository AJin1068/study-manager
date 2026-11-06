//! The real adapters: every call is one Supabase RPC through `SupabaseRest`.
//!
//! Answers are `{"code": ...}` objects (see `fixtures/phone/rpc-contract.json`). A code other than
//! `ok` becomes `PhoneError::Rejected`; `stale_seq` also carries the server's current sequence
//! number. Request bodies are the sealed structs of `phone::payload`.

use std::sync::Arc;

use link_protocol::{LockLease, PhoneBeat, PickupReport};
use serde_json::Value;

use super::model::{ClaimedPair, LeaseFetch, NewPairCode, PhoneCode, PhoneError, PickupRow, PublishAck, WireStatus};
use super::payload::{BeatArgs, ClaimPairArgs, PickupArgs, PublishLockArgs};
use super::port::{PairingApi, PhoneLink};
use crate::remote::payload::{EgressPayload, NoArgs};
use crate::remote::supabase::{RemoteError, SupabaseRest};

fn call<P: EgressPayload>(rest: &SupabaseRest, kind: &'static str, name: &str, args: &P) -> Result<Value, PhoneError> {
    let answer = rest.rpc(kind, name, args)?;
    match answer.get("code").and_then(Value::as_str).and_then(PhoneCode::parse) {
        Some(PhoneCode::Ok) => Ok(answer),
        Some(PhoneCode::StaleSeq) => match answer.get("current_seq").and_then(Value::as_i64) {
            Some(current_seq) => Err(PhoneError::Stale { current_seq }),
            None => Err(PhoneError::Rejected(PhoneCode::StaleSeq)),
        },
        Some(code) => Err(PhoneError::Rejected(code)),
        None => Err(PhoneError::Remote(RemoteError::Decode)),
    }
}

fn text(answer: &Value, field: &str) -> Result<String, PhoneError> {
    answer.get(field).and_then(Value::as_str).map(str::to_string).ok_or(PhoneError::Remote(RemoteError::Decode))
}

fn number(answer: &Value, field: &str) -> Result<i64, PhoneError> {
    answer.get(field).and_then(Value::as_i64).ok_or(PhoneError::Remote(RemoteError::Decode))
}

pub struct RestPairing {
    rest: Arc<SupabaseRest>,
}

impl RestPairing {
    pub fn new(rest: Arc<SupabaseRest>) -> Self {
        Self { rest }
    }
}

impl PairingApi for RestPairing {
    fn create_pair_code(&self) -> Result<NewPairCode, PhoneError> {
        let answer = call(&self.rest, "rpc_create_pair_code", "create_pair_code", &NoArgs {})?;
        Ok(NewPairCode { pair_code: text(&answer, "pair_code")?, expires_at_ms: number(&answer, "expires_at_ms")? })
    }

    fn revoke_pair(&self) -> Result<(), PhoneError> {
        call(&self.rest, "rpc_revoke_pair", "revoke_pair", &NoArgs {}).map(|_| ())
    }

    fn status(&self) -> Result<WireStatus, PhoneError> {
        let answer = call(&self.rest, "rpc_phone_status", "phone_status", &NoArgs {})?;
        serde_json::from_value(answer).map_err(|_| PhoneError::Remote(RemoteError::Decode))
    }

    fn publish_lock(&self, lease: &LockLease) -> Result<PublishAck, PhoneError> {
        lease.validate().map_err(|_| PhoneError::Rejected(PhoneCode::InvalidInput))?;
        let answer = call(&self.rest, "rpc_publish_phone_lock", "publish_phone_lock", &PublishLockArgs::from_lease(lease))?;
        Ok(PublishAck {
            seq: number(&answer, "seq")?,
            replayed: answer.get("replayed").and_then(Value::as_bool).unwrap_or(false),
            server_now_ms: number(&answer, "server_now_ms")?,
        })
    }

    fn fetch_pickups(&self) -> Result<Vec<PickupRow>, PhoneError> {
        let rows = self.rest.select("select_phone_pickups", "phone_pickups", &[])?;
        let rows = rows.as_array().ok_or(PhoneError::Remote(RemoteError::Decode))?;
        // A row that does not read as (segment id, count) is skipped, never guessed.
        Ok(rows
            .iter()
            .filter_map(|row| {
                Some(PickupRow { segment_id: row.get("segment_id")?.as_str()?.to_string(), count: row.get("count")?.as_i64()? })
            })
            .collect())
    }
}

pub struct RestPhoneLink {
    rest: Arc<SupabaseRest>,
}

impl RestPhoneLink {
    pub fn new(rest: Arc<SupabaseRest>) -> Self {
        Self { rest }
    }
}

impl PhoneLink for RestPhoneLink {
    fn claim(&self, code: &str) -> Result<ClaimedPair, PhoneError> {
        let answer = call(&self.rest, "rpc_claim_pair", "claim_pair", &ClaimPairArgs { code: code.to_string() })?;
        Ok(ClaimedPair {
            pair_id: text(&answer, "pair_id")?,
            owner_user_id: text(&answer, "owner_user_id")?,
            server_now_ms: number(&answer, "server_now_ms")?,
        })
    }

    fn fetch_lease(&self) -> Result<LeaseFetch, PhoneError> {
        let answer = call(&self.rest, "rpc_phone_lease", "phone_lease", &NoArgs {})?;
        let server_now_ms = number(&answer, "server_now_ms")?;
        let lease = match answer.get("lease") {
            None | Some(Value::Null) => None,
            Some(value) => Some(serde_json::from_value::<LockLease>(value.clone()).map_err(|_| PhoneError::Remote(RemoteError::Decode))?),
        };
        Ok(LeaseFetch { lease, server_now_ms })
    }

    fn beat(&self, beat: &PhoneBeat) -> Result<(), PhoneError> {
        call(&self.rest, "rpc_phone_beat", "phone_beat", &BeatArgs::from(beat)).map(|_| ())
    }

    fn report_pickups(&self, report: &PickupReport) -> Result<(), PhoneError> {
        report.validate().map_err(|_| PhoneError::Rejected(PhoneCode::InvalidInput))?;
        call(&self.rest, "rpc_report_phone_pickups", "report_phone_pickups", &PickupArgs::from(report)).map(|_| ())
    }

    fn leave(&self) -> Result<(), PhoneError> {
        call(&self.rest, "rpc_revoke_pair", "revoke_pair", &NoArgs {}).map(|_| ())
    }
}
