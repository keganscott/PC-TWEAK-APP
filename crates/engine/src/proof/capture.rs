//! Capturing frames with PresentMon.
//!
//! `CaptureTool` is the seam: the real implementation runs the pinned
//! PresentMon; tests use one that writes a synthetic CSV. Whatever the game's
//! name, it reaches PresentMon as a single argv element after `--process_name`
//! (never through a shell), and names that could be mistaken for an option or a
//! path are refused first.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crate::error::{EngineError, Result};
use crate::proc::run_limited;

pub const MIN_SECONDS: u32 = 10;
pub const MAX_SECONDS: u32 = 600;
pub const MAX_DELAY_SECONDS: u32 = 120;

/// Fixed ETW session name; captures are serialised so one is enough.
const SESSION_NAME: &str = "PeakTweaks";

/// Extra time allowed on top of delay + capture before PresentMon is killed.
const GRACE: Duration = Duration::from_secs(45);

#[derive(Debug, Clone)]
pub struct CaptureRequest {
    pub exe: String,
    pub delay_seconds: u32,
    pub seconds: u32,
    pub out_csv: PathBuf,
}

pub trait CaptureTool: Send + Sync {
    fn capture(&self, req: &CaptureRequest) -> Result<()>;
    /// Version string recorded with every run.
    fn version(&self) -> String;
}

fn bad_request(detail: impl Into<String>) -> EngineError {
    EngineError::Command {
        what: "Start a capture".into(),
        exit_code: None,
        detail: detail.into(),
    }
}

/// A game executable name: letters, digits, `.`, `_`, `-`, space, `+`, `(`, `)`,
/// ending in `.exe`, not starting with `-` (an option) or `.`, no separators.
pub fn validate_exe_name(name: &str) -> Result<()> {
    let ok_chars = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ' | '+' | '(' | ')'));
    let stem_ok = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let lower = name.to_ascii_lowercase();
    if name.len() <= 4 || name.len() > 64 || !ok_chars || !stem_ok || !lower.ends_with(".exe") {
        return Err(bad_request(format!(
            "{name:?} is not a game executable name (expected something like Game.exe, without a folder)"
        )));
    }
    Ok(())
}

pub fn validate_timing(seconds: u32, delay_seconds: u32) -> Result<()> {
    if !(MIN_SECONDS..=MAX_SECONDS).contains(&seconds) {
        return Err(bad_request(format!(
            "capture length must be between {MIN_SECONDS} and {MAX_SECONDS} seconds"
        )));
    }
    if delay_seconds > MAX_DELAY_SECONDS {
        return Err(bad_request(format!(
            "the start delay can be at most {MAX_DELAY_SECONDS} seconds"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Real tool
// ---------------------------------------------------------------------------

pub struct PresentMonTool {
    exe: PathBuf,
    version: String,
    /// Re-check the file against the pin before every run. Off only in tests
    /// that substitute a stand-in program.
    verify: bool,
    /// Replaces the computed time limit. Tests only.
    limit_override: Option<Duration>,
}

impl PresentMonTool {
    /// The pinned, hash-checked PresentMon at `exe`.
    pub fn new(exe: PathBuf) -> Self {
        Self {
            exe,
            version: super::presentmon::pinned().version,
            verify: true,
            limit_override: None,
        }
    }

    #[cfg(all(test, unix))]
    fn stand_in(exe: PathBuf) -> Self {
        Self {
            exe,
            version: "test".into(),
            verify: false,
            limit_override: None,
        }
    }

    /// The exact argument list. Kept separate so tests can assert on it.
    pub fn args(req: &CaptureRequest) -> Vec<String> {
        let mut a = vec![
            "--process_name".to_owned(),
            req.exe.clone(),
            "--output_file".to_owned(),
            req.out_csv.display().to_string(),
        ];
        if req.delay_seconds > 0 {
            a.push("--delay".into());
            a.push(req.delay_seconds.to_string());
        }
        a.extend([
            "--timed".to_owned(),
            req.seconds.to_string(),
            "--terminate_after_timed".to_owned(),
            "--no_console_stats".to_owned(),
            "--session_name".to_owned(),
            SESSION_NAME.to_owned(),
            "--stop_existing_session".to_owned(),
        ]);
        a
    }
}

impl CaptureTool for PresentMonTool {
    fn version(&self) -> String {
        self.version.clone()
    }

    fn capture(&self, req: &CaptureRequest) -> Result<()> {
        validate_exe_name(&req.exe)?;
        validate_timing(req.seconds, req.delay_seconds)?;
        if self.verify && !super::presentmon::is_pinned_file(&self.exe, &super::presentmon::pinned()) {
            return Err(EngineError::Command {
                what: "PresentMon".into(),
                exit_code: None,
                detail: format!(
                    "{} is not the pinned PresentMon release; refusing to run it",
                    self.exe.display()
                ),
            });
        }
        let fail = |detail: String, code: Option<i32>| EngineError::Command {
            what: "PresentMon".into(),
            exit_code: code,
            detail,
        };

        let mut cmd = Command::new(&self.exe);
        cmd.args(Self::args(req));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let limit = self
            .limit_override
            .unwrap_or_else(|| Duration::from_secs(u64::from(req.delay_seconds + req.seconds)) + GRACE);
        let out = run_limited(cmd, "PresentMon", limit, Duration::from_millis(100))?;
        if out.exit_code != Some(0) {
            return Err(fail(out.failure_detail(), out.exit_code));
        }
        match std::fs::metadata(&req.out_csv) {
            Ok(m) if m.len() > 0 => Ok(()),
            _ => Err(fail(
                "PresentMon finished but wrote no data. Is the game running, and drawing frames in the foreground?"
                    .into(),
                out.exit_code,
            )),
        }
    }
}

/// Stands in when no PresentMon could be provided, so the failure is reported
/// where the user asked for a capture, with the reason.
pub struct UnavailableTool(pub String);

impl CaptureTool for UnavailableTool {
    fn version(&self) -> String {
        "unavailable".into()
    }

    fn capture(&self, _req: &CaptureRequest) -> Result<()> {
        Err(EngineError::Command {
            what: "PresentMon".into(),
            exit_code: None,
            detail: self.0.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Fake for tests
// ---------------------------------------------------------------------------

#[cfg(any(test, feature = "test-support"))]
pub use fake::FakeCapture;

#[cfg(any(test, feature = "test-support"))]
mod fake {
    use std::sync::Mutex;

    use super::*;

    type FrameSource = Box<dyn Fn(usize, &CaptureRequest) -> Vec<f64> + Send + Sync>;

    /// Writes a PresentMon-shaped CSV whose frame times come from a closure
    /// `(call_index, request) -> frame times in ms`.
    pub struct FakeCapture {
        frames: FrameSource,
        pub calls: Mutex<Vec<CaptureRequest>>,
    }

    impl FakeCapture {
        pub fn new(frames: impl Fn(usize, &CaptureRequest) -> Vec<f64> + Send + Sync + 'static) -> Self {
            Self {
                frames: Box::new(frames),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl CaptureTool for FakeCapture {
        fn version(&self) -> String {
            "fake".into()
        }

        fn capture(&self, req: &CaptureRequest) -> Result<()> {
            validate_exe_name(&req.exe)?;
            validate_timing(req.seconds, req.delay_seconds)?;
            let index = {
                let mut calls = self.calls.lock().unwrap();
                calls.push(req.clone());
                calls.len() - 1
            };
            let mut csv = String::from("Application,ProcessID,SwapChainAddress,MsBetweenPresents,DisplayedTime\n");
            for ms in (self.frames)(index, req) {
                csv.push_str(&format!("{},100,0x1,{ms},{ms}\n", req.exe));
            }
            if let Some(dir) = req.out_csv.parent() {
                std::fs::create_dir_all(dir).map_err(|e| EngineError::storage(dir.display().to_string(), e))?;
            }
            std::fs::write(&req.out_csv, csv).map_err(|e| EngineError::storage(req.out_csv.display().to_string(), e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_names_that_could_be_options_or_paths_are_refused() {
        for ok in [
            "Game.exe",
            "FortniteClient-Win64-Shipping.exe",
            "my game (x64).exe",
            "GAME.EXE",
            "a_b+c.exe",
        ] {
            assert!(validate_exe_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            ".exe",
            "-rf.exe",
            "--output_file.exe",
            "..\\evil.exe",
            "C:\\Games\\g.exe",
            "dir/g.exe",
            "g.exe.txt",
            "g",
            "g.exe; calc",
            "g\u{0}.exe",
            "g\".exe",
            ".hidden.exe",
            &format!("{}.exe", "a".repeat(80)),
        ] {
            assert!(validate_exe_name(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn timing_bounds() {
        assert!(validate_timing(10, 0).is_ok());
        assert!(validate_timing(600, 120).is_ok());
        assert!(validate_timing(9, 0).is_err());
        assert!(validate_timing(601, 0).is_err());
        assert!(validate_timing(60, 121).is_err());
    }

    #[test]
    fn the_command_line_is_exactly_what_presentmon_documents() {
        let req = CaptureRequest {
            exe: "Game.exe".into(),
            delay_seconds: 10,
            seconds: 60,
            out_csv: PathBuf::from("out.csv"),
        };
        assert_eq!(
            PresentMonTool::args(&req),
            [
                "--process_name",
                "Game.exe",
                "--output_file",
                "out.csv",
                "--delay",
                "10",
                "--timed",
                "60",
                "--terminate_after_timed",
                "--no_console_stats",
                "--session_name",
                "PeakTweaks",
                "--stop_existing_session",
            ]
        );
        let no_delay = PresentMonTool::args(&CaptureRequest {
            delay_seconds: 0,
            ..req
        });
        assert!(!no_delay.iter().any(|a| a == "--delay"));
    }

    /// Every flag we pass appears in PresentMon's own README (fetched for the
    /// pinned release), so a rename upstream is caught by a test, not a user.
    #[test]
    fn every_flag_we_use_is_documented_upstream() {
        const README: &str = include_str!("../../../../vendor/presentmon/README-ConsoleApplication.md");
        let req = CaptureRequest {
            exe: "Game.exe".into(),
            delay_seconds: 1,
            seconds: 10,
            out_csv: PathBuf::from("o.csv"),
        };
        for arg in PresentMonTool::args(&req).iter().filter(|a| a.starts_with("--")) {
            assert!(
                README.contains(&format!("`{arg}")),
                "{arg} is not in PresentMon's README"
            );
        }
        assert!(README.contains("MsBetweenPresents"));
        assert!(README.contains("SwapChainAddress"));
    }

    #[cfg(unix)]
    mod with_a_stand_in_program {
        use std::os::unix::fs::PermissionsExt;

        use super::*;

        /// Writing an executable and running it while another thread forks can
        /// fail with ETXTBSY ("Text file busy"), because the fork inherits the
        /// still-open write handle. These tests take turns.
        static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

        fn script(dir: &std::path::Path, body: &str) -> PathBuf {
            let path = dir.join("fake-presentmon.sh");
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn request(dir: &std::path::Path) -> CaptureRequest {
            CaptureRequest {
                exe: "Game.exe".into(),
                delay_seconds: 0,
                seconds: 10,
                out_csv: dir.join("out.csv"),
            }
        }

        #[test]
        fn a_successful_run_needs_a_non_empty_csv() {
            let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
            let d = tempfile::tempdir().unwrap();
            // Writes the file named after --output_file.
            let ok = script(
                d.path(),
                r#"while [ $# -gt 0 ]; do [ "$1" = "--output_file" ] && out="$2"; shift; done; echo "MsBetweenPresents" > "$out""#,
            );
            PresentMonTool::stand_in(ok).capture(&request(d.path())).unwrap();

            let empty = script(d.path(), "exit 0");
            std::fs::remove_file(d.path().join("out.csv")).unwrap();
            let e = PresentMonTool::stand_in(empty).capture(&request(d.path())).unwrap_err();
            assert!(
                matches!(&e, EngineError::Command { detail, .. } if detail.contains("wrote no data")),
                "{e:?}"
            );
        }

        #[test]
        fn a_failing_run_returns_its_stderr_and_exit_code() {
            let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
            let d = tempfile::tempdir().unwrap();
            let bad = script(d.path(), "echo 'access denied' >&2; exit 5");
            let e = PresentMonTool::stand_in(bad).capture(&request(d.path())).unwrap_err();
            assert!(
                matches!(&e, EngineError::Command { exit_code: Some(5), detail, .. } if detail.contains("access denied")),
                "{e:?}"
            );
        }

        #[test]
        fn a_program_that_is_not_the_pinned_release_is_never_run() {
            let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
            let d = tempfile::tempdir().unwrap();
            let marker = d.path().join("ran");
            let sneaky = script(d.path(), &format!("touch {}", marker.display()));
            let e = PresentMonTool::new(sneaky).capture(&request(d.path())).unwrap_err();
            assert!(
                matches!(&e, EngineError::Command { detail, .. } if detail.contains("not the pinned")),
                "{e:?}"
            );
            assert!(!marker.exists(), "the unverified program must not have executed");
        }

        #[test]
        fn a_hung_run_is_killed_at_the_limit() {
            let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
            let d = tempfile::tempdir().unwrap();
            let hang = script(d.path(), "sleep 60");
            let mut tool = PresentMonTool::stand_in(hang);
            tool.limit_override = Some(Duration::from_secs(1));
            let t = std::time::Instant::now();
            let e = tool.capture(&request(d.path())).unwrap_err();
            assert!(t.elapsed() < Duration::from_secs(10), "took {:?}", t.elapsed());
            assert!(
                matches!(&e, EngineError::Command { detail, .. } if detail.contains("was stopped")),
                "{e:?}"
            );
        }
    }

    /// End to end on real frames: our tool runs the real, pinned PresentMon
    /// against a program that is drawing, and the parser and statistics run on
    /// what it wrote. CI starts a browser window with an animation and sets
    /// `PEAKTWEAKS_REAL_CAPTURE_EXE`; anywhere else this test does nothing.
    #[cfg(windows)]
    #[test]
    fn real_presentmon_captures_real_frames_through_our_tool() {
        let Ok(exe) = std::env::var("PEAKTWEAKS_REAL_CAPTURE_EXE") else {
            println!("skipped: PEAKTWEAKS_REAL_CAPTURE_EXE is not set");
            return;
        };
        let strict = std::env::var_os("PEAKTWEAKS_REAL_CAPTURE_STRICT").is_some();
        let pm = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/presentmon/PresentMon-x64.exe");
        let dir = tempfile::tempdir().unwrap();
        let req = CaptureRequest {
            exe,
            delay_seconds: 2,
            seconds: 10,
            out_csv: dir.path().join("real.csv"),
        };
        let tool = PresentMonTool::new(pm);
        let captured = tool.capture(&req);
        println!("capture: {captured:?}");
        if let Err(e) = &captured {
            assert!(!strict, "strict mode: the capture must work: {e}");
            println!("NOT VERIFIED: no frames could be captured on this machine");
            return;
        }
        let text = std::fs::read_to_string(&req.out_csv).unwrap();
        println!("header: {}", text.lines().next().unwrap_or(""));
        for row in text.lines().skip(1).take(3) {
            println!("row: {row}");
        }
        let frames = crate::proof::metrics::parse_frame_times(&text).expect("the real CSV must parse");
        println!(
            "frames: {} (ignored {} other-swap-chain rows, {} unusable rows)",
            frames.ms.len(),
            frames.ignored_rows,
            frames.unusable_rows
        );
        match crate::proof::metrics::compute_stats(&frames.ms) {
            Ok(stats) => println!("stats: {stats:#?}"),
            Err(e) => {
                assert!(!strict, "strict mode: need enough frames: {e}");
                println!("NOT VERIFIED beyond parsing: {e}");
            }
        }
    }
}
