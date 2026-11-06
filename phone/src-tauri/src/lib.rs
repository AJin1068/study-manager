//! Study Manager phone app (stage 5): a Tauri 2 mobile shell. The logic is in `study-phone-core`
//! (host-tested) and `app` (state and persistence, host-tested); this file only wires Tauri:
//! the commands the screens call, the Kotlin blocker plugin and the poll thread.
//!
//! Not built for Android here (no SDK/NDK): V5-1, V5-4. The poll thread lives in the app process,
//! so Android can stop it when it kills the process; the Kotlin plugin keeps an already engaged
//! lock until its own deadline either way. A foreground service that keeps this process alive is
//! NOT written (V5-5).

mod app;
mod bridge;
mod clock;
mod net;

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tauri::{Emitter, Manager, State};

use app::{Ack, PhoneApp, PhoneView, ScheduleView};
use bridge::PluginSlot;
use study_phone_core::poller::POLL_INTERVAL_MS;

const VIEW_EVENT: &str = "phone://view";

#[tauri::command]
fn phone_view(app: State<'_, Arc<PhoneApp>>) -> PhoneView {
    app.view()
}

#[tauri::command]
async fn phone_poll_now(app: State<'_, Arc<PhoneApp>>) -> Result<PhoneView, String> {
    let app = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || app.tick()).await.map_err(|_| "poll_failed".to_string())
}

/// `text` is what the QR scanner read; it is parsed as a pairing offer, nothing else.
#[tauri::command]
async fn phone_pair(app: State<'_, Arc<PhoneApp>>, text: String) -> Result<Ack, String> {
    let app = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || app.pair(&text)).await.map_err(|_| "pair_failed".to_string())
}

#[tauri::command]
async fn phone_unpair(app: State<'_, Arc<PhoneApp>>) -> Result<Ack, String> {
    let app = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || app.unpair()).await.map_err(|_| "unpair_failed".to_string())
}

/// Forwarded by the webview from the plugin event channel (`addPluginListener('blocker', ...)`).
#[tauri::command]
fn phone_blocker_event(app: State<'_, Arc<PhoneApp>>, event: Value) -> Ack {
    app.blocker_event(&event)
}

#[tauri::command]
fn phone_get_allow_list(app: State<'_, Arc<PhoneApp>>) -> Value {
    app.allow_list()
}

#[tauri::command]
fn phone_set_allow_list(app: State<'_, Arc<PhoneApp>>, wire: Value) -> Ack {
    app.set_allow_list(&wire)
}

#[tauri::command]
fn phone_schedule(app: State<'_, Arc<PhoneApp>>) -> ScheduleView {
    app.schedule()
}

/// The Kotlin plugin (`phone/plugins/blocker/android/`). Only registered on Android.
fn blocker_plugin(slot: PluginSlot) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("blocker")
        .setup(move |_app, _api| {
            #[cfg(target_os = "android")]
            slot.set(_api.register_android_plugin("app.studymanager.blocker", "BlockerPlugin")?);
            #[cfg(not(target_os = "android"))]
            let _ = &slot;
            Ok(())
        })
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let slot = PluginSlot::default();
    let builder = tauri::Builder::default().plugin(blocker_plugin(slot.clone()));
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    let result = builder
        .setup(move |tauri_app| {
            let dir = tauri_app.path().app_local_data_dir()?;
            let phone = Arc::new(PhoneApp::new(dir, Arc::new(net::UreqTransport::new()), Arc::new(clock::BootClock::new()), Arc::new(slot)));
            tauri_app.manage(phone.clone());
            let handle = tauri_app.handle().clone();
            std::thread::spawn(move || {
                loop {
                    let view = phone.tick();
                    let _ = handle.emit(VIEW_EVENT, &view);
                    std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            phone_view,
            phone_poll_now,
            phone_pair,
            phone_unpair,
            phone_blocker_event,
            phone_get_allow_list,
            phone_set_allow_list,
            phone_schedule
        ])
        .run(tauri::generate_context!());
    if let Err(err) = result {
        eprintln!("phone app failed to start: {err}");
    }
}
