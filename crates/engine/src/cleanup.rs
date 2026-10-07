//! Clear out files nothing needs (catalogue H28, Hone's junk cleaner).
//!
//! Deleting is the one thing PeakTweaks does that cannot be undone, so the
//! screen shows what each area holds, asks once and says so. No setting
//! changes, so no restore point is needed; a restore point would not keep
//! these files anyway.
//!
//! The engine is elevated, and most of these folders are ones the user, and
//! anything running as the user, can write to. A planted link must not be able
//! to steer a delete anywhere else, so:
//! - Before anything in a folder is touched, that folder and every folder
//!   above it are opened without sharing delete access and checked to be real
//!   folders, not links or junctions. While they are held open none of them
//!   can be renamed or replaced, and the folder's final path must be the one
//!   expected.
//! - Inside, a link is removed as a link and never followed, and a subfolder
//!   is held the same way while it is emptied.
//! - Temporary folders keep anything created or changed in the last 7 days,
//!   as Windows' Disk Cleanup keeps temporary files "modified in the last
//!   week" (VERIFY), in case a program or an installer still needs it. A
//!   folder created in that time is kept with everything in it.
//! - A file a program has open is left where it is, and counted.
//!
//! VERIFY: where Windows and the graphics drivers keep these files (`spots`)
//! is from memory; a folder that is not there counts as empty (NOTES N70).

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Temporary folders keep anything created or changed this recently.
pub const KEEP_RECENT: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Folders nested deeper than this inside an area are left alone.
const MAX_DEPTH: usize = 32;

/// How many of the files that could not be deleted a result names.
const EXAMPLES: usize = 5;

#[cfg(any(windows, test))]
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
#[cfg(any(windows, test))]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// One kind of junk the cleaner can clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CleanupArea {
    /// The user's temporary folder.
    UserTemp,
    /// Windows' own temporary folder.
    WindowsTemp,
    /// File Explorer's thumbnail cache.
    Thumbnails,
    /// DirectX and graphics-driver shader caches.
    ShaderCaches,
    /// Crash dumps and Windows Error Reporting files.
    CrashDumps,
}

impl CleanupArea {
    /// Every area, in the order a cleanup goes through them.
    pub const ALL: [Self; 5] = [
        Self::UserTemp,
        Self::WindowsTemp,
        Self::Thumbnails,
        Self::ShaderCaches,
        Self::CrashDumps,
    ];

    /// For progress messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::UserTemp => "your temporary files",
            Self::WindowsTemp => "Windows temporary files",
            Self::Thumbnails => "the thumbnail cache",
            Self::ShaderCaches => "shader caches",
            Self::CrashDumps => "crash reports and memory dumps",
        }
    }
}

/// What one area holds that a cleanup would delete now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AreaSize {
    pub area: CleanupArea,
    pub bytes: u64,
    pub files: u64,
    /// Folders left alone, and why. A folder that does not exist is not
    /// listed: there is nothing in it.
    pub skipped: Vec<String>,
}

/// What clearing one area did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AreaCleanup {
    pub area: CleanupArea,
    pub removed_bytes: u64,
    pub removed_files: u64,
    /// Files it tried to delete and could not: a program has them open, or
    /// Windows would not allow it.
    pub left_bytes: u64,
    pub left_files: u64,
    /// The first few of those, with Windows' reason.
    pub left_examples: Vec<String>,
    pub skipped: Vec<String>,
}

/// One cleanup: each chosen area once, in the order of `CleanupArea::ALL`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub areas: Vec<AreaCleanup>,
    pub unix_ms: u64,
}

/// The folders the areas are found under. `None` when it could not be found;
/// the areas under it then say so.
#[derive(Debug, Clone, Default)]
pub struct Places {
    /// `C:\Windows`.
    pub windows: Option<PathBuf>,
    /// `C:\ProgramData`.
    pub program_data: Option<PathBuf>,
    /// The interactive user's profile folder, `C:\Users\<name>`.
    pub profile: Option<PathBuf>,
}

/// Where the areas are on this PC for the user with this SID. The Windows
/// folder comes from Windows itself, not `%SystemRoot%`, which the user can
/// set for programs they start.
#[cfg(windows)]
pub fn places(user_sid: &str) -> Places {
    Places {
        windows: crate::sysdirs::windows_dir().ok(),
        program_data: imp::program_data().ok(),
        profile: crate::identity::profile_dir_for_sid(user_sid).ok().flatten(),
    }
}

/// Builds that are not Windows have none of these folders.
#[cfg(not(windows))]
pub fn places(_user_sid: &str) -> Places {
    Places::default()
}

/// What each chosen area holds that `clean` would delete now. Reads only.
pub fn measure(places: &Places, areas: &[CleanupArea], now: SystemTime) -> Vec<AreaSize> {
    chosen(areas)
        .map(|area| {
            let t = go_through(places, area, now, Mode::Look);
            AreaSize {
                area,
                bytes: t.bytes,
                files: t.files,
                skipped: t.skipped,
            }
        })
        .collect()
}

/// Delete what `measure` counts in each chosen area. Cannot be undone.
/// `on_area` is told as each area starts.
pub fn clean(
    places: &Places,
    areas: &[CleanupArea],
    now: SystemTime,
    unix_ms: u64,
    on_area: &dyn Fn(CleanupArea),
) -> CleanupReport {
    let areas = chosen(areas)
        .map(|area| {
            on_area(area);
            let t = go_through(places, area, now, Mode::Delete);
            AreaCleanup {
                area,
                removed_bytes: t.bytes,
                removed_files: t.files,
                left_bytes: t.left_bytes,
                left_files: t.left_files,
                left_examples: t.left_examples,
                skipped: t.skipped,
            }
        })
        .collect();
    CleanupReport { areas, unix_ms }
}

fn chosen(areas: &[CleanupArea]) -> impl Iterator<Item = CleanupArea> + '_ {
    CleanupArea::ALL.into_iter().filter(move |a| areas.contains(a))
}

// ---------------------------------------------------------------------------
// Where each area is
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Base {
    Windows,
    ProgramData,
    Profile,
}

/// Which entries in a folder count.
#[derive(Clone, Copy)]
enum Pick {
    /// Everything, subfolders included.
    All,
    /// Files directly in the folder whose names start and end so, in any case.
    Files { prefix: &'static str, suffix: &'static str },
    /// One file directly in the folder, in any case.
    Named(&'static str),
}

impl Pick {
    fn takes(self, name: &OsStr, kind: Kind) -> bool {
        let name = name.to_string_lossy().to_lowercase();
        match self {
            Self::All => true,
            Self::Files { prefix, suffix } => {
                kind == Kind::File
                    && name.len() >= prefix.len() + suffix.len()
                    && name.starts_with(prefix)
                    && name.ends_with(suffix)
            }
            Self::Named(file) => kind == Kind::File && name == file.to_lowercase(),
        }
    }
}

/// One folder an area clears.
struct Spot {
    base: Base,
    /// Below the base, `\`-separated.
    rel: &'static str,
    pick: Pick,
    /// Keep anything created or changed in the last `KEEP_RECENT`.
    keep_recent: bool,
}

fn everything(base: Base, rel: &'static str) -> Spot {
    Spot {
        base,
        rel,
        pick: Pick::All,
        keep_recent: false,
    }
}

/// VERIFY each folder (module docs).
fn spots(area: CleanupArea) -> Vec<Spot> {
    use Base::{Profile, ProgramData, Windows};
    match area {
        CleanupArea::UserTemp => vec![Spot {
            keep_recent: true,
            ..everything(Profile, r"AppData\Local\Temp")
        }],
        CleanupArea::WindowsTemp => vec![Spot {
            keep_recent: true,
            ..everything(Windows, "Temp")
        }],
        CleanupArea::Thumbnails => vec![Spot {
            pick: Pick::Files {
                prefix: "thumbcache_",
                suffix: ".db",
            },
            ..everything(Profile, r"AppData\Local\Microsoft\Windows\Explorer")
        }],
        CleanupArea::ShaderCaches => [
            r"AppData\Local\D3DSCache",
            r"AppData\Local\NVIDIA\DXCache",
            r"AppData\Local\NVIDIA\GLCache",
            r"AppData\LocalLow\NVIDIA\PerDriverVersion\DXCache",
            r"AppData\LocalLow\NVIDIA\PerDriverVersion\GLCache",
            r"AppData\Local\AMD\DxCache",
            r"AppData\Local\AMD\DxcCache",
            r"AppData\Local\AMD\VkCache",
            r"AppData\LocalLow\Intel\ShaderCache",
        ]
        .into_iter()
        .map(|rel| everything(Profile, rel))
        .collect(),
        CleanupArea::CrashDumps => vec![
            everything(Windows, "Minidump"),
            Spot {
                pick: Pick::Named("MEMORY.DMP"),
                ..everything(Windows, "")
            },
            everything(Profile, r"AppData\Local\CrashDumps"),
            everything(Profile, r"AppData\Local\Microsoft\Windows\WER\ReportArchive"),
            everything(Profile, r"AppData\Local\Microsoft\Windows\WER\ReportQueue"),
            everything(ProgramData, r"Microsoft\Windows\WER\ReportArchive"),
            everything(ProgramData, r"Microsoft\Windows\WER\ReportQueue"),
        ],
    }
}

fn folder(places: &Places, spot: &Spot) -> Result<PathBuf, String> {
    let (base, what) = match spot.base {
        Base::Windows => (&places.windows, "the Windows folder"),
        Base::ProgramData => (&places.program_data, "the ProgramData folder"),
        Base::Profile => (&places.profile, "your user folder"),
    };
    let mut dir = base.clone().ok_or_else(|| format!("Could not find {what}."))?;
    dir.extend(spot.rel.split('\\').filter(|s| !s.is_empty()));
    Ok(dir)
}

// ---------------------------------------------------------------------------
// Going through a folder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Count what would go.
    Look,
    Delete,
}

#[derive(Default)]
struct Tally {
    /// Counted (`Look`) or deleted (`Delete`).
    bytes: u64,
    files: u64,
    left_bytes: u64,
    left_files: u64,
    left_examples: Vec<String>,
    skipped: Vec<String>,
}

impl Tally {
    fn add(&mut self, len: u64) {
        self.bytes += len;
        self.files += 1;
    }

    fn left(&mut self, path: &Path, len: u64, e: &io::Error) {
        self.left_bytes += len;
        self.left_files += 1;
        if self.left_examples.len() < EXAMPLES {
            self.left_examples.push(format!("{}: {e}", path.display()));
        }
    }

    fn skip(&mut self, why: String) {
        if !self.skipped.contains(&why) {
            self.skipped.push(why);
        }
    }
}

fn go_through(places: &Places, area: CleanupArea, now: SystemTime, mode: Mode) -> Tally {
    let mut t = Tally::default();
    for spot in spots(area) {
        let dir = match folder(places, &spot) {
            Ok(dir) => dir,
            Err(why) => {
                t.skip(why);
                continue;
            }
        };
        let cutoff = spot
            .keep_recent
            .then(|| now.checked_sub(KEEP_RECENT).unwrap_or(SystemTime::UNIX_EPOCH));
        match hold_chain(&dir) {
            Ok(Some(_held)) => sweep(&dir, 0, &spot, cutoff, mode, &mut t),
            Ok(None) => {}
            Err(why) => t.skip(why),
        }
    }
    t
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Folder,
    /// A symbolic link or junction.
    Link,
    /// Anything else (another kind of reparse point, a device): left alone.
    Other,
}

impl Kind {
    fn of(meta: &fs::Metadata) -> Self {
        let ft = meta.file_type();
        if ft.is_symlink() {
            return Self::Link;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Self::Other;
            }
        }
        if ft.is_dir() {
            Self::Folder
        } else if ft.is_file() {
            Self::File
        } else {
            Self::Other
        }
    }
}

/// Created and last changed no later than `cutoff` (no cutoff: always). A
/// time that cannot be read does not count; with neither, the entry is kept.
fn older_than(created: Option<SystemTime>, changed: Option<SystemTime>, cutoff: Option<SystemTime>) -> bool {
    let Some(cutoff) = cutoff else { return true };
    created.max(changed).is_some_and(|newest| newest <= cutoff)
}

/// Count or delete what `spot` takes in `dir`, which the caller holds.
fn sweep(dir: &Path, depth: usize, spot: &Spot, cutoff: Option<SystemTime>, mode: Mode, t: &mut Tally) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => return t.skip(format!("{}: {e}", dir.display())),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                t.skip(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        let path = entry.path();
        // From the listing: a link is described, never followed.
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                t.skip(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let kind = Kind::of(&meta);
        if depth == 0 && !spot.pick.takes(&entry.file_name(), kind) {
            continue;
        }
        let (created, changed) = (meta.created().ok(), meta.modified().ok());
        // A folder's own change time moves whenever something in it does, so
        // a folder is judged by when it was made, and its files one by one.
        let old = match kind {
            Kind::Folder => older_than(created.or(changed), None, cutoff),
            _ => older_than(created, changed, cutoff),
        };
        if !old {
            continue;
        }
        match kind {
            Kind::Link if mode == Mode::Delete => {
                // Whatever it leads to stays; at worst the link stays too.
                let _ = remove_link(&path, &meta);
            }
            Kind::Folder if depth + 1 > MAX_DEPTH => t.skip(format!("{}: nested too deep; left alone", path.display())),
            Kind::Folder if mode == Mode::Look => sweep(&path, depth + 1, spot, cutoff, mode, t),
            Kind::Folder => {
                let emptied = match hold_one(&path) {
                    Ok(Some(_held)) => {
                        sweep(&path, depth + 1, spot, cutoff, mode, t);
                        true
                    }
                    Ok(None) => false,
                    Err(why) => {
                        t.skip(why);
                        false
                    }
                };
                // Fails, harmlessly, when something in it was kept or in use.
                if emptied {
                    let _ = fs::remove_dir(&path);
                }
            }
            Kind::File if mode == Mode::Look => t.add(meta.len()),
            Kind::File => match fs::remove_file(&path) {
                Ok(()) => t.add(meta.len()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => t.left(&path, meta.len(), &e),
            },
            Kind::Link | Kind::Other => {}
        }
    }
}

/// Remove a link or junction itself, never what it leads to.
#[cfg_attr(not(windows), allow(unused_variables))]
fn remove_link(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        if meta.file_type().is_symlink_dir() {
            return fs::remove_dir(path);
        }
    }
    fs::remove_file(path)
}

// ---------------------------------------------------------------------------
// Holding folders
// ---------------------------------------------------------------------------

/// What holds one folder: an open handle on Windows; elsewhere nothing, as
/// only the checks can be made.
#[cfg(windows)]
type Handle = fs::File;
#[cfg(not(windows))]
struct Handle;

/// Folders held open, so none can be renamed, replaced or deleted until this
/// is dropped.
struct Held {
    _open: Vec<Handle>,
}

/// Check `dir` and every folder above it is a real folder on a local drive,
/// not a link, and hold them all. `Ok(None)`: it does not exist.
fn hold_chain(dir: &Path) -> Result<Option<Held>, String> {
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
fn hold_one(dir: &Path) -> Result<Option<Handle>, String> {
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
fn hold_one(dir: &Path) -> Result<Option<Handle>, String> {
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
fn real_folder(dir: &Path, attributes: u32) -> Result<(), String> {
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
fn same_folder(a: &Path, b: &Path) -> bool {
    fn plain(p: &Path) -> String {
        let s = p.to_string_lossy().replace('/', "\\");
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        s.trim_end_matches('\\').to_lowercase()
    }
    plain(a) == plain(b)
}

#[cfg(windows)]
mod imp {
    use std::ffi::{c_void, OsString};
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::PathBuf;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED};
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

    use crate::sysdirs::wide_path;

    pub fn program_data() -> windows::core::Result<PathBuf> {
        // SAFETY: the returned string is copied, then freed once.
        unsafe {
            let p = SHGetKnownFolderPath(&FOLDERID_ProgramData, KF_FLAG_DEFAULT, None)?;
            let path = PathBuf::from(OsString::from_wide(p.as_wide()));
            CoTaskMemFree(Some(p.0 as *const c_void));
            Ok(path)
        }
    }

    /// Where an open folder really is, after every link and drive mapping.
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

    /// A pretend PC in a temporary folder.
    struct Pc {
        _tmp: tempfile::TempDir,
        base: PathBuf,
        places: Places,
    }

    /// The real path, without `\\?\`, so the checks see the names Windows
    /// would report (Windows runners keep temp under an 8.3 short name).
    fn real(p: &Path) -> PathBuf {
        let c = fs::canonicalize(p).unwrap();
        PathBuf::from(c.to_string_lossy().trim_start_matches(r"\\?\"))
    }

    fn pc() -> Pc {
        let tmp = tempfile::tempdir().unwrap();
        let base = real(tmp.path());
        let places = Places {
            windows: Some(base.join("Windows")),
            program_data: Some(base.join("ProgramData")),
            profile: Some(base.join("Users").join("kegan")),
        };
        Pc {
            _tmp: tmp,
            base,
            places,
        }
    }

    impl Pc {
        fn profile(&self, rel: &str) -> PathBuf {
            under(self.places.profile.as_ref().unwrap(), rel)
        }
        fn windows(&self, rel: &str) -> PathBuf {
            under(self.places.windows.as_ref().unwrap(), rel)
        }
        fn program_data(&self, rel: &str) -> PathBuf {
            under(self.places.program_data.as_ref().unwrap(), rel)
        }
    }

    fn under(base: &Path, rel: &str) -> PathBuf {
        let mut p = base.to_path_buf();
        p.extend(rel.split('\\').filter(|s| !s.is_empty()));
        p
    }

    fn file(path: &Path, len: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7u8; len]).unwrap();
    }

    /// Mark a file as changed at `t`.
    fn changed_at(path: &Path, t: SystemTime) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    /// A clock a week and a day ahead: everything made by a test is old.
    fn later() -> SystemTime {
        SystemTime::now() + KEEP_RECENT + Duration::from_secs(24 * 60 * 60)
    }

    fn exists(p: &Path) -> bool {
        fs::symlink_metadata(p).is_ok()
    }

    fn clean_now(places: &Places, areas: &[CleanupArea], now: SystemTime) -> CleanupReport {
        clean(places, areas, now, 1, &|_| {})
    }

    #[test]
    fn temporary_files_older_than_a_week_go_and_newer_ones_stay() {
        let pc = pc();
        let temp = pc.profile(r"AppData\Local\Temp");
        file(&temp.join("old.tmp"), 100);
        file(&temp.join("setup").join("old.dll"), 50);
        file(&temp.join("new.tmp"), 10);
        // Changed after any cutoff used here.
        changed_at(&temp.join("new.tmp"), SystemTime::now() + KEEP_RECENT * 4);

        let a = &clean_now(&pc.places, &[CleanupArea::UserTemp], later()).areas[0];
        assert_eq!((a.removed_files, a.removed_bytes), (2, 150));
        assert!(!exists(&temp.join("old.tmp")) && !exists(&temp.join("setup")));
        assert!(exists(&temp.join("new.tmp")));
        assert!(exists(&temp), "the folder itself stays");

        // On today's clock, nothing made today goes.
        file(&temp.join("today.tmp"), 5);
        let a = &clean_now(&pc.places, &[CleanupArea::UserTemp], SystemTime::now()).areas[0];
        assert_eq!(a.removed_files, 0);
        assert!(exists(&temp.join("today.tmp")));
    }

    #[test]
    fn a_new_folder_is_kept_with_everything_in_it() {
        let pc = pc();
        let temp = pc.windows("Temp");
        let unpacked = temp.join("installer");
        file(&unpacked.join("payload.cab"), 30);
        // An unpacked archive keeps its files' old dates; the folder is new.
        changed_at(
            &unpacked.join("payload.cab"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        );
        let a = &clean_now(&pc.places, &[CleanupArea::WindowsTemp], SystemTime::now()).areas[0];
        assert_eq!(a.removed_files, 0);
        assert!(exists(&unpacked.join("payload.cab")));
    }

    #[test]
    fn the_newest_of_made_and_changed_decides() {
        let t = |s: u64| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(s));
        assert!(older_than(t(1), t(2), t(2)));
        assert!(!older_than(t(1), t(3), t(2)), "changed after the cutoff");
        assert!(!older_than(t(3), t(1), t(2)), "made after the cutoff");
        assert!(older_than(None, t(1), t(2)));
        assert!(!older_than(None, None, t(2)), "no time at all: kept");
        assert!(older_than(None, None, None), "no cutoff: everything goes");
    }

    #[test]
    fn measuring_counts_exactly_what_cleaning_deletes() {
        let pc = pc();
        file(&pc.profile(r"AppData\Local\Temp\a.tmp"), 10);
        file(&pc.profile(r"AppData\Local\Temp\x\y\z.tmp"), 20);
        file(&pc.windows(r"Temp\w.log"), 30);
        file(&pc.profile(r"AppData\Local\D3DSCache\1\cache.bin"), 40);
        file(
            &pc.profile(r"AppData\LocalLow\NVIDIA\PerDriverVersion\DXCache\nv.bin"),
            50,
        );
        file(&pc.windows(r"Minidump\100126-1.dmp"), 60);
        file(&pc.program_data(r"Microsoft\Windows\WER\ReportQueue\r\Report.wer"), 70);

        let sizes = measure(&pc.places, &CleanupArea::ALL, later());
        let report = clean_now(&pc.places, &CleanupArea::ALL, later());
        let looked: Vec<_> = sizes.iter().map(|s| (s.area, s.files, s.bytes)).collect();
        let did: Vec<_> = report
            .areas
            .iter()
            .map(|a| (a.area, a.removed_files, a.removed_bytes))
            .collect();
        assert_eq!(looked, did);
        assert_eq!(did.iter().map(|d| d.1).sum::<u64>(), 7);
        assert_eq!(did.iter().map(|d| d.2).sum::<u64>(), 280);
        assert!(measure(&pc.places, &CleanupArea::ALL, later())
            .iter()
            .all(|s| s.files == 0));
    }

    #[test]
    fn thumbnails_and_memory_dumps_take_only_their_own_files() {
        let pc = pc();
        let explorer = pc.profile(r"AppData\Local\Microsoft\Windows\Explorer");
        for name in [
            "thumbcache_32.db",
            "THUMBCACHE_IDX.DB",
            "iconcache_32.db",
            "thumbcache_.txt",
        ] {
            file(&explorer.join(name), 1);
        }
        file(&explorer.join("sub").join("thumbcache_1.db"), 1);
        file(&pc.windows("MEMORY.DMP"), 1000);
        file(&pc.windows("notepad.exe"), 1);
        file(&pc.windows(r"Minidump\a.dmp"), 1);

        let report = clean_now(&pc.places, &[CleanupArea::Thumbnails, CleanupArea::CrashDumps], later());
        assert_eq!(report.areas[0].removed_files, 2);
        assert_eq!(report.areas[1].removed_files, 2);
        assert!(!exists(&explorer.join("thumbcache_32.db")) && !exists(&explorer.join("THUMBCACHE_IDX.DB")));
        for kept in ["iconcache_32.db", "thumbcache_.txt", r"sub\thumbcache_1.db"] {
            assert!(exists(&under(&explorer, kept)), "{kept} was deleted");
        }
        assert!(!exists(&pc.windows("MEMORY.DMP")) && exists(&pc.windows("notepad.exe")));
        assert!(!exists(&pc.windows(r"Minidump\a.dmp")) && exists(&pc.windows("Minidump")));
    }

    #[test]
    fn only_the_chosen_areas_are_touched_each_once_in_order() {
        let pc = pc();
        file(&pc.profile(r"AppData\Local\Temp\a.tmp"), 1);
        file(&pc.profile(r"AppData\Local\CrashDumps\game.dmp"), 1);
        let report = clean_now(
            &pc.places,
            &[
                CleanupArea::CrashDumps,
                CleanupArea::ShaderCaches,
                CleanupArea::CrashDumps,
            ],
            later(),
        );
        let areas: Vec<_> = report.areas.iter().map(|a| a.area).collect();
        assert_eq!(areas, [CleanupArea::ShaderCaches, CleanupArea::CrashDumps]);
        assert!(exists(&pc.profile(r"AppData\Local\Temp\a.tmp")));
        assert!(!exists(&pc.profile(r"AppData\Local\CrashDumps\game.dmp")));
    }

    #[test]
    fn a_missing_folder_holds_nothing_and_a_missing_place_says_so() {
        let pc = pc();
        let places = Places {
            profile: None,
            ..pc.places.clone()
        };
        let areas = [
            CleanupArea::UserTemp,
            CleanupArea::WindowsTemp,
            CleanupArea::ShaderCaches,
        ];
        let sizes = measure(&places, &areas, later());
        assert_eq!(sizes[0].skipped, ["Could not find your user folder."]);
        assert_eq!(
            (sizes[1].files, sizes[1].skipped.len()),
            (0, 0),
            "no Windows\\Temp: nothing to say"
        );
        assert_eq!(
            sizes[2].skipped,
            ["Could not find your user folder."],
            "said once per area"
        );
    }

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

    #[cfg(unix)]
    #[test]
    fn a_link_inside_is_removed_and_what_it_leads_to_survives() {
        use std::os::unix::fs::symlink;
        let pc = pc();
        let temp = pc.profile(r"AppData\Local\Temp");
        let precious = pc.base.join("precious");
        file(&precious.join("keep.dll"), 1000);
        file(&temp.join("junk.tmp"), 1);
        symlink(&precious, temp.join("to_folder")).unwrap();
        symlink(precious.join("keep.dll"), temp.join("to_file")).unwrap();

        let sizes = measure(&pc.places, &[CleanupArea::UserTemp], later());
        assert_eq!(
            (sizes[0].files, sizes[0].bytes),
            (1, 1),
            "links are not counted, nor followed"
        );
        let a = &clean_now(&pc.places, &[CleanupArea::UserTemp], later()).areas[0];
        assert_eq!(a.removed_files, 1);
        assert!(exists(&precious.join("keep.dll")));
        assert!(!exists(&temp.join("to_folder")) && !exists(&temp.join("to_file")));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_is_a_link_is_left_alone_with_what_it_leads_to() {
        use std::os::unix::fs::symlink;
        let pc = pc();
        let precious = pc.base.join("precious");
        file(&precious.join("keep.dll"), 1000);
        fs::create_dir_all(pc.profile(r"AppData\Local")).unwrap();
        symlink(&precious, pc.profile(r"AppData\Local\Temp")).unwrap();

        let sizes = measure(&pc.places, &[CleanupArea::UserTemp], later());
        assert_eq!(sizes[0].files, 0);
        assert!(sizes[0].skipped[0].contains("is a link"), "{:?}", sizes[0].skipped);
        clean_now(&pc.places, &[CleanupArea::UserTemp], later());
        assert!(exists(&precious.join("keep.dll")));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_further_up_is_refused_too() {
        use std::os::unix::fs::symlink;
        let pc = pc();
        let elsewhere = pc.base.join("elsewhere");
        file(&under(&elsewhere, r"Local\Temp\keep.dll"), 10);
        fs::create_dir_all(pc.profile("")).unwrap();
        symlink(&elsewhere, pc.profile("AppData")).unwrap();

        let a = &clean_now(&pc.places, &[CleanupArea::UserTemp], later()).areas[0];
        assert!(a.skipped[0].contains("is a link"), "{:?}", a.skipped);
        assert!(exists(&under(&elsewhere, r"Local\Temp\keep.dll")));
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

    #[cfg(windows)]
    #[test]
    fn a_junction_planted_in_temp_is_removed_and_what_it_leads_to_survives() {
        let pc = pc();
        let temp = pc.profile(r"AppData\Local\Temp");
        let precious = pc.base.join("precious");
        file(&precious.join("keep.dll"), 1000);
        fs::create_dir_all(&temp).unwrap();
        junction(&temp.join("j"), &precious);

        assert_eq!(measure(&pc.places, &[CleanupArea::UserTemp], later())[0].files, 0);
        clean_now(&pc.places, &[CleanupArea::UserTemp], later());
        assert!(exists(&precious.join("keep.dll")));
        assert!(!exists(&temp.join("j")), "the junction itself goes");

        // A junction in place of Temp itself is refused.
        fs::remove_dir(&temp).unwrap();
        junction(&temp, &precious);
        let a = &clean_now(&pc.places, &[CleanupArea::UserTemp], later()).areas[0];
        assert!(a.skipped[0].contains("is a link"), "{:?}", a.skipped);
        assert!(exists(&precious.join("keep.dll")));
    }

    #[cfg(windows)]
    #[test]
    fn a_held_folder_cannot_be_renamed_until_it_is_let_go() {
        let pc = pc();
        let dir = pc.base.join("a").join("b");
        fs::create_dir_all(&dir).unwrap();
        let held = hold_chain(&dir).expect("a real folder").expect("it exists");
        let moved = pc.base.join("a").join("moved");
        let err = fs::rename(&dir, &moved).expect_err("renamed while held");
        eprintln!("renaming a held folder: {err}");
        assert!(fs::rename(pc.base.join("a"), pc.base.join("moved")).is_err());
        drop(held);
        fs::rename(&dir, &moved).expect("renames once let go");
    }

    /// Reads only. Shows the real folders resolve and can be held on a real
    /// Windows install, and prints what they hold for the evidence log.
    #[cfg(windows)]
    #[test]
    fn measures_the_real_areas_on_this_pc() {
        let sid = crate::identity::current_process_sid().expect("own SID");
        let places = places(&sid);
        eprintln!("cleanup places: {places:?}");
        assert!(places.windows.is_some() && places.program_data.is_some() && places.profile.is_some());
        for s in measure(&places, &CleanupArea::ALL, SystemTime::now()) {
            eprintln!(
                "cleanup {:?}: {} files, {} MiB; left alone: {:?}",
                s.area,
                s.files,
                s.bytes >> 20,
                s.skipped
            );
            if s.area == CleanupArea::WindowsTemp {
                assert!(
                    s.skipped.iter().all(|why| !why.contains("left alone")),
                    "{:?}",
                    s.skipped
                );
            }
        }
    }
}
