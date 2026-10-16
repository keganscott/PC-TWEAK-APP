#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(test)]
mod command_audit;
mod commands;
mod play;
mod tray;
mod window_place;

use peaktweaks_engine::env::License;
use peaktweaks_engine::tweaks;
use peaktweaks_engine::Engine;

/// Build the engine. A normal build uses the real probes and the real restore
/// service, and the Free license until Phase 7 (licensing) exists, so nothing
/// in the catalogue can be applied yet. The `tester` feature keeps all of that
/// but unlocks every plan, for trying the app on a real PC. The `dev-stubs`
/// feature swaps in an open restore gate and an Ultimate license so the engine
/// can be driven by hand on a machine without System Restore.
#[cfg(not(feature = "dev-stubs"))]
fn start_engine() -> peaktweaks_engine::error::Result<Engine> {
    #[cfg(feature = "tester")]
    let license = License::tester();
    #[cfg(not(feature = "tester"))]
    let license = License::free();
    Engine::start_windows(tweaks::catalogue(), license)
}

// A tester build must keep the real restore gate; dev-stubs opens it.
#[cfg(all(feature = "tester", feature = "dev-stubs"))]
compile_error!("the tester and dev-stubs features cannot be combined");

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

/// Writes why the engine did not start to `%LOCALAPPDATA%\PeakTweaks\startup-error.log`
/// (newest last). Best effort: the UI shows the same reason.
fn record_startup_failure(error: &peaktweaks_engine::error::EngineError) {
    use std::io::Write;
    let Some(base) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let dir = std::path::PathBuf::from(base).join("PeakTweaks");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("startup-error.log"))
    {
        let _ = writeln!(f, "{secs} engine failed to start: {error} ({error:?})");
    }
}

fn main() {
    prepare_webview2_data_dir();
    tauri::Builder::default()
        .setup(|app| {
            let handle = match start_engine() {
                Ok(engine) => commands::EngineHandle::ready(engine),
                Err(e) => {
                    // Keep the window: the UI shows this reason on its start-up
                    // screen. Also leave it on disk, for support and for when the
                    // window itself cannot render.
                    record_startup_failure(&e);
                    commands::EngineHandle::failed(e)
                }
            };
            let status = play::new_status();
            // Placed where it was last closed, then shown. Without an engine
            // nothing is remembered, but the window still shows its reason.
            let store = handle
                .shared()
                .and_then(|e| e.lock().ok().map(|e| e.window_store()))
                .unwrap_or_default();
            if let Some(window) = tauri::Manager::get_webview_window(app, "main") {
                tauri::Manager::manage(app, window_place::restore(&window, store));
            }
            // The icon by the clock (Gaming Mode, Open). A tray that cannot
            // be made leaves the app as it was without one.
            if let Err(e) = tray::build(app, handle.shared()) {
                eprintln!("tray icon not shown: {e}");
            }
            // Started after the icon exists, so the icon hears of the first
            // game it sees.
            if let Some(engine) = handle.shared() {
                play::start(app.handle().clone(), engine, status.clone());
            }
            tauri::Manager::manage(app, handle);
            tauri::Manager::manage(app, status);
            Ok(())
        })
        .on_window_event(window_place::on_event)
        .invoke_handler(tauri::generate_handler![
            commands::engine_context,
            commands::list_tweaks,
            commands::list_games,
            commands::live_readings,
            commands::play_status,
            commands::forget_play_history,
            commands::get_settings,
            commands::set_settings,
            commands::select_target_game,
            commands::rescan,
            commands::audit_system,
            commands::create_restore_point,
            commands::proof_begin_session,
            commands::proof_capture,
            commands::proof_compare,
            commands::proof_list_sessions,
            commands::proof_runs,
            commands::proof_auto_record,
            commands::proof_stop_auto_record,
            commands::apply_tweak,
            commands::revert_tweak,
            commands::revert_all,
            commands::list_journal,
            commands::purge_standby_memory,
            commands::cleanup_measure,
            commands::cleanup_run,
            commands::optimize_drive,
            commands::list_startup_apps,
            commands::list_msi_devices,
            commands::launch_game,
            commands::check_connection,
            commands::open_driver_page,
            commands::install_gpu_driver,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PeakTweaks");
}
