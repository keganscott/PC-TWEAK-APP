//! The few choices a user makes that are not tweaks: a rig-class override
//! (plan 6.1: "User can override"), the plain/technical wording (plan
//! section 7), whether the first-run welcome was seen, the two "while you
//! play" switches (Gaming Mode, game timer) and the gentle reminders on Home
//! (on or off, and how long each "Not now" lasts), and which restart's check
//! was already seen. Stored as `settings.json`
//! in the protected data directory.
//!
//! Settings are preferences only. They change defaults, copy and what happens
//! while a game runs (each change still under the usual rules); they never
//! open a gate, change a licence or hide a safety check, and nothing in here is
//! read by a predicate. A missing, unreadable or corrupt file gives the
//! defaults; the corrupt file is left in place until the next save replaces it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::{EngineError, Result};
use super::fsutil;
use super::hardware::RigClass;
use super::secure_dir::TrustedDir;

const FILE: &str = "settings.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    /// Plain words, no registry paths. The default.
    #[default]
    Plain,
    /// Registry paths and technical names visible.
    Technical,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// When set, replaces the detected rig class for defaults and copy.
    pub rig_class_override: Option<RigClass>,
    pub language: Language,
    /// The first-run welcome was shown and closed. Missing in older files,
    /// which then show it once.
    pub welcome_seen: bool,
    /// Gaming Mode: notifications off and search indexing paused while a known
    /// game runs (`tweaks::session`). Off unless the user turns it on.
    pub gaming_mode: bool,
    /// Ask Windows for its finest timer while a known game runs (`play.rs`).
    /// Off unless the user turns it on.
    pub game_timer: bool,
    /// The gentle reminders on Home (junk cleanup due, an old graphics
    /// driver) are turned off. On unless the user turns them off.
    pub reminders_off: bool,
    /// "Not now" on the junk cleanup reminder: not shown again before this
    /// time (Unix ms).
    pub cleanup_reminder_snoozed_until: Option<u64>,
    /// "Not now" on the graphics driver reminder: not shown again before
    /// this time (Unix ms).
    pub driver_reminder_snoozed_until: Option<u64>,
    /// The Windows start (Unix ms, `boot.rs`) whose check after a restart
    /// was closed on Home, so it is shown once per restart.
    pub restart_check_seen_boot: Option<u64>,
}

/// Where settings live. `None` (tests, dev) keeps them in memory only.
#[derive(Debug, Clone, Default)]
pub struct SettingsStore {
    path: Option<PathBuf>,
}

impl SettingsStore {
    pub fn in_memory() -> Self {
        Self { path: None }
    }

    /// The directory is a `TrustedDir`, so the file is not in a place a standard
    /// user could have pre-created.
    pub fn in_dir(dir: &TrustedDir) -> Self {
        Self {
            path: Some(dir.path().join(FILE)),
        }
    }

    pub fn load(&self) -> Settings {
        let Some(path) = &self.path else {
            return Settings::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Replace the stored settings: write a temporary file, make it durable, then
    /// rename it over the old one, so a crash leaves the old or the new file.
    pub fn save(&self, settings: &Settings) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(settings).map_err(|e| EngineError::Internal {
            detail: format!("could not encode settings: {e}"),
        })?;
        let tmp = path.with_extension("json.tmp");
        fsutil::write_durable(&tmp, &bytes)?;
        std::fs::rename(&tmp, path).map_err(|e| EngineError::storage(path.display().to_string(), e))?;
        fsutil::sync_dir(path.parent().unwrap_or_else(|| Path::new(".")))
    }
}

impl Settings {
    /// What the window meant by a save: the fields it changed from `base` (the
    /// settings it was showing) put onto `current` (what is saved now). A
    /// field the window did not touch keeps its saved value, so a switch made
    /// from the tray meanwhile is not put back by a save of something else.
    pub fn with_changes(current: &Settings, base: &Settings, wanted: &Settings) -> Result<Settings> {
        let json = |s: &Settings| {
            serde_json::to_value(s).map_err(|e| EngineError::Internal {
                detail: format!("settings: {e}"),
            })
        };
        let (base, wanted) = (json(base)?, json(wanted)?);
        let mut out = json(current)?;
        if let (Some(base), Some(wanted), Some(out)) = (base.as_object(), wanted.as_object(), out.as_object_mut()) {
            let keys: std::collections::BTreeSet<&String> = base.keys().chain(wanted.keys()).collect();
            for key in keys {
                match (base.get(key), wanted.get(key)) {
                    (b, Some(w)) if b != Some(w) => {
                        out.insert(key.clone(), w.clone());
                    }
                    // Left out of the window's settings (an empty optional
                    // field): back to its default.
                    (Some(_), None) => {
                        out.remove(key);
                    }
                    _ => {}
                }
            }
        }
        serde_json::from_value(out).map_err(|e| EngineError::Internal {
            detail: format!("settings: {e}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_changes_only_what_the_window_changed() {
        let base = Settings::default();
        // The tray turned Gaming Mode on after the window read its settings.
        let current = Settings {
            gaming_mode: true,
            ..base.clone()
        };
        // The window saves a new wording, still showing Gaming Mode off.
        let wanted = Settings {
            language: Language::Technical,
            ..base.clone()
        };
        let out = Settings::with_changes(&current, &base, &wanted).unwrap();
        assert!(out.gaming_mode, "the tray's switch is kept");
        assert_eq!(out.language, Language::Technical);
        // Turning it off from the window still turns it off.
        let off = Settings::with_changes(
            &current,
            &current,
            &Settings {
                gaming_mode: false,
                ..current.clone()
            },
        )
        .unwrap();
        assert!(!off.gaming_mode);
        // Clearing an optional field is a change too.
        let set = Settings {
            rig_class_override: Some(RigClass::Low),
            ..base.clone()
        };
        let cleared = Settings::with_changes(&set, &set, &base).unwrap();
        assert_eq!(cleared.rig_class_override, None);
    }

    fn store(dir: &Path) -> SettingsStore {
        SettingsStore::in_dir(&TrustedDir::insecure_for_tests(dir))
    }

    #[test]
    fn defaults_are_plain_language_and_no_override() {
        let s = Settings::default();
        assert_eq!(s.language, Language::Plain);
        assert_eq!(s.rig_class_override, None);
        assert!(!s.gaming_mode && !s.game_timer);
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings {
            rig_class_override: Some(RigClass::High),
            language: Language::Technical,
            welcome_seen: true,
            gaming_mode: true,
            game_timer: true,
            reminders_off: true,
            cleanup_reminder_snoozed_until: Some(1_760_000_000_000),
            driver_reminder_snoozed_until: None,
            restart_check_seen_boot: Some(1_760_000_000_500),
        };
        store(dir.path()).save(&s).unwrap();
        assert_eq!(store(dir.path()).load(), s);
        assert!(
            !dir.path().join("settings.json.tmp").exists(),
            "temp file is renamed away"
        );
    }

    #[test]
    fn a_missing_or_corrupt_file_gives_the_defaults_and_is_replaced_on_save() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(store(dir.path()).load(), Settings::default());
        std::fs::write(dir.path().join("settings.json"), "{ not json").unwrap();
        assert_eq!(store(dir.path()).load(), Settings::default());
        let s = Settings {
            rig_class_override: Some(RigClass::Low),
            ..Settings::default()
        };
        store(dir.path()).save(&s).unwrap();
        assert_eq!(store(dir.path()).load(), s);
    }

    #[test]
    fn unknown_fields_and_missing_fields_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"language":"technical","fromANewerVersion":42}"#,
        )
        .unwrap();
        let s = store(dir.path()).load();
        assert_eq!(s.language, Language::Technical);
        assert_eq!(s.rig_class_override, None);
        assert!(!s.welcome_seen, "a file from before the welcome existed shows it once");
        assert!(!s.gaming_mode && !s.game_timer, "the while-you-play switches start off");
        assert!(!s.reminders_off, "the reminders start on");
        assert_eq!(
            (s.cleanup_reminder_snoozed_until, s.driver_reminder_snoozed_until),
            (None, None)
        );
    }

    #[test]
    fn having_seen_the_welcome_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings {
            welcome_seen: true,
            ..Settings::default()
        };
        store(dir.path()).save(&s).unwrap();
        assert!(store(dir.path()).load().welcome_seen);
    }

    #[test]
    fn an_invalid_value_gives_the_defaults_not_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), r#"{"rigClassOverride":"ultra"}"#).unwrap();
        assert_eq!(store(dir.path()).load(), Settings::default());
    }

    #[test]
    fn the_in_memory_store_keeps_nothing() {
        let s = SettingsStore::in_memory();
        s.save(&Settings {
            rig_class_override: Some(RigClass::High),
            ..Settings::default()
        })
        .unwrap();
        assert_eq!(s.load(), Settings::default());
    }
}
