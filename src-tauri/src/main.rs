#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Chrome starts the app binary as the native-messaging host; that must not boot the UI.
    if let Some(code) = study_manager_lib::run_native_host_if_invoked() {
        std::process::exit(code);
    }
    study_manager_lib::run();
}
