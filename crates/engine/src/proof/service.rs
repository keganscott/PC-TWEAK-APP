//! The proof workflow: begin a session, capture "before" and "after" runs, and
//! compare them.
//!
//! One capture at a time. Every run is stored; the comparison is computed from
//! stored runs only, and its wording comes from `verdict::compare`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;

use super::capture::{validate_exe_name, validate_timing, CaptureRequest, CaptureTool};
use super::metrics::{compute_stats, parse_frame_times};
use super::nvml::{NoSampler, ThrottleReason, ThrottleSampler, ThrottleSummary, ThrottleTally};
use super::store::{ProofRun, ProofSession, ProofSessionSummary, ProofStore, Side};
use super::verdict::{compare, Comparison, RunSummary};
use crate::env::KNOWN_GAMES;
use crate::error::{EngineError, Result};
use crate::hardware::RigClass;
use crate::probe::Probe;
use crate::restore::{Clock, SystemClock};
use crate::types::{BlockedCode, BlockedReason, Tier};

/// The free plan includes one proof (one before/after session).
pub const FREE_MAX_SESSIONS: usize = 1;

const MAX_BUILD_LEN: usize = 64;

#[derive(Debug, Clone)]
pub struct BeginSession {
    pub exe: String,
    pub game_id: Option<String>,
    pub game_build: Option<String>,
}

pub struct ProofService {
    store: ProofStore,
    tool: Arc<dyn CaptureTool>,
    sampler: Arc<dyn ThrottleSampler>,
    clock: Arc<dyn Clock>,
    busy: Mutex<()>,
}

/// How often the GPU is asked whether it is being held back during a capture.
const SAMPLE_EVERY: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(100);

/// Reads the GPU at the start, about once a second, and at the end.
struct Sampling {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<ThrottleTally>,
}

impl Sampling {
    fn start(sampler: Arc<dyn ThrottleSampler>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut tally = ThrottleTally::default();
            let first = sampler.sample();
            let usable = first.is_yes();
            tally.add(first);
            if !usable {
                return tally; // nothing to gain from asking again
            }
            'outer: loop {
                let mut waited = Duration::ZERO;
                while waited < SAMPLE_EVERY {
                    if flag.load(Ordering::Relaxed) {
                        break 'outer;
                    }
                    std::thread::sleep(POLL);
                    waited += POLL;
                }
                tally.add(sampler.sample());
            }
            tally.add(sampler.sample());
            tally
        });
        Self { stop, handle }
    }

    fn finish(self) -> Probe<ThrottleSummary> {
        self.stop.store(true, Ordering::Relaxed);
        match self.handle.join() {
            Ok(t) => t.finish(),
            Err(_) => Probe::unknown("the GPU reading thread failed"),
        }
    }
}

fn command_error(detail: impl Into<String>) -> EngineError {
    EngineError::Command {
        what: "Proof run".into(),
        exit_code: None,
        detail: detail.into(),
    }
}

impl ProofService {
    pub fn new(root: PathBuf, tool: Arc<dyn CaptureTool>) -> Self {
        Self {
            store: ProofStore::new(root),
            tool,
            sampler: Arc::new(NoSampler("GPU throttle readings are not enabled in this build".into())),
            clock: Arc::new(SystemClock),
            busy: Mutex::new(()),
        }
    }

    pub fn with_sampler(mut self, sampler: Arc<dyn ThrottleSampler>) -> Self {
        self.sampler = sampler;
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// An id not already used, based on the clock.
    fn fresh_id(&self, prefix: &str, taken: impl Fn(&str) -> bool) -> String {
        let mut n = self.clock.now_ms();
        loop {
            let id = format!("{prefix}-{n:013}");
            if !taken(&id) {
                return id;
            }
            n += 1;
        }
    }

    pub fn begin_session(&self, req: BeginSession, rig: Option<RigClass>, tier: Tier) -> Result<ProofSession> {
        validate_exe_name(&req.exe)?;
        if let Some(g) = &req.game_id {
            if !KNOWN_GAMES.iter().any(|k| k.id == g) {
                return Err(EngineError::UnknownGame { game_id: g.clone() });
            }
        }
        let game_build = match req.game_build.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
            Some(b) if b.chars().count() > MAX_BUILD_LEN || b.chars().any(char::is_control) => {
                return Err(command_error(format!(
                    "the game build note must be plain text of at most {MAX_BUILD_LEN} characters"
                )))
            }
            other => other.map(str::to_owned),
        };

        let existing = self.store.sessions()?;
        if tier == Tier::Free && existing.len() >= FREE_MAX_SESSIONS {
            return Err(EngineError::Blocked {
                reason: BlockedReason::new(
                    BlockedCode::TierRequired,
                    "The free plan includes one proof run. Pro has unlimited proof runs.",
                )
                .with_trigger("pro"),
            });
        }

        let session = ProofSession {
            session_id: self.fresh_id("session", |id| existing.iter().any(|s| s.session_id == id)),
            created_unix_ms: self.clock.now_ms(),
            exe: req.exe,
            game_id: req.game_id,
            game_build,
            rig_class: rig,
            tool_version: self.tool.version(),
        };
        self.store.create_session(&session)?;
        Ok(session)
    }

    /// Capture one run on one side. `applied_tweaks` is what the engine says is
    /// applied right now, recorded with the run so the result says what it
    /// measured. `progress(stage, message)` is advisory.
    pub fn capture(
        &self,
        session_id: &str,
        side: Side,
        seconds: u32,
        delay_seconds: u32,
        applied_tweaks: Vec<String>,
        progress: &dyn Fn(&str, &str),
    ) -> Result<ProofRun> {
        validate_timing(seconds, delay_seconds)?;
        let session = self.store.session(session_id)?;

        let _turn = match self.busy.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => {
                return Err(command_error(
                    "another capture is already running; wait for it to finish",
                ))
            }
        };

        let existing = self.store.runs(session_id)?;
        let index = existing.iter().filter(|r| r.side == side).count() as u32 + 1;
        let run_id = self.fresh_id("run", |id| self.store.run_dir(session_id, id).is_ok_and(|d| d.exists()));
        let csv_path = self.store.prepare_run(session_id, &run_id)?;
        let started = self.clock.now_ms();

        let result = (|| {
            if delay_seconds > 0 {
                progress("proof_delay", "Get into the scene you want to measure");
            }
            progress("proof_capture", "Capturing frames");
            let sampling = Sampling::start(self.sampler.clone());
            let captured = self.tool.capture(&CaptureRequest {
                exe: session.exe.clone(),
                delay_seconds,
                seconds,
                out_csv: csv_path.clone(),
            });
            // Always stop the reader, whether or not the capture worked.
            let gpu_throttle = sampling.finish();
            captured?;
            progress("proof_analyze", "Working out the numbers");
            let text = std::fs::read_to_string(&csv_path)
                .map_err(|e| EngineError::storage(csv_path.display().to_string(), e))?;
            let frames = parse_frame_times(&text)?;
            let stats = compute_stats(&frames.ms)?;
            Ok(ProofRun {
                run_id: run_id.clone(),
                session_id: session_id.to_owned(),
                side,
                index,
                started_unix_ms: started,
                seconds,
                delay_seconds,
                applied_tweaks,
                stats,
                csv_file: "capture.csv".into(),
                ignored_rows: frames.ignored_rows as u32,
                unusable_rows: frames.unusable_rows as u32,
                gpu_throttle,
            })
        })();

        match result {
            Ok(run) => {
                self.store.save_run(&run)?;
                Ok(run)
            }
            Err(e) => {
                // A capture that did not yield a usable result leaves no half-run behind.
                self.store.discard_run(session_id, &run_id);
                Err(e)
            }
        }
    }

    pub fn runs(&self, session_id: &str) -> Result<Vec<ProofRun>> {
        self.store.runs(session_id)
    }

    pub fn sessions(&self) -> Result<Vec<ProofSessionSummary>> {
        self.store
            .sessions()?
            .into_iter()
            .map(|session| {
                let runs = self.store.runs(&session.session_id)?;
                let count = |side| runs.iter().filter(|r| r.side == side).count() as u32;
                Ok(ProofSessionSummary {
                    before_runs: count(Side::Before),
                    after_runs: count(Side::After),
                    session,
                })
            })
            .collect()
    }

    /// Compare the "before" runs with the "after" runs of a session.
    pub fn compare(&self, session_id: &str) -> Result<Comparison> {
        let runs = self.store.runs(session_id)?;
        let summarise = |side| -> Vec<RunSummary> {
            runs.iter()
                .filter(|r| r.side == side)
                .map(|r| RunSummary {
                    run_id: r.run_id.clone(),
                    avg_fps: r.stats.avg_fps,
                    one_percent_low_fps: r.stats.one_percent_low_fps,
                })
                .collect()
        };
        let mut comparison = compare(&summarise(Side::Before), &summarise(Side::After));
        comparison.warnings = change_warnings(&runs);
        comparison.warnings.extend(runs.iter().filter_map(throttle_warning));
        Ok(comparison)
    }
}

/// Whether the runs can say anything about a PeakTweaks change: every run on
/// a side recorded with the same applied changes, and the two sides differing.
/// Each run stores what was applied when it was captured (`applied_tweaks`).
fn change_warnings(runs: &[ProofRun]) -> Vec<String> {
    let set = |r: &ProofRun| {
        let mut v = r.applied_tweaks.clone();
        v.sort();
        v
    };
    let side_set = |side: Side| -> Option<Option<Vec<String>>> {
        let mut sets = runs.iter().filter(|r| r.side == side).map(set);
        let first = sets.next()?;
        // Some(Some(set)): one set for the whole side; Some(None): mixed.
        Some(sets.all(|s| s == first).then_some(first))
    };
    let mut out = Vec::new();
    let (before, after) = (side_set(Side::Before), side_set(Side::After));
    for (name, side) in [("before", &before), ("after", &after)] {
        if let Some(None) = side {
            out.push(format!(
                "The {name} runs were not all recorded with the same PeakTweaks changes in place, so they do \
                 not describe one setup."
            ));
        }
    }
    if let (Some(Some(b)), Some(Some(a))) = (&before, &after) {
        if a == b {
            out.push(
                "The before and after runs were recorded with the same PeakTweaks changes in place, so any \
                 difference between them did not come from a PeakTweaks change."
                    .to_owned(),
            );
        }
    }
    out
}

/// A sentence about a run whose GPU was held back by heat or power, or `None`.
fn throttle_warning(run: &ProofRun) -> Option<String> {
    let summary = run.gpu_throttle.value()?;
    let limiting: Vec<ThrottleReason> = summary.limiting();
    if limiting.is_empty() {
        return None;
    }
    let what: Vec<&str> = limiting.iter().map(|r| r.describe()).collect();
    Some(format!(
        "{} ({}): {} during this run, so its numbers may understate what the PC can do.",
        run.run_id,
        match run.side {
            Side::Before => "before",
            Side::After => "after",
        },
        what.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use super::*;
    use crate::proof::capture::FakeCapture;
    use crate::proof::verdict::Verdict;

    struct StepClock(AtomicU64);
    impl Clock for StepClock {
        fn now_ms(&self) -> u64 {
            self.0.fetch_add(1000, Ordering::Relaxed)
        }
    }

    struct Frozen(u64);
    impl Clock for Frozen {
        fn now_ms(&self) -> u64 {
            self.0
        }
    }

    fn frames(ms: f64) -> Vec<f64> {
        vec![ms; 300]
    }

    fn service(dir: &std::path::Path, tool: FakeCapture) -> ProofService {
        ProofService::new(dir.join("proof"), Arc::new(tool))
            .with_clock(Arc::new(StepClock(AtomicU64::new(1_700_000_000_000))))
    }

    fn begin(s: &ProofService, tier: Tier) -> Result<ProofSession> {
        s.begin_session(
            BeginSession {
                exe: "Game.exe".into(),
                game_id: Some("fortnite".into()),
                game_build: Some("  build 1234  ".into()),
            },
            Some(RigClass::Mid),
            tier,
        )
    }

    fn quiet(_: &str, _: &str) {}

    /// Captures two runs per side with the given applied changes per run.
    fn session_with(applied: [&[&str]; 4]) -> Comparison {
        let d = tempfile::tempdir().unwrap();
        let svc = service(d.path(), FakeCapture::new(|call, _| frames(10.0 + call as f64 * 0.01)));
        let session = begin(&svc, Tier::Pro).unwrap();
        for (i, set) in applied.iter().enumerate() {
            let side = if i < 2 { Side::Before } else { Side::After };
            let ids = set.iter().map(|s| s.to_string()).collect();
            svc.capture(&session.session_id, side, 30, 5, ids, &quiet).unwrap();
        }
        svc.compare(&session.session_id).unwrap()
    }

    #[test]
    fn a_comparison_says_when_no_peaktweaks_change_lies_between_the_sides() {
        let c = session_with([&["a"], &["a"], &["a"], &["a"]]);
        assert!(
            c.warnings.iter().any(|w| w.contains("same PeakTweaks changes")),
            "{:?}",
            c.warnings
        );
        let c = session_with([&[], &[], &["a"], &["a"]]);
        assert!(c.warnings.is_empty(), "a change between the sides: {:?}", c.warnings);
    }

    #[test]
    fn a_comparison_says_when_a_side_was_recorded_with_different_changes() {
        let c = session_with([&[], &["a"], &["a", "b"], &["a", "b"]]);
        assert!(
            c.warnings
                .iter()
                .any(|w| w.contains("before runs were not all recorded")),
            "{:?}",
            c.warnings
        );
        let c = session_with([&[], &[], &["a"], &["b"]]);
        assert!(
            c.warnings
                .iter()
                .any(|w| w.contains("after runs were not all recorded")),
            "{:?}",
            c.warnings
        );
    }

    #[test]
    fn a_before_after_session_produces_a_better_verdict_backed_by_stored_runs() {
        let d = tempfile::tempdir().unwrap();
        // Calls 0,1 are "before" at ~100 FPS, 2,3 "after" at ~125 FPS, each with small jitter.
        let tool = FakeCapture::new(|call, _| {
            frames(if call < 2 {
                10.0 + call as f64 * 0.05
            } else {
                8.0 + (call - 2) as f64 * 0.05
            })
        });
        let svc = service(d.path(), tool);
        let session = begin(&svc, Tier::Pro).unwrap();
        assert_eq!(session.game_build.as_deref(), Some("build 1234"), "trimmed");
        assert_eq!(session.tool_version, "fake");

        for _ in 0..2 {
            svc.capture(&session.session_id, Side::Before, 30, 5, vec![], &quiet)
                .unwrap();
        }
        for _ in 0..2 {
            svc.capture(
                &session.session_id,
                Side::After,
                30,
                5,
                vec!["input.mouseaccel".into()],
                &quiet,
            )
            .unwrap();
        }
        let runs = svc.runs(&session.session_id).unwrap();
        assert_eq!(runs.len(), 4);
        assert_eq!(
            runs.iter()
                .filter(|r| r.side == Side::After)
                .map(|r| r.index)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(runs[3].applied_tweaks, vec!["input.mouseaccel"]);
        // The raw frames are kept next to the numbers.
        let csv = d
            .path()
            .join("proof")
            .join(&session.session_id)
            .join(&runs[0].run_id)
            .join("capture.csv");
        assert!(csv.is_file());

        let c = svc.compare(&session.session_id).unwrap();
        assert_eq!(c.overall, Verdict::Better, "{}", c.headline);
        assert_eq!(c.before_run_ids.len(), 2);
        assert_eq!(c.after_run_ids.len(), 2);
        let all: Vec<&String> = c.before_run_ids.iter().chain(&c.after_run_ids).collect();
        assert!(
            all.iter().all(|id| runs.iter().any(|r| &&r.run_id == id)),
            "every id in the verdict is a stored run"
        );

        let listed = svc.sessions().unwrap();
        assert_eq!((listed[0].before_runs, listed[0].after_runs), (2, 2));
    }

    #[test]
    fn identical_before_and_after_is_not_called_a_change() {
        let d = tempfile::tempdir().unwrap();
        let tool = FakeCapture::new(|call, _| frames(10.0 + (call % 2) as f64 * 0.4));
        let svc = service(d.path(), tool);
        let s = begin(&svc, Tier::Pro).unwrap();
        for side in [Side::Before, Side::Before, Side::After, Side::After] {
            svc.capture(&s.session_id, side, 30, 0, vec![], &quiet).unwrap();
        }
        assert_eq!(svc.compare(&s.session_id).unwrap().overall, Verdict::NoMeasurableChange);
    }

    #[test]
    fn one_run_a_side_says_not_enough_data() {
        let d = tempfile::tempdir().unwrap();
        let svc = service(d.path(), FakeCapture::new(|_, _| frames(10.0)));
        let s = begin(&svc, Tier::Pro).unwrap();
        svc.capture(&s.session_id, Side::Before, 30, 0, vec![], &quiet).unwrap();
        svc.capture(&s.session_id, Side::After, 30, 0, vec![], &quiet).unwrap();
        assert_eq!(svc.compare(&s.session_id).unwrap().overall, Verdict::NotEnoughData);
    }

    #[test]
    fn the_free_plan_gets_one_session_pro_gets_more_and_it_persists_across_restarts() {
        let d = tempfile::tempdir().unwrap();
        let svc = service(d.path(), FakeCapture::new(|_, _| frames(10.0)));
        begin(&svc, Tier::Free).unwrap();
        let e = begin(&svc, Tier::Free).unwrap_err();
        assert!(
            matches!(&e, EngineError::Blocked { reason } if reason.code == BlockedCode::TierRequired),
            "{e:?}"
        );

        // A fresh service over the same folder still knows.
        let again = service(d.path(), FakeCapture::new(|_, _| frames(10.0)));
        assert!(begin(&again, Tier::Free).is_err());
        assert!(begin(&again, Tier::Pro).is_ok());
        assert!(begin(&again, Tier::Ultimate).is_ok());
    }

    #[test]
    fn bad_input_is_refused_before_anything_is_created() {
        let d = tempfile::tempdir().unwrap();
        let svc = service(d.path(), FakeCapture::new(|_, _| frames(10.0)));
        let bad_exe = svc.begin_session(
            BeginSession {
                exe: "--output_file.exe".into(),
                game_id: None,
                game_build: None,
            },
            None,
            Tier::Pro,
        );
        assert!(bad_exe.is_err());
        let bad_game = svc.begin_session(
            BeginSession {
                exe: "Game.exe".into(),
                game_id: Some("nope".into()),
                game_build: None,
            },
            None,
            Tier::Pro,
        );
        assert!(matches!(bad_game, Err(EngineError::UnknownGame { .. })));
        let long = svc.begin_session(
            BeginSession {
                exe: "Game.exe".into(),
                game_id: None,
                game_build: Some("x".repeat(65)),
            },
            None,
            Tier::Pro,
        );
        assert!(long.is_err());
        let ctrl = svc.begin_session(
            BeginSession {
                exe: "Game.exe".into(),
                game_id: None,
                game_build: Some("a\u{7}b".into()),
            },
            None,
            Tier::Pro,
        );
        assert!(ctrl.is_err());
        assert!(svc.sessions().unwrap().is_empty());

        let s = begin(&svc, Tier::Pro).unwrap();
        assert!(
            svc.capture(&s.session_id, Side::Before, 5, 0, vec![], &quiet).is_err(),
            "too short"
        );
        assert!(svc
            .capture("session-../..", Side::Before, 30, 0, vec![], &quiet)
            .is_err());
        assert!(svc.runs("../..").is_err());
    }

    #[test]
    fn a_capture_with_too_few_frames_leaves_no_half_run() {
        let d = tempfile::tempdir().unwrap();
        let svc = service(d.path(), FakeCapture::new(|_, _| vec![16.0; 10]));
        let s = begin(&svc, Tier::Pro).unwrap();
        let e = svc
            .capture(&s.session_id, Side::Before, 30, 0, vec![], &quiet)
            .unwrap_err();
        assert!(
            matches!(&e, EngineError::Command { detail, .. } if detail.contains("at least 60")),
            "{e:?}"
        );
        assert!(svc.runs(&s.session_id).unwrap().is_empty());
        let dir = d.path().join("proof").join(&s.session_id);
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();
        assert_eq!(leftovers.len(), 1, "only session.json remains: {leftovers:?}");
    }

    #[test]
    fn an_unavailable_tool_reports_why_and_cleans_up() {
        let d = tempfile::tempdir().unwrap();
        let svc = ProofService::new(
            d.path().join("proof"),
            Arc::new(crate::proof::capture::UnavailableTool(
                "PresentMon is not bundled".into(),
            )),
        );
        let s = begin(&svc, Tier::Pro).unwrap();
        let e = svc
            .capture(&s.session_id, Side::Before, 30, 0, vec![], &quiet)
            .unwrap_err();
        assert!(
            matches!(&e, EngineError::Command { detail, .. } if detail.contains("not bundled")),
            "{e:?}"
        );
        assert!(svc.runs(&s.session_id).unwrap().is_empty());
    }

    #[test]
    fn only_one_capture_runs_at_a_time() {
        let d = tempfile::tempdir().unwrap();
        let svc = Arc::new(service(
            d.path(),
            FakeCapture::new(|_, _| {
                std::thread::sleep(std::time::Duration::from_millis(600));
                frames(10.0)
            }),
        ));
        let s = begin(&svc, Tier::Pro).unwrap();
        let first = {
            let svc = svc.clone();
            let id = s.session_id.clone();
            std::thread::spawn(move || svc.capture(&id, Side::Before, 30, 0, vec![], &quiet))
        };
        std::thread::sleep(std::time::Duration::from_millis(150));
        let second = svc.capture(&s.session_id, Side::After, 30, 0, vec![], &quiet);
        assert!(
            matches!(&second, Err(EngineError::Command { detail, .. }) if detail.contains("already running")),
            "{second:?}"
        );
        assert!(first.join().unwrap().is_ok());
    }

    #[test]
    fn ids_stay_unique_even_when_the_clock_does_not_move() {
        let d = tempfile::tempdir().unwrap();
        let svc = ProofService::new(d.path().join("proof"), Arc::new(FakeCapture::new(|_, _| frames(10.0))))
            .with_clock(Arc::new(Frozen(1_700_000_000_000)));
        let a = begin(&svc, Tier::Pro).unwrap();
        let b = begin(&svc, Tier::Pro).unwrap();
        assert_ne!(a.session_id, b.session_id);
        let r1 = svc.capture(&a.session_id, Side::Before, 30, 0, vec![], &quiet).unwrap();
        let r2 = svc.capture(&a.session_id, Side::Before, 30, 0, vec![], &quiet).unwrap();
        assert_ne!(r1.run_id, r2.run_id);
    }

    #[test]
    fn progress_stages_are_reported_in_order() {
        let d = tempfile::tempdir().unwrap();
        let svc = service(d.path(), FakeCapture::new(|_, _| frames(10.0)));
        let s = begin(&svc, Tier::Pro).unwrap();
        let seen = Mutex::new(Vec::new());
        svc.capture(&s.session_id, Side::Before, 30, 5, vec![], &|st, _| {
            seen.lock().unwrap().push(st.to_owned())
        })
        .unwrap();
        assert_eq!(*seen.lock().unwrap(), ["proof_delay", "proof_capture", "proof_analyze"]);
    }

    struct FixedSampler(Vec<ThrottleReason>);
    impl ThrottleSampler for FixedSampler {
        fn sample(&self) -> Probe<Vec<ThrottleReason>> {
            Probe::yes(self.0.clone())
        }
    }

    #[test]
    fn a_run_records_what_the_gpu_said_and_heat_limited_runs_produce_a_warning() {
        let d = tempfile::tempdir().unwrap();
        let svc = service(d.path(), FakeCapture::new(|_, _| frames(10.0))).with_sampler(Arc::new(FixedSampler(vec![
            ThrottleReason::HardwareThermalSlowdown,
            ThrottleReason::GpuIdle,
        ])));
        let s = begin(&svc, Tier::Pro).unwrap();
        let run = svc.capture(&s.session_id, Side::Before, 30, 0, vec![], &quiet).unwrap();
        // One reading before the capture and one after, even for an instant capture.
        let summary = run.gpu_throttle.value().expect("readings were taken");
        assert!(summary.samples >= 2, "{summary:?}");
        assert_eq!(summary.limiting(), vec![ThrottleReason::HardwareThermalSlowdown]);

        let c = svc.compare(&s.session_id).unwrap();
        assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
        assert!(
            c.warnings[0].contains(&run.run_id) && c.warnings[0].contains("because of heat"),
            "{}",
            c.warnings[0]
        );
    }

    #[test]
    fn a_run_without_gpu_readings_says_unknown_and_raises_no_warning() {
        let d = tempfile::tempdir().unwrap();
        let svc = service(d.path(), FakeCapture::new(|_, _| frames(10.0)));
        let s = begin(&svc, Tier::Pro).unwrap();
        let run = svc.capture(&s.session_id, Side::Before, 30, 0, vec![], &quiet).unwrap();
        assert!(run.gpu_throttle.is_unknown());
        assert!(svc.compare(&s.session_id).unwrap().warnings.is_empty());
    }

    #[test]
    fn benign_reasons_such_as_idle_do_not_warn() {
        let d = tempfile::tempdir().unwrap();
        let svc = service(d.path(), FakeCapture::new(|_, _| frames(10.0))).with_sampler(Arc::new(FixedSampler(vec![
            ThrottleReason::GpuIdle,
            ThrottleReason::ApplicationClocks,
        ])));
        let s = begin(&svc, Tier::Pro).unwrap();
        svc.capture(&s.session_id, Side::Before, 30, 0, vec![], &quiet).unwrap();
        assert!(svc.compare(&s.session_id).unwrap().warnings.is_empty());
    }
}
