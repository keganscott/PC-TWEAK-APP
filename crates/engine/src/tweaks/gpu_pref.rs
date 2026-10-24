//! Plan 6.2 item 4, the one-click half: run one game on the high-performance
//! graphics chip, a `User` (HKCU) tweak. The scanner's "Graphics chip for your
//! games" finding names it when one game is not set so (`scanner::gpu_choice`).
//!
//! The same setting as Settings > System > Display > Graphics > the game's
//! options > High performance: one string value per program under
//! `HKCU\Software\Microsoft\DirectX\UserGpuPreferences`, named by the program's
//! full path, holding `GpuPreference=2;` among other `Name=Value;` entries
//! Windows may keep there, which are left as they are (`gpu_choice.rs`;
//! VERIFY on a real PC, NOTES N56, N96). Windows applies it when the game
//! starts; nothing here touches a game process.
//!
//! As for the fullscreen tool (`fullscreen.rs`), the value is named by the
//! program's path on this PC, declared as `*\<program>`, and games whose
//! folder changes with every update (Roblox) are left out.

use crate::context::ContextResolver;
use crate::env::KNOWN_GAMES;
use crate::error::{EngineError, Result};
use crate::games;
use crate::gpu_choice::{parse_preference, GpuPreference, KEY};
use crate::probe::Probe;
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, PredicateOutcome, RegRoot, RegTarget, SafetyTier, SystemEnv,
    Tier, Tweak, TweakMetadata, TweakState,
};

use super::ifeo_priority::per_game_block;
use super::registry_values::list_set;

pub const ID_PREFIX: &str = "gpu.choice.";

/// A program's value: its data, and the choice in it if there is one.
type Saved = Option<(String, Option<GpuPreference>)>;

pub struct HighPerformanceGpu {
    id: String,
    game_id: &'static str,
    game_name: &'static str,
    programs: &'static [&'static str],
    /// Tests only: as if the game's anti-cheat were cleared.
    cleared: bool,
}

impl HighPerformanceGpu {
    /// The tool for a game with its own program in a folder that stays put;
    /// `None` otherwise.
    pub fn for_game(game_id: &str) -> Option<Self> {
        let facts = games::facts(game_id)?;
        (facts.stable_path && !facts.programs.is_empty()).then(|| Self {
            id: format!("{ID_PREFIX}{}", facts.id),
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

    /// The program's path and what its value holds now, if anything. A
    /// choice in a form this does not recognise is an error, so the state
    /// reads as unknown and nothing is written on top of it.
    fn current(&self, res: &ContextResolver) -> Result<(String, Saved)> {
        let exe = res.game_program(self.game_id)?;
        let Some(data) = res.read_string(RegRoot::InteractiveUser, KEY, &exe)? else {
            return Ok((exe, None));
        };
        let has_pair = super::registry_values::list_get(&data, "GpuPreference").is_some();
        match (has_pair, parse_preference(&data)) {
            (true, None) => Err(EngineError::registry_msg(
                res.display_path(RegRoot::InteractiveUser, KEY),
                Some(&exe),
                format!(
                    "{}'s graphics choice reads {data:?}, which PeakTweaks does not recognise, so it leaves it \
                     alone. It can be changed in Settings > System > Display > Graphics.",
                    self.game_name
                ),
            )),
            (_, p) => Ok((exe, Some((data, p)))),
        }
    }
}

/// Two graphics chips (or more), as Windows lists them. Unknown counts as
/// possible: the state read then says what Windows has.
fn one_chip(env: &SystemEnv) -> bool {
    let Some(hw) = &env.hardware else {
        return false;
    };
    matches!(&hw.gpus, Probe::Yes { value } if value.len() < 2)
}

impl Tweak for HighPerformanceGpu {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: self.id.clone().into(),
            name: format!("{} on the high-performance graphics chip", self.game_name).into(),
            summary: format!(
                "Sets {} to High performance in Settings > System > Display > Graphics, so Windows runs it on \
                 the more capable of this PC's graphics chips.",
                self.game_name
            )
            .into(),
            target: format!(r"HKCU\{KEY}\<{}'s program file>: GpuPreference=2;", self.game_name).into(),
            category: "gaming".into(),
            tier: Tier::Pro,
            safety: SafetyTier::Safe,
            impact: Impact::Moderate,
            tradeoff: Some(
                "On a laptop running on battery, the high-performance chip uses more power. Takes effect the \
                 next time the game starts."
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
        vec![RegTarget::new(RegRoot::InteractiveUser, KEY, &names)]
    }

    fn evaluate_predicate(&self, env: &SystemEnv) -> PredicateOutcome {
        if let Some(reason) = per_game_block(self.game_id, env, self.cleared) {
            return PredicateOutcome::Block(reason);
        }
        if one_chip(env) {
            return PredicateOutcome::Block(BlockedReason::new(
                BlockedCode::HardwareUnsupported,
                "This PC has one graphics chip, so there is no choice to make.",
            ));
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
        let high = matches!(self.current(res)?, (_, Some((_, Some(GpuPreference::HighPerformance)))));
        Ok(match (high, has_journal_entry) {
            (false, _) => TweakState::Default,
            (true, true) => TweakState::Applied,
            (true, false) => TweakState::Foreign,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let (exe, data) = match self.current(tx.resolver())? {
            (_, Some((_, Some(GpuPreference::HighPerformance)))) => return Ok(()),
            (exe, Some((current, _))) => (exe, list_set(&current, "GpuPreference", "2")),
            (exe, None) => (exe, "GpuPreference=2;".to_owned()),
        };
        tx.set_string(RegRoot::InteractiveUser, KEY, &exe, &data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Through the engine: `per_game_tests.rs`.
    #[test]
    fn games_whose_folder_moves_with_each_update_get_no_tool() {
        assert!(HighPerformanceGpu::for_game("roblox").is_none());
        assert!(HighPerformanceGpu::for_game("fortnite").is_some());
    }
}
