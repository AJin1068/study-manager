//! The Android blocker as the Rust side sees it: three plugin commands and three plugin events,
//! all JSON, all described by `fixtures/phone/blocker-bridge.json` (the TS mirror and the Kotlin
//! plugin in `phone/plugins/blocker/android/` read the same file).
//!
//! Commands (Rust -> Kotlin), each answered by one reply object:
//! * `engage`  `{ run_id, seq, remaining_ms, allow }` -> `{ ok: true }` | `{ ok: false, code }`
//! * `release` `{}` -> `{ ok: true }`
//! * `status`  `{}` -> `{ kind: "active" | "inactive" | "service_disabled" }`
//!
//! Events (Kotlin -> Rust): `service_disabled`, `service_enabled`, `lock_ended`. Each turns the
//! phone's last heartbeat into the next one (`BridgeEvent::apply`), so a service switched off
//! during a lock reaches the desktop as `service_enabled: false`. No event or reply has a field
//! for an app name: the plugin reports nothing about what the user opened.
//!
//! `PluginBridge` is the seam to the Tauri mobile plugin. The phone project implements it over the
//! plugin handle; this crate cannot compile that part, and the tests run `ScriptedBridge`.

use std::sync::{Arc, Mutex};

use link_protocol::PhoneBeat;
use serde::Deserialize;
use serde_json::{Value, json};

use super::allow_list::AllowList;
use super::blocker::BlockerStatus;
use super::blocker_port::{BlockerError, Engagement, PhoneBlocker};

pub const COMMAND_ENGAGE: &str = "engage";
pub const COMMAND_RELEASE: &str = "release";
pub const COMMAND_STATUS: &str = "status";

/// Why a command did not get a usable reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeFailure {
    Unavailable,
    Timeout,
}

pub trait PluginBridge: Send + Sync {
    fn invoke(&self, command: &str, payload: Value) -> Result<Value, BridgeFailure>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    ok: bool,
    code: Option<ReplyCode>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplyCode {
    ServiceDisabled,
    InvalidRequest,
    Internal,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StatusReply {
    Active,
    Inactive,
    ServiceDisabled,
}

pub struct AndroidAccessibilityBlocker {
    bridge: Arc<dyn PluginBridge>,
}

impl AndroidAccessibilityBlocker {
    pub fn new(bridge: Arc<dyn PluginBridge>) -> Self {
        Self { bridge }
    }

    fn call(&self, command: &str, payload: Value) -> Result<Value, BlockerError> {
        self.bridge
            .invoke(command, payload)
            .map_err(|_| BlockerError::BridgeUnreachable)
    }

    fn expect_ok(&self, command: &str, payload: Value) -> Result<(), BlockerError> {
        let reply: Reply = serde_json::from_value(self.call(command, payload)?)
            .map_err(|_| BlockerError::BridgeMalformed)?;
        match (reply.ok, reply.code) {
            (true, None) => Ok(()),
            (false, Some(ReplyCode::ServiceDisabled)) => Err(BlockerError::ServiceDisabled),
            (false, Some(ReplyCode::InvalidRequest | ReplyCode::Internal)) => {
                Err(BlockerError::Refused)
            }
            // `ok: true` with a code, or `ok: false` without one, says nothing the caller can act on.
            _ => Err(BlockerError::BridgeMalformed),
        }
    }
}

impl PhoneBlocker for AndroidAccessibilityBlocker {
    fn status(&self) -> BlockerStatus {
        let reply = match self.call(COMMAND_STATUS, json!({})) {
            Ok(reply) => reply,
            Err(_) => return BlockerStatus::Unreachable,
        };
        match serde_json::from_value::<StatusReply>(reply) {
            Ok(StatusReply::Active) => BlockerStatus::Active,
            Ok(StatusReply::Inactive) => BlockerStatus::Inactive,
            Ok(StatusReply::ServiceDisabled) => BlockerStatus::ServiceDisabled,
            Err(_) => BlockerStatus::Unreachable,
        }
    }

    fn engage(&self, engagement: &Engagement, allow: &AllowList) -> Result<(), BlockerError> {
        self.expect_ok(
            COMMAND_ENGAGE,
            json!({
                "run_id": engagement.run_id,
                "seq": engagement.seq,
                "remaining_ms": engagement.remaining_ms,
                "allow": allow.to_wire(),
            }),
        )
    }

    fn release(&self) -> Result<(), BlockerError> {
        self.expect_ok(COMMAND_RELEASE, json!({}))
    }
}

/// Something the plugin tells the app on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// The accessibility service was switched off (or revoked) - during a lock this is a gap.
    ServiceDisabled,
    ServiceEnabled,
    /// The device ended the lock by its own deadline, or on `release`.
    LockEnded,
}

impl BridgeEvent {
    pub fn parse(raw: &Value) -> Result<Self, BlockerError> {
        // serde ignores extra fields on a unit variant of a tagged enum; the contract does not.
        if raw.as_object().is_none_or(|object| object.len() != 1) {
            return Err(BlockerError::BridgeMalformed);
        }
        serde_json::from_value(raw.clone()).map_err(|_| BlockerError::BridgeMalformed)
    }

    /// The heartbeat that follows `current` after this event.
    pub fn apply(self, current: &PhoneBeat) -> PhoneBeat {
        let mut next = current.clone();
        match self {
            BridgeEvent::ServiceDisabled => next.service_enabled = false,
            BridgeEvent::ServiceEnabled => next.service_enabled = true,
            BridgeEvent::LockEnded => next.locked = false,
        }
        next
    }
}

/// Test bridge: answers from a script and records `(command, payload)` pairs.
#[derive(Default)]
pub struct ScriptedBridge {
    replies: Mutex<std::collections::VecDeque<Result<Value, BridgeFailure>>>,
    seen: Mutex<Vec<(String, Value)>>,
}

impl ScriptedBridge {
    pub fn new(replies: Vec<Result<Value, BridgeFailure>>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(Vec::new()),
        }
    }

    pub fn seen(&self) -> Vec<(String, Value)> {
        match self.seen.lock() {
            Ok(seen) => seen.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl PluginBridge for ScriptedBridge {
    fn invoke(&self, command: &str, payload: Value) -> Result<Value, BridgeFailure> {
        match self.seen.lock() {
            Ok(mut seen) => seen.push((command.to_string(), payload)),
            Err(poisoned) => poisoned.into_inner().push((command.to_string(), payload)),
        }
        let mut replies = match self.replies.lock() {
            Ok(replies) => replies,
            Err(poisoned) => poisoned.into_inner(),
        };
        replies
            .pop_front()
            .unwrap_or(Err(BridgeFailure::Unavailable))
    }
}

#[cfg(test)]
mod tests {
    use link_protocol::LockLease;

    use super::*;

    fn contract() -> Value {
        serde_json::from_str(include_str!("../../../fixtures/phone/blocker-bridge.json"))
            .expect("fixture")
    }

    fn blocker(
        replies: Vec<Result<Value, BridgeFailure>>,
    ) -> (AndroidAccessibilityBlocker, Arc<ScriptedBridge>) {
        let bridge = Arc::new(ScriptedBridge::new(replies));
        (AndroidAccessibilityBlocker::new(bridge.clone()), bridge)
    }

    fn engagement_of(request: &Value) -> (Engagement, AllowList) {
        let lease = LockLease {
            run_id: request["run_id"].as_str().expect("run_id").to_string(),
            seq: request["seq"].as_i64().expect("seq"),
            state: link_protocol::LeaseState::Locked,
            release_cause: None,
            issued_at_ms: 0,
            valid_until_ms: request["remaining_ms"].as_i64().expect("remaining"),
        };
        let engagement =
            Engagement::new(&lease, request["remaining_ms"].as_i64().expect("remaining"))
                .expect("engagement");
        (
            engagement,
            AllowList::from_wire(&request["allow"]).expect("allow"),
        )
    }

    #[test]
    fn engage_sends_exactly_the_fixture_request_and_maps_each_reply() {
        let contract = contract();
        let request = &contract["commands"]["engage"]["request"];
        let (engagement, allow) = engagement_of(request);
        for case in contract["commands"]["engage"]["replies"]
            .as_array()
            .expect("replies")
        {
            let name = case["name"].as_str().expect("name");
            let (blocker, bridge) = blocker(vec![Ok(case["reply"].clone())]);
            let got = blocker.engage(&engagement, &allow);
            assert_eq!(
                bridge.seen(),
                vec![(COMMAND_ENGAGE.to_string(), request.clone())],
                "{name}"
            );
            match case["result"].as_str().expect("result") {
                "ok" => assert_eq!(got, Ok(()), "{name}"),
                code => assert_eq!(got.expect_err(name).code(), code, "{name}"),
            }
        }
    }

    #[test]
    fn release_and_status_follow_the_fixture() {
        let contract = contract();
        for case in contract["commands"]["release"]["replies"]
            .as_array()
            .expect("replies")
        {
            let name = case["name"].as_str().expect("name");
            let (blocker, bridge) = blocker(vec![Ok(case["reply"].clone())]);
            let got = blocker.release();
            assert_eq!(
                bridge.seen(),
                vec![(
                    COMMAND_RELEASE.to_string(),
                    contract["commands"]["release"]["request"].clone()
                )],
                "{name}"
            );
            match case["result"].as_str().expect("result") {
                "ok" => assert_eq!(got, Ok(()), "{name}"),
                code => assert_eq!(got.expect_err(name).code(), code, "{name}"),
            }
        }
        for case in contract["commands"]["status"]["replies"]
            .as_array()
            .expect("replies")
        {
            let name = case["name"].as_str().expect("name");
            let (blocker, bridge) = blocker(vec![Ok(case["reply"].clone())]);
            assert_eq!(
                serde_json::to_value(blocker.status()).expect("json"),
                case["status"],
                "{name}"
            );
            assert_eq!(
                bridge.seen(),
                vec![(
                    COMMAND_STATUS.to_string(),
                    contract["commands"]["status"]["request"].clone()
                )],
                "{name}"
            );
        }
    }

    #[test]
    fn a_silent_plugin_is_unreachable_not_inactive_and_never_a_silent_success() {
        let (blocker, _) = blocker(vec![
            Err(BridgeFailure::Timeout),
            Err(BridgeFailure::Unavailable),
            Err(BridgeFailure::Timeout),
        ]);
        assert_eq!(blocker.status(), BlockerStatus::Unreachable);
        let engagement = Engagement {
            run_id: "r".into(),
            seq: 1,
            remaining_ms: 1_000,
        };
        assert_eq!(
            blocker.engage(&engagement, &AllowList::default()),
            Err(BlockerError::BridgeUnreachable)
        );
        assert_eq!(blocker.release(), Err(BlockerError::BridgeUnreachable));
    }

    #[test]
    fn replies_outside_the_contract_are_malformed() {
        for reply in [
            json!({}),
            json!({ "ok": true, "code": "internal" }),
            json!({ "ok": false }),
            json!({ "ok": true, "extra": 1 }),
            json!("ok"),
        ] {
            let (blocker, _) = blocker(vec![Ok(reply.clone())]);
            assert_eq!(
                blocker.release(),
                Err(BlockerError::BridgeMalformed),
                "{reply}"
            );
        }
        let (blocker, _) = blocker(vec![Ok(json!({ "kind": "locked" }))]);
        assert_eq!(blocker.status(), BlockerStatus::Unreachable);
    }

    #[test]
    fn events_turn_the_last_beat_into_the_next_one() {
        let contract = contract();
        for case in contract["events"].as_array().expect("events") {
            let name = case["name"].as_str().expect("name");
            let event =
                BridgeEvent::parse(&case["event"]).unwrap_or_else(|e| panic!("{name}: {e}"));
            let before: PhoneBeat = serde_json::from_value(case["before"].clone()).expect("before");
            let after: PhoneBeat = serde_json::from_value(case["after"].clone()).expect("after");
            assert_eq!(event.apply(&before), after, "{name}");
        }
        assert_eq!(
            BridgeEvent::parse(&json!({ "event": "package_opened" })),
            Err(BlockerError::BridgeMalformed)
        );
        assert_eq!(
            BridgeEvent::parse(&json!({ "event": "lock_ended", "package": "com.example" })),
            Err(BlockerError::BridgeMalformed)
        );
    }

    #[test]
    fn nothing_on_the_wire_can_name_an_app_the_user_opened() {
        let contract = contract();
        let text =
            contract["commands"]["engage"]["request"].to_string() + &contract["events"].to_string();
        for field in [
            "package_name",
            "app_name",
            "foreground",
            "usage",
            "title",
            "url",
        ] {
            assert!(!text.contains(field), "{field}");
        }
    }
}
