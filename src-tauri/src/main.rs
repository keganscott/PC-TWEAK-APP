#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use std::sync::{Arc, Mutex};

use peaktweaks_engine::env::{License, StubProbe};
use peaktweaks_engine::tweaks;
use peaktweaks_engine::Engine;

/// Probe and license for this build. Until Phase 3 (real probes and restore
/// points) and Phase 7 (license tokens) exist, production builds are a closed
/// restore gate and the Free tier: nothing can be applied. The `dev-stubs`
/// feature opens both so the engine can be exercised by hand.
#[cfg(not(feature = "dev-stubs"))]
fn probe_and_license() -> (StubProbe, License) {
    (StubProbe::closed(), License::free())
}

#[cfg(feature = "dev-stubs")]
fn probe_and_license() -> (StubProbe, License) {
    (
        StubProbe::open_for_dev(),
        License::dev(peaktweaks_engine::types::Tier::Ultimate),
    )
}

/// Under Administrator Protection the elevated process is a different, hidden
/// user and WebView2 cannot start in that user's profile. Point it at a folder
/// in the interactive user's profile before the webview is created.
fn prepare_webview2_data_dir() {
    if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_some() {
        return; // an explicit choice wins
    }
    if let Some(dir) = peaktweaks_engine::identity::webview2_user_data_dir() {
        if std::fs::create_dir_all(&dir).is_ok() {
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", dir);
        }
    }
}

fn main() {
    prepare_webview2_data_dir();
    tauri::Builder::default()
        .setup(|app| {
            let (probe, license) = probe_and_license();
            let engine = Engine::start_windows(tweaks::catalogue(), Box::new(probe), license)
                .map_err(|e| format!("engine failed to start: {e}"))?;
            tauri::Manager::manage(app, Arc::new(Mutex::new(engine)) as commands::SharedEngine);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::engine_context,
            commands::list_tweaks,
            commands::list_games,
            commands::select_target_game,
            commands::rescan,
            commands::apply_tweak,
            commands::revert_tweak,
            commands::revert_all,
            commands::list_journal,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PeakTweaks");
}
