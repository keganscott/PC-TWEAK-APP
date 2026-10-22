//! Running PowerShell for the few things Windows only exposes that way.
//!
//! Scripts are `&'static str`, so they are fixed at compile time: there is no
//! way to interpolate anything from the webview, a game name or a file path into
//! a command line. The executable is addressed by absolute path under
//! System32 as Windows reports it (not `%SystemRoot%`, which whoever starts
//! PeakTweaks can set), never looked up on `PATH`. Every call has a timeout, and the
//! exit code and stderr come back to the caller.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::error::{EngineError, Result};
use super::proc::run_limited;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub struct ShellOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn powershell_path() -> Result<PathBuf> {
    let dir = crate::sysdirs::system32().map_err(|e| EngineError::Command {
        what: "PowerShell".into(),
        exit_code: None,
        detail: format!("could not find the System32 folder: {e}"),
    })?;
    let path = dir.join(r"WindowsPowerShell\v1.0\powershell.exe");
    if path.is_file() {
        Ok(path)
    } else {
        Err(EngineError::Command {
            what: "PowerShell".into(),
            exit_code: None,
            detail: format!("{} does not exist", path.display()),
        })
    }
}

/// Run a fixed script. `what` names it in errors. A non-zero exit is returned as
/// data (`ShellOutput`), not an error; a timeout or failure to start is an error.
pub fn run_powershell(what: &'static str, script: &'static str, timeout: Duration) -> Result<ShellOutput> {
    let mut cmd = Command::new(powershell_path()?);
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW);
    let out = run_limited(cmd, what, timeout, Duration::from_millis(50))?;
    Ok(ShellOutput {
        exit_code: out.exit_code,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// Like `run_powershell`, but a non-zero exit becomes an error carrying stderr.
pub fn run_powershell_checked(what: &'static str, script: &'static str, timeout: Duration) -> Result<ShellOutput> {
    let out = run_powershell(what, script, timeout)?;
    if out.exit_code == Some(0) {
        return Ok(out);
    }
    let detail = crate::proc::Output {
        exit_code: out.exit_code,
        stdout: out.stdout,
        stderr: out.stderr,
    }
    .failure_detail();
    Err(EngineError::Command {
        what: what.into(),
        exit_code: out.exit_code,
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_a_script_and_captures_output_and_exit_code() {
        let ok = run_powershell("test", "Write-Output 'hello'", Duration::from_secs(60)).unwrap();
        assert_eq!(ok.exit_code, Some(0));
        assert_eq!(ok.stdout.trim(), "hello");

        let bad = run_powershell(
            "test",
            "[Console]::Error.WriteLine('oops'); exit 3",
            Duration::from_secs(60),
        )
        .unwrap();
        assert_eq!(bad.exit_code, Some(3));
        assert!(bad.stderr.contains("oops"), "{bad:?}");

        let checked =
            run_powershell_checked("test", "Write-Error 'nope'; exit 1", Duration::from_secs(60)).unwrap_err();
        assert!(
            matches!(checked, EngineError::Command { exit_code: Some(1), .. }),
            "{checked:?}"
        );
    }

    #[test]
    fn a_hung_script_is_stopped_at_the_timeout() {
        let t = std::time::Instant::now();
        let err = run_powershell("test", "Start-Sleep -Seconds 60", Duration::from_secs(2)).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(20), "took {:?}", t.elapsed());
        assert!(
            matches!(&err, EngineError::Command { detail, .. } if detail.contains("stopped")),
            "{err:?}"
        );
    }
}
