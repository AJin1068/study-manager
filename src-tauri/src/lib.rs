//! Study Manager core. OS integration, SQLite access, and state decisions live here; the
//! React UI only calls the commands in `commands/` and listens for events.

mod browser_link;
mod clock;
mod commands;
mod dashboard;
mod db;
mod downtime;
mod enforce;
mod error;
mod focus;
mod plan;
mod plan_shift;
mod platform;
mod points;
mod power;
mod presets;
mod settings;
mod site_rules;
mod store;
mod study;
#[cfg(test)]
mod testutil;
mod timer;
mod tray;

use std::sync::{mpsc, Arc, Mutex};

use tauri::{Manager, RunEvent, WindowEvent};

use commands::{AppState, PowerSourceStatus};

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let mut conn = db::open(&data_dir.join("study-manager.sqlite"))?;

    let now = clock::now_ms()?;
    // Fails startup if the lock row is missing: a deleted lock must never read as "lock free".
    study::read_lock_state(&conn)?;
    let handle = app.handle().clone();
    let autostart = platform::autostart_backend(&handle)?;
    // Relaunch first: the login item is written with the stored supervision choice.
    settings::bootstrap_relaunch(&mut conn, autostart.as_ref(), now)?;
    settings::bootstrap_autostart(&mut conn, autostart.as_ref(), now)?;
    // A catalog update must not keep the app from starting; the block list simply stays as it was.
    if let Err(err) = presets::refresh(&mut conn, now) {
        tracing::error!(error = %err, "preset refresh failed");
    }
    downtime::recover_unclean_sessions(&mut conn, now)?;
    let session_id = store::insert_session(&conn, now)?;
    // The highest heartbeat ever stored is the floor for the engine's clock: a clock set back
    // while the app was closed must not rewind a run.
    let high_water = Arc::new(timer::HighWater::new(store::max_heartbeat(&conn)?));

    let (sender, events) = mpsc::channel();
    let power = match platform::power_event_source().start(sender) {
        Ok(()) => PowerSourceStatus { active: true, reason: None },
        Err(err) => {
            tracing::warn!(error = %err, "power events unavailable; heartbeat fallback only");
            PowerSourceStatus { active: false, reason: Some(err.to_string()) }
        }
    };

    let db = Arc::new(Mutex::new(conn));
    let browser_link = Arc::new(browser_link::LinkHub::for_app(handle.clone()));
    let process = platform::process_control();
    let focus_probe = platform::focus_probe();
    let signals = Arc::new(focus::signals::SignalHub::new(Box::<focus::signals::SystemRandom>::default()));
    let (engine, wakes) = timer::engine::channel();
    let health = Arc::new(timer::engine::EngineHealth::default());
    app.manage(AppState {
        db: Arc::clone(&db),
        session_id,
        autostart,
        power,
        engine: engine.clone(),
        high_water: Arc::clone(&high_water),
        health: Arc::clone(&health),
        release: timer::release::ReleaseDesk::default(),
        process: Arc::clone(&process),
        focus_probe: Arc::clone(&focus_probe),
        browser_link: Arc::clone(&browser_link),
        signals: Arc::clone(&signals),
    });
    tray::install(&handle)?;
    install_quit_gate(&handle);
    let notifier: Arc<dyn platform::Notifier> = Arc::new(platform::PluginNotifier::new(handle.clone()));
    timer::engine::spawn(
        handle.clone(),
        Arc::clone(&db),
        Arc::clone(&high_water),
        health,
        wakes,
        Arc::clone(&notifier),
    );
    browser_link::spawn(Arc::clone(&db), Arc::clone(&high_water), browser_link, data_dir);
    focus::spawn(Arc::clone(&db), Arc::clone(&high_water), focus_probe);
    // A shortcut that cannot be registered is reported in the status, not fatal: the in-app
    // button does the same job.
    signals.set_status(focus::signals::install_shortcut(&handle, Arc::clone(&db), Arc::clone(&high_water)))?;
    focus::signals::spawn(handle.clone(), Arc::clone(&db), Arc::clone(&high_water), signals, Arc::clone(&notifier));
    enforce::spawn(handle.clone(), Arc::clone(&db), high_water, process, notifier);
    power::spawn(handle, db, session_id, events, engine);
    Ok(())
}

/// Routes `terminate:` (Apple Event quit, Dock "Quit") through the study lock. If the native hook
/// cannot be installed the menu/tray gate still holds; an Apple Event quit then bypasses the lock
/// but leaves the session unclean, so the next launch records it as `forced_quit`.
fn install_quit_gate(handle: &tauri::AppHandle) {
    let gate_handle = handle.clone();
    let gate = Box::new(move |reason: platform::QuitReason| {
        let allowed = commands::allow_quit(&gate_handle.state::<AppState>(), reason);
        if !allowed {
            tracing::info!(event = "exit_blocked", reason = "lock_held", source = reason.as_str());
        }
        allowed
    });
    if let Err(err) = platform::install_quit_gate(gate) {
        tracing::warn!(error = %err, "native quit gate unavailable; Apple Event quit is not gated");
    }
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

/// Native-messaging host mode: `Some(exit code)` when Chrome started this process (it passes the
/// caller's `chrome-extension://` origin). Runs before any Tauri code so the single-instance
/// plugin never sees the host as a second app.
pub fn run_native_host_if_invoked() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    browser_link::host::run_if_invoked(&args)
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
        .plugin(tauri_plugin_notification::init())
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
            commands::settings::get_settings,
            commands::settings::update_setting,
            commands::settings::get_setting_history,
            commands::study::get_lock_state,
            commands::study::get_mode_config,
            commands::study::set_daily_mode,
            commands::study::update_mode_config,
            commands::downtime::get_downtime_records,
            commands::downtime::get_downtime_summary,
            commands::power::get_autostart_status,
            commands::power::get_power_status,
            commands::plan::create_plan,
            commands::plan::update_plan,
            commands::plan::delete_plan,
            commands::plan::undo_plan_batch,
            commands::plan::list_occurrences,
            commands::plan::get_plan_history,
            commands::plan::get_plan_stats,
            commands::timer::get_timer_state,
            commands::release::begin_release,
            commands::release::submit_release,
            commands::release::get_pass_status,
            commands::release::list_passes,
            commands::timer::get_engine_health,
            commands::enforce::list_block_entries,
            commands::enforce::add_block_entry,
            commands::enforce::remove_block_entry,
            commands::enforce::get_enforcement_status,
            commands::enforce::list_running_apps,
            commands::enforce::set_enforcement_mode,
            commands::browser_link::get_browser_link_status,
            commands::browser_link::get_site_rules,
            commands::browser_link::update_site_rules,
            commands::browser_link::list_site_entries,
            commands::browser_link::add_site_entry,
            commands::browser_link::get_preset_status,
            commands::browser_link::set_preset_categories,
            commands::browser_link::get_browser_host_registration,
            commands::browser_link::set_browser_host_registered,
            commands::browser_link::get_run_browser_coverage,
            commands::focus::list_study_apps,
            commands::focus::add_study_app,
            commands::focus::remove_study_app,
            commands::focus::set_study_app_enabled,
            commands::focus::get_focus_setup,
            commands::focus::report_distraction,
            commands::focus::get_self_report_status,
            commands::focus::get_pending_checkin,
            commands::focus::answer_checkin,
            commands::points::get_points_status,
            commands::points::list_point_ledger,
            commands::dashboard::get_dashboard_today,
            commands::dashboard::get_dashboard_week,
            commands::dashboard::get_weakness,
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
                    tracing::info!(event = "exit_blocked", reason = "lock_held");
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
