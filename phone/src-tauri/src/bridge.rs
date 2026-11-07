//! The seam to the Kotlin blocker plugin (`phone/plugins/blocker/android/`). On Android the slot
//! holds the plugin handle registered at startup and `invoke` runs the plugin command; on every
//! other target (desktop development, iOS) it is empty and every command is `Unavailable`, so
//! nothing is ever reported as blocked there.
//!
//! The Android branch cannot be compiled in this repository's environment (no Android SDK/NDK):
//! it is unverified (V5-1, V5-4).

use serde_json::Value;
use study_phone_core::android_blocker::{BridgeFailure, PluginBridge};

#[cfg(target_os = "android")]
use std::sync::{Arc, OnceLock};

#[derive(Clone, Default)]
pub struct PluginSlot {
    #[cfg(target_os = "android")]
    handle: Arc<OnceLock<tauri::plugin::PluginHandle<tauri::Wry>>>,
}

impl PluginSlot {
    #[cfg(target_os = "android")]
    pub fn set(&self, handle: tauri::plugin::PluginHandle<tauri::Wry>) {
        let _ = self.handle.set(handle);
    }
}

impl PluginBridge for PluginSlot {
    #[cfg(target_os = "android")]
    fn invoke(&self, command: &str, payload: Value) -> Result<Value, BridgeFailure> {
        let handle = self.handle.get().ok_or(BridgeFailure::Unavailable)?;
        handle
            .run_mobile_plugin::<Value>(command, payload)
            .map_err(|_| BridgeFailure::Unavailable)
    }

    #[cfg(not(target_os = "android"))]
    fn invoke(&self, _command: &str, _payload: Value) -> Result<Value, BridgeFailure> {
        Err(BridgeFailure::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_os = "android"))]
    fn an_empty_slot_answers_unavailable_never_ok() {
        let slot = PluginSlot::default();
        for command in ["engage", "release", "status"] {
            assert_eq!(
                slot.invoke(command, serde_json::json!({})),
                Err(BridgeFailure::Unavailable)
            );
        }
    }
}
