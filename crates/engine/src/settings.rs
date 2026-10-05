//! The few choices a user makes that are not tweaks: a rig-class override
//! (plan 6.1: "User can override"), the plain/technical wording (plan
//! section 7), and whether the first-run welcome was seen. Stored as `settings.json` in the protected data directory.
//!
//! Settings are preferences only. They change defaults and copy; they never
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

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> SettingsStore {
        SettingsStore::in_dir(&TrustedDir::insecure_for_tests(dir))
    }

    #[test]
    fn defaults_are_plain_language_and_no_override() {
        let s = Settings::default();
        assert_eq!(s.language, Language::Plain);
        assert_eq!(s.rig_class_override, None);
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings {
            rig_class_override: Some(RigClass::High),
            language: Language::Technical,
            welcome_seen: true,
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
