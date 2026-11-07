//! Study Manager core. OS integration, SQLite access, and state decisions live here; the
//! React UI only calls the commands in `commands/` and listens for events.

mod browser_link;
mod character;
mod chat;
mod clock;
mod commands;
mod dashboard;
mod db;
mod downtime;
mod enforce;
mod error;
mod focus;
mod growth;
pub mod llm;
mod messages;
pub mod phone;
mod plan;
mod plan_adjust;
mod plan_ics;
mod plan_import;
mod plan_shift;
mod platform;
mod points;
mod power;
mod presets;
mod profile;
pub mod quiz;
pub mod remote;
mod rooms;
mod settings;
mod shared_wiki;
mod shop;
mod site_rules;
mod store;
mod study;
mod study_context;
mod sync;
#[cfg(test)]
mod testutil;
mod timer;
mod tray;
pub mod wiki;

use std::sync::{Arc, Mutex, mpsc};

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
    remote::config::ensure_device_id(&conn, now)?;

    let (sender, events) = mpsc::channel();
    let power = match platform::power_event_source().start(sender) {
        Ok(()) => PowerSourceStatus {
            active: true,
            reason: None,
        },
        Err(err) => {
            tracing::warn!(error = %err, "power events unavailable; heartbeat fallback only");
            PowerSourceStatus {
                active: false,
                reason: Some(err.to_string()),
            }
        }
    };

    let db = Arc::new(Mutex::new(conn));
    let browser_link = Arc::new(browser_link::LinkHub::for_app(handle.clone()));
    let process = platform::process_control();
    let focus_probe = platform::focus_probe();
    let signals = Arc::new(focus::signals::SignalHub::new(Box::<
        focus::signals::SystemRandom,
    >::default()));
    // Keychain self-check runs once here; an unusable keychain degrades to session-only secrets
    // and is shown in the integrations screen.
    let secrets =
        remote::secrets::select_secret_store(Arc::new(remote::secrets::KeyringSecretStore));
    let remote = Arc::new(
        remote::runtime::RemoteRuntime::new(
            secrets,
            Arc::new(remote::SystemClock),
            Arc::new(remote::audit::DbSink::new(Arc::clone(&db))),
            remote::runtime::ureq_factory(),
        )
        .with_llm_transports(llm::anthropic::ureq_factory()),
    );
    // Mirror and presence workers: idle (nothing sent) until a server is configured and the user
    // is signed in. Sleep/wake events reach the presence worker through power::spawn below.
    let rooms = Arc::new(rooms::service::RoomsService::new(
        Arc::clone(&db),
        Arc::clone(&remote),
        Arc::new(remote::SystemClock),
    ));
    app.manage(Arc::clone(&rooms));
    app.manage(Arc::new(profile::service::ProfileService::new(
        Arc::clone(&db),
        Arc::clone(&remote),
        Arc::new(remote::SystemClock),
    )));
    app.manage(Arc::new(phone::pairing::PairingService::new(
        Arc::clone(&db),
        Arc::clone(&remote),
        Arc::new(remote::SystemClock),
    )));
    // Publishes the phone lock lease and watches the phone's heartbeat from its own thread; idle
    // until a phone is paired. The timer engine and the release commands only wake it.
    let phone_diag = Arc::new(phone::publisher::Diagnostics::default());
    let phone_publisher = phone::publisher::spawn(phone::publisher::Publisher::new(
        Arc::clone(&db),
        Arc::new(remote::SystemClock),
        phone::pairing::app_backend(Arc::clone(&db), Arc::clone(&remote)),
        chrono::Local,
        Arc::clone(&high_water),
        Arc::clone(&phone_diag),
    ));
    app.manage(phone_diag);
    let sync = sync::app::start(
        Arc::clone(&db),
        Arc::clone(&remote),
        Arc::new(remote::SystemClock),
    );
    let llm = Arc::new(llm::service::LlmService::new(
        Arc::clone(&db),
        Arc::clone(&remote),
        Arc::new(remote::SystemClock),
    ));
    let wiki = Arc::new(wiki::service::WikiService::new(
        Arc::clone(&db),
        Arc::clone(&llm),
        Arc::new(remote::SystemClock),
        Arc::new(wiki::pdf::NoPdfExtractor),
    ));
    let quiz = Arc::new(quiz::service::QuizService::new(
        Arc::clone(&db),
        Arc::clone(&wiki),
        Arc::clone(&llm),
        Arc::new(remote::SystemClock),
    ));
    let retro = Arc::new(wiki::retro_service::RetroService::new(
        Arc::clone(&db),
        Arc::clone(&llm),
        Arc::new(remote::SystemClock),
    ));
    app.manage(retro);
    app.manage(Arc::new(chat::service::ChatService::new(
        Arc::clone(&db),
        Arc::clone(&llm),
        Arc::new(remote::SystemClock),
    )));
    app.manage(Arc::new(character::service::CharacterService::new(
        Arc::clone(&llm),
        Arc::new(remote::SystemClock),
    )));
    app.manage(Arc::new(growth::GrowthService::new(
        Arc::clone(&db),
        Arc::clone(&llm),
        Arc::new(remote::SystemClock),
    )));
    app.manage(Arc::new(shared_wiki::service::SharedWikiService::new(
        Arc::clone(&db),
        Arc::clone(&remote),
        Arc::new(remote::SystemClock),
        Arc::clone(&llm),
    )));
    let browser_llm = Arc::clone(&llm);
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
        remote,
        sync: Arc::clone(&sync),
        llm,
        wiki,
        quiz: Arc::clone(&quiz),
        phone: phone_publisher.clone(),
    });
    tray::install(&handle)?;
    character::refresh_tray(&handle, &db);
    install_quit_gate(&handle);
    let notifier: Arc<dyn platform::Notifier> =
        Arc::new(platform::PluginNotifier::new(handle.clone()));
    timer::engine::spawn(
        handle.clone(),
        Arc::clone(&db),
        Arc::clone(&high_water),
        health,
        wakes,
        Arc::clone(&notifier),
        phone_publisher,
    );
    browser_link::spawn(
        Arc::clone(&db),
        Arc::clone(&high_water),
        browser_link,
        browser_llm,
        data_dir,
    );
    focus::spawn(Arc::clone(&db), Arc::clone(&high_water), focus_probe);
    // A shortcut that cannot be registered is reported in the status, not fatal: the in-app
    // button does the same job.
    signals.set_status(focus::signals::install_shortcut(
        &handle,
        Arc::clone(&db),
        Arc::clone(&high_water),
    ))?;
    focus::signals::spawn(
        handle.clone(),
        Arc::clone(&db),
        Arc::clone(&high_water),
        signals,
        Arc::clone(&notifier),
    );
    messages::service::spawn(
        handle.clone(),
        Arc::new(messages::service::MessageService::new(
            Arc::clone(&db),
            Arc::new(remote::SystemClock),
            Arc::clone(&notifier),
        )),
    );
    enforce::spawn(
        handle.clone(),
        Arc::clone(&db),
        high_water,
        process,
        notifier,
    );
    quiz::watch::spawn(handle.clone(), Arc::clone(&db), quiz);
    let notice_handle = handle.clone();
    rooms::service::spawn(rooms, move |notice| {
        if let Err(err) = tauri::Emitter::emit(&notice_handle, "rooms://distraction", notice) {
            tracing::error!(error = %err, "failed to emit a room notice");
        }
    });
    power::spawn(
        handle,
        db,
        session_id,
        events,
        engine,
        sync.presence.clone(),
    );
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
            tracing::info!(
                event = "exit_blocked",
                reason = "lock_held",
                source = reason.as_str()
            );
        }
        allowed
    });
    if let Err(err) = platform::install_quit_gate(gate) {
        tracing::warn!(error = %err, "native quit gate unavailable; only the ExitRequested paths are gated");
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

/// Windows relaunch supervision: true when the watchdog task started this process after a
/// deliberate quit, so it must exit before anything boots. Any other start re-arms the watchdog.
pub fn start_is_suppressed() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    platform::supervised_start_suppressed(browser_link::BUNDLE_IDENTIFIER, &args)
}

pub fn run() {
    // Logs carry only event kinds, timestamps, and counts: no window titles or URLs.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    // Single instance must be registered first: a second process exits here, before setup,
    // so it cannot run session recovery against the live first session.
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // The Windows watchdog task starts the app every minute; that is not a user action.
            if !platform::is_supervised_start(&args) {
                tray::show_main_window(app);
            }
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
            commands::plan::get_calendar_io_enabled,
            commands::plan::export_plans_ics,
            commands::plan::preview_ics_import,
            commands::plan::apply_ics_import,
            commands::plan::undo_ics_import,
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
            commands::rooms::rooms_list,
            commands::rooms::rooms_create,
            commands::rooms::rooms_join,
            commands::rooms::rooms_leave,
            commands::rooms::rooms_kick,
            commands::rooms::rooms_create_invite,
            commands::rooms::rooms_revoke_invite,
            commands::rooms::rooms_parse_invite,
            commands::rooms::rooms_open,
            commands::rooms::rooms_close,
            commands::rooms::rooms_feed,
            commands::rooms::rooms_friend_settings,
            commands::rooms::rooms_update_friend_setting,
            commands::rooms::rooms_share_schedule,
            commands::rooms::rooms_import_together,
            commands::remote::get_integration_states,
            commands::remote::set_integration_mode,
            commands::remote::get_remote_settings,
            commands::remote::update_remote_setting,
            commands::remote::save_secret,
            commands::remote::clear_secret,
            commands::remote::test_connection,
            commands::remote::list_remote_events,
            commands::remote::get_egress_summary,
            commands::remote::auth_request_otp,
            commands::remote::auth_verify_otp,
            commands::remote::auth_sign_out,
            commands::remote::get_sync_status,
            commands::remote::generate_ntfy_topic,
            commands::remote::get_ntfy_subscription,
            commands::remote::test_notification,
            commands::llm::get_llm_settings,
            commands::llm::update_llm_setting,
            commands::llm::get_llm_status,
            commands::phone::phone_pair_start,
            commands::phone::phone_pair_status,
            commands::phone::phone_pair_revoke,
            commands::phone::get_phone_link,
            commands::phone_install::get_phone_install,
            commands::phone_install::update_phone_apk_link,
            commands::wiki::list_wiki_subjects,
            commands::wiki::create_wiki_subject,
            commands::wiki::ensure_wiki_subject,
            commands::wiki::set_wiki_subject_archived,
            commands::wiki::wiki_subject_for_plan,
            commands::wiki::list_wiki_pages,
            commands::wiki::list_wiki_sources,
            commands::wiki::read_wiki_page,
            commands::wiki::reindex_wiki,
            commands::wiki::ingest_wiki_text,
            commands::wiki::ingest_wiki_file,
            commands::wiki::ingest_wiki_folder,
            commands::wiki::draft_wiki_outline,
            commands::wiki::resume_wiki_ingest,
            commands::wiki::cancel_wiki_ingest,
            commands::wiki::apply_wiki_ingest,
            commands::wiki::ask_wiki,
            commands::quiz::get_quiz_settings,
            commands::quiz::update_quiz_setting,
            commands::quiz::get_quiz_stats,
            commands::quiz::generate_quiz_bank,
            commands::quiz::flag_quiz_item,
            commands::quiz::start_review_quiz,
            commands::quiz::get_session_end_quiz,
            commands::quiz::get_checkin_quiz,
            commands::quiz::answer_quiz_card,
            commands::release::submit_release_quiz,
            commands::release::get_release_quiz_status,
            commands::wiki::preview_wiki_answer_page,
            commands::retro::submit_retro,
            commands::retro::retry_retro,
            commands::retro::apply_retro,
            commands::retro::cancel_retro,
            commands::retro::undo_retro,
            commands::retro::skip_retro,
            commands::retro::get_retro,
            commands::retro::list_open_retros,
            commands::retro::lint_wiki,
            commands::retro::get_retro_auto_apply,
            commands::retro::set_retro_auto_apply,
            commands::topic::get_today_topic,
            commands::topic::assign_next_topics,
            commands::topic::set_topic_assignment,
            commands::topic::list_topic_assignments,
            commands::topic::get_topic_history,
            commands::messages::get_message_settings,
            commands::messages::update_message_setting,
            commands::context::get_context_status,
            commands::context::update_context_setting,
            commands::context::delete_study_context,
            commands::chat::send_chat_message,
            commands::chat::apply_chat_draft,
            commands::chat::discard_chat_draft,
            commands::chat::get_chat_draft,
            commands::chat::open_chat_input,
            commands::growth::get_growth_status,
            commands::growth::get_weekly_insight,
            commands::growth::get_insight_subject,
            commands::growth::set_insight_subject,
            commands::character::get_character,
            commands::character::roll_character_candidates,
            commands::character::choose_character,
            commands::shared_wiki::shared_wiki_preview,
            commands::shared_wiki::shared_wiki_share,
            commands::shared_wiki::shared_wiki_sync,
            commands::shared_wiki::shared_wiki_withdraw,
            commands::shared_wiki::shared_wiki_room,
            commands::shared_wiki::shared_wiki_acknowledge,
            commands::shared_wiki::shared_wiki_merge,
            commands::shared_wiki::shared_wiki_quiz_start,
            commands::shared_wiki::shared_wiki_quiz_answer,
            commands::shop::get_shop,
            commands::shop::purchase_item,
            commands::shop::equip_item,
            commands::shop::unequip_slot,
            commands::profile::profile_home,
            commands::profile::profile_set_name,
            commands::profile::profile_set_scope,
            commands::profile::profile_set_wiki_titles,
            commands::profile::profile_set_web_base,
            commands::profile::profile_publish,
            commands::profile::profile_my_view,
            commands::profile::profile_member_view,
            commands::profile::profile_sign_guestbook,
            commands::profile::profile_delete_entry,
            commands::profile::profile_report_entry,
            commands::profile::gift_send,
            commands::profile::gift_overview,
            commands::profile::gift_accept,
            commands::profile::gift_decline,
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
                Ok(true) => platform::note_clean_exit(browser_link::BUNDLE_IDENTIFIER),
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
