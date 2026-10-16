//! Small durable-write helpers.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use super::error::{EngineError, Result};

fn io(path: &Path, e: std::io::Error) -> EngineError {
    EngineError::storage(path.display().to_string(), e)
}

/// Flush a directory's entries to disk so a just-created file survives a power
/// loss. On Windows a directory handle needs `FILE_FLAG_BACKUP_SEMANTICS`.
pub fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir)
            .map_err(|e| io(dir, e))?;
        f.sync_all().map_err(|e| io(dir, e))
    }
    #[cfg(not(windows))]
    {
        File::open(dir).and_then(|f| f.sync_all()).map_err(|e| io(dir, e))
    }
}

/// Create (or replace) a file with `bytes` and make it durable, including its
/// directory entry.
pub fn write_durable(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let mut f = File::create(path).map_err(|e| io(path, e))?;
    f.write_all(bytes).map_err(|e| io(path, e))?;
    f.sync_all().map_err(|e| io(path, e))?;
    drop(f);
    sync_dir(dir)
}

/// `create_dir_all`, then flush each directory that may be new.
pub fn create_dir_durable(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    sync_dir(dir)?;
    if let Some(parent) = dir.parent() {
        sync_dir(parent)?;
    }
    Ok(())
}

/// Truncate a file to `len` bytes and make it durable.
pub fn truncate_durable(path: &Path, len: u64) -> Result<()> {
    let f = OpenOptions::new().write(true).open(path).map_err(|e| io(path, e))?;
    f.set_len(len).map_err(|e| io(path, e))?;
    f.sync_all().map_err(|e| io(path, e))
}
