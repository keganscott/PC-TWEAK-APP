//! One engine per PC at a time.
//!
//! The app and the field-check tool each load the journal into memory and
//! append to it. Two of them at once (the app opened twice, or the field check
//! run while the app is open) would each miss the other's changes: one could
//! back up the other's value as "the original" and Undo would put back the
//! wrong thing. So an engine holds an exclusive lock on a file in the
//! protected data folder for as long as it lives, and a second engine refuses
//! to start. The operating system drops the lock when the process ends, even
//! after a crash, so a stale lock cannot block the next start.
//!
//! PeakTweaks keeps running by the clock after its window closes, so a second
//! start is usually someone wanting the window back. It looks whether the lock
//! is held (`held_elsewhere`); if so it asks the running copy to show its
//! window (`show::ask`) and ends. The lock decides, not the signal, so a
//! program that made the signal first cannot keep PeakTweaks from starting.

use std::fs::{File, OpenOptions, TryLockError};

use crate::error::{EngineError, Result};
use crate::secure_dir::TrustedDir;

/// The lock file, in the protected data folder. Its contents are never read.
pub const LOCK_FILE: &str = "engine.lock";

/// Held for the engine's lifetime; dropping it lets another engine start.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(dir: &TrustedDir) -> Result<Self> {
        let path = dir.path().join(LOCK_FILE);
        let storage = |detail: String| EngineError::Storage {
            path: path.display().to_string(),
            detail,
        };
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| storage(e.to_string()))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(EngineError::AlreadyRunning),
            Err(TryLockError::Error(e)) => Err(storage(format!("could not lock it: {e}"))),
        }
    }
}

/// Whether another engine holds the lock now. A quick look: a lock taken
/// here is let go at once.
pub fn held_elsewhere(dir: &TrustedDir) -> bool {
    matches!(InstanceLock::acquire(dir), Err(EngineError::AlreadyRunning))
}

/// The "show your window" signal between a second start and the running copy:
/// a named Windows event in this sign-in session.
pub mod show {
    #[cfg(windows)]
    use windows::core::w;
    #[cfg(windows)]
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    #[cfg(windows)]
    use windows::Win32::System::Threading::{
        CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE,
    };

    /// Ask the running copy to show its window. False when there is no
    /// running copy listening.
    #[cfg(windows)]
    pub fn ask() -> bool {
        unsafe {
            let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, w!("Local\\PeakTweaks.ShowWindow")) else {
                return false;
            };
            let sent = SetEvent(event).is_ok();
            let _ = CloseHandle(event);
            sent
        }
    }

    /// Call `on_ask` each time a second start asks, on a thread of its own,
    /// for as long as the process runs.
    #[cfg(windows)]
    pub fn listen(on_ask: impl Fn() + Send + 'static) -> crate::error::Result<()> {
        let event = unsafe { CreateEventW(None, false, false, w!("Local\\PeakTweaks.ShowWindow")) }.map_err(|e| {
            crate::error::EngineError::Internal {
                detail: format!("the show-window signal could not be made: {e}"),
            }
        })?;
        // A handle is a number; it lives as long as the process.
        let raw = event.0 as usize;
        std::thread::Builder::new()
            .name("show-window".into())
            .spawn(move || loop {
                let event = HANDLE(raw as *mut core::ffi::c_void);
                if unsafe { WaitForSingleObject(event, INFINITE) }.0 != 0 {
                    return;
                }
                on_ask();
            })
            .map_err(|e| crate::error::EngineError::Internal {
                detail: format!("the show-window thread did not start: {e}"),
            })?;
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn ask() -> bool {
        false
    }

    #[cfg(not(windows))]
    pub fn listen(_on_ask: impl Fn() + Send + 'static) -> crate::error::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EngineError;
    use crate::secure_dir::TrustedDir;

    #[test]
    fn a_second_engine_on_the_same_data_is_refused_until_the_first_has_gone() {
        let dir = tempfile::tempdir().unwrap();
        let trusted = TrustedDir::insecure_for_tests(dir.path());

        let first = InstanceLock::acquire(&trusted).unwrap();
        let second = InstanceLock::acquire(&trusted);
        assert!(matches!(second, Err(EngineError::AlreadyRunning)), "{second:?}");

        drop(first);
        InstanceLock::acquire(&trusted).expect("free again once the first engine is gone");
    }

    #[test]
    fn a_look_at_the_lock_sees_a_running_engine_and_leaves_a_free_lock_free() {
        let dir = tempfile::tempdir().unwrap();
        let trusted = TrustedDir::insecure_for_tests(dir.path());
        assert!(!held_elsewhere(&trusted));
        let running = InstanceLock::acquire(&trusted).expect("the look let the lock go");
        assert!(held_elsewhere(&trusted));
        drop(running);
    }
}
