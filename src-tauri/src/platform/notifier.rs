//! Desktop notifications via tauri-plugin-notification (macOS Notification Center, Windows
//! toast). The plugin hides the OS split, so this file has no `cfg` branches.
//!
//! Permissions: macOS asks on the first notification; an unpackaged Windows dev build shows
//! toasts under a generic app id.

use tauri::{AppHandle, Runtime};
use tauri_plugin_notification::NotificationExt;

use super::{Notifier, PlatformError};

pub struct PluginNotifier<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> PluginNotifier<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> Notifier for PluginNotifier<R> {
    fn notify(&self, title: &str, body: &str) -> Result<(), PlatformError> {
        self.app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|e| PlatformError::Notification(e.to_string()))
    }
}
