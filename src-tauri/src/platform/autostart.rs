//! Login-item control. macOS uses our own LaunchAgent (`launch_agent.rs`) so the entry can carry
//! relaunch-on-kill supervision; Windows uses tauri-plugin-autostart (HKCU Run key).
//!
//! Permissions: macOS may show a "background item added" notice on first enable; Windows
//! writes HKCU Run. Neither needs elevated rights.
//!
//! Windows relaunch supervision stays `Unsupported`: a Run-key entry is started by the shell,
//! not by Task Scheduler, and Task Scheduler's restart-on-failure only restarts tasks it started
//! itself. Doing it would need a separate scheduled task that owns the process (and a way to stop
//! it on a normal quit); that is not designed yet and cannot be verified without a Windows device.

use std::sync::Arc;

#[cfg(target_os = "macos")]
use tauri::Manager;
use tauri::{AppHandle, Runtime};
#[cfg(not(target_os = "macos"))]
use tauri_plugin_autostart::ManagerExt;

use super::{AutostartBackend, PlatformError};

/// Backend for the current OS. Fails if the macOS home directory or executable path is unknown.
pub fn autostart_backend<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<dyn AutostartBackend>, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        let home = app.path().home_dir().map_err(|e| PlatformError::Autostart(e.to_string()))?;
        let exe = std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .map_err(|e| PlatformError::Autostart(e.to_string()))?;
        Ok(Arc::new(super::launch_agent::LaunchAgentAutostart::new(
            &home.join("Library").join("LaunchAgents"),
            &app.package_info().name,
            vec![exe.display().to_string()],
            &app.config().identifier,
        )))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(Arc::new(PluginAutostart::new(app.clone())))
    }
}

#[cfg(not(target_os = "macos"))]
struct PluginAutostart<R: Runtime> {
    app: AppHandle<R>,
}

#[cfg(not(target_os = "macos"))]
impl<R: Runtime> PluginAutostart<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

#[cfg(not(target_os = "macos"))]
impl<R: Runtime> AutostartBackend for PluginAutostart<R> {
    fn is_enabled(&self) -> Result<bool, PlatformError> {
        self.app
            .autolaunch()
            .is_enabled()
            .map_err(|e| PlatformError::Autostart(e.to_string()))
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError> {
        let manager = self.app.autolaunch();
        let result = if enabled { manager.enable() } else { manager.disable() };
        result.map_err(|e| PlatformError::Autostart(e.to_string()))
    }
}
