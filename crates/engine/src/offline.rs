//! Undo from outside Windows, for an installation that will not start
//! (NOTES.md N48).
//!
//! The `session_*.reg` files restore from Safe Mode, where the installed
//! Windows' own hives are live. From the Windows Recovery Environment they
//! would change the recovery system's hives instead, and `CurrentControlSet`
//! does not exist in a hive file. So after every change PeakTweaks also keeps
//! `offline\` in its protected data folder in step with what "Undo all" would
//! restore, with every path rewritten to point at the installation's hive
//! files loaded under temporary names:
//!
//! - `HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\...` ->
//!   `HKEY_LOCAL_MACHINE\PT_OFFLINE_SYSTEM\ControlSet00<n>\...`, `<n>` read
//!   from `SYSTEM\Select\Current` when the change was made;
//! - `HKEY_LOCAL_MACHINE\SYSTEM\...` and `...\SOFTWARE\...` -> `PT_OFFLINE_SYSTEM`
//!   and `PT_OFFLINE_SOFTWARE`;
//! - `HKEY_USERS\<sid>\...` -> `HKEY_LOCAL_MACHINE\PT_OFFLINE_<sid>\...`, the
//!   user's `NTUSER.DAT` at their profile folder.
//!
//! `recover.cmd` (run there, see `SCRIPT`) loads those hive files, imports the
//! files in name order and unloads them. A change that cannot be mapped (any
//! other hive, a profile on another drive) gets no file and is named in
//! `README.txt`. Removing keys PeakTweaks created is left out: a `.reg` file
//! can only delete a key with everything under it.

use std::collections::BTreeMap;
use std::path::Path;

use super::error::{EngineError, Result};
use super::fsutil;
use super::journal::JournalEntry;
use super::reg_export::reg_value_line;
use super::registry::components;

pub const DIR: &str = "offline";
const USERS_FILE: &str = "users.txt";
const README_FILE: &str = "README.txt";
const SCRIPT_FILE: &str = "recover.cmd";

const SYSTEM_MOUNT: &str = "PT_OFFLINE_SYSTEM";
const SOFTWARE_MOUNT: &str = "PT_OFFLINE_SOFTWARE";
const USER_MOUNT_PREFIX: &str = "PT_OFFLINE_";

/// What the remapping needs to know about this PC, read when a change is made.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// `HKLM\SYSTEM\Select\Current`: which `ControlSet00<n>` is current.
    pub control_set: Option<u32>,
    /// `"C:"`.
    pub system_drive: String,
    /// SID -> `ProfileImagePath` as Windows stores it, `None` when absent.
    pub profiles: BTreeMap<String, Option<String>>,
}

/// The files `refresh` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// `(file name, .reg text)`, in import order.
    pub files: Vec<(String, String)>,
    /// `(sid, profile folder without the drive)`, for every SID the files use.
    pub users: Vec<(String, String)>,
    /// `(tweak id, why)` for changes with no file.
    pub not_covered: Vec<(String, String)>,
}

fn looks_like_sid(s: &str) -> bool {
    s.len() > 4
        && s.get(..4).is_some_and(|h| h.eq_ignore_ascii_case("S-1-"))
        && s[4..].chars().all(|c| c.is_ascii_digit() || c == '-')
}

/// A profile folder as a path on the Windows drive without its letter
/// (`\Users\kid`), or why it cannot be found offline.
pub fn profile_on_windows_drive(image_path: &str, system_drive: &str) -> std::result::Result<String, String> {
    let p = image_path.trim();
    let head = |n: usize| p.get(..n).unwrap_or_default();
    let rest = if head(13).eq_ignore_ascii_case("%SystemDrive%") {
        &p[13..]
    } else if p.as_bytes().get(1) == Some(&b':') && p.as_bytes()[0].is_ascii_alphabetic() {
        if !head(2).eq_ignore_ascii_case(system_drive) {
            return Err(format!(
                "the profile folder {p} is not on the Windows drive {system_drive}"
            ));
        }
        &p[2..]
    } else {
        return Err(format!("the profile folder {p} is not a path on a drive"));
    };
    if !rest.starts_with('\\') || rest.contains('%') || rest.contains('|') {
        return Err(format!("the profile folder {p} cannot be found offline"));
    }
    // `cmd` reads users.txt in the console's code page, which is unknown
    // offline, so only plain ASCII survives the trip to reg.exe; and the
    // script's delayed expansion eats `!` and `^` in a `for` variable.
    if !rest.is_ascii() || rest.contains('!') || rest.contains('^') {
        return Err(format!(
            "the profile folder {p} has characters the recovery script cannot pass on"
        ));
    }
    Ok(rest.trim_end_matches('\\').to_owned())
}

/// `display_path` rewritten to the loaded hive files, and the SID whose hive it
/// needs (if a user's), or why it cannot be.
pub fn remap(display_path: &str, facts: &Facts) -> std::result::Result<(String, Option<String>), String> {
    let parts = components(display_path);
    let is = |i: usize, name: &str| parts.get(i).is_some_and(|p| p.eq_ignore_ascii_case(name));
    let tail = |from: usize| parts[from..].iter().map(|p| format!("\\{p}")).collect::<String>();
    if is(0, "HKEY_LOCAL_MACHINE") && is(1, "SYSTEM") && is(2, "CurrentControlSet") {
        let n = facts
            .control_set
            .ok_or("which control set Windows starts with (SYSTEM\\Select\\Current) could not be read")?;
        return Ok((
            format!("HKEY_LOCAL_MACHINE\\{SYSTEM_MOUNT}\\ControlSet{n:03}{}", tail(3)),
            None,
        ));
    }
    if is(0, "HKEY_LOCAL_MACHINE") && is(1, "SYSTEM") {
        return Ok((format!("HKEY_LOCAL_MACHINE\\{SYSTEM_MOUNT}{}", tail(2)), None));
    }
    if is(0, "HKEY_LOCAL_MACHINE") && is(1, "SOFTWARE") {
        return Ok((format!("HKEY_LOCAL_MACHINE\\{SOFTWARE_MOUNT}{}", tail(2)), None));
    }
    if is(0, "HKEY_USERS") {
        let sid = parts.get(1).copied().unwrap_or_default();
        if !looks_like_sid(sid) {
            return Err(format!("{sid} under HKEY_USERS is not a user's own settings"));
        }
        let image = facts
            .profiles
            .get(sid)
            .cloned()
            .flatten()
            .ok_or_else(|| format!("the profile folder of {sid} is not known"))?;
        profile_on_windows_drive(&image, &facts.system_drive)?;
        // A user's `Software\Classes` is their UsrClass.dat, linked in at sign
        // in; it is not in NTUSER.DAT, the file the script loads.
        if is(2, "Software") && is(3, "Classes") {
            return Err(format!(
                "{display_path} is in {sid}'s own file types and programs (UsrClass.dat), which the recovery \
                 script does not load"
            ));
        }
        return Ok((
            format!("HKEY_LOCAL_MACHINE\\{USER_MOUNT_PREFIX}{sid}{}", tail(2)),
            Some(sid.to_owned()),
        ));
    }
    Err(format!(
        "{display_path} is in a part of the registry the recovery script does not load"
    ))
}

/// One file per tweak with outstanding applies, in the order "Undo all" uses
/// (`outstanding` is most recently applied first, each tweak's writes oldest
/// first). Inside a file the newest write comes first, so when writes stacked
/// on one value the oldest prior value is the one left, as in-app undo does.
pub fn plan(outstanding: &[(String, Vec<JournalEntry>)], facts: &Facts) -> Plan {
    let mut files = Vec::new();
    let mut users: BTreeMap<String, String> = BTreeMap::new();
    let mut not_covered = Vec::new();
    for (tweak_id, writes) in outstanding {
        let mut text = String::from("Windows Registry Editor Version 5.00\r\n\r\n");
        let mut tweak_users = Vec::new();
        let mut failed = None;
        for e in writes.iter().rev() {
            match remap(&e.display_path, facts) {
                Ok((path, sid)) => {
                    text.push_str(&format!(
                        "[{path}]\r\n{}\r\n\r\n",
                        reg_value_line(&e.value_name, e.previous.as_ref())
                    ));
                    tweak_users.extend(sid);
                }
                Err(why) => {
                    failed = Some(why);
                    break;
                }
            }
        }
        // All of a change or none of it: half an undo is a state nobody chose.
        if let Some(why) = failed {
            not_covered.push((tweak_id.clone(), why));
            continue;
        }
        for sid in tweak_users {
            let image = facts.profiles.get(&sid).cloned().flatten().unwrap_or_default();
            if let Ok(rel) = profile_on_windows_drive(&image, &facts.system_drive) {
                users.insert(sid, rel);
            }
        }
        files.push((format!("{:03}_{}.reg", files.len() + 1, file_safe(tweak_id)), text));
    }
    Plan {
        files,
        users: users.into_iter().collect(),
        not_covered,
    }
}

fn file_safe(s: &str) -> String {
    s.chars()
        .take(48)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn utf16_bom(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

fn io(path: &Path, e: std::io::Error) -> EngineError {
    EngineError::storage(path.display().to_string(), e)
}

/// Bring `<root>\offline` in line with `plan`: write every file, then remove
/// `.reg` files the plan no longer has. Written before removed, so a crash in
/// between leaves a file twice, which imports the same values twice, rather
/// than a change with no file.
pub fn refresh(root: &Path, plan: &Plan) -> Result<()> {
    let dir = root.join(DIR);
    fsutil::create_dir_durable(&dir)?;
    for (name, text) in &plan.files {
        fsutil::write_durable(&dir.join(name), &utf16_bom(text))?;
    }
    let users: String = plan.users.iter().map(|(sid, rel)| format!("{sid}|{rel}\r\n")).collect();
    fsutil::write_durable(&dir.join(USERS_FILE), users.as_bytes())?;
    fsutil::write_durable(&dir.join(README_FILE), readme(plan).as_bytes())?;
    fsutil::write_durable(&dir.join(SCRIPT_FILE), script_crlf().as_bytes())?;

    let keep: Vec<&str> = plan.files.iter().map(|(n, _)| n.as_str()).collect();
    for entry in std::fs::read_dir(&dir).map_err(|e| io(&dir, e))? {
        let path = entry.map_err(|e| io(&dir, e))?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        if name.to_ascii_lowercase().ends_with(".reg") && !keep.contains(&name.as_str()) {
            std::fs::remove_file(&path).map_err(|e| io(&path, e))?;
        }
    }
    fsutil::sync_dir(&dir)
}

fn readme(plan: &Plan) -> String {
    let mut s = String::from(
        "PeakTweaks: undo its registry changes on a Windows installation that will not start.\r\n\
         \r\n\
         Use this only when Windows cannot start normally or in Safe Mode. If Windows starts, use\r\n\
         Undo in PeakTweaks instead, or import the session_*.reg files in the backups folder from\r\n\
         Safe Mode.\r\n\
         \r\n\
         1. Start the Windows Recovery Environment (hold Shift while choosing Restart, or let Windows\r\n\
         \x20  open it after failed starts), then Troubleshoot > Advanced options > Command Prompt.\r\n\
         2. Find the Windows drive: the letter can differ there. Try  dir C:\\Windows  then D:, E:.\r\n\
         \x20  If the drive is locked by BitLocker, unlock it first with your recovery key:\r\n\
         \x20  manage-bde -unlock D: -RecoveryPassword <key>\r\n\
         3. Run  D:\\ProgramData\\PeakTweaks\\offline\\recover.cmd  (with your drive letter).\r\n\
         4. Close the Command Prompt and choose Continue to start Windows.\r\n\
         \r\n\
         The script loads that Windows' registry files, imports the .reg files here in name order\r\n\
         (most recent change first), and unloads them again. Keys PeakTweaks created are left in\r\n\
         place, empty. PeakTweaks still lists the changes as applied afterwards; Undo there finishes\r\n\
         the record.\r\n",
    );
    if !plan.not_covered.is_empty() {
        s.push_str("\r\nNot covered by this script (undo these in PeakTweaks or Safe Mode):\r\n");
        for (id, why) in &plan.not_covered {
            s.push_str(&format!("- {id}: {why}\r\n"));
        }
    }
    s
}

/// `cmd` can lose its place in a batch file with bare LF line endings (a
/// `goto` lands mid-line), so the script is written with CRLF whatever the
/// source file uses.
fn script_crlf() -> String {
    SCRIPT.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Run from the recovery environment's Command Prompt. The Windows folder is
/// taken from the script's own drive unless given as the first argument (CI
/// passes a folder holding test hive files). Every hive that was loaded is
/// unloaded, even after a failure, because unloading is what writes the
/// changes to disk and leaves the files usable.
pub const SCRIPT: &str = r#"@echo off
rem PeakTweaks offline recovery. See README.txt in this folder before running.
setlocal EnableExtensions EnableDelayedExpansion
set "HERE=%~dp0"
set "W=%~d0"
if not "%~1"=="" set "W=%~1"
if not exist "%W%\Windows\System32\config\SYSTEM" (
  echo No Windows installation at %W%. Run this from the Windows drive, or give its letter: recover.cmd D:
  exit /b 2
)
set "FAILED=0"
reg load HKLM\PT_OFFLINE_SYSTEM "%W%\Windows\System32\config\SYSTEM" >nul
if errorlevel 1 (
  echo Could not load %W%\Windows\System32\config\SYSTEM. If this Windows is running, use Undo in PeakTweaks instead.
  exit /b 3
)
reg load HKLM\PT_OFFLINE_SOFTWARE "%W%\Windows\System32\config\SOFTWARE" >nul
if errorlevel 1 (
  echo Could not load %W%\Windows\System32\config\SOFTWARE.
  set "FAILED=1"
  goto unload
)
for /f "usebackq tokens=1,* delims=|" %%a in ("%HERE%users.txt") do (
  reg load "HKLM\PT_OFFLINE_%%a" "%W%%%b\NTUSER.DAT" >nul
  if errorlevel 1 (
    echo Could not load the settings of %%a from %W%%%b\NTUSER.DAT.
    set "FAILED=1"
  )
)
if "!FAILED!"=="1" goto unload
for /f "delims=" %%f in ('dir /b /a-d /on "%HERE%*.reg" 2^>nul') do (
  echo Undoing %%f
  reg import "%HERE%%%f"
  if errorlevel 1 set "FAILED=1"
)
:unload
for /f "usebackq tokens=1,* delims=|" %%a in ("%HERE%users.txt") do reg unload "HKLM\PT_OFFLINE_%%a" >nul 2>&1
reg unload HKLM\PT_OFFLINE_SOFTWARE >nul 2>&1
reg unload HKLM\PT_OFFLINE_SYSTEM >nul 2>&1
if "!FAILED!"=="1" (
  echo Not everything was undone. Nothing above was skipped silently; see the messages.
  exit /b 1
)
echo Done. Close this window and choose Continue to start Windows.
exit /b 0
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::JournalAction;
    use crate::types::RawValue;

    const SID: &str = "S-1-5-21-1-2-3-1001";

    fn facts() -> Facts {
        Facts {
            control_set: Some(1),
            system_drive: "C:".into(),
            profiles: [(SID.to_owned(), Some(r"C:\Users\kid".to_owned()))].into(),
        }
    }

    fn write(seq: u64, tweak: &str, display_path: &str, name: &str, previous: Option<RawValue>) -> JournalEntry {
        let mut e = crate::journal::tests::entry(seq, seq, tweak, JournalAction::Apply);
        e.display_path = display_path.into();
        e.value_name = name.into();
        e.previous = previous;
        e
    }

    #[test]
    fn a_users_own_classes_are_not_covered() {
        let why = remap(
            &format!(
                r"HKEY_USERS\{SID}\Software\Classes\CLSID\{{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}}\InprocServer32"
            ),
            &facts(),
        )
        .unwrap_err();
        assert!(why.contains("UsrClass.dat"), "{why}");
        assert!(remap(&format!(r"HKEY_USERS\{SID}\Software\Microsoft\GameBar"), &facts()).is_ok());
    }

    #[test]
    fn paths_are_rewritten_to_the_loaded_hive_files() {
        let f = facts();
        assert_eq!(
            remap(
                r"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Control\PriorityControl",
                &f
            )
            .unwrap(),
            (
                r"HKEY_LOCAL_MACHINE\PT_OFFLINE_SYSTEM\ControlSet001\Control\PriorityControl".into(),
                None
            )
        );
        assert_eq!(
            remap(r"hkey_local_machine\system\Setup", &f).unwrap().0,
            r"HKEY_LOCAL_MACHINE\PT_OFFLINE_SYSTEM\Setup"
        );
        assert_eq!(
            remap(
                r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore",
                &f
            )
            .unwrap()
            .0,
            r"HKEY_LOCAL_MACHINE\PT_OFFLINE_SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore"
        );
        assert_eq!(
            remap(&format!(r"HKEY_USERS\{SID}\Control Panel\Mouse"), &f).unwrap(),
            (
                format!(r"HKEY_LOCAL_MACHINE\PT_OFFLINE_{SID}\Control Panel\Mouse"),
                Some(SID.to_owned())
            )
        );
    }

    #[test]
    fn what_cannot_be_found_offline_is_refused_with_a_reason() {
        let mut f = facts();
        for (path, why) in [
            (r"HKEY_CLASSES_ROOT\.txt", "does not load"),
            (r"HKEY_LOCAL_MACHINE\SAM\x", "does not load"),
            (r"HKEY_USERS\.DEFAULT\x", "not a user's own settings"),
            (&format!(r"HKEY_USERS\{SID}_Classes\x"), "not a user's own settings"),
            (r"HKEY_USERS\S-1-5-21-9\x", "is not known"),
        ] {
            let err = remap(path, &f).unwrap_err();
            assert!(err.contains(why), "{path}: {err}");
        }
        f.control_set = None;
        assert!(remap(r"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\x", &f)
            .unwrap_err()
            .contains("Select"));
        f.profiles.insert(SID.into(), Some(r"D:\Users\kid".into()));
        assert!(remap(&format!(r"HKEY_USERS\{SID}\x"), &f)
            .unwrap_err()
            .contains("not on the Windows drive"));
    }

    #[test]
    fn profile_folders_lose_their_drive() {
        assert_eq!(profile_on_windows_drive(r"C:\Users\kid", "C:").unwrap(), r"\Users\kid");
        assert_eq!(profile_on_windows_drive(r"c:\Users\kid\", "C:").unwrap(), r"\Users\kid");
        assert_eq!(
            profile_on_windows_drive(r"%SystemDrive%\Users\kid", "C:").unwrap(),
            r"\Users\kid"
        );
        assert!(profile_on_windows_drive(r"\\server\profiles\kid", "C:").is_err());
        assert!(profile_on_windows_drive(r"%USERPROFILE%", "C:").is_err());
        assert!(profile_on_windows_drive(r"C:\Users\a|b", "C:").is_err());
        assert!(profile_on_windows_drive(r"C:\Users\José", "C:").is_err());
        assert!(profile_on_windows_drive("é:\\Users", "C:").is_err());
        assert!(profile_on_windows_drive("%SystemDriveé", "C:").is_err());
        assert!(!looks_like_sid("Sé-1-5"));
        // recover.cmd runs with delayed expansion on, which eats `!` (and the
        // `^` before it) in a `for` variable: the hive path would be wrong.
        assert!(profile_on_windows_drive(r"C:\Users\Bob!", "C:").is_err());
        assert!(profile_on_windows_drive(r"C:\Users\a^b", "C:").is_err());
        assert_eq!(
            profile_on_windows_drive(r"C:\Users\Tom & Jerry (2)", "C:").unwrap(),
            r"\Users\Tom & Jerry (2)",
            "quoted in the script, so & and brackets are fine"
        );
    }

    #[test]
    fn one_file_per_change_in_undo_all_order_with_the_oldest_prior_value_last() {
        let mouse = format!(r"HKEY_USERS\{SID}\Control Panel\Mouse");
        let outstanding = vec![
            (
                "input.mouseaccel".to_owned(),
                vec![
                    write(10, "input.mouseaccel", &mouse, "MouseSpeed", Some(RawValue::sz("1"))),
                    write(20, "input.mouseaccel", &mouse, "MouseSpeed", Some(RawValue::sz("0"))),
                ],
            ),
            (
                "system.restore.frequency".to_owned(),
                vec![write(
                    5,
                    "system.restore.frequency",
                    r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore",
                    "SystemRestorePointCreationFrequency",
                    None,
                )],
            ),
            (
                "classes".to_owned(),
                vec![write(3, "classes", r"HKEY_CLASSES_ROOT\x", "v", None)],
            ),
        ];
        let p = plan(&outstanding, &facts());
        assert_eq!(
            p.files.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["001_input.mouseaccel.reg", "002_system.restore.frequency.reg"]
        );
        let mouse_file = &p.files[0].1;
        let newer = mouse_file.find("\"MouseSpeed\"=\"0\"").unwrap();
        let older = mouse_file.find("\"MouseSpeed\"=\"1\"").unwrap();
        assert!(newer < older, "the oldest prior value is imported last:\n{mouse_file}");
        assert!(mouse_file.contains(&format!(r"[HKEY_LOCAL_MACHINE\PT_OFFLINE_{SID}\Control Panel\Mouse]")));
        assert!(p.files[1].1.contains("\"SystemRestorePointCreationFrequency\"=-"));
        assert_eq!(p.users, vec![(SID.to_owned(), r"\Users\kid".to_owned())]);
        assert_eq!(p.not_covered.len(), 1);
        assert_eq!(p.not_covered[0].0, "classes");
    }

    #[test]
    fn a_change_with_one_unmappable_write_gets_no_file_at_all() {
        let outstanding = vec![(
            "mixed".to_owned(),
            vec![
                write(1, "mixed", r"HKEY_LOCAL_MACHINE\SOFTWARE\A", "v", None),
                write(2, "mixed", r"HKEY_LOCAL_MACHINE\SAM\B", "v", None),
            ],
        )];
        let p = plan(&outstanding, &facts());
        assert!(p.files.is_empty());
        assert_eq!(p.not_covered.len(), 1);
        assert!(readme(&p).contains("- mixed: "));
    }

    #[test]
    fn refresh_writes_the_set_and_removes_files_no_longer_needed() {
        let dir = tempfile::tempdir().unwrap();
        let first = Plan {
            files: vec![
                ("001_a.reg".into(), "Windows Registry Editor Version 5.00\r\n".into()),
                ("002_b.reg".into(), "Windows Registry Editor Version 5.00\r\n".into()),
            ],
            users: vec![(SID.into(), r"\Users\kid".into())],
            not_covered: vec![],
        };
        refresh(dir.path(), &first).unwrap();
        let off = dir.path().join(DIR);
        let bytes = std::fs::read(off.join("001_a.reg")).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xFE], "UTF-16LE with a BOM, as reg.exe needs");
        assert_eq!(
            std::fs::read_to_string(off.join(USERS_FILE)).unwrap(),
            format!("{SID}|\\Users\\kid\r\n")
        );
        let script = std::fs::read_to_string(off.join(SCRIPT_FILE)).unwrap();
        assert!(script.contains("reg import"));
        assert_eq!(
            script.matches('\n').count(),
            script.matches("\r\n").count(),
            "CRLF only"
        );

        let second = Plan {
            files: vec![("001_b.reg".into(), "Windows Registry Editor Version 5.00\r\n".into())],
            users: vec![],
            not_covered: vec![],
        };
        refresh(dir.path(), &second).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&off)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".reg"))
            .collect();
        names.sort();
        assert_eq!(names, ["001_b.reg"]);
        assert_eq!(std::fs::read_to_string(off.join(USERS_FILE)).unwrap(), "");
    }

    #[test]
    fn the_script_unloads_every_hive_it_loads() {
        for mount in [SYSTEM_MOUNT, SOFTWARE_MOUNT] {
            assert!(SCRIPT.contains(&format!("reg load HKLM\\{mount}")));
            assert!(SCRIPT.contains(&format!("reg unload HKLM\\{mount}")));
        }
        assert!(SCRIPT.contains(&format!("reg load \"HKLM\\{USER_MOUNT_PREFIX}%%a\"")));
        assert!(SCRIPT.contains(&format!("reg unload \"HKLM\\{USER_MOUNT_PREFIX}%%a\"")));
        assert!(SCRIPT.lines().all(|l| !l.ends_with(' ')), "no trailing spaces");
    }
}
