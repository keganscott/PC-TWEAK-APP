//! The reports of the last games PeakTweaks watched (`play::PlayReport`),
//! kept as `play-history.json` in the protected data directory so the Games
//! cards can show them after a restart. Only what the report holds is kept:
//! the game's id, when it was watched and the graphics card's readings. It
//! stays on this PC, like everything else PeakTweaks keeps.
//!
//! A missing, unreadable or corrupt file is an empty history; the corrupt
//! file is replaced by the next save.

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
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
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
        std::fs::write(dir.path().join(FILE), "{ not json").unwrap();
        assert!(store.load().is_empty());
        assert!(PlayHistoryStore::in_memory().load().is_empty());
        PlayHistoryStore::in_memory().save(&history).unwrap();
    }
}
