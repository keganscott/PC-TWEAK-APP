//! Running an external program with a time limit. Shared by the PowerShell
//! runner (`shell.rs`) and the PresentMon capture (`proof/capture.rs`), so a fix
//! to how a stuck child is stopped lands in both.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::error::{EngineError, Result};

#[derive(Debug, Clone)]
pub struct Output {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn drain<R: Read + Send + 'static>(mut r: R) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// Run `cmd` to completion or until `limit`. Stdin is closed and both output
/// pipes are read on their own threads, so a chatty child cannot block on a full
/// pipe while we wait. A non-zero exit is returned as data; a failure to start,
/// a failure to wait, or the time limit is an error named `what`.
pub fn run_limited(mut cmd: Command, what: &str, limit: Duration, poll: Duration) -> Result<Output> {
    let fail = |detail: String| EngineError::Command {
        what: what.into(),
        exit_code: None,
        detail,
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| fail(format!("could not start {what}: {e}")))?;
    let out = drain(child.stdout.take().expect("piped stdout"));
    let err = drain(child.stderr.take().expect("piped stderr"));

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                // Do not join the readers: a grandchild that inherited the pipes
                // would keep them open and turn "stopped" into "waited for it
                // anyway". They end when the pipes close.
                drop((out, err));
                return Err(fail(format!(
                    "did not finish within {} s and was stopped",
                    limit.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(poll),
            Err(e) => return Err(fail(format!("could not wait for {what}: {e}"))),
        }
    };
    // The pipes close when the child exits, unless a process it started
    // inherited them and runs on (an installer handing over to its setup):
    // then the output read so far is not waited for past `AFTER_EXIT`.
    let deadline = Instant::now() + AFTER_EXIT;
    Ok(Output {
        exit_code: status.code(),
        stdout: collect(out, deadline),
        stderr: collect(err, deadline),
    })
}

/// How long the output readers may run on after the child has exited.
const AFTER_EXIT: Duration = Duration::from_secs(5);

/// The reader's text if it ends by `deadline`; otherwise it is left to end
/// when the pipe closes, and nothing is waited for.
fn collect(reader: std::thread::JoinHandle<String>, deadline: Instant) -> String {
    while !reader.is_finished() {
        if Instant::now() >= deadline {
            return String::new();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    reader.join().unwrap_or_default()
}

impl Output {
    /// The first non-empty of stderr and stdout, at most six lines, on one line.
    pub fn failure_detail(&self) -> String {
        [self.stderr.trim(), self.stdout.trim()]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or("no output")
            .lines()
            .take(6)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn output_is_returned_when_the_child_exits() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo hi; echo oops >&2; exit 3"]);
        let out = run_limited(cmd, "sh", Duration::from_secs(10), Duration::from_millis(10)).unwrap();
        assert_eq!(
            (out.exit_code, out.stdout.trim(), out.stderr.trim()),
            (Some(3), "hi", "oops")
        );
    }

    #[test]
    fn a_grandchild_holding_the_pipes_does_not_hold_the_caller() {
        // The child exits at once; the process it started keeps both pipes
        // open for 30 seconds.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30 & echo started"]);
        let started = Instant::now();
        let out = run_limited(cmd, "sh", Duration::from_secs(60), Duration::from_millis(10)).unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());
    }
}
