//! Repair Windows (game plan 2026-10-22, C): Windows' own repair tools, run
//! in the order Microsoft's "Use the System File Checker tool" page gives.
//! First `DISM /Online /Cleanup-Image /RestoreHealth` repairs the store
//! Windows keeps its system files in, then `sfc /scannow` checks every
//! protected system file against that store and replaces any that differ.
//!
//! It changes Windows' own files, not settings, so there is no Undo; the
//! engine asks for a verified restore point first, as for a driver install,
//! and the journal keeps a line for the history (`WindowsRepair::done`). DISM
//! can fetch the files it needs from Windows Update: that is Windows' tool
//! using the network, not PeakTweaks (NOTES N146).
//!
//! Both run from System32 by full path, never looked up on `PATH`.
//!
//! VERIFY (NOTES N146): that `sfc` writes UTF-16 when its output is piped
//! (the NUL bytes are dropped here so English text reads), and which exit
//! codes it uses; its result is shown as Windows' own last line, not
//! interpreted.

use std::time::{Duration, Instant};

use serde::Serialize;
use ts_rs::TS;

use super::error::{EngineError, Result};
use super::journal::ActionDone;
use super::proc::Output;

/// Each tool can take a long time on a slow drive; past this it is stopped.
pub const LIMIT: Duration = Duration::from_secs(2 * 60 * 60);

/// How many lines of each tool's report are kept for the technical view.
const REPORT_LINES: usize = 30;

/// The name a failure goes by on screen: "Windows repair did not finish."
const WHAT: &str = "Windows repair";

/// Windows' repair tools, in System32.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairTool {
    Dism,
    Sfc,
}

impl RepairTool {
    pub const fn exe(self) -> &'static str {
        match self {
            Self::Dism => "dism.exe",
            Self::Sfc => "sfc.exe",
        }
    }
}

pub const DISM_ARGS: &[&str] = &["/Online", "/Cleanup-Image", "/RestoreHealth"];
pub const SFC_ARGS: &[&str] = &["/scannow"];

/// What one tool did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RepairStep {
    pub exit_code: Option<i32>,
    /// The tool's last line, in the PC's language: its own verdict.
    pub summary: String,
    /// Its last lines, for the technical view.
    pub report: Vec<String>,
}

/// One run of Windows' repair tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct WindowsRepair {
    pub unix_ms: u64,
    /// How long both took.
    pub seconds: u64,
    pub dism: RepairStep,
    pub sfc: RepairStep,
}

impl WindowsRepair {
    /// The line the journal keeps for the history.
    pub fn done(&self) -> ActionDone {
        ActionDone::RepairWindows {
            seconds: self.seconds,
            sfc_summary: self.sfc.summary.clone(),
        }
    }
}

/// Runs a repair tool with these arguments: the real ones on Windows.
pub trait RepairRunner: Send + Sync {
    fn run(&self, tool: RepairTool, args: &[&str], limit: Duration) -> Result<Output>;
}

/// DISM, then SFC. A DISM failure stops there, with DISM's reason: SFC
/// repairs from the store DISM could not fix. SFC's own outcome, whatever it
/// is, is reported as Windows words it.
pub fn repair(runner: &dyn RepairRunner, unix_ms: u64) -> Result<WindowsRepair> {
    let started = Instant::now();
    let dism = step(&runner.run(RepairTool::Dism, DISM_ARGS, LIMIT)?);
    if dism.exit_code != Some(0) {
        let tail = &dism.report[dism.report.len().saturating_sub(2)..];
        return Err(EngineError::Command {
            what: WHAT.into(),
            exit_code: dism.exit_code,
            detail: format!("DISM /RestoreHealth: {}", tail.join(" ")),
        });
    }
    let sfc = step(&runner.run(RepairTool::Sfc, SFC_ARGS, LIMIT)?);
    Ok(WindowsRepair {
        unix_ms,
        seconds: started.elapsed().as_secs(),
        dism,
        sfc,
    })
}

fn step(out: &Output) -> RepairStep {
    let report = report_lines(&format!("{}\n{}", out.stdout, out.stderr));
    RepairStep {
        exit_code: out.exit_code,
        summary: report.last().cloned().unwrap_or_default(),
        report,
    }
}

/// Non-empty lines, last `REPORT_LINES` of them, without NUL bytes (UTF-16
/// read as UTF-8). Progress is redrawn with carriage returns, so those split
/// lines too.
fn report_lines(text: &str) -> Vec<String> {
    let text = text.replace('\0', "");
    let lines: Vec<String> = text
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    lines[lines.len().saturating_sub(REPORT_LINES)..].to_vec()
}

/// The real tools on Windows; elsewhere, a refusal.
pub fn system() -> Box<dyn RepairRunner> {
    #[cfg(windows)]
    {
        Box::new(WinRepair)
    }
    #[cfg(not(windows))]
    {
        Box::new(Unavailable)
    }
}

/// Builds that are not Windows have no Windows to repair.
#[cfg(not(windows))]
struct Unavailable;

#[cfg(not(windows))]
impl RepairRunner for Unavailable {
    fn run(&self, _tool: RepairTool, _args: &[&str], _limit: Duration) -> Result<Output> {
        Err(EngineError::Internal {
            detail: "Windows repair exists only on Windows".into(),
        })
    }
}

/// The real tools, from System32, without a window.
#[cfg(windows)]
pub struct WinRepair;

#[cfg(windows)]
impl RepairRunner for WinRepair {
    fn run(&self, tool: RepairTool, args: &[&str], limit: Duration) -> Result<Output> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let fail = |detail: String| EngineError::Command {
            what: WHAT.into(),
            exit_code: None,
            detail,
        };
        let exe = crate::sysdirs::system32()
            .map_err(|e| fail(format!("could not find System32: {e}")))?
            .join(tool.exe());
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
        dism: Output,
        sfc: Output,
        calls: Mutex<Vec<String>>,
    }

    impl RepairRunner for Fake {
        fn run(&self, tool: RepairTool, args: &[&str], limit: Duration) -> Result<Output> {
            assert_eq!(limit, LIMIT);
            self.calls
                .lock()
                .unwrap()
                .push(format!("{} {}", tool.exe(), args.join(" ")));
            Ok(match tool {
                RepairTool::Dism => self.dism.clone(),
                RepairTool::Sfc => self.sfc.clone(),
            })
        }
    }

    fn out(exit_code: i32, stdout: &str) -> Output {
        Output {
            exit_code: Some(exit_code),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    const DISM_OK: &str = "Deployment Image Servicing and Management tool\r\nVersion: 10.0.26100.1\r\n\r\nImage Version: 10.0.26100.2033\r\n\r\n[==========================100.0%==========================] The restore operation completed successfully.\r\nThe operation completed successfully.\r\n";

    /// SFC's output as UTF-16LE read as UTF-8: a NUL after each character.
    fn utf16ish(text: &str) -> String {
        text.chars().flat_map(|c| [c, '\0']).collect()
    }

    #[test]
    fn dism_runs_first_then_sfc_and_sfc_has_the_last_word() {
        let f = Fake {
            dism: out(0, DISM_OK),
            sfc: out(
                0,
                &utf16ish("Beginning system scan.\r\nVerification 100% complete.\r\n\r\nWindows Resource Protection did not find any integrity violations.\r\n"),
            ),
            calls: Mutex::new(Vec::new()),
        };
        let r = repair(&f, 7).unwrap();
        assert_eq!(
            *f.calls.lock().unwrap(),
            ["dism.exe /Online /Cleanup-Image /RestoreHealth", "sfc.exe /scannow"]
        );
        assert_eq!(r.unix_ms, 7);
        assert_eq!(r.dism.summary, "The operation completed successfully.");
        assert_eq!(
            r.sfc.summary,
            "Windows Resource Protection did not find any integrity violations."
        );
        assert_eq!(
            r.done(),
            ActionDone::RepairWindows {
                seconds: r.seconds,
                sfc_summary: r.sfc.summary.clone()
            }
        );
    }

    #[test]
    fn a_dism_failure_stops_before_sfc_with_dism_s_reason() {
        let f = Fake {
            dism: out(
                -2_146_498_529,
                "Image Version: 10.0.26100.2033\r\n\r\nError: 0x800f081f\r\n\r\nThe source files could not be found.\r\n",
            ),
            sfc: out(0, ""),
            calls: Mutex::new(Vec::new()),
        };
        let EngineError::Command {
            what,
            exit_code,
            detail,
        } = repair(&f, 0).unwrap_err()
        else {
            panic!("expected a command error");
        };
        assert_eq!((what.as_str(), exit_code), (WHAT, Some(-2_146_498_529)));
        assert_eq!(
            detail,
            "DISM /RestoreHealth: Error: 0x800f081f The source files could not be found."
        );
        assert_eq!(f.calls.lock().unwrap().len(), 1, "SFC did not run");
    }

    #[test]
    fn sfc_s_own_outcome_is_reported_whatever_its_exit_code() {
        let f = Fake {
            dism: out(0, DISM_OK),
            sfc: out(
                1,
                "Windows Resource Protection found corrupt files but was unable to fix some of them.\r\n",
            ),
            calls: Mutex::new(Vec::new()),
        };
        let r = repair(&f, 0).unwrap();
        assert_eq!(r.sfc.exit_code, Some(1));
        assert!(r.sfc.summary.contains("unable to fix some"));
    }

    #[test]
    fn the_report_keeps_the_last_lines() {
        let many: String = (0..100).map(|i| format!("line {i}\r\n")).collect();
        let lines = report_lines(&many);
        assert_eq!(lines.len(), REPORT_LINES);
        assert_eq!(lines.last().unwrap(), "line 99");
    }

    /// Read-only: `DISM /CheckHealth` changes nothing and shows DISM runs
    /// from System32 elevated and its report comes back.
    #[cfg(windows)]
    #[test]
    fn dism_runs_from_system32_on_this_pc() {
        if !crate::identity::is_elevated() {
            eprintln!("SKIPPED: DISM needs an elevated process");
            return;
        }
        let out = WinRepair
            .run(
                RepairTool::Dism,
                &["/Online", "/Cleanup-Image", "/CheckHealth"],
                Duration::from_secs(10 * 60),
            )
            .expect("DISM starts");
        eprintln!(
            "DISM /CheckHealth exit {:?}:\n{}",
            out.exit_code,
            report_lines(&format!("{}\n{}", out.stdout, out.stderr)).join("\n")
        );
        assert!(out.exit_code.is_some());
    }
}
