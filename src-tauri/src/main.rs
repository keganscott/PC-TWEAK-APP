#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod tweaks;

use std::sync::Mutex;

use tauri::Manager;

use engine::types::SystemEnv;
use engine::Engine;

/// True when the process token carries the Administrators group with the
/// enabled attribute. The manifest requests `requireAdministrator`, so this
/// should always be true — we check anyway, because a manifest can be stripped
/// and a mutating engine that assumes its own privilege is a bad engine.
fn is_elevated() -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut core::ffi::c_void),
            size,
            &mut size,
        )
        .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data)?;

            let elevated = is_elevated();
            let engine = Engine::new(&app_data, elevated, tweaks::catalogue())
                .map_err(|e| format!("engine failed to start: {e}"))?;

            // Hardware probes land in Phase 3. Until then predicates run against
            // a mostly-empty environment, which means they allow by default —
            // apart from the restore-point gate, which reads false and so blocks
            // every mutation. That is the correct failure direction.
            let mut engine = engine;
            engine.set_env(SystemEnv { elevated, ..Default::default() });

            app.manage(Mutex::new(engine));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine::engine_context,
            engine::list_tweaks,
            engine::set_environment,
            engine::apply_tweak,
            engine::revert_tweak,
            engine::revert_all,
            engine::list_journal,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PeakTweaks");
}
