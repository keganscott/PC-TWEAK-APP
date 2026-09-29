#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(test)]
mod command_audit;
mod commands;

use std::sync::{Arc, Mutex};

use peaktweaks_engine::env::License;
use peaktweaks_engine::tweaks;
use peaktweaks_engine::Engine;

/// Build the engine. A normal build uses the real probes and the real restore
/// service, and the Free license until Phase 7 (licensing) exists, so nothing
/// in the catalogue can be applied yet. The `dev-stubs` feature swaps in an open
/// restore gate and an Ultimate license so the engine can be driven by hand.
#[cfg(not(feature = "dev-stubs"))]
fn start_engine() -> peaktweaks_engine::error::Result<Engine> {
    Engine::start_windows(tweaks::catalogue(), License::free())
}

#[cfg(feature = "dev-stubs")]
fn start_engine() -> peaktweaks_engine::error::Result<Engine> {
    use peaktweaks_engine::env::StubProbe;
    Engine::start_windows_with_probe(
        tweaks::catalogue(),
        Box::new(StubProbe::open_for_dev()),
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
            let engine = start_engine().map_err(|e| format!("engine failed to start: {e}"))?;
            tauri::Manager::manage(app, Arc::new(Mutex::new(engine)) as commands::SharedEngine);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::engine_context,
            commands::list_tweaks,
            commands::list_games,
            commands::select_target_game,
            commands::rescan,
            commands::audit_system,
            commands::create_restore_point,
            commands::apply_tweak,
            commands::revert_tweak,
            commands::revert_all,
            commands::list_journal,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PeakTweaks");
}
