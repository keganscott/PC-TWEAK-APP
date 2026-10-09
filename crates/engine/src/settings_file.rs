//! Settings files that belong to someone else, a game's above all (plan
//! section 8, Phase 6; NOTES N55): read one, and replace it whole.
//!
//! The engine is elevated, and a game keeps its settings in the user's own
//! folders, which the user and anything running as the user can change. So
//! nothing on the way may steer the elevated read or write elsewhere:
//! - the folder and every folder above it are held open (`held.rs`): each is
//!   a real folder, not a link or junction, none can be renamed or swapped
//!   while held, and the folder's final path is the one asked for;
//! - the file itself must be a plain file: not a link, not a folder, not one
//!   of several names for the same file (a hard link), and its final path is
//!   the one asked for;
//! - a read-only file is left as it is: someone set that on purpose, and
//!   some players do, so a game does not overwrite their settings;
//! - a new version is written next to it under another name, flushed, and
//!   renamed over it, so a crash leaves the old file or the new one, never
//!   half of one. The rename replaces the name, never writes through it.
//!
//! The transaction keeps a copy of the old bytes and journals it before any
//! of this (`Transaction::write_file`), and undo writes them back the same way.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use super::error::{EngineError, Result};
use super::held::{hold_chain, Held};

/// What a new version is written as before it replaces the file. Hidden by
/// its leading dot from the game's own listing, and named for PeakTweaks.
const TEMP_SUFFIX: &str = ".peaktweaks-new";

fn refused(path: &Path, detail: impl Into<String>) -> EngineError {
    EngineError::SettingsFile {
        path: path.display().to_string(),
        detail: detail.into(),
    }
}

fn io_err(path: &Path, e: io::Error) -> EngineError {
    refused(path, e.to_string())
}

/// The folder and file name of a full path to a file.
fn split(path: &Path) -> Result<(&Path, &std::ffi::OsStr)> {
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) if path.is_absolute() => Ok((dir, name)),
        _ => Err(refused(path, "is not a full path to a file")),
    }
}

/// Hold the folder a file is in. `Ok(None)`: the folder does not exist.
fn hold_folder(path: &Path) -> Result<Option<Held>> {
    let (dir, _) = split(path)?;
    hold_chain(dir).map_err(|detail| refused(path, detail))
}

/// A file's bytes, or `None` when it (or its folder) does not exist.
pub fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    let Some(_held) = hold_folder(path)? else {
        return Ok(None);
    };
    let Some(mut f) = open_plain(path)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).map_err(|e| io_err(path, e))?;
    Ok(Some(bytes))
}

/// Replace a file with `bytes`, or delete it with `None`. Writing needs the
/// folder to exist: a game makes it the first time it saves its settings.
pub fn write(path: &Path, bytes: Option<&[u8]>) -> Result<()> {
    let Some(_held) = hold_folder(path)? else {
        return match bytes {
            None => Ok(()),
            Some(_) => Err(refused(
                path,
                "its folder does not exist yet; the game makes it the first time it saves its settings",
            )),
        };
    };
    let existing = open_plain(path)?;
    if let Some(f) = &existing {
        let meta = f.metadata().map_err(|e| io_err(path, e))?;
        if meta.permissions().readonly() {
            return Err(refused(path, "is read-only, so PeakTweaks leaves it as it is"));
        }
    }
    drop(existing);

    let Some(bytes) = bytes else {
        return match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(path, e)),
        };
    };

    let (dir, name) = split(path)?;
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(name);
    temp_name.push(TEMP_SUFFIX);
    let temp = dir.join(temp_name);
    write_new(&temp, bytes)?;
    if let Err(e) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(io_err(path, e));
    }
    crate::fsutil::sync_dir(dir).map_err(|e| refused(path, e.to_string()))
}

/// Write `bytes` to a file that must not exist yet. One left over from a
/// write that was cut short (a crash) is removed first, if it is a plain file.
fn write_new(temp: &Path, bytes: &[u8]) -> Result<()> {
    let create = || {
        let mut o = fs::OpenOptions::new();
        o.write(true).create_new(true);
        o.open(temp)
    };
    let mut f = match create() {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            drop(open_plain(temp)?);
            fs::remove_file(temp).map_err(|e| io_err(temp, e))?;
            create().map_err(|e| io_err(temp, e))?
        }
        Err(e) => return Err(io_err(temp, e)),
    };
    let written = f.write_all(bytes).and_then(|()| f.sync_all());
    drop(f);
    written.map_err(|e| {
        let _ = fs::remove_file(temp);
        io_err(temp, e)
    })
}

/// Open a file for reading, checking it is a plain file reached by its own
/// name. `Ok(None)`: it does not exist.
#[cfg(windows)]
fn open_plain(path: &Path) -> Result<Option<fs::File>> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};

    use super::held::{imp::final_path, same_folder, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT};

    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    // The link itself, never what it leads to; and nobody writes it while it
    // is read or checked.
    let f = match fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(path, e)),
    };
    let attributes = f.metadata().map_err(|e| io_err(path, e))?.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(refused(
            path,
            "is a link or other redirect, so PeakTweaks leaves it alone",
        ));
    }
    if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err(refused(path, "is a folder, not a file"));
    }
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: a handle this function holds open, and a struct to fill.
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle()), &mut info) }
        .map_err(|e| refused(path, format!("GetFileInformationByHandle: {e}")))?;
    if info.nNumberOfLinks > 1 {
        return Err(refused(
            path,
            "has more than one name (a hard link), so PeakTweaks leaves it alone",
        ));
    }
    let real = final_path(&f).map_err(|e| io_err(path, e))?;
    if !same_folder(&real, path) {
        return Err(refused(path, format!("leads to {}; left alone", real.display())));
    }
    Ok(Some(f))
}

#[cfg(not(windows))]
fn open_plain(path: &Path) -> Result<Option<fs::File>> {
    use std::os::unix::fs::MetadataExt;
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(path, e)),
    };
    if meta.file_type().is_symlink() {
        return Err(refused(
            path,
            "is a link or other redirect, so PeakTweaks leaves it alone",
        ));
    }
    if meta.is_dir() {
        return Err(refused(path, "is a folder, not a file"));
    }
    if meta.nlink() > 1 {
        return Err(refused(
            path,
            "has more than one name (a hard link), so PeakTweaks leaves it alone",
        ));
    }
    fs::File::open(path).map(Some).map_err(|e| io_err(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn place() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp
            .path()
            .join("FortniteGame")
            .join("Saved")
            .join("Config")
            .join("WindowsClient");
        fs::create_dir_all(&dir).unwrap();
        (tmp, dir)
    }

    fn detail(e: EngineError) -> String {
        match e {
            EngineError::SettingsFile { detail, .. } => detail,
            other => panic!("expected a settings-file error, got {other:?}"),
        }
    }

    #[test]
    fn a_file_is_replaced_whole_read_back_and_deleted() {
        let (_tmp, dir) = place();
        let file = dir.join("GameUserSettings.ini");
        assert_eq!(read(&file).unwrap(), None);

        let original = b"\xEF\xBB\xBF[S]\r\nFrameRateLimit=60\r\n";
        fs::write(&file, original).unwrap();
        assert_eq!(read(&file).unwrap().as_deref(), Some(&original[..]));

        write(&file, Some(b"[S]\r\nFrameRateLimit=0\r\n")).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"[S]\r\nFrameRateLimit=0\r\n");
        let left: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(
            left,
            vec![std::ffi::OsString::from("GameUserSettings.ini")],
            "no new-version file left behind"
        );

        write(&file, None).unwrap();
        assert!(!file.exists());
        write(&file, None).expect("deleting what is not there is done already");
    }

    #[test]
    fn a_file_whose_folder_does_not_exist_is_not_created() {
        let (_tmp, dir) = place();
        let file = dir.join("missing").join("GameUserSettings.ini");
        assert_eq!(read(&file).unwrap(), None);
        assert!(detail(write(&file, Some(b"x")).unwrap_err()).contains("does not exist yet"));
        assert!(!dir.join("missing").exists());
        write(&file, None).unwrap();
    }

    #[test]
    fn a_read_only_file_is_left_as_it_is() {
        let (_tmp, dir) = place();
        let file = dir.join("GameUserSettings.ini");
        fs::write(&file, b"mine").unwrap();
        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&file, perms).unwrap();

        assert!(detail(write(&file, Some(b"ours")).unwrap_err()).contains("read-only"));
        assert!(detail(write(&file, None).unwrap_err()).contains("read-only"));
        assert_eq!(fs::read(&file).unwrap(), b"mine");
        assert_eq!(
            read(&file).unwrap().as_deref(),
            Some(&b"mine"[..]),
            "reading it is fine"
        );

        let mut perms = fs::metadata(&file).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&file, perms).unwrap();
    }

    #[test]
    fn a_new_version_left_over_from_a_crash_does_not_stop_the_next_write() {
        let (_tmp, dir) = place();
        let file = dir.join("GameUserSettings.ini");
        fs::write(&file, b"old").unwrap();
        fs::write(dir.join(".GameUserSettings.ini.peaktweaks-new"), b"half").unwrap();
        write(&file, Some(b"new")).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"new");
        assert!(!dir.join(".GameUserSettings.ini.peaktweaks-new").exists());
    }

    #[test]
    fn a_hard_link_is_left_alone() {
        let (tmp, dir) = place();
        let elsewhere = tmp.path().join("precious.ini");
        fs::write(&elsewhere, b"precious").unwrap();
        let file = dir.join("GameUserSettings.ini");
        fs::hard_link(&elsewhere, &file).unwrap();

        assert!(detail(read(&file).unwrap_err()).contains("hard link"));
        assert!(detail(write(&file, Some(b"ours")).unwrap_err()).contains("hard link"));
        assert_eq!(fs::read(&elsewhere).unwrap(), b"precious");
    }

    #[test]
    fn a_relative_path_is_refused() {
        assert!(detail(read(Path::new("GameUserSettings.ini")).unwrap_err()).contains("full path"));
    }

    #[cfg(unix)]
    #[test]
    fn links_on_the_way_are_left_alone() {
        use std::os::unix::fs::symlink;
        let (tmp, dir) = place();
        let elsewhere = tmp.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("GameUserSettings.ini"), b"precious").unwrap();

        // The file is a link.
        let file = dir.join("GameUserSettings.ini");
        symlink(elsewhere.join("GameUserSettings.ini"), &file).unwrap();
        assert!(detail(read(&file).unwrap_err()).contains("is a link"));
        assert!(detail(write(&file, Some(b"ours")).unwrap_err()).contains("is a link"));

        // A folder on the way is a link.
        let linked = dir.join("Linked");
        symlink(&elsewhere, &linked).unwrap();
        let through = linked.join("GameUserSettings.ini");
        assert!(detail(read(&through).unwrap_err()).contains("is a link"));
        assert!(detail(write(&through, Some(b"ours")).unwrap_err()).contains("is a link"));
        assert_eq!(fs::read(elsewhere.join("GameUserSettings.ini")).unwrap(), b"precious");
    }

    /// `mklink /J`: a junction needs no privilege, so it is what a program
    /// running as the user would plant.
    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) {
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("cmd runs")
            .status
            .success();
        assert!(ok, "mklink /J {} {}", link.display(), target.display());
    }

    /// NOTES N55: a junction swapped in for a game's settings folder must not
    /// steer the elevated write into a folder only administrators can change.
    #[cfg(windows)]
    #[test]
    fn a_junction_on_the_way_is_left_alone() {
        let (tmp, dir) = place();
        let precious = tmp.path().join("precious");
        fs::create_dir_all(&precious).unwrap();
        fs::write(precious.join("GameUserSettings.ini"), b"precious").unwrap();

        let config = dir.parent().unwrap().to_path_buf();
        fs::remove_dir_all(&dir).unwrap();
        junction(&dir, &precious);
        let file = dir.join("GameUserSettings.ini");
        let err = detail(write(&file, Some(b"ours")).unwrap_err());
        eprintln!("settings file through a junction: {err}");
        assert!(err.contains("is a link"), "{err}");
        assert!(detail(read(&file).unwrap_err()).contains("is a link"));
        assert_eq!(fs::read(precious.join("GameUserSettings.ini")).unwrap(), b"precious");
        assert!(config.exists());
    }
}
