//! H19: fullscreen optimizations off for one game, a `User` (HKCU) tweak.
//!
//! Windows keeps the Compatibility tab's settings for a program under
//! `HKCU\Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers`,
//! one `REG_SZ` per program named by its full path. "Disable fullscreen
//! optimizations" is the `DISABLEDXMAXIMIZEDWINDOWEDMODE` layer, and the
//! Compatibility tab writes its layers after a `~` (VERIFY, NOTES N79): e.g.
//! `~ DISABLEDXMAXIMIZEDWINDOWEDMODE HIGHDPIAWARE`. This is the same setting as
//! ticking the box in the game's Properties; Windows applies it when it starts
//! the program, and nothing here touches a game process.
//!
//! The value is named by the program's full path, which differs per PC, so the
//! declared value name is `*\<program>` (`registry::value_name_matches`) and
//! the path is the one the engine found (`ContextResolver::game_program`).
//! Undo replays the journal, so it still removes the value written for the old
//! path after the game moved. Games whose folder changes with every update
//! (Roblox) are left out: the setting would not last (`GameFacts::stable_path`).
//!
//! A value already there with other layers keeps them: ours is added after
//! them. A value in a form this does not recognise is left alone.

use crate::context::ContextResolver;
use crate::env::KNOWN_GAMES;
use crate::error::{EngineError, Result};
use crate::games;
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, PredicateOutcome, RegRoot, RegTarget, SafetyTier, SystemEnv,
    Tier, Tweak, TweakMetadata, TweakState,
};

use super::ifeo_priority::per_game_block;

pub const LAYERS: &str = r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";
const LAYER: &str = "DISABLEDXMAXIMIZEDWINDOWEDMODE";

/// What a `Layers` value holds, as far as this tool is concerned.
#[derive(Debug, PartialEq, Eq)]
enum Layers {
    /// `~` and layer names; whether ours is among them.
    Known { has_ours: bool },
    /// Anything else. Not changed.
    Unrecognised,
}

fn parse(data: &str) -> Layers {
    let mut tokens = data.split_whitespace();
    if tokens.next() != Some("~") {
        return Layers::Unrecognised;
    }
    let names: Vec<&str> = tokens.collect();
    let is_layer = |t: &&str| t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if !names.iter().all(is_layer) {
        return Layers::Unrecognised;
    }
    Layers::Known {
        has_ours: names.iter().any(|t| t.eq_ignore_ascii_case(LAYER)),
    }
}

pub struct FullscreenOptimizations {
    id: String,
    game_id: &'static str,
    game_name: &'static str,
    programs: &'static [&'static str],
    /// Tests only: as if the game's anti-cheat were cleared.
    cleared: bool,
}

impl FullscreenOptimizations {
    /// The tool for a game with its own program in a folder that stays put;
    /// `None` otherwise.
    pub fn for_game(game_id: &str) -> Option<Self> {
        let facts = games::facts(game_id)?;
        (facts.stable_path && !facts.programs.is_empty()).then(|| Self {
            id: format!("gaming.fullscreen.{}", facts.id),
            game_id: facts.id,
            game_name: games::name(facts.id),
            programs: facts.programs,
            cleared: false,
        })
    }

    /// One tool per offered game it applies to.
    pub fn offered() -> Vec<Self> {
        KNOWN_GAMES.iter().filter_map(|g| Self::for_game(g.id)).collect()
    }

    #[cfg(test)]
    pub fn cleared_for_tests(mut self) -> Self {
        self.cleared = true;
        self
    }

    /// The game's `Layers` value: its name (the program's full path) and what
    /// it holds now, with whether our layer is in it. A form this does not
    /// recognise is an error, so the state reads as unknown and nothing is
    /// written on top of it.
    fn current(&self, res: &ContextResolver) -> Result<(String, Option<(String, bool)>)> {
        let exe = res.game_program(self.game_id)?;
        let Some(data) = res.read_string(RegRoot::InteractiveUser, LAYERS, &exe)? else {
            return Ok((exe, None));
        };
        match parse(&data) {
            Layers::Known { has_ours } => Ok((exe, Some((data, has_ours)))),
            Layers::Unrecognised => Err(EngineError::registry_msg(
                res.display_path(RegRoot::InteractiveUser, LAYERS),
                Some(&exe),
                format!(
                    "{}'s compatibility settings are in a form PeakTweaks does not recognise, so it leaves them \
                     alone. They can be changed in the Compatibility tab of the game's Properties.",
                    self.game_name
                ),
            )),
        }
    }
}

impl Tweak for FullscreenOptimizations {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: self.id.clone().into(),
            name: format!("{} fullscreen optimizations off", self.game_name).into(),
            summary: format!(
                "Ticks \"Disable fullscreen optimizations\" for {}, the same setting as in the Compatibility tab \
                 of the game's Properties.",
                self.game_name
            )
            .into(),
            target: format!(r"HKCU\{LAYERS}\<{}'s program file>", self.game_name).into(),
            category: "gaming".into(),
            tier: Tier::Pro,
            safety: SafetyTier::Moderate,
            impact: Impact::Moderate,
            tradeoff: Some(
                "In exclusive fullscreen, switching to other windows can take longer and the screen can flash. \
                 Takes effect the next time the game starts."
                    .into(),
            ),
            requires_reboot: false,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::User
    }

    fn touches(&self) -> Vec<RegTarget> {
        let names: Vec<String> = self.programs.iter().map(|p| format!(r"*\{p}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        vec![RegTarget::new(RegRoot::InteractiveUser, LAYERS, &names)]
    }

    fn evaluate_predicate(&self, env: &SystemEnv) -> PredicateOutcome {
        if let Some(reason) = per_game_block(self.game_id, env, self.cleared) {
            return PredicateOutcome::Block(reason);
        }
        let found = env
            .game_installs
            .as_ref()
            .and_then(|all| all.iter().find(|g| g.game_id == self.game_id));
        match found {
            Some(install) if install.exe.is_none() => PredicateOutcome::Block(
                BlockedReason::new(
                    BlockedCode::GameNotInstalled,
                    crate::game_installs::program_file_missing(install),
                )
                .with_trigger(self.game_id),
            ),
            _ => PredicateOutcome::Allow,
        }
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let has_ours = matches!(self.current(res)?, (_, Some((_, true))));
        Ok(match (has_ours, has_journal_entry) {
            (false, _) => TweakState::Default,
            (true, true) => TweakState::Applied,
            (true, false) => TweakState::Foreign,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let (exe, data) = match self.current(tx.resolver())? {
            (_, Some((_, true))) => return Ok(()),
            (exe, Some((current, false))) => (exe, format!("{} {LAYER}", current.trim_end())),
            (exe, None) => (exe, format!("~ {LAYER}")),
        };
        tx.set_string(RegRoot::InteractiveUser, LAYERS, &exe, &data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_values_are_read_only_in_the_form_the_compatibility_tab_writes() {
        assert_eq!(
            parse("~ DISABLEDXMAXIMIZEDWINDOWEDMODE"),
            Layers::Known { has_ours: true }
        );
        assert_eq!(
            parse("~ HIGHDPIAWARE disabledxmaximizedwindowedmode"),
            Layers::Known { has_ours: true }
        );
        assert_eq!(parse("~ RUNASADMIN"), Layers::Known { has_ours: false });
        assert_eq!(parse("~"), Layers::Known { has_ours: false });
        for odd in [
            "",
            "DISABLEDXMAXIMIZEDWINDOWEDMODE",
            "$ ~ RUNASADMIN",
            "~ WIN7RTM; rm",
            "~WIN8RTM",
        ] {
            assert_eq!(parse(odd), Layers::Unrecognised, "{odd:?}");
        }
    }

    #[test]
    fn only_games_in_a_folder_that_stays_put_get_the_tool() {
        let ids: Vec<&str> = crate::games::GAME_FACTS
            .iter()
            .filter_map(|f| FullscreenOptimizations::for_game(f.id))
            .map(|t| t.game_id)
            .collect();
        assert_eq!(ids, ["fortnite", "valorant", "cs2", "apex"]);
        assert!(FullscreenOptimizations::for_game("roblox").is_none());
        assert!(FullscreenOptimizations::for_game("unknown").is_none());
    }
}
