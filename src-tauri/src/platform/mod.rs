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
#[cfg(any(target_os = "windows", test))]
mod task_scheduler;
#[cfg(any(target_os = "windows", test))]
mod windows_ipc;

pub use autostart::autostart_backend;
#[cfg(test)]
pub use browser_ipc::test_stream_pair;
pub use browser_ipc::{
    HostTarget, LinkStream, app_data_dir, bind_listener, chrome_host_target,
    connect as connect_browser_link, ensure_token, is_host_registered, read_token, register_host,
    unregister_host,
};
pub use focus::{FocusProbe, TitleAccess};
pub use notifier::PluginNotifier;
pub use process::{AppIdentity, ProcessControl, is_protected_id, normalize_app_id};
pub use quit::QuitReason;

/// Records a deliberate exit so Windows relaunch supervision leaves the app alone until the next
/// normal start. macOS needs nothing: its LaunchAgent already ignores a successful exit.
pub fn note_clean_exit(identifier: &str) {
    #[cfg(target_os = "windows")]
    if let Some(dir) = app_data_dir(identifier)
        && let Err(err) = task_scheduler::note_clean_exit(&dir)
    {
        tracing::warn!(error = %err, "could not record the clean exit for relaunch supervision");
    }
    #[cfg(not(target_os = "windows"))]
    let _ = identifier;
}

/// Whether this process start must be dropped before the UI boots: a Windows watchdog start
/// (`--supervised`) after a clean quit. Any other start re-arms the watchdog. Always false
/// elsewhere.
pub fn supervised_start_suppressed(identifier: &str, args: &[String]) -> bool {
    #[cfg(target_os = "windows")]
    {
        match app_data_dir(identifier) {
            Some(dir) => task_scheduler::supervised_start_suppressed(&dir, args),
            None => false,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (identifier, args);
        false
    }
}

/// Whether the args mark a Windows watchdog start (the second-instance callback ignores those).
pub fn is_supervised_start(args: &[String]) -> bool {
    #[cfg(target_os = "windows")]
    {
        task_scheduler::is_supervised_start(args)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = args;
        false
    }
}

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

/// Process control for the current OS: macOS `NSRunningApplication`, Windows Win32
/// (`EnumWindows`, `WM_CLOSE`, `TerminateProcess`). Other targets report `Unsupported`.
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
/// CoreGraphics, Windows `GetForegroundWindow` and `GetLastInputInfo`. Other targets report
/// `Unsupported` from every call.
pub fn focus_probe() -> std::sync::Arc<dyn FocusProbe> {
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(macos::MacFocusProbe)
    }
    #[cfg(target_os = "windows")]
    {
        std::sync::Arc::new(windows::WindowsFocusProbe)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::sync::Arc::new(focus::UnsupportedFocusProbe)
    }
}

/// Login-item / startup-entry control. macOS: our own LaunchAgent. Windows: the HKCU Run entry
/// through tauri-plugin-autostart, or the Task Scheduler pair while relaunch supervision is on.
pub trait AutostartBackend: Send + Sync {
    fn is_enabled(&self) -> Result<bool, PlatformError>;
    fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError>;
    /// Relaunch-on-kill supervision: the OS restarts the app after a signal or non-zero exit,
    /// never after a normal exit. macOS implements it with LaunchAgent `KeepAlive`, Windows with
    /// the logon + watchdog tasks; every other backend reports `Unsupported` explicitly.
    fn set_relaunch(&self, _enabled: bool) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("relaunch supervision"))
    }
}

// Only the macOS and Windows sources construct these; on other targets the variants are never
// built even though the core matches them.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
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

/// Returns the power-event source for the current OS. Modern Standby hardware may not deliver
/// Windows suspend notifications; the core's heartbeat-gap fallback runs either way.
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
    #[cfg(target_os = "windows")]
    _activity: windows::WinActivity,
}

/// Starts a background activity: the macOS App Nap opt-out, or on Windows the process power
/// throttling opt-out. Other targets report `Unsupported`; the engine's wall-clock catch-up
/// bounds any tick delay there.
pub fn begin_activity(reason: &str) -> Result<ActivityGuard, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        Ok(ActivityGuard {
            _activity: macos::begin_activity(reason),
        })
    }
    #[cfg(target_os = "windows")]
    {
        let _ = reason;
        Ok(ActivityGuard {
            _activity: windows::begin_activity()?,
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = reason;
        Err(PlatformError::Unsupported("background activity guard"))
    }
}

/// Installs the quit gate on the app's native quit path (macOS: `applicationShouldTerminate:`
/// injected into the tao delegate, because tao 0.37 has none and `terminate:` otherwise skips
/// `RunEvent::ExitRequested`). `gate` returns whether this quit may proceed; it is consulted
/// only for non-system reasons. Windows has no native quit path beyond Tauri's
/// `RunEvent::ExitRequested` (tray quit, `app.exit`), which `lib.rs` already gates, so the
/// install is a defined no-op there: the window close button hides the window, and a session end
/// must never be refused.
pub fn install_quit_gate(
    gate: Box<dyn Fn(QuitReason) -> bool + Send + Sync>,
) -> Result<(), PlatformError> {
    #[cfg(target_os = "macos")]
    {
        macos_quit::install_quit_gate(gate)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = gate;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = gate;
        Err(PlatformError::Unsupported("native quit gate"))
    }
}
