//! Which graphics chip Windows is told to run each found game on. Plan 6.2
//! item 4 ("game on the integrated GPU on laptops"), read-only half: this
//! reads the setting and changes nothing.
//!
//! Windows keeps the per-app choice (Settings > System > Display > Graphics)
//! in the user's hive, one string value per program, named by the program's
//! full path, with data such as `GpuPreference=2;`. 0 = let Windows decide,
//! 1 = power saving, 2 = high performance. Key, value format and numbers are
//! from memory (VERIFY, NOTES.md N56).
//!
//! Which program file each game runs (VERIFY, N7 and N56):
//! - Fortnite: `<install>\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe`.
//! - Roblox: `RobloxPlayerBeta.exe` in the newest folder under `Versions`. Each
//!   Roblox update installs to a new folder, so a choice saved for one version's
//!   path does not carry over.
//! - Minecraft (Java Edition) runs inside a Java program the launcher picks, so
//!   it is not checked.

use serde::Serialize;
use ts_rs::TS;

use super::error::Result;
use super::game_installs::{program_file_is_looked_for, program_file_missing, GameInstall};
use super::probe::Probe;
use super::types::RawValue;

/// Under the interactive user's hive.
pub const KEY: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum GpuPreference {
    /// No value for this program: Windows or the graphics driver chooses.
    NotSet,
    LetWindowsDecide,
    PowerSaving,
    HighPerformance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GameGpuChoice {
    /// A `KNOWN_GAMES` id.
    pub game_id: String,
    pub name: String,
    /// The program file the choice is saved under, when it was found.
    pub exe: Option<String>,
    pub preference: Probe<GpuPreference>,
}

/// `GpuPreference=<n>;` among other `key=value;` pairs. `None` when the data
/// has no such pair or a number we do not know.
pub fn parse_preference(data: &str) -> Option<GpuPreference> {
    let n = data.split(';').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        k.trim()
            .eq_ignore_ascii_case("GpuPreference")
            .then(|| v.trim().parse::<u32>().ok())?
    })?;
    match n {
        0 => Some(GpuPreference::LetWindowsDecide),
        1 => Some(GpuPreference::PowerSaving),
        2 => Some(GpuPreference::HighPerformance),
        _ => None,
    }
}

/// Reads one value under `KEY` in the user's hive, by program path.
pub type ReadSetting<'a> = &'a dyn Fn(&str) -> Result<Option<RawValue>>;

/// Read the saved choice for every found game that is checked. `read` returns
/// the value named by a program path under `KEY` in the user's hive; `None`
/// for `read` means whose hive to read could not be worked out.
pub fn probe_gpu_choices(installs: &[GameInstall], read: Option<ReadSetting<'_>>) -> Vec<GameGpuChoice> {
    installs
        .iter()
        .filter_map(|g| {
            if !program_file_is_looked_for(&g.game_id) {
                return None;
            }
            let (exe, preference) = match &g.exe {
                None => (None, Probe::unknown(program_file_missing(g))),
                Some(exe) => {
                    let exe = exe.clone();
                    let preference = match read {
                        None => Probe::unknown("whose Windows settings to read could not be worked out"),
                        Some(read) => preference_from(read(&exe)),
                    };
                    (Some(exe), preference)
                }
            };
            Some(GameGpuChoice {
                game_id: g.game_id.clone(),
                name: g.name.clone(),
                exe,
                preference,
            })
        })
        .collect()
}

fn preference_from(value: Result<Option<RawValue>>) -> Probe<GpuPreference> {
    match value {
        Err(e) => Probe::unknown(format!("the setting could not be read: {e}")),
        Ok(None) => Probe::yes(GpuPreference::NotSet),
        Ok(Some(v)) => match v.as_sz() {
            None => Probe::unknown(format!("the setting has registry type {}, not text", v.vtype)),
            Some(data) => match parse_preference(&data) {
                Some(p) => Probe::yes(p),
                None => Probe::unknown(format!(
                    "the setting reads {data:?}, which PeakTweaks does not recognise"
                )),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::game_installs::program_file;
    use crate::hardware::{BootDisk, DiskMedia};

    fn install(id: &str, name: &str, path: &Path) -> GameInstall {
        GameInstall {
            exe: program_file(id, &path.to_string_lossy()),
            game_id: id.into(),
            name: name.into(),
            path: path.to_string_lossy().into_owned(),
            drive: "C:".into(),
            disk: Probe::yes(BootDisk {
                media: DiskMedia::Ssd,
                name: "disk".into(),
            }),
            steam_app: None,
        }
    }

    #[test]
    fn the_preference_is_read_from_among_other_pairs() {
        assert_eq!(
            parse_preference("GpuPreference=2;"),
            Some(GpuPreference::HighPerformance)
        );
        assert_eq!(
            parse_preference("SwapEffectUpgradeEnable=1;GpuPreference=1;"),
            Some(GpuPreference::PowerSaving)
        );
        assert_eq!(
            parse_preference("gpupreference = 0"),
            Some(GpuPreference::LetWindowsDecide)
        );
        assert_eq!(parse_preference("GpuPreference=7;"), None);
        assert_eq!(parse_preference("SwapEffectUpgradeEnable=1;"), None);
        assert_eq!(parse_preference(""), None);
    }

    #[test]
    fn a_value_we_cannot_read_is_unknown_and_a_missing_one_is_not_set() {
        assert_eq!(preference_from(Ok(None)), Probe::yes(GpuPreference::NotSet));
        assert_eq!(
            preference_from(Ok(Some(RawValue::sz("GpuPreference=2;")))),
            Probe::yes(GpuPreference::HighPerformance)
        );
        assert!(preference_from(Ok(Some(RawValue::dword(2)))).is_unknown());
        assert!(preference_from(Ok(Some(RawValue::sz("GpuPreference=9;")))).is_unknown());
        assert!(preference_from(Err(crate::error::EngineError::Internal { detail: "x".into() })).is_unknown());
    }

    #[test]
    fn each_game_is_read_under_its_program_file_and_minecraft_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let fortnite = dir.path().join("Fortnite");
        let bin = fortnite.join("FortniteGame").join("Binaries").join("Win64");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("FortniteClient-Win64-Shipping.exe"), b"").unwrap();
        let versions = dir.path().join("Versions");
        let old = versions.join("version-old");
        let new = versions.join("version-new");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("RobloxPlayerBeta.exe"), b"").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(new.join("RobloxPlayerBeta.exe"), b"").unwrap();
        // Studio sits next to the player and is not a game.
        std::fs::write(versions.join("RobloxStudioBeta.exe"), b"").unwrap();

        let installs = vec![
            install("fortnite", "Fortnite", &fortnite),
            install("roblox", "Roblox", &versions),
            install("minecraft", "Minecraft (Java Edition)", dir.path()),
        ];
        let asked = std::cell::RefCell::new(Vec::new());
        let read = |exe: &str| -> Result<Option<RawValue>> {
            asked.borrow_mut().push(exe.to_owned());
            Ok(exe.contains("Fortnite").then(|| RawValue::sz("GpuPreference=2;")))
        };
        let found = probe_gpu_choices(&installs, Some(&read));
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].preference, Probe::yes(GpuPreference::HighPerformance));
        assert!(found[0]
            .exe
            .as_deref()
            .unwrap()
            .ends_with(r"FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe"));
        assert_eq!(found[1].preference, Probe::yes(GpuPreference::NotSet));
        assert!(found[1].exe.as_deref().unwrap().contains("version-new"), "{found:?}");
        assert_eq!(asked.borrow().len(), 2);

        // Without knowing whose hive to read, nothing is guessed.
        let unknown = probe_gpu_choices(&installs, None);
        assert!(unknown.iter().all(|c| c.preference.is_unknown()));
    }

    #[test]
    fn a_missing_program_file_is_unknown_not_not_set() {
        let dir = tempfile::tempdir().unwrap();
        let installs = vec![install("fortnite", "Fortnite", dir.path())];
        let read = |_: &str| -> Result<Option<RawValue>> { panic!("nothing to read") };
        let found = probe_gpu_choices(&installs, Some(&read));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].exe, None);
        assert!(
            matches!(&found[0].preference, Probe::Unknown { reason } if reason.contains("program file was not found")),
            "{found:?}"
        );
    }
}
