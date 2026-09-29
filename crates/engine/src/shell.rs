//! Running PowerShell for the few things Windows only exposes that way.
//!
//! Scripts are `&'static str`, so they are fixed at compile time: there is no
//! way to interpolate anything from the webview, a game name or a file path into
//! a command line. The executable is addressed by absolute path under
//! `%SystemRoot%`, not looked up on `PATH`. Every call has a timeout, and the
//! exit code and stderr come back to the caller.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::error::{EngineError, Result};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub struct ShellOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn powershell_path() -> Result<PathBuf> {
    let root = std::env::var_os("SystemRoot").ok_or_else(|| EngineError::Command {
        what: "PowerShell".into(),
        exit_code: None,
        detail: "%SystemRoot% is not set".into(),
    })?;
    let path = PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
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

fn drain<R: Read + Send + 'static>(mut r: R) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// Run a fixed script. `what` names it in errors. A non-zero exit is returned as
/// data (`ShellOutput`), not an error; a timeout or failure to start is an error.
pub fn run_powershell(what: &'static str, script: &'static str, timeout: Duration) -> Result<ShellOutput> {
    let fail = |detail: String, code: Option<i32>| EngineError::Command {
        what: what.into(),
        exit_code: code,
        detail,
    };

    let mut child = Command::new(powershell_path()?)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| fail(format!("could not start PowerShell: {e}"), None))?;

    // Read both pipes on their own threads so a chatty child cannot block on a
    // full pipe while we wait for it.
    let out = drain(child.stdout.take().expect("piped stdout"));
    let err = drain(child.stderr.take().expect("piped stderr"));

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out.join();
                let _ = err.join();
                return Err(fail(
                    format!("did not finish within {} s and was stopped", timeout.as_secs()),
                    None,
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(fail(format!("could not wait for PowerShell: {e}"), None)),
        }
    };

    Ok(ShellOutput {
        exit_code: status.code(),
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// Like `run_powershell`, but a non-zero exit becomes an error carrying stderr.
pub fn run_powershell_checked(what: &'static str, script: &'static str, timeout: Duration) -> Result<ShellOutput> {
    let out = run_powershell(what, script, timeout)?;
    if out.exit_code == Some(0) {
        return Ok(out);
    }
    let detail = [out.stderr.trim(), out.stdout.trim()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("no output")
        .lines()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");
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
        let t = Instant::now();
        let err = run_powershell("test", "Start-Sleep -Seconds 60", Duration::from_secs(2)).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(20), "took {:?}", t.elapsed());
        assert!(
            matches!(&err, EngineError::Command { detail, .. } if detail.contains("stopped")),
            "{err:?}"
        );
    }
}
