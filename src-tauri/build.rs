/// Commands the webview may call. Restricting them here means a command that is
/// registered but not listed cannot be invoked, and each needs an explicit
/// `allow-<name>` permission in `capabilities/default.json`.
const COMMANDS: &[&str] = &[
    "engine_context",
    "list_tweaks",
    "list_games",
    "play_status",
    "get_settings",
    "set_settings",
    "select_target_game",
    "rescan",
    "audit_system",
    "create_restore_point",
    "proof_begin_session",
    "proof_capture",
    "proof_compare",
    "proof_list_sessions",
    "proof_runs",
    "apply_tweak",
    "revert_tweak",
    "revert_all",
    "list_journal",
    "purge_standby_memory",
    "cleanup_measure",
    "cleanup_run",
    "optimize_drive",
    "list_startup_apps",
    "list_msi_devices",
];

fn main() {
    // The app is a single elevated binary (`requireAdministrator`). The custom
    // manifest replaces Tauri's default one; it keeps the Common Controls v6
    // dependency. Do not also embed a manifest through a bundler config or a
    // second .rc file, or the linker reports a duplicate-resource error
    // (tauri-apps/tauri#6732, #10154).
    let windows = tauri_build::WindowsAttributes::new().app_manifest(include_str!("peaktweaks.manifest"));

    let attributes = tauri_build::Attributes::new()
        .windows_attributes(windows)
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));

    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
