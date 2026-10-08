//! Optimize the Windows drive now (catalogue H29, Hone's drive optimisation):
//! Windows' own `defrag <drive> /O` ("performs the proper optimization for
//! each media type"), the kind of job Optimize Drives runs as a maintenance
//! task, "which typically runs every week" (Microsoft's `defrag` page). On an
//! SSD it tells the drive which space is free (TRIM); on a hard drive it
//! defragments (VERIFY: Microsoft spells out that split for `Optimize-Volume`,
//! not for `/O`), which can take an hour or more. No setting changes, so there is nothing to undo and no
//! restore point is needed; the journal keeps a line for the history
//! (`DriveOptimization::done`).
//!
//! `defrag.exe` runs from System32 by full path, never looked up on `PATH` or
//! next to the app, and the drive must be a plain `X:`.

use std::time::{Duration, Instant};

use serde::Serialize;
use ts_rs::TS;

use super::error::{EngineError, Result};
use super::journal::ActionDone;
use super::proc::Output;

/// A hard drive can take hours; past this the run is stopped. Windows moves
/// one file at a time, so stopping part way leaves every file whole (VERIFY,
/// NOTES N71).
pub const LIMIT: Duration = Duration::from_secs(4 * 60 * 60);

/// How many lines of Windows' report are kept for the technical view.
const REPORT_LINES: usize = 40;

/// The name a failure goes by on screen: "Drive optimization did not finish."
const WHAT: &str = "Drive optimization";

/// One run of Windows' drive optimisation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct DriveOptimization {
    /// The drive, `C:`.
    pub drive: String,
    pub unix_ms: u64,
    /// How long Windows took.
    pub seconds: u64,
    /// What `defrag` printed, in the PC's language, for the technical view.
    pub report: Vec<String>,
}

impl DriveOptimization {
    /// The line the journal keeps for the history, without Windows' report.
    pub fn done(&self) -> ActionDone {
        ActionDone::OptimizeDrive {
            drive: self.drive.clone(),
            seconds: self.seconds,
        }
    }
}

/// Runs `defrag` with these arguments: the real one on Windows.
pub trait Defrag: Send + Sync {
    fn run(&self, args: &[&str], limit: Duration) -> Result<Output>;
}

/// Optimize `drive` (`C:`) the way Windows' own schedule does.
pub fn optimize(defrag: &dyn Defrag, drive: &str, unix_ms: u64) -> Result<DriveOptimization> {
    if !plain_drive(drive) {
        return Err(EngineError::Internal {
            detail: format!("not a drive letter: {drive:?}"),
        });
    }
    let started = Instant::now();
    let out = defrag.run(&[drive, "/O", "/V"], LIMIT)?;
    let report = report_lines(&out.stdout);
    if out.exit_code != Some(0) {
        // The reason is at the end; the start is a banner.
        let tail = report_lines(&format!("{}\n{}", out.stdout, out.stderr));
        return Err(EngineError::Command {
            what: WHAT.into(),
            exit_code: out.exit_code,
            detail: format!("defrag {drive} /O: {}", tail[tail.len().saturating_sub(2)..].join(" ")),
        });
    }
    Ok(DriveOptimization {
        drive: drive.to_owned(),
        unix_ms,
        seconds: started.elapsed().as_secs(),
        report,
    })
}

/// `C:` and nothing else: it goes on a command line.
fn plain_drive(drive: &str) -> bool {
    matches!(drive.as_bytes(), [letter, b':'] if letter.is_ascii_alphabetic())
}

/// Non-empty lines, last `REPORT_LINES` of them. Progress is redrawn with
/// carriage returns, so those split lines too.
fn report_lines(text: &str) -> Vec<String> {
    let lines: Vec<String> = text
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    lines[lines.len().saturating_sub(REPORT_LINES)..].to_vec()
}

/// The real `defrag.exe` on Windows; elsewhere, a refusal.
pub fn system() -> Box<dyn Defrag> {
    #[cfg(windows)]
    {
        Box::new(WinDefrag)
    }
    #[cfg(not(windows))]
    {
        Box::new(Unavailable)
    }
}

/// Builds that are not Windows have no drive optimisation.
#[cfg(not(windows))]
struct Unavailable;

#[cfg(not(windows))]
impl Defrag for Unavailable {
    fn run(&self, _args: &[&str], _limit: Duration) -> Result<Output> {
        Err(EngineError::Internal {
            detail: "drive optimisation exists only on Windows".into(),
        })
    }
}

#[cfg(not(windows))]
pub fn windows_drive() -> Result<String> {
    Err(EngineError::Internal {
        detail: "drive optimisation exists only on Windows".into(),
    })
}

/// The drive Windows is on, as `C:`.
#[cfg(windows)]
pub fn windows_drive() -> Result<String> {
    let dir = crate::sysdirs::windows_dir().map_err(|e| EngineError::Internal {
        detail: format!("could not find the Windows folder: {e}"),
    })?;
    let drive: String = dir.to_string_lossy().chars().take(2).collect();
    if plain_drive(&drive) {
        Ok(drive.to_uppercase())
    } else {
        Err(EngineError::Internal {
            detail: format!("Windows is not on a drive letter: {}", dir.display()),
        })
    }
}

/// The real `defrag.exe`, from System32, without a window.
#[cfg(windows)]
pub struct WinDefrag;

#[cfg(windows)]
impl Defrag for WinDefrag {
    fn run(&self, args: &[&str], limit: Duration) -> Result<Output> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let fail = |detail: String| EngineError::Command {
            what: WHAT.into(),
            exit_code: None,
            detail,
        };
        let exe = crate::sysdirs::system32()
            .map_err(|e| fail(format!("could not find System32: {e}")))?
            .join("defrag.exe");
        if !exe.is_file() {
            return Err(fail(format!("{} does not exist", exe.display())));
        }
        let mut cmd = std::process::Command::new(exe);
        cmd.args(args).creation_flags(CREATE_NO_WINDOW);
        super::proc::run_limited(cmd, WHAT, limit, Duration::from_millis(250))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct Fake {
        out: Output,
        args: Mutex<Vec<String>>,
    }

    impl Defrag for Fake {
        fn run(&self, args: &[&str], limit: Duration) -> Result<Output> {
            assert_eq!(limit, LIMIT);
            *self.args.lock().unwrap() = args.iter().map(|a| a.to_string()).collect();
            Ok(self.out.clone())
        }
    }

    fn fake(exit_code: i32, stdout: &str) -> Fake {
        Fake {
            out: Output {
                exit_code: Some(exit_code),
                stdout: stdout.into(),
                stderr: String::new(),
            },
            args: Mutex::new(Vec::new()),
        }
    }

    const BANNER: &str = "Microsoft Drive Optimizer\r\nCopyright (c) Microsoft Corp.\r\n\r\n";

    #[test]
    fn runs_windows_own_optimisation_on_the_drive_and_keeps_its_report() {
        let f = fake(0, &format!("{BANNER}Invoking retrim on (C:)...\r\n\tRetrim:  10% complete...\r\tRetrim:  100% complete.\r\n\r\nThe operation completed successfully.\r\n"));
        let out = optimize(&f, "C:", 7).unwrap();
        assert_eq!(*f.args.lock().unwrap(), ["C:", "/O", "/V"]);
        assert_eq!((out.drive.as_str(), out.unix_ms), ("C:", 7));
        assert_eq!(out.report.last().unwrap(), "The operation completed successfully.");
        assert!(out.report.contains(&"Retrim:  100% complete.".to_string()));
    }

    #[test]
    fn a_failure_says_why_from_the_end_of_the_report() {
        let f = fake(
            -1_996_488_662,
            &format!("{BANNER}Invoking slab consolidation on (C:)...\r\nThe operation requested is not supported by the hardware backing the volume. (0x8900002A)\r\n"),
        );
        let EngineError::Command {
            what,
            exit_code,
            detail,
        } = optimize(&f, "C:", 0).unwrap_err()
        else {
            panic!("expected a command error");
        };
        assert_eq!((what.as_str(), exit_code), (WHAT, Some(-1_996_488_662)));
        assert!(detail.starts_with("defrag C: /O: Invoking slab"), "{detail}");
        assert!(detail.ends_with("(0x8900002A)"), "{detail}");
        assert!(!detail.contains("Copyright"), "{detail}");
    }

    #[test]
    fn only_a_plain_drive_letter_reaches_the_command_line() {
        let f = fake(0, "");
        for bad in ["", "C", "C:\\", "C: /D", "1:", "CC:", "\\\\?\\C:"] {
            assert!(optimize(&f, bad, 0).is_err(), "{bad:?} was accepted");
        }
        assert!(f.args.lock().unwrap().is_empty());
        assert!(optimize(&f, "d:", 0).is_ok());
    }

    #[test]
    fn the_report_keeps_the_last_lines() {
        let many: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let lines = report_lines(&many);
        assert_eq!(lines.len(), REPORT_LINES);
        assert_eq!(lines.last().unwrap(), "line 99");
    }

    /// Analysis only (`/A`), which changes nothing: shows `defrag.exe` runs
    /// from System32 elevated and its report comes back.
    #[cfg(windows)]
    #[test]
    fn defrag_runs_from_system32_on_this_pc() {
        if !crate::identity::is_elevated() {
            eprintln!("SKIPPED: defrag needs an elevated process");
            return;
        }
        let drive = windows_drive().unwrap();
        let out = WinDefrag
            .run(&[&drive, "/A", "/V"], Duration::from_secs(10 * 60))
            .expect("defrag starts");
        eprintln!(
            "defrag {drive} /A exit {:?}:\n{}",
            out.exit_code,
            report_lines(&format!("{}\n{}", out.stdout, out.stderr)).join("\n")
        );
        assert!(out.exit_code.is_some());
    }
}
