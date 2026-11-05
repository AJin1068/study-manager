//! Study Manager core. OS integration, SQLite access, and state decisions live here; the
//! React UI only calls the commands in `commands.rs` and listens for events.

mod clock;
mod commands;
mod db;
mod downtime;
mod error;
mod platform;
mod power;
mod settings;
mod store;
mod study;
mod tray;

use std::sync::{mpsc, Arc, Mutex};

use tauri::{Manager, RunEvent, WindowEvent};

use commands::{AppState, PowerSourceStatus};

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let mut conn = db::open(&data_dir.join("study-manager.sqlite"))?;

    let now = clock::now_ms()?;
    // Fails startup if the lock row is missing: a deleted lock must never read as "not studying".
    study::read_state(&conn)?;
    let handle = app.handle().clone();
    let autostart = Arc::new(platform::PluginAutostart::new(handle.clone()));
    settings::bootstrap_autostart(&mut conn, autostart.as_ref(), now)?;
    downtime::recover_unclean_sessions(&mut conn, now)?;
    let session_id = store::insert_session(&conn, now)?;

    let (sender, events) = mpsc::channel();
    let power = match platform::power_event_source().start(sender) {
        Ok(()) => PowerSourceStatus { active: true, reason: None },
        Err(err) => {
            tracing::warn!(error = %err, "power events unavailable; heartbeat fallback only");
            PowerSourceStatus { active: false, reason: Some(err.to_string()) }
        }
    };

    let db = Arc::new(Mutex::new(conn));
    app.manage(AppState {
        db: Arc::clone(&db),
        session_id,
        autostart,
        power,
    });
    tray::install(&handle)?;
    power::spawn(handle, db, session_id, events);
    Ok(())
}

/// App menu. Quit is a custom item, not the predefined one: the predefined Quit calls NSApp
/// `terminate:`, which never reaches `RunEvent::ExitRequested` on macOS, so the study lock could
/// not stop Cmd+Q. The custom item goes through `app.exit(0)` and the same exit gate as the tray.
fn app_menu(handle: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};

    let quit = MenuItem::with_id(handle, "app_quit", "종료", true, Some("CmdOrCtrl+Q"))?;
    let hide = PredefinedMenuItem::hide(handle, None)?;
    let app_submenu = Submenu::with_items(handle, "Study Manager", true, &[&hide, &quit])?;

    // Keeps copy/paste shortcuts working in the webview's text fields.
    let edit = Submenu::with_items(
        handle,
        "편집",
        true,
        &[
            &PredefinedMenuItem::undo(handle, None)?,
            &PredefinedMenuItem::redo(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::cut(handle, None)?,
            &PredefinedMenuItem::copy(handle, None)?,
            &PredefinedMenuItem::paste(handle, None)?,
            &PredefinedMenuItem::select_all(handle, None)?,
        ],
    )?;

    Menu::with_items(handle, &[&app_submenu, &edit])
}

pub fn run() {
    // Logs carry only event kinds, timestamps, and counts: no window titles or URLs.
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    // Single instance must be registered first: a second process exits here, before setup,
    // so it cannot run session recovery against the live first session.
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .menu(app_menu)
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "app_quit" {
                app.exit(0);
            }
        })
        .setup(setup)
        .on_window_event(|window, event| {
            // Closing hides the window; quitting is a separate, lock-checked action.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(err) = window.hide() {
                    tracing::error!(error = %err, "failed to hide window");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::update_setting,
            commands::get_setting_history,
            commands::get_study_state,
            commands::set_study_state_test,
            commands::get_mode_config,
            commands::set_daily_mode,
            commands::get_downtime_records,
            commands::get_autostart_status,
            commands::get_power_status,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|err| {
            tracing::error!(error = %err, "failed to build application");
            std::process::exit(1);
        });

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            let state = app_handle.state::<AppState>();
            match commands::allow_exit(&state) {
                Ok(true) => {}
                Ok(false) => {
                    api.prevent_exit();
                    tracing::info!(event = "exit_blocked", reason = "studying");
                }
                Err(err) => {
                    // Fail closed: without a lock check, the exit is refused.
                    api.prevent_exit();
                    tracing::error!(error = %err, "exit gate failed; exit refused");
                }
            }
        }
    });
}
