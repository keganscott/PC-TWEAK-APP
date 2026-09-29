fn main() {
    // The app is a single elevated binary (`requireAdministrator`). The custom
    // manifest replaces Tauri's default one; it keeps the Common Controls v6
    // dependency. Do not also embed a manifest through a bundler config or a
    // second .rc file, or the linker reports a duplicate-resource error
    // (tauri-apps/tauri#6732, #10154).
    let windows = tauri_build::WindowsAttributes::new().app_manifest(include_str!("peaktweaks.manifest"));

    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
}
