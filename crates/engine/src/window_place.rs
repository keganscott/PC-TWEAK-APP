//! Where the app's window was when it was last closed: its size and place
//! while not maximised, and whether it was maximised, so the next start opens
//! it the same way. Stored as `window.json` next to the settings, in the
//! protected data directory.
//!
//! Only a place that can still be reached is used: one whose title bar falls
//! off every screen now (a monitor unplugged, the resolution lowered) gives
//! Windows' default, as does a missing or unreadable file. A window larger
//! than its screen is now is shrunk and moved to fit on it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{EngineError, Result};
use super::fsutil;
use super::secure_dir::TrustedDir;

const FILE: &str = "window.json";

/// The smallest window the app allows (`tauri.conf.json`), and the largest
/// size believed: past it the file is not ours to trust.
pub const MIN_WIDTH: u32 = 1024;
pub const MIN_HEIGHT: u32 = 640;
const MAX_SIDE: u32 = 16_384;
/// How much of the window's top must be on a screen for it to be reachable:
/// enough of the title bar to grab.
const GRAB: i64 = 100;
/// Windows draws invisible resize borders around a window, so one snapped to
/// a screen's edge sits a few pixels past it.
const BORDER: i64 = 16;

/// In physical pixels, as Windows places windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPlace {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

/// A screen's work area, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl WindowPlace {
    /// A size the app allows.
    pub fn is_sane(&self) -> bool {
        (MIN_WIDTH..=MAX_SIDE).contains(&self.width) && (MIN_HEIGHT..=MAX_SIDE).contains(&self.height)
    }

    /// The place to open at on these screens, or `None` when not enough of
    /// the title bar is on any of them to grab it, so the window is never
    /// restored somewhere it cannot be reached. `frame` is what the window's
    /// border and title bar add to its inside size (`width`, `height`); a
    /// window larger than its screen's work area is shrunk to it and moved
    /// onto it, so every edge can be reached.
    pub fn placed_on(&self, screens: &[Screen], frame: (u32, u32)) -> Option<WindowPlace> {
        let (left, top) = (i64::from(self.x), i64::from(self.y));
        let right = left + i64::from(self.width) + i64::from(frame.0);
        let screen = screens.iter().find(|s| {
            let (sl, st) = (i64::from(s.x), i64::from(s.y));
            let (sr, sb) = (sl + i64::from(s.width), st + i64::from(s.height));
            let overlap = right.min(sr) - left.max(sl);
            overlap >= GRAB && top >= st - BORDER && top + GRAB <= sb
        })?;
        let width = self.width.min(room(screen.width, frame.0));
        let height = self.height.min(room(screen.height, frame.1));
        Some(WindowPlace {
            x: fit_axis(self.x, width + frame.0, screen.x, screen.width),
            y: fit_axis(self.y, height + frame.1, screen.y, screen.height),
            width,
            height,
            ..*self
        })
    }
}

/// The inside size that fits in a screen's length once the frame is added.
fn room(screen: u32, frame: u32) -> u32 {
    screen.saturating_sub(frame).max(1)
}

/// Moves a window's start back so its far edge is on the screen (allowing for
/// Windows' invisible border), never past the screen's near edge.
fn fit_axis(start: i32, outer: u32, screen_start: i32, screen_len: u32) -> i32 {
    let screen_end = i64::from(screen_start) + i64::from(screen_len);
    let start = i64::from(start);
    if start + i64::from(outer) <= screen_end + BORDER {
        return start as i32;
    }
    let moved = (screen_end - i64::from(outer)).max(i64::from(screen_start));
    moved.min(start) as i32
}

/// Whether Windows shows the window maximised or minimised. Read from Windows
/// itself: the window library's own flag is set only after it reports the
/// move that maximising makes, so that move would be taken for a normal place.
#[cfg(windows)]
pub fn maximized_or_minimized(hwnd: *mut std::ffi::c_void) -> (bool, bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsZoomed};
    let hwnd = HWND(hwnd);
    // SAFETY: both only read the window's style; an invalid handle reads false.
    unsafe { (IsZoomed(hwnd).as_bool(), IsIconic(hwnd).as_bool()) }
}

/// Where the window's place lives. `None` (tests, dev) keeps nothing.
#[derive(Debug, Clone, Default)]
pub struct WindowPlaceStore {
    path: Option<PathBuf>,
}

impl WindowPlaceStore {
    pub fn in_memory() -> Self {
        Self { path: None }
    }

    /// Next to the settings, in a directory a standard user could not have
    /// pre-created.
    pub fn in_dir(dir: &TrustedDir) -> Self {
        Self {
            path: Some(dir.path().join(FILE)),
        }
    }

    /// The last saved place, if there is one and its size is one the app allows.
    pub fn load(&self) -> Option<WindowPlace> {
        let text = std::fs::read_to_string(self.path.as_ref()?).ok()?;
        serde_json::from_str::<WindowPlace>(&text)
            .ok()
            .filter(WindowPlace::is_sane)
    }

    /// Replace the saved place the same crash-safe way as the settings.
    pub fn save(&self, place: &WindowPlace) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(place).map_err(|e| EngineError::Internal {
            detail: format!("could not encode the window's place: {e}"),
        })?;
        let tmp = path.with_extension("json.tmp");
        fsutil::write_durable(&tmp, &bytes)?;
        std::fs::rename(&tmp, path).map_err(|e| EngineError::storage(path.display().to_string(), e))?;
        fsutil::sync_dir(path.parent().unwrap_or_else(|| Path::new(".")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLACE: WindowPlace = WindowPlace {
        x: 200,
        y: 100,
        width: 1400,
        height: 900,
        maximized: true,
    };
    const MAIN: Screen = Screen {
        x: 0,
        y: 0,
        width: 2560,
        height: 1400,
    };

    #[test]
    fn a_place_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = WindowPlaceStore::in_dir(&TrustedDir::insecure_for_tests(dir.path()));
        assert_eq!(store.load(), None, "nothing saved yet");
        store.save(&PLACE).unwrap();
        assert_eq!(store.load(), Some(PLACE));
        assert!(!dir.path().join("window.json.tmp").exists());

        std::fs::write(dir.path().join(FILE), "not json").unwrap();
        assert_eq!(store.load(), None, "unreadable: the default");
        let tiny = WindowPlace { width: 10, ..PLACE };
        store.save(&tiny).unwrap();
        assert_eq!(store.load(), None, "smaller than the app allows");
        let huge = WindowPlace {
            height: 100_000,
            ..PLACE
        };
        store.save(&huge).unwrap();
        assert_eq!(store.load(), None, "larger than any screen");
        assert_eq!(WindowPlaceStore::in_memory().load(), None);
    }

    const FRAME: (u32, u32) = (16, 39);

    fn reachable(place: WindowPlace, screens: &[Screen]) -> bool {
        place.placed_on(screens, FRAME).is_some()
    }

    #[test]
    fn a_place_is_used_only_while_its_title_bar_is_on_a_screen() {
        assert_eq!(PLACE.placed_on(&[MAIN], FRAME), Some(PLACE), "fits as it is");
        // A second screen to the left, at negative coordinates.
        let left = Screen {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1040,
        };
        let there = WindowPlace {
            x: -1700,
            y: 50,
            ..PLACE
        };
        assert_eq!(there.placed_on(&[left, MAIN], FRAME), Some(there));
        assert!(!reachable(there, &[MAIN]), "that screen was unplugged");
        // Mostly off the right edge: only a sliver of the title bar shows.
        assert!(!reachable(WindowPlace { x: 2500, ..PLACE }, &[MAIN]));
        // Above the top, or with the title bar under the bottom edge.
        assert!(!reachable(WindowPlace { y: -40, ..PLACE }, &[MAIN]));
        // Snapped to the left half: its invisible border is off the screen.
        let snapped = WindowPlace { x: -7, y: -7, ..PLACE };
        assert_eq!(snapped.placed_on(&[MAIN], FRAME), Some(snapped));
        assert!(!reachable(WindowPlace { y: 1350, ..PLACE }, &[MAIN]));
        assert!(!reachable(PLACE, &[]), "no screens known");
    }

    #[test]
    fn a_window_larger_than_its_screen_now_is_shrunk_onto_it() {
        // Saved on a 2560x1400 screen; the resolution is now 1366x728.
        let small = Screen {
            x: 0,
            y: 0,
            width: 1366,
            height: 728,
        };
        let big = WindowPlace {
            x: 0,
            y: 0,
            width: 2500,
            height: 1360,
            maximized: false,
        };
        let placed = big.placed_on(&[small], FRAME).unwrap();
        assert_eq!((placed.width, placed.height), (1366 - 16, 728 - 39));
        assert_eq!((placed.x, placed.y), (0, 0));
        // Partly past the right and bottom edges: moved back so all of it shows.
        let late = WindowPlace {
            x: 800,
            y: 300,
            width: 1100,
            height: 650,
            ..big
        };
        let placed = late.placed_on(&[small], FRAME).unwrap();
        assert_eq!((placed.width, placed.height), (1100, 650), "small enough already");
        assert_eq!((placed.x, placed.y), (1366 - 1100 - 16, 728 - 650 - 39));
        assert!(!placed.maximized);
        // On a screen to the left, it stays on that screen.
        let left = Screen {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1040,
        };
        let there = WindowPlace {
            x: -1900,
            width: 2400,
            ..late
        };
        let placed = there.placed_on(&[left, MAIN], FRAME).unwrap();
        assert_eq!((placed.x, placed.width), (-1920, 1920 - 16));
    }
}
