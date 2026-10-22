//! Where proof runs live.
//!
//! ```text
//! <data dir>/proof/<session id>/session.json
//! <data dir>/proof/<session id>/<run id>/capture.csv
//! <data dir>/proof/<session id>/<run id>/run.json
//! ```
//!
//! Every run is kept (CSV plus metadata), so any number shown to a user can be
//! traced to a `runId` and recomputed from the raw frames. Ids arriving from the
//! webview are validated against a strict pattern before they touch a path.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::auto::AutoRecord;
use super::metrics::FrameStats;
use super::nvml::ThrottleSummary;
use crate::error::{EngineError, Result};
use crate::fsutil;
use crate::hardware::RigClass;
use crate::probe::Probe;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProofSession {
    pub session_id: String,
    pub created_unix_ms: u64,
    pub exe: String,
    pub game_id: Option<String>,
    /// Free text the user gave, e.g. the game's build or patch.
    pub game_build: Option<String>,
    pub rig_class: Option<RigClass>,
    /// PresentMon version that captured the runs.
    pub tool_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProofRun {
    pub run_id: String,
    pub session_id: String,
    pub side: Side,
    /// 1-based position among the runs on this side.
    pub index: u32,
    pub started_unix_ms: u64,
    pub seconds: u32,
    pub delay_seconds: u32,
    /// Tweaks that were applied when this run was captured.
    pub applied_tweaks: Vec<String>,
    pub stats: FrameStats,
    pub csv_file: String,
    /// Rows from other swap chains that were set aside.
    pub ignored_rows: u32,
    /// Rows with no usable frame time.
    pub unusable_rows: u32,
    /// What the GPU said about being held back while this run was captured.
    pub gpu_throttle: Probe<ThrottleSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProofSessionSummary {
    pub session: ProofSession,
    pub before_runs: u32,
    pub after_runs: u32,
}

/// `<prefix>-<digits>`, digits 10 to 16 long. Nothing else can name a directory.
pub fn valid_id(prefix: &str, id: &str) -> bool {
    id.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|d| (10..=16).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit()))
}

pub struct ProofStore {
    root: PathBuf,
}

fn invalid(kind: &str, id: &str) -> EngineError {
    EngineError::Command {
        what: "Proof store".into(),
        exit_code: None,
        detail: format!("{id:?} is not a valid {kind} id"),
    }
}

impl ProofStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn session_dir(&self, session_id: &str) -> Result<PathBuf> {
        if !valid_id("session", session_id) {
            return Err(invalid("session", session_id));
        }
        Ok(self.root.join(session_id))
    }

    pub fn run_dir(&self, session_id: &str, run_id: &str) -> Result<PathBuf> {
        if !valid_id("run", run_id) {
            return Err(invalid("run", run_id));
        }
        Ok(self.session_dir(session_id)?.join(run_id))
    }

    fn write_json<T: Serialize>(&self, path: PathBuf, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| EngineError::Storage {
            path: path.display().to_string(),
            detail: format!("serialising: {e}"),
        })?;
        fsutil::write_durable(&path, &bytes)
    }

    fn read_json<T: for<'de> Deserialize<'de>>(&self, path: &std::path::Path) -> Result<T> {
        let bytes = std::fs::read(path).map_err(|e| EngineError::storage(path.display().to_string(), e))?;
        serde_json::from_slice(&bytes).map_err(|e| EngineError::Storage {
            path: path.display().to_string(),
            detail: format!("not valid JSON for this record: {e}"),
        })
    }

    pub fn create_session(&self, session: &ProofSession) -> Result<()> {
        let dir = self.session_dir(&session.session_id)?;
        self.write_json(dir.join("session.json"), session)
    }

    pub fn session(&self, session_id: &str) -> Result<ProofSession> {
        self.read_json(&self.session_dir(session_id)?.join("session.json"))
    }

    /// All sessions, oldest first. Unreadable entries are skipped, not fatal.
    pub fn sessions(&self) -> Result<Vec<ProofSession>> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(EngineError::storage(self.root.display().to_string(), e)),
        };
        let mut out: Vec<ProofSession> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| valid_id("session", name))
            .filter_map(|name| self.session(&name).ok())
            .collect();
        out.sort_by_key(|s| s.created_unix_ms);
        Ok(out)
    }

    /// Where a new run's CSV should be written (its directory is created).
    pub fn prepare_run(&self, session_id: &str, run_id: &str) -> Result<PathBuf> {
        let dir = self.run_dir(session_id, run_id)?;
        fsutil::create_dir_durable(&dir)?;
        Ok(dir.join("capture.csv"))
    }

    pub fn save_run(&self, run: &ProofRun) -> Result<()> {
        let dir = self.run_dir(&run.session_id, &run.run_id)?;
        self.write_json(dir.join("run.json"), run)
    }

    /// Throw away a run directory whose capture did not produce a usable result.
    pub fn discard_run(&self, session_id: &str, run_id: &str) {
        if let Ok(dir) = self.run_dir(session_id, run_id) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    fn auto_path(&self) -> PathBuf {
        self.root.join("auto-record.json")
    }

    /// The side set to record while its game runs (`auto.rs`), if any.
    pub fn auto_record(&self) -> Result<Option<AutoRecord>> {
        let path = self.auto_path();
        if !path.exists() {
            return Ok(None);
        }
        let record: AutoRecord = self.read_json(&path)?;
        // Checked like an id from the webview, as it names a folder.
        self.session_dir(&record.session_id)?;
        Ok(Some(record))
    }

    pub fn save_auto_record(&self, record: &AutoRecord) -> Result<()> {
        self.session_dir(&record.session_id)?;
        self.write_json(self.auto_path(), record)
    }

    pub fn clear_auto_record(&self) -> Result<()> {
        let path = self.auto_path();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(EngineError::storage(path.display().to_string(), e)),
        }
    }

    /// All saved runs of a session, ordered by start time.
    pub fn runs(&self, session_id: &str) -> Result<Vec<ProofRun>> {
        let dir = self.session_dir(session_id)?;
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(EngineError::storage(dir.display().to_string(), e)),
        };
        let mut out: Vec<ProofRun> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| valid_id("run", name))
            .filter_map(|name| self.read_json::<ProofRun>(&dir.join(name).join("run.json")).ok())
            .collect();
        out.sort_by_key(|r| r.started_unix_ms);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::metrics::compute_stats;

    fn run(session: &str, id: &str, side: Side, at: u64) -> ProofRun {
        ProofRun {
            run_id: id.into(),
            session_id: session.into(),
            side,
            index: 1,
            started_unix_ms: at,
            seconds: 30,
            delay_seconds: 5,
            applied_tweaks: vec!["input.mouseaccel".into()],
            stats: compute_stats(&vec![10.0; 100]).unwrap(),
            csv_file: "capture.csv".into(),
            ignored_rows: 0,
            unusable_rows: 0,
            gpu_throttle: Probe::unknown("test"),
        }
    }

    fn session(id: &str, at: u64) -> ProofSession {
        ProofSession {
            session_id: id.into(),
            created_unix_ms: at,
            exe: "Game.exe".into(),
            game_id: Some("fortnite".into()),
            game_build: None,
            rig_class: Some(RigClass::Mid),
            tool_version: "2.6.0".into(),
        }
    }

    #[test]
    fn ids_must_match_the_strict_pattern() {
        assert!(valid_id("session", "session-1700000000000"));
        assert!(valid_id("run", "run-1700000000001"));
        for bad in [
            "session-",
            "session-abc",
            "session-12345",
            "session-17000000000000000",
            "run-1700000000000",
            "session-1700000000000/../x",
            "../session-1700000000000",
            "session-1700000000000\\x",
            "",
        ] {
            assert!(!valid_id("session", bad), "{bad}");
        }
        assert!(!valid_id("run", "session-1700000000000"));
    }

    #[test]
    fn hostile_ids_never_reach_the_filesystem() {
        let d = tempfile::tempdir().unwrap();
        let store = ProofStore::new(d.path().join("proof"));
        assert!(store.session("../../etc").is_err());
        assert!(store.runs("session-1/../..").is_err());
        assert!(store.run_dir("session-1700000000000", "../x").is_err());
        assert!(store.prepare_run("..", "run-1700000000000").is_err());
        assert!(!d.path().join("proof").exists() || std::fs::read_dir(d.path().join("proof")).unwrap().count() == 0);
    }

    #[test]
    fn sessions_and_runs_round_trip_in_order() {
        let d = tempfile::tempdir().unwrap();
        let store = ProofStore::new(d.path().join("proof"));
        assert!(store.sessions().unwrap().is_empty());

        store.create_session(&session("session-1700000000200", 200)).unwrap();
        store.create_session(&session("session-1700000000100", 100)).unwrap();
        let s: Vec<_> = store.sessions().unwrap().into_iter().map(|s| s.session_id).collect();
        assert_eq!(s, vec!["session-1700000000100", "session-1700000000200"]);

        let sid = "session-1700000000100";
        let csv = store.prepare_run(sid, "run-1700000000005").unwrap();
        std::fs::write(&csv, "MsBetweenPresents\n10\n").unwrap();
        store.save_run(&run(sid, "run-1700000000005", Side::After, 5)).unwrap();
        let csv2 = store.prepare_run(sid, "run-1700000000002").unwrap();
        std::fs::write(&csv2, "x").unwrap();
        store.save_run(&run(sid, "run-1700000000002", Side::Before, 2)).unwrap();

        let runs = store.runs(sid).unwrap();
        assert_eq!(
            runs.iter().map(|r| r.run_id.as_str()).collect::<Vec<_>>(),
            ["run-1700000000002", "run-1700000000005"]
        );
        assert_eq!(runs[0].side, Side::Before);
        assert_eq!(runs[1].applied_tweaks, vec!["input.mouseaccel"]);
        assert!(store.runs("session-1700000000200").unwrap().is_empty());
    }

    #[test]
    fn a_discarded_run_leaves_nothing_and_a_corrupt_one_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        let store = ProofStore::new(d.path().join("proof"));
        let sid = "session-1700000000100";
        store.create_session(&session(sid, 1)).unwrap();

        store.prepare_run(sid, "run-1700000000001").unwrap();
        store.discard_run(sid, "run-1700000000001");
        assert!(!store.run_dir(sid, "run-1700000000001").unwrap().exists());

        let dir = store.run_dir(sid, "run-1700000000002").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.json"), "{ not json").unwrap();
        assert!(
            store.runs(sid).unwrap().is_empty(),
            "a corrupt run is skipped, not fatal"
        );
    }
}
