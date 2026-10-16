//! Phase 6 (plan section 8): a game's own settings, changed in the file the
//! game saves them to, the same lines its settings menu writes.
//!
//! One tool is a few lines in one settings file under the signed-in user's
//! profile folder: `Key=Value` lines of an INI file (Unreal Engine games), or
//! `"key" "value"` lines of a Valve KeyValues file (`vdf.rs`; Apex Legends). The file is read and replaced only through held
//! folders (`settings_file.rs`, DECISIONS 15.25), every other byte is kept
//! (`ini.rs`), and the whole file is copied and journalled before the change,
//! so Undo puts back exactly what was there. A game that has not saved its
//! settings yet has no file, and the tool says so instead of making one.
//!
//! Nothing here touches the game while it runs. A game that is running may
//! write its settings back when it closes, over ours; the tool then reads as
//! changed outside PeakTweaks, and the summary says to close the game first.

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::games;
use crate::ini;
use crate::system::SysItem;
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, PredicateOutcome, RegTarget, SafetyTier, SystemEnv, Tier,
    Tweak, TweakMetadata, TweakState,
};
use crate::vdf;

use super::ifeo_priority::per_game_block;

/// How the settings file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// `[Section]` and `Key=Value` lines (`ini.rs`).
    Ini,
    /// One outer `"Name" { "key" "value" ... }` block (`vdf.rs`); edits'
    /// `section` is unused and left empty.
    KeyValues,
}

/// One line the tool sets.
#[derive(Debug, Clone, Copy)]
pub struct IniEdit {
    /// The INI section; empty for a KeyValues file.
    pub section: &'static str,
    pub key: &'static str,
    pub value: &'static str,
}

pub struct GameIniSetting {
    pub id: &'static str,
    pub game_id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    /// Declared as `<profile>\...` (`system::PROFILE_PREFIX`).
    pub file: &'static str,
    pub format: FileFormat,
    pub edits: &'static [IniEdit],
    pub safety: SafetyTier,
    pub tradeoff: Option<&'static str>,
    /// Tests only: as if the game's anti-cheat were cleared.
    pub cleared: bool,
}

/// Fortnite's settings file. Path from Epic's help article "How do I fix my
/// settings not saving in Fortnite?" (`%localappdata%\FortniteGame\Saved\
/// Config\WindowsClient\GameUserSettings.ini`, the same article tells
/// players to untick Read-only on it). The section and key names are from
/// copies of the file people have posted, not from Epic (VERIFY against a
/// real file, NOTES N55).
const FORTNITE_FILE: &str = r"<profile>\AppData\Local\FortniteGame\Saved\Config\WindowsClient\GameUserSettings.ini";
const FORTNITE_SECTION: &str = "/Script/FortniteGame.FortGameUserSettings";

/// Apex Legends' video settings. Path and key from PCGamingWiki and players'
/// posted copies, not from EA or Respawn (VERIFY against a real file, NOTES
/// N55).
const APEX_FILE: &str = r"<profile>\Saved Games\Respawn\Apex\local\videoconfig.txt";

/// The game settings tools, for each offered game (`env::KNOWN_GAMES`). Only
/// settings the game's own menu offers, set to the value the menu writes, and
/// only where the publisher's own advice backs the choice (V-Sync off: Epic's
/// low-frame-rate guidance, plan section 8). Every one is behind the per-game
/// anti-cheat gate (`per_game_block`, NOTES N75).
pub fn offered() -> Vec<GameIniSetting> {
    all()
        .into_iter()
        .filter(|t| crate::env::is_offered(t.game_id))
        .collect()
}

pub(crate) fn all() -> Vec<GameIniSetting> {
    vec![
        GameIniSetting {
            id: "game.fortnite.vsync",
            game_id: "fortnite",
            name: "Fortnite: V-Sync off",
            summary: "Sets Fortnite's own V-Sync option to off in its settings file, the same line its Video menu writes. Close Fortnite first: it writes its settings back when it closes. Undo puts the whole file back as it was.",
            file: FORTNITE_FILE,
            format: FileFormat::Ini,
            edits: &[IniEdit {
                section: FORTNITE_SECTION,
                key: "bUseVSync",
                value: "False",
            }],
            safety: SafetyTier::Safe,
            tradeoff: Some("Without V-Sync the picture can tear when the frame rate and the screen's refresh rate differ."),
            cleared: false,
        },
        GameIniSetting {
            id: "game.apex.vsync",
            game_id: "apex",
            name: "Apex Legends: V-Sync off",
            summary: "Sets Apex Legends' own V-Sync option to off (0) in its video settings file. Close the game first. Undo puts the whole file back as it was.",
            file: APEX_FILE,
            format: FileFormat::KeyValues,
            edits: &[IniEdit {
                section: "",
                key: "setting.mat_vsync_mode",
                value: "0",
            }],
            safety: SafetyTier::Safe,
            tradeoff: Some("Without V-Sync the picture can tear when the frame rate and the screen's refresh rate differ."),
            cleared: false,
        },
    ]
}

/// What the file says, as far as this tool is concerned.
enum Current {
    /// The game has not saved its settings on this PC.
    NoFile,
    /// The file, decoded, and whether every line already has our value.
    File { ini: ini::IniText, ours: bool },
}

impl GameIniSetting {
    fn path(&self, res: &ContextResolver) -> Result<String> {
        res.profile_path(self.file)
    }

    fn not_saved_yet(&self) -> BlockedReason {
        BlockedReason::new(
            BlockedCode::GameNotInstalled,
            format!(
                "{} has not saved its settings on this PC yet. Start it once, then check again.",
                games::name(self.game_id)
            ),
        )
        .with_trigger(self.game_id)
    }

    fn get(&self, text: &str, e: &IniEdit) -> Result<Option<String>> {
        match self.format {
            FileFormat::Ini => ini::get(text, e.section, e.key),
            FileFormat::KeyValues => vdf::get(text, e.key),
        }
    }

    fn set(&self, text: &str, e: &IniEdit) -> Result<String> {
        match self.format {
            FileFormat::Ini => ini::set(text, e.section, e.key, e.value),
            FileFormat::KeyValues => vdf::set(text, e.key, e.value),
        }
    }

    fn current(&self, res: &ContextResolver, path: &str) -> Result<Current> {
        let Some(bytes) = res.read_file(path)? else {
            return Ok(Current::NoFile);
        };
        // Either format is text; `ini.rs` keeps its encoding and BOM.
        let ini = ini::decode(&bytes)?;
        let mut ours = true;
        for e in self.edits {
            let now = self.get(&ini.text, e)?;
            // Unreal reads True and true alike.
            ours &= now.is_some_and(|v| v.trim().eq_ignore_ascii_case(e.value));
        }
        Ok(Current::File { ini, ours })
    }
}

impl Tweak for GameIniSetting {
    fn id(&self) -> &str {
        self.id
    }

    fn metadata(&self) -> TweakMetadata {
        let lines: Vec<String> = self
            .edits
            .iter()
            .map(|e| match self.format {
                FileFormat::Ini => format!("[{}] {}={}", e.section, e.key, e.value),
                FileFormat::KeyValues => format!("\"{}\" \"{}\"", e.key, e.value),
            })
            .collect();
        TweakMetadata {
            id: self.id.into(),
            name: self.name.into(),
            summary: self.summary.into(),
            target: format!(
                "{} ({})",
                self.file.replace(crate::system::PROFILE_PREFIX, r"%USERPROFILE%\"),
                lines.join("; ")
            )
            .into(),
            category: "gaming".into(),
            tier: Tier::Pro,
            safety: self.safety,
            impact: Impact::Moderate,
            tradeoff: self.tradeoff.map(Into::into),
            requires_reboot: false,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::User
    }

    fn touches(&self) -> Vec<RegTarget> {
        Vec::new()
    }

    fn system_targets(&self) -> Vec<SysItem> {
        vec![SysItem::File {
            path: self.file.to_owned(),
        }]
    }

    fn evaluate_predicate(&self, env: &SystemEnv) -> PredicateOutcome {
        match per_game_block(self.game_id, env, self.cleared) {
            Some(reason) => PredicateOutcome::Block(reason),
            None => PredicateOutcome::Allow,
        }
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let path = self.path(res)?;
        Ok(match self.current(res, &path)? {
            Current::NoFile => TweakState::Blocked {
                reason: self.not_saved_yet(),
            },
            Current::File { ours: false, .. } => TweakState::Default,
            Current::File { ours: true, .. } if has_journal_entry => TweakState::Applied,
            Current::File { ours: true, .. } => TweakState::Foreign,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let path = self.path(tx.resolver())?;
        let mut ini = match self.current(tx.resolver(), &path)? {
            Current::NoFile => {
                return Err(EngineError::Blocked {
                    reason: self.not_saved_yet(),
                })
            }
            Current::File { ours: true, .. } => return Ok(()),
            Current::File { ini, ours: false } => ini,
        };
        for e in self.edits {
            ini.text = self.set(&ini.text, e)?;
        }
        tx.write_file(&path, &ini::encode(&ini))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{set_profile_dir, Harness};

    const FILE: &str = r"<profile>\AppData\Local\Game\Saved\Config\WindowsClient\GameUserSettings.ini";
    const ON_DISK: &str = r"C:\Users\Kegan\AppData\Local\Game\Saved\Config\WindowsClient\GameUserSettings.ini";
    const SECTION: &str = "/Script/Game.GameUserSettings";

    fn tool() -> GameIniSetting {
        GameIniSetting {
            id: "game.ini.test",
            game_id: "fortnite",
            name: "Test game V-Sync off",
            summary: "SAMPLE",
            file: FILE,
            format: FileFormat::Ini,
            edits: &[IniEdit {
                section: SECTION,
                key: "bUseVSync",
                value: "False",
            }],
            safety: SafetyTier::Safe,
            tradeoff: None,
            cleared: true,
        }
    }

    fn harness() -> Harness {
        let h = Harness::new(vec![Box::new(tool())]);
        set_profile_dir(&h.fake, r"C:\Users\Kegan");
        h
    }

    fn state(h: &Harness) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|t| t.metadata.id == "game.ini.test")
            .unwrap()
            .state
    }

    #[test]
    fn one_line_changes_every_other_byte_stays_and_undo_puts_the_file_back() {
        let mut h = harness();
        let original =
            "\u{feff}[/Script/Game.GameUserSettings]\r\nbUseVSync=True\r\n; mine\r\nFrameRateLimit=144.000000\r\n";
        h.sys.set_file(ON_DISK, original.as_bytes());
        assert_eq!(state(&h), TweakState::Default);

        h.engine.apply("game.ini.test").unwrap();
        let after = String::from_utf8(h.sys.file(ON_DISK).unwrap()).unwrap();
        assert_eq!(after, original.replace("bUseVSync=True", "bUseVSync=False"));
        assert_eq!(state(&h), TweakState::Applied);

        h.engine.revert("game.ini.test").unwrap();
        assert_eq!(h.sys.file(ON_DISK).unwrap(), original.as_bytes());
        assert_eq!(state(&h), TweakState::Default);
    }

    #[test]
    fn a_game_that_has_not_saved_its_settings_gets_no_file() {
        let mut h = harness();
        assert!(matches!(
            state(&h),
            TweakState::Blocked { reason } if reason.code == BlockedCode::GameNotInstalled
        ));
        assert!(h.engine.apply("game.ini.test").is_err());
        assert_eq!(h.sys.file(ON_DISK), None);
    }

    #[test]
    fn a_value_set_by_the_player_reads_as_already_set_and_case_does_not_matter() {
        let h = harness();
        h.sys
            .set_file(ON_DISK, b"[/Script/Game.GameUserSettings]\nbUseVSync=false\n");
        assert_eq!(state(&h), TweakState::Foreign);
    }

    #[test]
    fn a_key_unreal_repeats_is_not_guessed_at() {
        let mut h = harness();
        let text = b"[/Script/Game.GameUserSettings]\nbUseVSync=True\nbUseVSync=False\n";
        h.sys.set_file(ON_DISK, text);
        assert!(matches!(state(&h), TweakState::Unknown { .. }));
        assert!(h.engine.apply("game.ini.test").is_err());
        assert_eq!(h.sys.file(ON_DISK).unwrap(), text);
    }

    #[test]
    fn without_a_profile_folder_nothing_is_read_or_written() {
        let mut h = Harness::new(vec![Box::new(tool())]);
        h.sys
            .set_file(ON_DISK, b"[/Script/Game.GameUserSettings]\nbUseVSync=True\n");
        assert!(matches!(state(&h), TweakState::Unknown { .. }));
        assert!(h.engine.apply("game.ini.test").is_err());
    }

    fn cleared(mut t: GameIniSetting) -> GameIniSetting {
        t.cleared = true;
        t
    }

    fn by_id(id: &str) -> GameIniSetting {
        all().into_iter().find(|t| t.id == id).unwrap()
    }

    fn state_of(h: &Harness, id: &str) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|t| t.metadata.id == id)
            .unwrap()
            .state
    }

    #[test]
    fn apex_vsync_changes_one_value_in_its_keyvalues_file_and_undo_puts_it_back() {
        let mut h = Harness::new(vec![Box::new(cleared(by_id("game.apex.vsync")))]);
        set_profile_dir(&h.fake, r"C:\Users\Kegan");
        let on_disk = r"C:\Users\Kegan\Saved Games\Respawn\Apex\local\videoconfig.txt";
        let original = "\"VideoConfig\"\r\n{\r\n\t\"setting.mat_vsync_mode\"\t\t\"1\"\r\n\t\"setting.defaultres\"\t\t\"1920\"\r\n}\r\n";
        h.sys.set_file(on_disk, original.as_bytes());
        assert_eq!(state_of(&h, "game.apex.vsync"), TweakState::Default);

        h.engine.apply("game.apex.vsync").unwrap();
        let after = String::from_utf8(h.sys.file(on_disk).unwrap()).unwrap();
        assert_eq!(
            after,
            original.replace("\"1\"\r\n\t\"setting.defaultres", "\"0\"\r\n\t\"setting.defaultres")
        );
        assert_eq!(state_of(&h, "game.apex.vsync"), TweakState::Applied);

        h.engine.revert("game.apex.vsync").unwrap();
        assert_eq!(h.sys.file(on_disk).unwrap(), original.as_bytes());
    }

    #[test]
    fn fortnite_vsync_writes_fortnites_own_line() {
        let mut h = Harness::new(vec![Box::new(cleared(by_id("game.fortnite.vsync")))]);
        set_profile_dir(&h.fake, r"C:\Users\Kegan");
        let on_disk = r"C:\Users\Kegan\AppData\Local\FortniteGame\Saved\Config\WindowsClient\GameUserSettings.ini";
        let original = "[/Script/FortniteGame.FortGameUserSettings]\r\nbUseVSync=True\r\nFrameRateLimit=0.000000\r\n";
        h.sys.set_file(on_disk, original.as_bytes());
        h.engine.apply("game.fortnite.vsync").unwrap();
        assert_eq!(
            String::from_utf8(h.sys.file(on_disk).unwrap()).unwrap(),
            original.replace("bUseVSync=True", "bUseVSync=False")
        );
        h.engine.revert("game.fortnite.vsync").unwrap();
        assert_eq!(h.sys.file(on_disk).unwrap(), original.as_bytes());
    }

    #[test]
    fn the_game_settings_tools_stay_behind_the_anti_cheat_gate() {
        for t in all() {
            assert!(!t.cleared, "{}", t.id);
            assert!(t.file.starts_with(crate::system::PROFILE_PREFIX), "{}", t.id);
            if crate::games::anti_cheat_block(t.game_id).is_some() {
                let env = SystemEnv::default();
                assert!(
                    matches!(t.evaluate_predicate(&env), PredicateOutcome::Block(r) if r.code == BlockedCode::AntiCheatEligibility),
                    "{}",
                    t.id
                );
            }
        }
        let ids: Vec<&str> = offered().iter().map(|t| t.id).collect();
        assert!(ids.contains(&"game.fortnite.vsync") && ids.contains(&"game.apex.vsync"));
    }

    #[test]
    fn a_profile_path_is_made_concrete_for_this_user_and_others_are_left_as_they_are() {
        let h = harness();
        let res = ContextResolver::new(crate::testutil::user(true), true, h.fake.clone());
        assert_eq!(res.profile_path(FILE).unwrap(), ON_DISK);
        assert_eq!(res.profile_path(r"C:\Games\x.ini").unwrap(), r"C:\Games\x.ini");
    }
}
