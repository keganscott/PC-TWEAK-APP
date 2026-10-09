//! Opens the window where it was last closed: its size and place, and
//! maximised if it was (engine `window_place`). The window starts hidden
//! (`tauri.conf.json`) and is shown once placed, so it never jumps.

use std::sync::Mutex;

use peaktweaks_engine::window_place::{Screen, WindowPlace, WindowPlaceStore};
use tauri::{Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow, Window, WindowEvent};

/// The window's size and place the last time it was neither maximised nor
/// minimised, which is what a maximised window goes back to.
pub struct Tracker {
    store: WindowPlaceStore,
    last: Mutex<Option<WindowPlace>>,
}

/// Put the window where it was, if that is still on a screen, and show it.
pub fn restore(window: &WebviewWindow, store: WindowPlaceStore) -> Tracker {
    let screens: Vec<Screen> = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let area = m.work_area();
            Screen {
                x: area.position.x,
                y: area.position.y,
                width: area.size.width,
                height: area.size.height,
            }
        })
        .collect();
    let saved = store.load().filter(|p| p.fits(&screens));
    if let Some(p) = saved {
        let _ = window.set_size(PhysicalSize::new(p.width, p.height));
        let _ = window.set_position(PhysicalPosition::new(p.x, p.y));
        if p.maximized {
            let _ = window.maximize();
        }
    }
    let _ = window.show();
    Tracker {
        store,
        last: Mutex::new(saved),
    }
}

/// Keeps the window's place as it moves and saves it when it closes.
pub fn on_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != "main" {
        return;
    }
    let Some(tracker) = window.try_state::<Tracker>() else {
        return;
    };
    let Ok(mut last) = tracker.last.lock() else {
        return;
    };
    match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
            if let Some(place) = normal_place(window) {
                *last = Some(place);
            }
        }
        WindowEvent::CloseRequested { .. } => {
            let maximized = window.is_maximized().unwrap_or(false);
            if let Some(place) = normal_place(window).or(*last) {
                // Best effort: a place not saved only means the default next time.
                let _ = tracker.store.save(&WindowPlace { maximized, ..place });
            }
        }
        _ => {}
    }
}

/// Where the window is, when it is shown at its own size.
fn normal_place<R: Runtime>(window: &Window<R>) -> Option<WindowPlace> {
    if window.is_maximized().ok()? || window.is_minimized().ok()? {
        return None;
    }
    let position = window.outer_position().ok()?;
    let size = window.inner_size().ok()?;
    Some(WindowPlace {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        maximized: false,
    })
}
