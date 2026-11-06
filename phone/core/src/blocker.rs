//! What a phone-side blocker reports. iPhone cannot be locked by this app: the only Apple route is
//! Screen Time, which needs the user's own authorization on the device and an Apple-granted
//! entitlement, so the iPhone blocker always answers `Unsupported` and no Swift code exists.

use serde::Serialize;

/// Why a platform cannot block. Serialized as snake_case codes the UI maps to text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedReason {
    AppleScreenTimeAuthorizationRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockerStatus {
    Active,
    Inactive,
    /// The accessibility service is off or was never granted: nothing is being blocked.
    ServiceDisabled,
    /// The plugin did not answer; the state is not known.
    Unreachable,
    Unsupported { reason: UnsupportedReason },
}

/// The iPhone blocker. It holds no state and never reports anything but `Unsupported`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScreenTimeBlocker;

/// Alias kept for call sites that name the behavior rather than the platform.
pub type UnsupportedBlocker = ScreenTimeBlocker;

impl ScreenTimeBlocker {
    pub fn status(&self) -> BlockerStatus {
        BlockerStatus::Unsupported { reason: UnsupportedReason::AppleScreenTimeAuthorizationRequired }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_iphone_blocker_is_always_unsupported_with_the_screen_time_reason() {
        let blocker = ScreenTimeBlocker;
        for _ in 0..3 {
            assert_eq!(
                blocker.status(),
                BlockerStatus::Unsupported { reason: UnsupportedReason::AppleScreenTimeAuthorizationRequired }
            );
        }
        assert_eq!(UnsupportedBlocker::default().status(), blocker.status());
    }

    #[test]
    fn the_wire_form_carries_the_exact_reason_code() {
        let wire = serde_json::to_value(ScreenTimeBlocker.status()).expect("json");
        assert_eq!(wire, json!({ "kind": "unsupported", "reason": "apple_screen_time_authorization_required" }));
        assert_eq!(serde_json::to_value(BlockerStatus::Active).expect("json"), json!({ "kind": "active" }));
        assert_eq!(serde_json::to_value(BlockerStatus::Inactive).expect("json"), json!({ "kind": "inactive" }));
        assert_eq!(serde_json::to_value(BlockerStatus::ServiceDisabled).expect("json"), json!({ "kind": "service_disabled" }));
        assert_eq!(serde_json::to_value(BlockerStatus::Unreachable).expect("json"), json!({ "kind": "unreachable" }));
    }

    #[test]
    fn no_swift_source_exists_in_the_repository() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut found = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if path.is_dir() {
                    if !matches!(name.as_str(), "node_modules" | "target" | ".git" | ".gjc") {
                        stack.push(path);
                    }
                } else if name.ends_with(".swift") {
                    found.push(path);
                }
            }
        }
        assert!(found.is_empty(), "iPhone support is Unsupported-only: {found:?}");
    }
}
