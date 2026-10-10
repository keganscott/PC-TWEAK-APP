//! The icon by the clock while PeakTweaks is open (game plan new idea 12):
//! Gaming Mode on or off, and a way back to the window. Closing the window
//! still quits the app; the icon goes with it. Nothing here that cannot be
//! undone: Undo all stays in Backups, behind its confirmation.
//!
//! Gaming Mode is the same setting as the switch in Tools, saved through the
//! engine (`Engine::set_settings`, so turning it off mid-game puts its changes
//! back at once). After a change from here the window is told the new
//! settings (`engine://settings`); after a change from the window the tick
//! here follows (`sync`).

use tauri::menu::{CheckMenuItem, MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::commands::SharedEngine;

const GAMING: &str = "gaming_mode";
const OPEN: &str = "open";

/// The menu's Gaming Mode item, kept so its tick can follow the setting.
pub struct Tray {
    gaming: CheckMenuItem<Wry>,
}

/// Put the icon by the clock. Without an engine Gaming Mode is greyed out,
/// and the icon still brings the window back with its reason.
pub fn build(app: &tauri::App, engine: Option<SharedEngine>) -> tauri::Result<()> {
    let on = engine
        .as_ref()
        .and_then(|e| e.lock().ok().map(|e| e.settings().gaming_mode))
        .unwrap_or(false);
    let gaming = CheckMenuItem::with_id(app, GAMING, "Gaming Mode", engine.is_some(), on, None::<&str>)?;
    let open = MenuItem::with_id(app, OPEN, "Open PeakTweaks", true, None::<&str>)?;
    let menu = MenuBuilder::new(app).item(&gaming).separator().item(&open).build()?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("PeakTweaks")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            GAMING => {
                if let Some(engine) = engine.clone() {
                    toggle_gaming_mode(app.clone(), engine);
                }
            }
            OPEN => show_window(app),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.manage(Tray { gaming });
    Ok(())
}

/// Make the tick match the saved setting.
pub fn sync(app: &AppHandle, gaming_mode: bool) {
    if let Some(tray) = app.try_state::<Tray>() {
        let _ = tray.gaming.set_checked(gaming_mode);
    }
}

/// Flip Gaming Mode on a worker thread, so the engine lock is never waited
/// for on the thread that runs the window. Whatever happens, the tick and the
/// window then show what is saved.
fn toggle_gaming_mode(app: AppHandle, engine: SharedEngine) {
    std::thread::spawn(move || {
        let Ok(mut engine) = engine.lock() else {
            // Windows has already flipped the tick; nothing was saved, so
            // flip it back.
            if let Some(tray) = app.try_state::<Tray>() {
                if let Ok(ticked) = tray.gaming.is_checked() {
                    let _ = tray.gaming.set_checked(!ticked);
                }
            }
            return;
        };
        let mut wanted = engine.settings();
        wanted.gaming_mode = !wanted.gaming_mode;
        // Turning it off can fail to put a change back; the setting is saved
        // either way, and Backups lists what is still in effect.
        if let Err(e) = engine.set_settings(wanted) {
            eprintln!("Gaming Mode from the tray: {e}");
        }
        let saved = engine.settings();
        drop(engine);
        sync(&app, saved.gaming_mode);
        let _ = app.emit("engine://settings", saved);
    });
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
