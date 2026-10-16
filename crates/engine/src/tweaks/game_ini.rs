//! Phase 6 (plan section 8): a game's own settings, changed in the file the
//! game saves them to, the same lines its settings menu writes.
//!
//! One tool is a few `Key=Value` lines in one INI file under the signed-in
//! user's profile folder. The file is read and replaced only through held
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

use super::ifeo_priority::per_game_block;

/// One line the tool sets.
#[derive(Debug, Clone, Copy)]
pub struct IniEdit {
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
    pub edits: &'static [IniEdit],
    pub safety: SafetyTier,
    pub tradeoff: Option<&'static str>,
    /// Tests only: as if the game's anti-cheat were cleared.
    pub cleared: bool,
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

    fn current(&self, res: &ContextResolver, path: &str) -> Result<Current> {
        let Some(bytes) = res.read_file(path)? else {
            return Ok(Current::NoFile);
        };
        let ini = ini::decode(&bytes)?;
        let mut ours = true;
        for e in self.edits {
            let now = ini::get(&ini.text, e.section, e.key)?;
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
            .map(|e| format!("[{}] {}={}", e.section, e.key, e.value))
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
            ini.text = ini::set(&ini.text, e.section, e.key, e.value)?;
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

    #[test]
    fn a_profile_path_is_made_concrete_for_this_user_and_others_are_left_as_they_are() {
        let h = harness();
        let res = ContextResolver::new(crate::testutil::user(true), true, h.fake.clone());
        assert_eq!(res.profile_path(FILE).unwrap(), ON_DISK);
        assert_eq!(res.profile_path(r"C:\Games\x.ini").unwrap(), r"C:\Games\x.ini");
    }
}
