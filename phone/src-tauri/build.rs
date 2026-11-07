fn main() {
    // The blocker plugin is an inlined app plugin: its commands are only the two the webview
    // needs to subscribe to plugin events (`addPluginListener`). Rust calls the other commands
    // (`engage`, `release`, `status`) through the plugin handle, not through the webview.
    let blocker = tauri_build::InlinedPlugin::new()
        .commands(&["registerListener", "removeListener"])
        .default_permission(tauri_build::DefaultPermissionRule::AllowAllCommands);
    if let Err(err) =
        tauri_build::try_build(tauri_build::Attributes::new().plugin("blocker", blocker))
    {
        panic!("tauri build failed: {err:#}");
    }
}
