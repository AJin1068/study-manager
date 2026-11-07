//! Argument structs of the phone RPCs, reviewed like `remote::payload`. Each field name is the RPC
//! parameter name. `Debug` hides every value.
//!
//! What can leave: the one-time pairing code, a lease (run id, sequence number, locked or free,
//! release cause, end time), a heartbeat (sequence number and two booleans) and a pick-up count
//! with a segment id. There is no field for app names, package names, usage history, window
//! titles, URLs or typed text.

use link_protocol::{LockLease, PhoneBeat, PickupReport};
use serde::Serialize;

use super::model::{cause_text, state_text};
use crate::remote::payload::egress_payload;

/// `claim_pair`: the code the phone scanned.
#[derive(Serialize)]
pub struct ClaimPairArgs {
    #[serde(rename = "p_code")]
    pub code: String,
}
egress_payload!(ClaimPairArgs);

/// `publish_phone_lock`, built from a validated lease.
#[derive(Serialize)]
pub struct PublishLockArgs {
    #[serde(rename = "p_seq")]
    pub seq: i64,
    #[serde(rename = "p_run_id")]
    pub run_id: String,
    #[serde(rename = "p_state")]
    pub state: &'static str,
    #[serde(rename = "p_release_cause")]
    pub release_cause: Option<&'static str>,
    #[serde(rename = "p_valid_until_ms")]
    pub valid_until_ms: i64,
}
egress_payload!(PublishLockArgs);

impl PublishLockArgs {
    pub fn from_lease(lease: &LockLease) -> Self {
        Self {
            seq: lease.seq,
            run_id: lease.run_id.clone(),
            state: state_text(lease.state),
            release_cause: lease.release_cause.map(cause_text),
            valid_until_ms: lease.valid_until_ms,
        }
    }
}

/// `phone_beat`.
#[derive(Serialize)]
pub struct BeatArgs {
    #[serde(rename = "p_lease_seq")]
    pub lease_seq: i64,
    #[serde(rename = "p_locked")]
    pub locked: bool,
    #[serde(rename = "p_service_enabled")]
    pub service_enabled: bool,
}
egress_payload!(BeatArgs);

impl From<&PhoneBeat> for BeatArgs {
    fn from(beat: &PhoneBeat) -> Self {
        Self {
            lease_seq: beat.lease_seq,
            locked: beat.locked,
            service_enabled: beat.service_enabled,
        }
    }
}

/// `report_phone_pickups`.
#[derive(Serialize)]
pub struct PickupArgs {
    #[serde(rename = "p_segment_id")]
    pub segment_id: String,
    #[serde(rename = "p_count")]
    pub count: i64,
}
egress_payload!(PickupArgs);

impl From<&PickupReport> for PickupArgs {
    fn from(report: &PickupReport) -> Self {
        Self {
            segment_id: report.segment_id.clone(),
            count: report.count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::payload::{EgressPayload, NoArgs, forbidden_key};

    fn json<P: EgressPayload>(p: &P) -> serde_json::Value {
        serde_json::to_value(p).expect("serialize")
    }

    fn keys(value: &serde_json::Value, out: &mut Vec<String>) {
        if let serde_json::Value::Object(map) = value {
            for (k, v) in map {
                out.push(k.clone());
                keys(v, out);
            }
        }
    }

    fn contract_params(rpc: &str) -> Vec<String> {
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/phone/rpc-contract.json"))
                .expect("contract");
        contract["rpcs"]
            .as_array()
            .and_then(|r| r.iter().find(|x| x["name"] == rpc))
            .and_then(|x| x["params"].as_array())
            .map(|p| {
                p.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn sorted_keys(value: &serde_json::Value) -> Vec<String> {
        let mut found: Vec<String> = value
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        found.sort();
        found
    }

    #[test]
    fn every_argument_is_an_rpc_parameter_and_every_parameter_is_sent() {
        let lease = LockLease::locked("run-1", 1, 0, 1_000).expect("lease");
        let cases = [
            (
                "claim_pair",
                json(&ClaimPairArgs {
                    code: "ABCDEFGHJK".into(),
                }),
            ),
            (
                "publish_phone_lock",
                json(&PublishLockArgs::from_lease(&lease)),
            ),
            (
                "phone_beat",
                json(&BeatArgs {
                    lease_seq: 1,
                    locked: true,
                    service_enabled: true,
                }),
            ),
            (
                "report_phone_pickups",
                json(&PickupArgs {
                    segment_id: "s".into(),
                    count: 1,
                }),
            ),
            ("create_pair_code", json(&NoArgs {})),
        ];
        for (rpc, value) in cases {
            let mut expected = contract_params(rpc);
            expected.sort();
            assert_eq!(sorted_keys(&value), expected, "{rpc}");
        }
    }

    #[test]
    fn no_payload_has_a_forbidden_or_usage_key() {
        let lease =
            LockLease::free("run-1", 2, 0, link_protocol::ReleaseCause::SkipToday).expect("lease");
        let samples = [
            json(&ClaimPairArgs {
                code: "ABCDEFGHJK".into(),
            }),
            json(&PublishLockArgs::from_lease(&lease)),
            json(&BeatArgs {
                lease_seq: 1,
                locked: false,
                service_enabled: false,
            }),
            json(&PickupArgs {
                segment_id: "s".into(),
                count: 1,
            }),
        ];
        for sample in samples {
            let mut found = Vec::new();
            keys(&sample, &mut found);
            assert!(!found.is_empty());
            for key in found {
                let lower = key.to_ascii_lowercase();
                assert!(!forbidden_key(&key), "{key}");
                for banned in ["app", "package", "usage", "history", "name"] {
                    assert!(!lower.contains(banned), "{key}");
                }
            }
        }
    }

    #[test]
    fn a_free_lease_carries_its_cause_and_a_locked_one_does_not() {
        let free = PublishLockArgs::from_lease(
            &LockLease::free("r", 3, 5, link_protocol::ReleaseCause::EmergencyPass).expect("lease"),
        );
        assert_eq!(
            (free.state, free.release_cause, free.valid_until_ms),
            ("free", Some("emergency_pass"), 5)
        );
        let locked = PublishLockArgs::from_lease(&LockLease::locked("r", 3, 0, 10).expect("lease"));
        assert_eq!((locked.state, locked.release_cause), ("locked", None));
    }

    #[test]
    fn debug_hides_values() {
        let args = ClaimPairArgs {
            code: "SECRETCODE".into(),
        };
        assert!(!format!("{args:?}").contains("SECRETCODE"));
    }
}
