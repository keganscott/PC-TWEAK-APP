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
    // What the border and title bar add, read off the window at its default size.
    let frame = match (window.outer_size(), window.inner_size()) {
        (Ok(outer), Ok(inner)) => (
            outer.width.saturating_sub(inner.width),
            outer.height.saturating_sub(inner.height),
        ),
        _ => (0, 0),
    };
    let saved = store.load().and_then(|p| p.placed_on(&screens, frame));
    if let Some(p) = saved {
        // Moved first, so the size is set at the scale of the screen it ends
        // up on: a size set before moving to a screen with another scale is
        // rescaled by Windows.
        let _ = window.set_position(PhysicalPosition::new(p.x, p.y));
        let _ = window.set_size(PhysicalSize::new(p.width, p.height));
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
            let maximized = shown(window).is_some_and(|(maximized, _)| maximized);
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
    let (maximized, minimized) = shown(window)?;
    if maximized || minimized {
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

/// Whether the window is maximised and whether it is minimised.
#[cfg(windows)]
fn shown<R: Runtime>(window: &Window<R>) -> Option<(bool, bool)> {
    let hwnd = window.hwnd().ok()?;
    Some(peaktweaks_engine::window_place::maximized_or_minimized(hwnd.0))
}

#[cfg(not(windows))]
fn shown<R: Runtime>(window: &Window<R>) -> Option<(bool, bool)> {
    Some((window.is_maximized().ok()?, window.is_minimized().ok()?))
}
