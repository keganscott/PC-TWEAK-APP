//! The icon by the clock while PeakTweaks is open (game plan new idea 12):
//! Gaming Mode on or off, and a way back to the window. Closing the window
//! still quits the app; the icon goes with it. Nothing here that cannot be
//! undone: Undo all stays in Backups, behind its confirmation.
//!
//! Gaming Mode is the same setting as the switch in Tools, saved through the
//! engine (`Engine::set_settings`, so turning it off mid-game puts its changes
//! back at once). After a change from here the window is told the new
//! settings (`engine://settings`); after a change from the window the tick
//! here follows (`sync`). Hovering the icon says which game is running and
//! whether Gaming Mode's changes are in effect (`show_play`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::menu::{CheckMenuItem, MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Wry};

use peaktweaks_engine::engine::Progress;
use peaktweaks_engine::env::KNOWN_GAMES;
use peaktweaks_engine::play::PlayStatus;

use crate::commands::SharedEngine;

const GAMING: &str = "gaming_mode";
const OPEN: &str = "open";

/// The menu's Gaming Mode item, kept so its tick can follow the setting,
/// and the number of the newest save it shows.
pub struct Tray {
    gaming: CheckMenuItem<Wry>,
    shown: Mutex<u64>,
}

/// Numbers saves in the order the engine made them. Taken while the engine
/// lock is held, so a save that finished later never shows before an
/// earlier one when their threads race to the tick.
static SAVES: AtomicU64 = AtomicU64::new(0);

/// The next save's number; call with the engine lock held.
pub fn next_seq() -> u64 {
    SAVES.fetch_add(1, Ordering::SeqCst) + 1
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
    app.manage(Tray {
        gaming,
        shown: Mutex::new(0),
    });
    Ok(())
}

/// Make the tick match save number `seq`, unless a later save already
/// shows. `also` runs in the same turn (the tray's own saves tell the window).
fn show_save(app: &AppHandle, gaming_mode: bool, seq: u64, also: impl FnOnce()) {
    let Some(tray) = app.try_state::<Tray>() else {
        also();
        return;
    };
    let mut shown = tray.shown.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if seq < *shown {
        return;
    }
    *shown = seq;
    let _ = tray.gaming.set_checked(gaming_mode);
    also();
}

/// Make the tick match the saved setting (a save from the window).
pub fn sync(app: &AppHandle, gaming_mode: bool, seq: u64) {
    show_save(app, gaming_mode, seq, || {});
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
        // either way, and Backups lists what is still in effect. The window's
        // activity log says so.
        let failed = engine.set_settings(wanted).err();
        let saved = engine.settings();
        let seq = next_seq();
        drop(engine);
        if let Some(e) = failed {
            let _ = app.emit(
                "engine://progress",
                Progress {
                    stage: "gaming mode".into(),
                    tweak_id: None,
                    message: format!("Gaming Mode from the tray: {e}. Backups lists what is still in effect."),
                },
            );
        }
        show_save(&app, saved.gaming_mode, seq, || {
            let _ = app.emit("engine://settings", saved);
        });
    });
}

/// Say on the icon what the play watcher sees now.
pub fn show_play(app: &AppHandle, status: &PlayStatus) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tooltip(status)));
    }
}

/// The icon's hover text: the game running and Gaming Mode's state, short
/// enough for Windows (it cuts tooltips at 127 characters).
fn tooltip(status: &PlayStatus) -> String {
    let Some(id) = status.game.as_deref() else {
        return "PeakTweaks".into();
    };
    let game = KNOWN_GAMES.iter().find(|g| g.id == id).map_or(id, |g| g.name);
    let state = if status.problem.is_some() {
        "something you turned on is not in effect (open PeakTweaks to see why)"
    } else if status.gaming_mode_active {
        "Gaming Mode in effect"
    } else {
        "Gaming Mode off"
    };
    format!("PeakTweaks: {game} running, {state}")
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(game: Option<&str>, active: bool, problem: Option<&str>) -> PlayStatus {
        PlayStatus {
            game: game.map(str::to_owned),
            gaming_mode_active: active,
            problem: problem.map(str::to_owned),
            ..PlayStatus::watching()
        }
    }

    #[test]
    fn tooltip_names_the_game_and_gaming_mode() {
        let fortnite = KNOWN_GAMES[0];
        assert_eq!(tooltip(&playing(None, false, None)), "PeakTweaks");
        assert_eq!(
            tooltip(&playing(Some(fortnite.id), true, None)),
            format!("PeakTweaks: {} running, Gaming Mode in effect", fortnite.name)
        );
        assert_eq!(
            tooltip(&playing(Some(fortnite.id), false, None)),
            format!("PeakTweaks: {} running, Gaming Mode off", fortnite.name)
        );
        let problem = tooltip(&playing(
            Some(fortnite.id),
            true,
            Some("Gaming Mode is not fully on: x"),
        ));
        assert!(problem.contains("not in effect"), "{problem}");
        for g in KNOWN_GAMES {
            let longest = tooltip(&playing(Some(g.id), true, Some("x")));
            assert!(longest.chars().count() <= 127, "{longest}");
        }
    }
}
