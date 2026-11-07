//! OS integration boundary. Every `#[cfg(target_os)]` branch lives in this module tree.
//! Callers use only the platform-neutral traits below. Unsupported capabilities return
//! `PlatformError::Unsupported` explicitly; they are never silently skipped.

use std::sync::mpsc::Sender;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_quit;
#[cfg(target_os = "windows")]
mod windows;

mod autostart;
mod browser_ipc;
mod focus;
#[cfg(any(target_os = "macos", test))]
mod launch_agent;
mod notifier;
mod process;
mod quit;

pub use autostart::autostart_backend;
pub use browser_ipc::{
    LinkStream, app_data_dir, bind_listener, chrome_manifest_dir, connect as connect_browser_link,
    ensure_token, is_host_registered, read_token, register_host, unregister_host,
};
pub use focus::{FocusProbe, TitleAccess};
pub use notifier::PluginNotifier;
pub use process::{AppIdentity, ProcessControl, is_protected_id, normalize_app_id};
pub use quit::QuitReason;

/// The operating system's name as it is sent to the server with the device registration.
pub fn name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "other"
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("unsupported on this platform: {0}")]
    // Constructed only by the Windows power source (and the non-macOS/Windows fallback).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unsupported(&'static str),
    #[error("autostart backend failed: {0}")]
    Autostart(String),
    #[error("notification backend failed: {0}")]
    Notification(String),
    #[error("tray backend failed: {0}")]
    Tray(String),
    #[error("process control failed: {0}")]
    Process(String),
    #[error("browser link IPC failed: {0}")]
    Ipc(String),
}

/// Desktop notifications. Bodies must not carry subject names: they can show on a lock screen.
pub trait Notifier: Send + Sync {
    fn notify(&self, title: &str, body: &str) -> Result<(), PlatformError>;
}

/// Sets the text next to the tray icon. Only the macOS menu bar has one; Windows reports
/// `Unsupported` and callers fall back to the tooltip.
pub fn set_tray_title<R: tauri::Runtime>(
    tray: &tauri::tray::TrayIcon<R>,
    title: Option<&str>,
) -> Result<(), PlatformError> {
    #[cfg(target_os = "macos")]
    {
        tray.set_title(title)
            .map_err(|e| PlatformError::Tray(e.to_string()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (tray, title);
        Err(PlatformError::Unsupported("tray title"))
    }
}

/// Process control for the current OS: macOS `NSRunningApplication`, Windows PowerShell and
/// `taskkill`. Other targets report `Unsupported` from every call.
pub fn process_control() -> std::sync::Arc<dyn ProcessControl> {
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(macos::MacProcessControl)
    }
    #[cfg(target_os = "windows")]
    {
        std::sync::Arc::new(windows::WindowsProcessControl)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::sync::Arc::new(process::UnsupportedProcessControl)
    }
}

/// Foreground-app and input-idle sensors for the current OS: macOS `NSWorkspace` and
/// CoreGraphics. Windows (and other targets) report `Unsupported` from every call.
pub fn focus_probe() -> std::sync::Arc<dyn FocusProbe> {
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(macos::MacFocusProbe)
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::sync::Arc::new(focus::UnsupportedFocusProbe)
    }
}

/// Login-item / startup-entry control. Backed by tauri-plugin-autostart on both OSes.
pub trait AutostartBackend: Send + Sync {
    fn is_enabled(&self) -> Result<bool, PlatformError>;
    fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError>;
    /// Relaunch-on-kill supervision: the OS restarts the app after a signal or non-zero exit,
    /// never after a normal exit. Only macOS (LaunchAgent `KeepAlive`) implements it; every
    /// other backend reports `Unsupported` explicitly.
    fn set_relaunch(&self, _enabled: bool) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("relaunch supervision"))
    }
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

/// Held while background timing must not be throttled. Dropping it ends the activity.
pub struct ActivityGuard {
    #[cfg(target_os = "macos")]
    _activity: macos::MacActivity,
}

/// Starts a background activity (macOS App Nap opt-out). Windows has no equivalent here yet:
/// it reports `Unsupported`, and the engine's wall-clock catch-up bounds any tick delay.
pub fn begin_activity(reason: &str) -> Result<ActivityGuard, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        Ok(ActivityGuard {
            _activity: macos::begin_activity(reason),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = reason;
        Err(PlatformError::Unsupported("background activity guard"))
    }
}

/// Installs the quit gate on the app's native quit path (macOS: `applicationShouldTerminate:`
/// injected into the tao delegate, because tao 0.37 has none and `terminate:` otherwise skips
/// `RunEvent::ExitRequested`). `gate` returns whether this quit may proceed; it is consulted
/// only for non-system reasons. Windows has no equivalent yet and reports `Unsupported`; the
/// ExitRequested gate then still covers the tray and `app.exit` paths.
pub fn install_quit_gate(
    gate: Box<dyn Fn(QuitReason) -> bool + Send + Sync>,
) -> Result<(), PlatformError> {
    #[cfg(target_os = "macos")]
    {
        macos_quit::install_quit_gate(gate)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = gate;
        Err(PlatformError::Unsupported("native quit gate"))
    }
}
