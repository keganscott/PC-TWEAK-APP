//! The reports of the last games PeakTweaks watched (`play::PlayReport`),
//! kept as `play-history.json` in the protected data directory so the Games
//! cards can show them after a restart. Only what the report holds is kept:
//! the game's id, when it was watched and the graphics card's readings. It
//! stays on this PC, like everything else PeakTweaks keeps.
//!
//! A missing or unreadable file is an empty history. Each report is read on
//! its own, so one PeakTweaks cannot read (damaged, or written by another
//! version) is left out without losing the others. A file that is not a list
//! at all is moved aside as `play-history.json.corrupt` before the next save
//! replaces it.

use std::path::{Path, PathBuf};

use super::error::{EngineError, Result};
use super::fsutil;
use super::play::PlayReport;
use super::secure_dir::TrustedDir;

const FILE: &str = "play-history.json";

/// Reports kept, oldest dropped first.
pub const KEEP: usize = 30;

/// Where the history lives. `None` (tests, dev) keeps it in memory only.
#[derive(Debug, Clone, Default)]
pub struct PlayHistoryStore {
    path: Option<PathBuf>,
}

impl PlayHistoryStore {
    pub fn in_memory() -> Self {
        Self { path: None }
    }

    pub fn in_dir(dir: &TrustedDir) -> Self {
        Self {
            path: Some(dir.path().join(FILE)),
        }
    }

    pub fn load(&self) -> Vec<PlayReport> {
        let Some(path) = &self.path else {
            return Vec::new();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        match serde_json::from_str::<Vec<serde_json::Value>>(&text) {
            Ok(entries) => entries
                .into_iter()
                .filter_map(|entry| serde_json::from_value(entry).ok())
                .collect(),
            Err(_) => {
                // Kept for a look; a failed move only means the next save
                // replaces it.
                let _ = std::fs::rename(path, path.with_extension("json.corrupt"));
                Vec::new()
            }
        }
    }

    /// Replace the stored history the way settings are saved: a temporary
    /// file made durable, then renamed over the old one.
    pub fn save(&self, history: &[PlayReport]) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(history).map_err(|e| EngineError::Internal {
            detail: format!("could not encode the game history: {e}"),
        })?;
        let tmp = path.with_extension("json.tmp");
        fsutil::write_durable(&tmp, &bytes)?;
        std::fs::rename(&tmp, path).map_err(|e| EngineError::storage(path.display().to_string(), e))?;
        fsutil::sync_dir(path.parent().unwrap_or_else(|| Path::new(".")))
    }
}

/// `history` with `report` added, newest last, at most `KEEP` long.
pub fn with_report(mut history: Vec<PlayReport>, report: PlayReport) -> Vec<PlayReport> {
    history.push(report);
    let extra = history.len().saturating_sub(KEEP);
    history.drain(..extra);
    history
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::Probe;

    fn report(game: &str, ended: u64) -> PlayReport {
        PlayReport {
            game: game.into(),
            started_unix_ms: 0,
            ended_unix_ms: ended,
            gpu_throttle: Probe::unknown("no NVIDIA card"),
            heat_readings: 0,
            hardware_readings: 0,
            gpu_hottest_c: Probe::unknown("no NVIDIA card"),
            temperature_missed: None,
            gpu_busy_average: Probe::unknown("no NVIDIA card"),
            cpu_busy_average: Probe::unknown("not read in tests"),
            memory_peak: Probe::unknown("not read in tests"),
            memory_cleans: 0,
            memory_cleaned_bytes: 0,
        }
    }

    #[test]
    fn the_history_keeps_the_newest_reports() {
        let mut history = Vec::new();
        for i in 0..(KEEP as u64 + 5) {
            history = with_report(history, report("fortnite", i));
        }
        assert_eq!(history.len(), KEEP);
        assert_eq!(history.first().map(|r| r.ended_unix_ms), Some(5));
        assert_eq!(history.last().map(|r| r.ended_unix_ms), Some(KEEP as u64 + 4));
    }

    #[test]
    fn a_saved_history_loads_back_and_a_corrupt_one_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = PlayHistoryStore {
            path: Some(dir.path().join(FILE)),
        };
        assert!(store.load().is_empty(), "no file yet");
        let history = vec![report("fortnite", 1), report("roblox", 2)];
        store.save(&history).unwrap();
        assert_eq!(store.load(), history);
        assert!(!dir.path().join("play-history.json.tmp").exists());

        // A report kept by an earlier version, without the processor and
        // memory readings, still loads, with those readings unknown.
        let mut older: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(dir.path().join(FILE)).unwrap()).unwrap();
        for key in ["gpuBusyAverage", "cpuBusyAverage", "memoryPeak"] {
            older[0].as_object_mut().unwrap().remove(key);
        }
        std::fs::write(dir.path().join(FILE), serde_json::to_vec(&older).unwrap()).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.len(), 2);
        assert!(matches!(&loaded[0].memory_peak, Probe::Unknown { reason } if reason.contains("not taken")));
        store.save(&history).unwrap();

        // One report this version cannot read leaves the others.
        let mut entries: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(dir.path().join(FILE)).unwrap()).unwrap();
        entries[0]["gpuThrottle"] = serde_json::json!({ "state": "yes", "value": { "samples": 1, "seen": [{ "reason": "a_reason_from_later", "samples": 1 }] } });
        std::fs::write(dir.path().join(FILE), serde_json::to_vec(&entries).unwrap()).unwrap();
        assert_eq!(store.load(), vec![report("roblox", 2)]);

        // A file that is not a list is moved aside, not lost.
        std::fs::write(dir.path().join(FILE), "{ not json").unwrap();
        assert!(store.load().is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("play-history.json.corrupt")).unwrap(),
            "{ not json"
        );
        assert!(PlayHistoryStore::in_memory().load().is_empty());
        PlayHistoryStore::in_memory().save(&history).unwrap();
    }
}
