//! Login-item control. macOS uses our own LaunchAgent (`launch_agent.rs`) so the entry can carry
//! relaunch-on-kill supervision; Windows uses tauri-plugin-autostart (HKCU Run key) while
//! relaunch is off and a Task Scheduler pair (`task_scheduler.rs`) while it is on, never both.
//!
//! Permissions: macOS may show a "background item added" notice on first enable; Windows
//! writes HKCU Run or per-user scheduled tasks (interactive token, least privilege). Neither
//! needs elevated rights.
//!
//! Windows supervision applies from the next task-started launch: a copy started by hand or by
//! the Run key is not owned by the scheduler, though the watchdog task still brings the app back
//! if that copy is killed.

use std::sync::Arc;

#[cfg(target_os = "macos")]
use tauri::Manager;
use tauri::{AppHandle, Runtime};
#[cfg(not(target_os = "macos"))]
use tauri_plugin_autostart::ManagerExt;

use super::{AutostartBackend, PlatformError};

/// Backend for the current OS. Fails if the macOS home directory or executable path is unknown.
pub fn autostart_backend<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Arc<dyn AutostartBackend>, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        let home = app
            .path()
            .home_dir()
            .map_err(|e| PlatformError::Autostart(e.to_string()))?;
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
    #[cfg(target_os = "windows")]
    {
        use super::task_scheduler::{
            LOGON_TASK, RealRunner, SupervisedAutostart, TaskSupervisor, WATCHDOG_TASK,
            current_user,
        };
        let exe = std::env::current_exe()
            .map_err(|e| PlatformError::Autostart(e.to_string()))?
            .display()
            .to_string();
        let user = current_user()
            .ok_or_else(|| PlatformError::Autostart("current user name is unknown".into()))?;
        let tasks = TaskSupervisor::new(
            Box::new(RealRunner),
            LOGON_TASK,
            WATCHDOG_TASK,
            std::env::temp_dir(),
        );
        Ok(Arc::new(SupervisedAutostart::new(
            PluginAutostart::new(app.clone()),
            tasks,
            exe,
            user,
        )))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
impl<R: Runtime> PluginAutostart<R> {
    fn plugin_is_enabled(&self) -> Result<bool, PlatformError> {
        self.app
            .autolaunch()
            .is_enabled()
            .map_err(|e| PlatformError::Autostart(e.to_string()))
    }

    fn plugin_set_enabled(&self, enabled: bool) -> Result<(), PlatformError> {
        let manager = self.app.autolaunch();
        let result = if enabled {
            manager.enable()
        } else {
            manager.disable()
        };
        result.map_err(|e| PlatformError::Autostart(e.to_string()))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl<R: Runtime> AutostartBackend for PluginAutostart<R> {
    fn is_enabled(&self) -> Result<bool, PlatformError> {
        self.plugin_is_enabled()
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError> {
        self.plugin_set_enabled(enabled)
    }
}

#[cfg(target_os = "windows")]
impl<R: Runtime> super::task_scheduler::RunEntry for PluginAutostart<R> {
    fn is_enabled(&self) -> Result<bool, PlatformError> {
        self.plugin_is_enabled()
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError> {
        self.plugin_set_enabled(enabled)
    }
}
