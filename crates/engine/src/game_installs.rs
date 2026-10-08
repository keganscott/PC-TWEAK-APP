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
//! - Valorant: the Riot Client's settings for it,
//!   `%ProgramData%\Riot Games\Metadata\valorant.live\valorant.live.product_settings.yaml`
//!   (`product_install_full_path: "C:/Riot Games/VALORANT/live"`).
//! - Counter-Strike 2 and Apex Legends on Steam: each library in Steam's
//!   `steamapps\libraryfolders.vdf` (`"path"`), then
//!   `steamapps\appmanifest_<app id>.acf` (`"installdir"`) under
//!   `steamapps\common`. App ids 730 and 1172470.
//! - Apex Legends from the EA app: the folder in the registry (`Launchers`).
//!
//! Valorant, CS2 and Apex are looked for but reported only once they are
//! offered (`env::OFFER_VALORANT_CS2_APEX`, NOTES N75).

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

/// Launcher folders read from the registry (`sysprobe.rs`), VERIFY.
#[derive(Debug, Clone, Default)]
pub struct Launchers {
    /// Steam's own folder: `HKLM\SOFTWARE\WOW6432Node\Valve\Steam`, `InstallPath`.
    pub steam: Option<PathBuf>,
    /// Apex Legends installed by the EA app: `HKLM\SOFTWARE\Respawn\Apex`,
    /// `Install Dir`.
    pub apex_ea: Option<PathBuf>,
}

/// Steam app ids (VERIFY).
const CS2_APP: u32 = 730;
const APEX_APP: u32 = 1_172_470;

/// Every offered game found, each with the kind of drive it is on.
/// `program_data` is `%ProgramData%`; `profile` the interactive user's profile
/// folder (`None` when it could not be resolved, so per-user games are not
/// looked for); `disk` maps a drive letter to its disk.
pub fn probe_game_installs(
    program_data: &Path,
    profile: Option<&Path>,
    launchers: &Launchers,
    disk: &dyn Fn(&str) -> Probe<BootDisk>,
) -> Vec<GameInstall> {
    probe_all_game_installs(program_data, profile, launchers, disk)
        .into_iter()
        .filter(|g| crate::env::is_offered(&g.game_id))
        .collect()
}

/// `probe_game_installs` before the games not offered yet are left out.
fn probe_all_game_installs(
    program_data: &Path,
    profile: Option<&Path>,
    launchers: &Launchers,
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
    if let Some(loc) = valorant_from_riot(program_data) {
        found.push(("valorant", "Valorant", PathBuf::from(loc)));
    }
    let libraries = launchers.steam.as_deref().map(steam_libraries).unwrap_or_default();
    if let Some(loc) = steam_app(&libraries, CS2_APP) {
        found.push(("cs2", "Counter-Strike 2", PathBuf::from(loc)));
    }
    let apex = steam_app(&libraries, APEX_APP).or_else(|| {
        let ea = launchers.apex_ea.as_deref()?;
        ea.is_dir().then(|| windows_path(&ea.to_string_lossy()))
    });
    if let Some(loc) = apex {
        found.push(("apex", "Apex Legends", PathBuf::from(loc)));
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

/// Valorant's install folder from the Riot Client's settings for it, if that
/// folder is there (the settings can outlive the game).
pub fn valorant_from_riot(program_data: &Path) -> Option<String> {
    let file = program_data.join(r"Riot Games\Metadata\valorant.live\valorant.live.product_settings.yaml");
    let text = std::fs::read_to_string(file).ok()?;
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix("product_install_full_path:")?.trim();
        let value = value.trim_matches(|c| c == '"' || c == '\'');
        (!value.is_empty() && Path::new(value).is_dir()).then(|| windows_path(value))
    })
}

/// Valve's KeyValues text (`.vdf`, `.acf`) as tokens: quoted strings, unescaped,
/// and the braces between them.
#[derive(Debug, PartialEq)]
enum Vdf {
    Text(String),
    Open,
    Close,
}

fn vdf_tokens(text: &str) -> Vec<Vdf> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '{' => out.push(Vdf::Open),
            '}' => out.push(Vdf::Close),
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => break,
                        },
                        other => s.push(other),
                    }
                }
                out.push(Vdf::Text(s));
            }
            _ => {}
        }
    }
    out
}

/// Every `"key" "value"` pair, at any depth, in order.
fn vdf_pairs(text: &str) -> Vec<(String, String)> {
    let tokens = vdf_tokens(text);
    let mut pairs = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match (&tokens[i], tokens.get(i + 1)) {
            (Vdf::Text(k), Some(Vdf::Text(v))) => {
                pairs.push((k.clone(), v.clone()));
                i += 2;
            }
            _ => i += 1,
        }
    }
    pairs
}

/// Steam's own folder and every library listed in its `libraryfolders.vdf`.
pub fn steam_libraries(steam: &Path) -> Vec<PathBuf> {
    let mut libraries = vec![steam.to_path_buf()];
    let vdf = steam.join("steamapps").join("libraryfolders.vdf");
    for (key, value) in std::fs::read_to_string(vdf).map(|t| vdf_pairs(&t)).unwrap_or_default() {
        let path = PathBuf::from(value);
        if key.eq_ignore_ascii_case("path") && !libraries.contains(&path) {
            libraries.push(path);
        }
    }
    libraries
}

/// A Steam game's folder, `<library>\steamapps\common\<installdir>`, from the
/// first library whose `appmanifest_<app id>.acf` names one that exists.
pub fn steam_app(libraries: &[PathBuf], app_id: u32) -> Option<String> {
    libraries.iter().find_map(|library| {
        let apps = library.join("steamapps");
        let manifest = std::fs::read_to_string(apps.join(format!("appmanifest_{app_id}.acf"))).ok()?;
        let (_, dir) = vdf_pairs(&manifest)
            .into_iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("installdir"))?;
        // A folder name, never a path out of `common`.
        if dir.is_empty() || dir.contains(['\\', '/']) || dir == "." || dir == ".." {
            return None;
        }
        let path = apps.join("common").join(dir);
        path.is_dir().then(|| windows_path(&path.to_string_lossy()))
    })
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
    matches!(game_id, "fortnite" | "roblox" | "valorant" | "cs2" | "apex")
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
/// VERIFY (NOTES.md N7, N56, N75): Fortnite's
/// `FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe` under its
/// install; Roblox's `RobloxPlayerBeta.exe` in the newest `Versions` folder;
/// Valorant's `ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe`; CS2's
/// `game\bin\win64\cs2.exe`; Apex's `r5apex.exe`, else `r5apex_dx12.exe`.
pub fn program_file(game_id: &str, install_path: &str) -> Option<String> {
    let base = Path::new(install_path.trim());
    let under = |parts: &[&str]| {
        Some(parts.iter().fold(base.to_path_buf(), |p, part| p.join(part))).filter(|exe| exe.is_file())
    };
    let found = match game_id {
        "fortnite" => under(&["FortniteGame", "Binaries", "Win64", "FortniteClient-Win64-Shipping.exe"]),
        "roblox" => newest_roblox_player(base),
        "valorant" => under(&["ShooterGame", "Binaries", "Win64", "VALORANT-Win64-Shipping.exe"]),
        "cs2" => under(&["game", "bin", "win64", "cs2.exe"]),
        "apex" => under(&["r5apex.exe"]).or_else(|| under(&["r5apex_dx12.exe"])),
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
    fn program_files_are_found_for_fortnite_and_roblox() {
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
        let none = Launchers::default();
        let found = probe_game_installs(&program_data, Some(&profile), &none, &lookup);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!((found[0].game_id.as_str(), found[0].drive.as_str()), ("fortnite", "D:"));
        assert!(on_hard_drive(&found[0]));
        assert_eq!(found[1].game_id, "roblox");
        assert!(!on_hard_drive(&found[1]));
        assert_eq!(asked.borrow()[0], "D:");

        // Without the user's profile, per-user games are not looked for.
        let found = probe_game_installs(&program_data, None, &none, &lookup);
        assert_eq!(found.len(), 1);
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn valorant_is_found_in_the_riot_client_settings() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir
            .path()
            .join(r"Riot Games\Metadata\valorant.live\valorant.live.product_settings.yaml");
        assert_eq!(valorant_from_riot(dir.path()), None);
        let live = dir.path().join("VALORANT").join("live");
        let settings = format!(
            "product_install_full_path: \"{}/\"\nproduct_install_root: \"D:/Riot Games\"\n",
            live.to_string_lossy()
        );
        write(&file, &settings);
        assert_eq!(valorant_from_riot(dir.path()), None, "the settings outlived the game");
        std::fs::create_dir_all(&live).unwrap();
        let found = valorant_from_riot(dir.path()).unwrap();
        assert_eq!(found, windows_path(&live.to_string_lossy()));
        assert!(!found.contains('/') && !found.ends_with('\\'), "{found}");
        write(&file, "product_install_full_path: \"\"\n");
        assert_eq!(valorant_from_riot(dir.path()), None, "an empty path is no install");
    }

    #[test]
    fn steam_games_are_found_in_any_library_and_installdir_cannot_leave_common() {
        let dir = tempfile::tempdir().unwrap();
        let steam = dir.path().join("Steam");
        let other = dir.path().join("Games");
        // Steam writes backslashes doubled inside its quoted strings.
        let other_quoted = other.to_string_lossy().replace('\\', "\\\\");
        write(
            &steam.join("steamapps").join("libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{other_quoted}\"\n\t\t\"apps\"\n\t\t{{\n\t\t\t\"730\"\t\t\"0\"\n\t\t}}\n\t}}\n}}\n",
                steam.to_string_lossy().replace('\\', "\\\\")
            ),
        );
        let libraries = steam_libraries(&steam);
        assert_eq!(
            libraries,
            vec![steam.clone(), other.clone()],
            "Steam itself is listed once"
        );

        let apps = other.join("steamapps");
        write(
            &apps.join("appmanifest_730.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"installdir\"\t\t\"Counter-Strike Global Offensive\"\n}\n",
        );
        assert_eq!(
            steam_app(&libraries, CS2_APP),
            None,
            "the manifest alone is not an install"
        );
        std::fs::create_dir_all(apps.join("common").join("Counter-Strike Global Offensive")).unwrap();
        let cs2 = steam_app(&libraries, CS2_APP).unwrap();
        assert!(cs2.ends_with("Counter-Strike Global Offensive"), "{cs2}");

        for bad in ["..", ".", "", r"..\\..\\Windows", "a/b"] {
            write(
                &apps.join("appmanifest_1172470.acf"),
                &format!("\"AppState\" {{ \"installdir\" \"{bad}\" }}"),
            );
            assert_eq!(steam_app(&libraries, APEX_APP), None, "{bad:?}");
        }
        assert_eq!(steam_libraries(&dir.path().join("nowhere")).len(), 1);
    }

    #[test]
    fn valorant_cs2_and_apex_are_looked_for_but_reported_only_once_offered() {
        let dir = tempfile::tempdir().unwrap();
        let program_data = dir.path().join("pd");
        let valorant = dir.path().join("VALORANT");
        std::fs::create_dir_all(&valorant).unwrap();
        write(
            &program_data.join(r"Riot Games\Metadata\valorant.live\valorant.live.product_settings.yaml"),
            &format!("product_install_full_path: \"{}\"\n", valorant.to_string_lossy()),
        );
        let steam = dir.path().join("Steam");
        let apps = steam.join("steamapps");
        write(
            &apps.join("appmanifest_730.acf"),
            r#""AppState" { "installdir" "cs2" }"#,
        );
        std::fs::create_dir_all(apps.join("common").join("cs2")).unwrap();
        let ea = dir.path().join("Apex");
        std::fs::create_dir_all(&ea).unwrap();
        let launchers = Launchers {
            steam: Some(steam),
            apex_ea: Some(ea),
        };
        let lookup = |_: &str| Probe::unknown("temp path");

        let all: Vec<String> = probe_all_game_installs(&program_data, None, &launchers, &lookup)
            .into_iter()
            .map(|g| g.game_id)
            .collect();
        assert_eq!(all, ["valorant", "cs2", "apex"]);
        let reported: Vec<String> = probe_game_installs(&program_data, None, &launchers, &lookup)
            .into_iter()
            .map(|g| g.game_id)
            .collect();
        let expected: Vec<&str> = all
            .iter()
            .map(String::as_str)
            .filter(|id| crate::env::is_offered(id))
            .collect();
        assert_eq!(reported, expected);
        // All three, or none until they are offered (NOTES N75).
        assert_eq!(reported.is_empty(), !crate::env::OFFER_VALORANT_CS2_APEX);
    }

    #[test]
    fn program_files_of_the_new_games() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_string_lossy().into_owned();
        let make = |parts: &[&str]| {
            let p = parts.iter().fold(dir.path().to_path_buf(), |p, part| p.join(part));
            write(&p, "");
        };
        assert_eq!(program_file("apex", &base), None);
        make(&["r5apex_dx12.exe"]);
        assert!(program_file("apex", &base).unwrap().ends_with("r5apex_dx12.exe"));
        make(&["r5apex.exe"]);
        assert!(
            program_file("apex", &base).unwrap().ends_with("r5apex.exe"),
            "the DirectX 11 build first"
        );
        make(&["game", "bin", "win64", "cs2.exe"]);
        assert!(program_file("cs2", &base).unwrap().ends_with(r"game\bin\win64\cs2.exe"));
        make(&["ShooterGame", "Binaries", "Win64", "VALORANT-Win64-Shipping.exe"]);
        assert!(program_file("valorant", &base)
            .unwrap()
            .ends_with(r"ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe"));
        for g in ["valorant", "cs2", "apex"] {
            assert!(program_file_is_looked_for(g), "{g}");
        }
    }
}
