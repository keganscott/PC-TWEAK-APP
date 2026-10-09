//! Where the app's window was when it was last closed: its size and place
//! while not maximised, and whether it was maximised, so the next start opens
//! it the same way. Stored as `window.json` next to the settings, in the
//! protected data directory.
//!
//! Only a place that still fits is used: one that falls off every screen now
//! (a monitor unplugged, the resolution lowered) gives Windows' default, as
//! does a missing or unreadable file.

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

    /// True when enough of the window's title bar is on one of the screens to
    /// grab it, so it is never restored somewhere it cannot be reached.
    pub fn fits(&self, screens: &[Screen]) -> bool {
        let (left, top) = (i64::from(self.x), i64::from(self.y));
        let right = left + i64::from(self.width);
        screens.iter().any(|s| {
            let (sl, st) = (i64::from(s.x), i64::from(s.y));
            let (sr, sb) = (sl + i64::from(s.width), st + i64::from(s.height));
            let overlap = right.min(sr) - left.max(sl);
            overlap >= GRAB && top >= st - BORDER && top + GRAB <= sb
        })
    }
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

    #[test]
    fn a_place_is_used_only_while_its_title_bar_is_on_a_screen() {
        assert!(PLACE.fits(&[MAIN]));
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
        assert!(there.fits(&[left, MAIN]));
        assert!(!there.fits(&[MAIN]), "that screen was unplugged");
        // Mostly off the right edge: only a sliver of the title bar shows.
        let edge = WindowPlace { x: 2500, ..PLACE };
        assert!(!edge.fits(&[MAIN]));
        // Above the top, or with the title bar under the bottom edge.
        assert!(!WindowPlace { y: -40, ..PLACE }.fits(&[MAIN]));
        // Snapped to the left half: its invisible border is off the screen.
        assert!(WindowPlace { x: -7, y: -7, ..PLACE }.fits(&[MAIN]));
        assert!(!WindowPlace { y: 1350, ..PLACE }.fits(&[MAIN]));
        assert!(!PLACE.fits(&[]), "no screens known");
    }
}
