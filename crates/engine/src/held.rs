//! Holding a folder, and every folder above it, open so that none of them can
//! be renamed or swapped for a link while the engine works inside it.
//!
//! The engine is elevated, and many folders it works in (a user's temporary
//! files, a game's settings) can be written by the user and anything running
//! as the user. A junction needs no privilege to make, so a program could
//! swap one in between a check and a write and steer the elevated write
//! somewhere the user could not reach. Each folder on the path is opened
//! without sharing delete access and checked to be a real folder, not a link;
//! while it is held open it cannot be renamed or replaced, and the last
//! folder's final path must be the one expected. Used by the cleaner
//! (`cleanup.rs`) and by game settings files (`game_file.rs`).

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

#[cfg(any(windows, test))]
pub(crate) const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
#[cfg(any(windows, test))]
pub(crate) const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// What holds one folder: an open handle on Windows; elsewhere nothing, as
/// only the checks can be made.
#[cfg(windows)]
pub(crate) type Handle = fs::File;
#[cfg(not(windows))]
pub(crate) struct Handle;

/// Folders held open, so none can be renamed, replaced or deleted until this
/// is dropped.
pub(crate) struct Held {
    _open: Vec<Handle>,
}

/// Check `dir` and every folder above it is a real folder on a local drive,
/// not a link, and hold them all. `Ok(None)`: it does not exist.
pub(crate) fn hold_chain(dir: &Path) -> Result<Option<Held>, String> {
    if !dir.is_absolute() {
        return Err(format!("{} is not a full path; left alone", dir.display()));
    }
    let mut so_far = PathBuf::new();
    let mut open = Vec::new();
    for part in dir.components() {
        so_far.push(part);
        match part {
            Component::Prefix(prefix) => {
                use std::path::Prefix::{Disk, VerbatimDisk};
                if !matches!(prefix.kind(), Disk(_) | VerbatimDisk(_)) {
                    return Err(format!("{} is not on a local drive; left alone", dir.display()));
                }
                continue;
            }
            Component::RootDir | Component::Normal(_) => {}
            Component::CurDir | Component::ParentDir => {
                return Err(format!("{} is not a plain path; left alone", dir.display()))
            }
        }
        match hold_one(&so_far)? {
            Some(handle) => open.push(handle),
            None => return Ok(None),
        }
    }
    #[cfg(windows)]
    {
        let leaf = open
            .last()
            .ok_or_else(|| format!("{} is not a folder", dir.display()))?;
        let real = imp::final_path(leaf).map_err(|e| format!("{}: {e}", dir.display()))?;
        if !same_folder(&real, dir) {
            return Err(format!("{} leads to {}; left alone", dir.display(), real.display()));
        }
    }
    Ok(Some(Held { _open: open }))
}

/// Open one folder without sharing delete access, checking it is a real
/// folder and not a link. `Ok(None)`: it does not exist.
#[cfg(windows)]
pub(crate) fn hold_one(dir: &Path) -> Result<Option<Handle>, String> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const FILE_LIST_DIRECTORY: u32 = 0x0001;
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    // Listing access, not only attributes: an open that asks for attributes
    // alone takes no part in sharing, so it would hold nothing.
    let f = match fs::OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(dir)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    // From the handle: exactly the folder now held, not whatever the path
    // names a moment later.
    let meta = f.metadata().map_err(|e| format!("{}: {e}", dir.display()))?;
    real_folder(dir, meta.file_attributes())?;
    Ok(Some(f))
}

#[cfg(not(windows))]
pub(crate) fn hold_one(dir: &Path) -> Result<Option<Handle>, String> {
    let meta = match fs::symlink_metadata(dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "{} is a link, junction or other redirect; left alone",
            dir.display()
        ));
    }
    if !meta.is_dir() {
        return Err(format!("{} is not a folder; left alone", dir.display()));
    }
    Ok(Some(Handle))
}

#[cfg(any(windows, test))]
pub(crate) fn real_folder(dir: &Path, attributes: u32) -> Result<(), String> {
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!(
            "{} is a link, junction or other redirect; left alone",
            dir.display()
        ));
    }
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(format!("{} is not a folder; left alone", dir.display()));
    }
    Ok(())
}

/// `C:\Users\Kegan` and `\\?\c:\users\kegan\` are the same folder.
#[cfg(any(windows, test))]
pub(crate) fn same_folder(a: &Path, b: &Path) -> bool {
    fn plain(p: &Path) -> String {
        let s = p.to_string_lossy().replace('/', "\\");
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        s.trim_end_matches('\\').to_lowercase()
    }
    plain(a) == plain(b)
}

#[cfg(windows)]
pub(crate) mod imp {
    use std::os::windows::io::AsRawHandle;
    use std::path::PathBuf;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED};

    use crate::sysdirs::wide_path;

    /// Where an open file or folder really is, after every link and drive mapping.
    pub fn final_path(f: &std::fs::File) -> std::io::Result<PathBuf> {
        let handle = HANDLE(f.as_raw_handle());
        // SAFETY: a handle this process holds open, and a buffer of the
        // length passed.
        wide_path(|buf| unsafe { GetFinalPathNameByHandleW(handle, buf, FILE_NAME_NORMALIZED) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reparse_point_or_a_file_is_not_a_folder_to_hold() {
        let p = Path::new("x");
        assert!(real_folder(p, FILE_ATTRIBUTE_DIRECTORY).is_ok());
        assert!(real_folder(p, FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT).is_err());
        assert!(real_folder(p, 0x20).is_err());
    }

    #[test]
    fn paths_match_however_windows_spells_them() {
        assert!(same_folder(
            Path::new(r"\\?\C:\Users\KFS\AppData\Local\Temp"),
            Path::new(r"c:\users\kfs\appdata\local\temp\")
        ));
        assert!(same_folder(Path::new("C:/Windows"), Path::new(r"C:\WINDOWS")));
        assert!(!same_folder(
            Path::new(r"\\?\C:\Windows\System32"),
            Path::new(r"C:\Users\KFS\AppData\Local\Temp")
        ));
        assert!(!same_folder(
            Path::new(r"\\?\UNC\server\share\Temp"),
            Path::new(r"C:\Temp")
        ));
    }

    #[test]
    fn a_relative_path_is_never_held() {
        assert!(hold_chain(Path::new("AppData")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_held_folder_cannot_be_renamed_until_it_is_let_go() {
        let (_tmp, base) = crate::testutil::real_temp();
        let dir = base.join("a").join("b");
        fs::create_dir_all(&dir).unwrap();
        let held = hold_chain(&dir).expect("a real folder").expect("it exists");
        let moved = base.join("a").join("moved");
        let err = fs::rename(&dir, &moved).expect_err("renamed while held");
        eprintln!("renaming a held folder: {err}");
        assert!(fs::rename(base.join("a"), base.join("moved")).is_err());
        drop(held);
        fs::rename(&dir, &moved).expect("renames once let go");
    }
}
