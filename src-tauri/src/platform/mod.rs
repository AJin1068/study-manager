//! OS integration boundary. Every `#[cfg(target_os)]` branch lives in this module tree.
//! Callers use only the platform-neutral traits below. Unsupported capabilities return
//! `PlatformError::Unsupported` explicitly; they are never silently skipped.

use std::sync::mpsc::Sender;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

mod autostart;

pub use autostart::PluginAutostart;

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("unsupported on this platform: {0}")]
    // Constructed only by the Windows power source (and the non-macOS/Windows fallback).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unsupported(&'static str),
    #[error("autostart backend failed: {0}")]
    Autostart(String),
}

/// Login-item / startup-entry control. Backed by tauri-plugin-autostart on both OSes.
pub trait AutostartBackend: Send + Sync {
    fn is_enabled(&self) -> Result<bool, PlatformError>;
    fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError>;
}

// Only the macOS source constructs these. Windows reports Unsupported until S7a, so on other
// targets the variants are never built even though the core matches them. Allowed only off macOS
// so a real dead variant on macOS still fails clippy.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    WillSleep,
    DidWake,
}

/// Source of system sleep/wake notifications. `start` must deliver events to `sender`
/// for the lifetime of the process.
pub trait PowerEventSource {
    fn start(self: Box<Self>, sender: Sender<PowerEvent>) -> Result<(), PlatformError>;
}

/// Returns the power-event source for the current OS. On Windows this is `Unsupported`
/// until S7a (real-device Modern Standby check) passes; the heartbeat fallback still runs.
pub fn power_event_source() -> Box<dyn PowerEventSource> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::NsWorkspacePowerSource)
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsPowerSource)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Box::new(UnsupportedPowerSource)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
struct UnsupportedPowerSource;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl PowerEventSource for UnsupportedPowerSource {
    fn start(self: Box<Self>, _sender: Sender<PowerEvent>) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("power events"))
    }
}
