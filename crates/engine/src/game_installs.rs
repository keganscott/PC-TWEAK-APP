//! Where the known games are installed, and on what kind of drive. Plan 6.2
//! item 7 ("game installed on a hard drive -> advise SSD"). Read-only: files
//! and folders are only looked at, never opened for writing, and no game
//! process is touched.
//!
//! Locations are from memory and marked VERIFY (NOTES.md N54):
//! - Fortnite: the Epic launcher's manifests,
//!   `%ProgramData%\Epic\EpicGamesLauncher\Data\Manifests\*.item` (JSON with
//!   `AppName`, `DisplayName`, `InstallLocation`).
//! - Roblox: per user, `<profile>\AppData\Local\Roblox\Versions`.
//! - Minecraft (Java Edition): per user, its game files in
//!   `<profile>\AppData\Roaming\.minecraft`. Bedrock Edition (Microsoft Store /
//!   Xbox app) is not located.

use std::path::{Path, PathBuf};

use serde::Serialize;
use ts_rs::TS;

use super::hardware::{BootDisk, DiskMedia};
use super::probe::Probe;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GameInstall {
    /// A `KNOWN_GAMES` id.
    pub game_id: String,
    pub name: String,
    pub path: String,
    /// `"D:"`.
    pub drive: String,
    pub disk: Probe<BootDisk>,
    /// The program file the game runs, when PeakTweaks looks for it and found
    /// it (`program_file`). Minecraft's is not looked for: it runs inside a
    /// Java program the launcher picks.
    pub exe: Option<String>,
}

/// The drive letter a Windows path is on, `"D:"`, if it starts with one.
pub fn drive_of(path: &str) -> Option<String> {
    let b = path.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
        .then(|| format!("{}:", (b[0] as char).to_ascii_uppercase()))
}

/// Fortnite's install folder from the Epic manifests, if any names it.
pub fn fortnite_from_epic(manifests: &Path) -> Option<String> {
    let entries = std::fs::read_dir(manifests).ok()?;
    let mut found: Vec<String> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("item")))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(|v| {
            let is = |k: &str| {
                v.get(k)
                    .and_then(|x| x.as_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("fortnite"))
            };
            is("AppName") || is("DisplayName")
        })
        .filter_map(|v| v.get("InstallLocation")?.as_str().map(str::to_owned))
        .filter(|s| !s.trim().is_empty())
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Every known game found, each with the kind of drive it is on.
/// `program_data` is `%ProgramData%`; `profile` the interactive user's profile
/// folder (`None` when it could not be resolved, so per-user games are not
/// looked for); `disk` maps a drive letter to its disk.
pub fn probe_game_installs(
    program_data: &Path,
    profile: Option<&Path>,
    disk: &dyn Fn(&str) -> Probe<BootDisk>,
) -> Vec<GameInstall> {
    let mut found: Vec<(&str, &str, PathBuf)> = Vec::new();
    let manifests = program_data.join(r"Epic\EpicGamesLauncher\Data\Manifests");
    if let Some(loc) = fortnite_from_epic(&manifests) {
        found.push(("fortnite", "Fortnite", PathBuf::from(loc)));
    }
    if let Some(profile) = profile {
        let roblox = profile.join(r"AppData\Local\Roblox\Versions");
        if roblox.is_dir() {
            found.push(("roblox", "Roblox", roblox));
        }
        let minecraft = profile.join(r"AppData\Roaming\.minecraft");
        if minecraft.is_dir() {
            found.push(("minecraft", "Minecraft (Java Edition)", minecraft));
        }
    }
    found
        .into_iter()
        .map(|(id, name, path)| {
            let path = path.to_string_lossy().into_owned();
            let drive = drive_of(&path);
            GameInstall {
                exe: program_file(id, &path),
                game_id: id.into(),
                name: name.into(),
                disk: match &drive {
                    Some(d) => disk(d),
                    None => Probe::unknown(format!("the install path {path:?} has no drive letter")),
                },
                drive: drive.unwrap_or_default(),
                path,
            }
        })
        .collect()
}

fn windows_path(p: &str) -> String {
    p.trim().replace('/', "\\").trim_end_matches('\\').to_owned()
}

/// The newest `RobloxPlayerBeta.exe` under `Versions\<version>\`.
fn newest_roblox_player(versions: &Path) -> Option<PathBuf> {
    std::fs::read_dir(versions)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("RobloxPlayerBeta.exe"))
        .filter_map(|exe| {
            let modified = std::fs::metadata(&exe).ok()?.modified().ok()?;
            Some((modified, exe))
        })
        .max()
        .map(|(_, exe)| exe)
}

/// True for the games whose program file is looked for (`program_file`).
pub fn program_file_is_looked_for(game_id: &str) -> bool {
    matches!(game_id, "fortnite" | "roblox")
}

/// Why a looked-for program file is missing, for an unknown reading.
pub fn program_file_missing(install: &GameInstall) -> String {
    format!(
        "{}'s program file was not found under {}",
        install.name,
        install.path.trim()
    )
}

/// The program file a found game runs, as Windows writes paths (backslashes,
/// no trailing separator). `None` when it is not looked for or not found.
/// VERIFY (NOTES.md N7, N56): Fortnite's
/// `FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe` under its
/// install; Roblox's `RobloxPlayerBeta.exe` in the newest `Versions` folder.
pub fn program_file(game_id: &str, install_path: &str) -> Option<String> {
    let base = Path::new(install_path.trim());
    let found = match game_id {
        "fortnite" => Some(
            base.join("FortniteGame")
                .join("Binaries")
                .join("Win64")
                .join("FortniteClient-Win64-Shipping.exe"),
        )
        .filter(|exe| exe.is_file()),
        "roblox" => newest_roblox_player(base),
        _ => None,
    };
    found.map(|exe| windows_path(&exe.to_string_lossy()))
}

/// True when the install is on a hard drive (not an SSD).
pub fn on_hard_drive(install: &GameInstall) -> bool {
    matches!(&install.disk, Probe::Yes { value } if value.media == DiskMedia::Hdd)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn disk(media: DiskMedia) -> Probe<BootDisk> {
        Probe::yes(BootDisk {
            media,
            name: "disk".into(),
        })
    }

    #[test]
    fn program_files_are_found_for_fortnite_and_roblox_only() {
        let dir = tempfile::tempdir().unwrap();
        let fortnite = dir.path().join("Fortnite");
        assert_eq!(
            program_file("fortnite", &fortnite.to_string_lossy()),
            None,
            "not there yet"
        );
        let bin = fortnite.join("FortniteGame").join("Binaries").join("Win64");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("FortniteClient-Win64-Shipping.exe"), b"").unwrap();
        let exe = program_file("fortnite", &format!("{}/", fortnite.to_string_lossy())).unwrap();
        assert!(
            exe.ends_with(r"\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe"),
            "{exe}"
        );
        assert!(!exe.contains('/'), "written the way Windows writes paths: {exe}");

        let versions = dir.path().join("Versions");
        std::fs::create_dir_all(versions.join("version-1")).unwrap();
        std::fs::write(versions.join("version-1").join("RobloxPlayerBeta.exe"), b"").unwrap();
        assert!(program_file("roblox", &versions.to_string_lossy())
            .unwrap()
            .ends_with(r"version-1\RobloxPlayerBeta.exe"));

        assert_eq!(program_file("minecraft", &dir.path().to_string_lossy()), None);
        assert!(program_file_is_looked_for("roblox") && !program_file_is_looked_for("minecraft"));
    }

    #[test]
    fn drive_letters() {
        assert_eq!(drive_of(r"d:\Games\Fortnite").as_deref(), Some("D:"));
        assert_eq!(drive_of(r"C:\Users\kid").as_deref(), Some("C:"));
        assert_eq!(drive_of(r"\\server\share"), None);
        assert_eq!(drive_of("relative"), None);
    }

    #[test]
    fn fortnite_is_found_in_the_epic_manifests_and_other_games_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let m = dir.path().join(r"Epic\EpicGamesLauncher\Data\Manifests");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("A.item"),
            r#"{"AppName":"Other","DisplayName":"Rocket","InstallLocation":"E:\\Rocket"}"#,
        )
        .unwrap();
        std::fs::write(m.join("B.item"), "not json").unwrap();
        std::fs::write(
            m.join("C.txt"),
            r#"{"AppName":"Fortnite","InstallLocation":"Z:\\nope"}"#,
        )
        .unwrap();
        assert_eq!(fortnite_from_epic(&m), None);
        std::fs::write(
            m.join("D.item"),
            r#"{"AppName":"Fortnite","DisplayName":"Fortnite","InstallLocation":"D:\\Epic Games\\Fortnite"}"#,
        )
        .unwrap();
        assert_eq!(fortnite_from_epic(&m).as_deref(), Some(r"D:\Epic Games\Fortnite"));
        assert_eq!(fortnite_from_epic(&dir.path().join("missing")), None);
    }

    #[test]
    fn games_are_found_and_each_drive_is_looked_up() {
        let dir = tempfile::tempdir().unwrap();
        let program_data = dir.path().join("pd");
        let m = program_data.join(r"Epic\EpicGamesLauncher\Data\Manifests");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("F.item"),
            r#"{"AppName":"Fortnite","InstallLocation":"D:\\Fortnite"}"#,
        )
        .unwrap();
        let profile = dir.path().join("profile");
        std::fs::create_dir_all(profile.join(r"AppData\Local\Roblox\Versions")).unwrap();

        let asked = RefCell::new(Vec::new());
        let lookup = |d: &str| {
            asked.borrow_mut().push(d.to_owned());
            if d == "D:" {
                disk(DiskMedia::Hdd)
            } else {
                Probe::unknown("no drive letter in a temp path")
            }
        };
        let found = probe_game_installs(&program_data, Some(&profile), &lookup);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!((found[0].game_id.as_str(), found[0].drive.as_str()), ("fortnite", "D:"));
        assert!(on_hard_drive(&found[0]));
        assert_eq!(found[1].game_id, "roblox");
        assert!(!on_hard_drive(&found[1]));
        assert_eq!(asked.borrow()[0], "D:");

        // Without the user's profile, per-user games are not looked for.
        let found = probe_game_installs(&program_data, None, &lookup);
        assert_eq!(found.len(), 1);
    }
}
