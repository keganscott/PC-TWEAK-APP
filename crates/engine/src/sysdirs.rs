//! Where Windows itself is, from Windows rather than `%SystemRoot%` or
//! `%windir%`, which the user can set for the programs they start, and so for
//! this elevated one.

#[cfg(windows)]
use std::path::PathBuf;

/// The shared Windows folder, `C:\Windows`.
#[cfg(windows)]
pub fn windows_dir() -> std::io::Result<PathBuf> {
    use windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
    // SAFETY: a buffer of the length passed.
    wide_path(|buf| unsafe { GetSystemWindowsDirectoryW(Some(buf)) })
}

/// `C:\Windows\System32`, where the tools PeakTweaks runs live.
#[cfg(windows)]
pub fn system32() -> std::io::Result<PathBuf> {
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    // SAFETY: a buffer of the length passed.
    wide_path(|buf| unsafe { GetSystemDirectoryW(Some(buf)) })
}

/// Calls `fill` with a growing buffer until the text fits. `fill` returns the
/// length written, or the size needed when the buffer is too small, or 0 on
/// failure (`GetLastError`).
#[cfg(windows)]
pub(crate) fn wide_path(mut fill: impl FnMut(&mut [u16]) -> u32) -> std::io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    let mut buf = vec![0u16; 300];
    loop {
        let n = fill(&mut buf) as usize;
        if n == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if n < buf.len() {
            buf.truncate(n);
            return Ok(PathBuf::from(OsString::from_wide(&buf)));
        }
        buf.resize(n + 1, 0);
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn windows_and_system32_are_real_folders() {
        let windows = super::windows_dir().unwrap();
        let system32 = super::system32().unwrap();
        eprintln!("Windows: {}, System32: {}", windows.display(), system32.display());
        assert!(windows.is_dir() && system32.is_dir());
        let lower = |p: &std::path::Path| p.to_string_lossy().to_lowercase();
        assert!(lower(&system32).starts_with(&lower(&windows)));
    }
}
