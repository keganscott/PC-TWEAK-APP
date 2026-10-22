//! When Windows last started, so Home can check on the changes that waited
//! for a restart (game plan new idea 5, `src/lib/restartCheck.ts`).
//!
//! It is the current time less Windows' tick count, which starts at zero when
//! Windows boots and keeps counting through sleep and hibernation. A restart
//! always boots afresh; a shutdown with Fast Startup resumes a hibernated
//! kernel and keeps counting, which is right here too, because changes that
//! need a restart only take effect on a real one. The value moves by a few
//! milliseconds between calls, so compare it with some slack.

/// Unix ms when Windows last booted, or `None` off Windows.
pub fn booted_unix_ms() -> Option<u64> {
    let up = uptime_ms()?;
    crate::journal::now_ms().checked_sub(up)
}

#[cfg(windows)]
fn uptime_ms() -> Option<u64> {
    // SAFETY: no arguments; reads a counter.
    Some(unsafe { windows::Win32::System::SystemInformation::GetTickCount64() })
}

#[cfg(not(windows))]
fn uptime_ms() -> Option<u64> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn windows_booted_in_the_past() {
        let booted = super::booted_unix_ms().unwrap();
        let now = crate::journal::now_ms();
        assert!(booted <= now && booted > 1_500_000_000_000, "{booted}");
        println!("booted {} s ago", (now - booted) / 1000);
    }
}
