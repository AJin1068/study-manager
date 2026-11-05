//! Menu-bar (macOS) / tray (Windows) icon. Menu items are "설정 열기" and "종료" only.
//! Closing the window hides it (see lib.rs); "종료" goes through the same exit gate as Cmd+Q,
//! so a study-time quit is rejected and recorded in one place.

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let open_settings = MenuItem::with_id(app, "open_settings", "설정 열기", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_settings, &quit])?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("Study Manager")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open_settings" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(err) = window.show() {
            tracing::error!(error = %err, "failed to show main window");
        }
        if let Err(err) = window.set_focus() {
            tracing::warn!(error = %err, "failed to focus main window");
        }
    }
}
