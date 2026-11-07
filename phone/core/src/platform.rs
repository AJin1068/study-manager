//! Which blocker this build uses. iOS cannot be locked by this app (the only Apple route is
//! Screen Time, which needs an Apple-granted entitlement), so an iOS build gets the
//! `ScreenTimeBlocker`, which answers `Unsupported` and never pretends to block. Every other
//! target (Android) talks to the Kotlin plugin through the bridge. No Swift code exists.

use std::sync::Arc;

use crate::android_blocker::PluginBridge;
use crate::blocker_port::PhoneBlocker;

#[cfg(target_os = "ios")]
pub fn platform_blocker(_bridge: Arc<dyn PluginBridge>) -> Arc<dyn PhoneBlocker> {
    Arc::new(crate::blocker::ScreenTimeBlocker)
}

#[cfg(not(target_os = "ios"))]
pub fn platform_blocker(bridge: Arc<dyn PluginBridge>) -> Arc<dyn PhoneBlocker> {
    Arc::new(crate::android_blocker::AndroidAccessibilityBlocker::new(
        bridge,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android_blocker::{BridgeFailure, ScriptedBridge};
    use crate::blocker::BlockerStatus;

    #[test]
    fn a_non_ios_build_asks_the_plugin() {
        // On iOS this test would not apply: the blocker there never reads the bridge.
        let bridge = Arc::new(ScriptedBridge::new(vec![
            Ok(serde_json::json!({ "kind": "active" })),
            Err(BridgeFailure::Unavailable),
        ]));
        let blocker = platform_blocker(bridge.clone());
        if cfg!(target_os = "ios") {
            assert!(matches!(
                blocker.status(),
                BlockerStatus::Unsupported { .. }
            ));
        } else {
            assert_eq!(blocker.status(), BlockerStatus::Active);
            assert_eq!(blocker.status(), BlockerStatus::Unreachable);
            assert_eq!(bridge.seen().len(), 2);
        }
    }
}
