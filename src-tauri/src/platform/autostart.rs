//! Login-item control via tauri-plugin-autostart. The plugin maps to a macOS LaunchAgent
//! and a Windows Run-key entry, so this file has no `cfg` branches.
//!
//! Permissions: macOS may show a "background item added" notice on first enable; Windows
//! writes HKCU Run. Neither needs elevated rights.

use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt;

use super::{AutostartBackend, PlatformError};

pub struct PluginAutostart<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> PluginAutostart<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

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
