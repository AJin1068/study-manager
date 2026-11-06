// Prevents an extra console window on Windows desktop dev builds; no effect on Android.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    study_phone_lib::run();
}
