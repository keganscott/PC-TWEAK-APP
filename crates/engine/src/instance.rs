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
}
