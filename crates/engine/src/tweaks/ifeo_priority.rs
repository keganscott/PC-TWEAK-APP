//! Reference `Service` (HKLM) tweak: game process priority through IFEO
//! PerfOptions.
//!
//! `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution
//! Options\<exe>\PerfOptions\CpuPriorityClass` makes Windows start that
//! executable at the given CPU priority class. Values: 1 Idle, 2 Normal,
//! 3 High, 4 Realtime, 5 Below normal, 6 Above normal. We only ever write 3
//! (High). Realtime can starve the input and audio threads and hang the machine.
//!
//! This is not the `Debugger` or `MonitorProcess` IFEO mechanism that security
//! tools flag, but that says nothing about any particular game's anti-cheat.
//! Until a title has been tested against its anti-cheat vendor it is absent
//! from `CLEARED_GAMES` and the predicate blocks it.
//!
//! Neither key exists on a stock machine, so applying creates them; the
//! journal records that (`created_keys`) and revert removes them again if they
//! are empty.

use crate::context::ContextResolver;
use crate::env::KNOWN_GAMES;
use crate::error::Result;
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, PredicateOutcome, RegRoot, RegTarget, SafetyTier, SystemEnv,
    Tier, Tweak, TweakMetadata, TweakState,
};

const IFEO: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options";
const VALUE: &str = "CpuPriorityClass";
const HIGH: u32 = 3;

/// Games whose anti-cheat has been tested with this tweak. Empty: none yet.
const CLEARED_GAMES: &[&str] = &[];

pub struct IfeoPriority {
    id: String,
    game_id: &'static str,
    game_name: &'static str,
    exe: &'static str,
    cleared: bool,
}

impl IfeoPriority {
    /// `exe` is the file name Windows matches against, e.g. `Game.exe`.
    pub fn new(game_id: &'static str, exe: &'static str) -> Self {
        let game_name = KNOWN_GAMES.iter().find(|g| g.id == game_id).map_or(game_id, |g| g.name);
        Self {
            id: format!("priority.ifeo.{game_id}"),
            game_id,
            game_name,
            exe,
            cleared: CLEARED_GAMES.contains(&game_id),
        }
    }

    /// Fortnite's shipping executable. VERIFY the file name against a current
    /// install before this is ever cleared.
    pub fn fortnite() -> Self {
        Self::new("fortnite", "FortniteClient-Win64-Shipping.exe")
    }

    #[cfg(test)]
    pub fn cleared_for_tests(mut self) -> Self {
        self.cleared = true;
        self
    }

    fn key(&self) -> String {
        format!(r"{IFEO}\{}\PerfOptions", self.exe)
    }
}

impl Tweak for IfeoPriority {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: self.id.clone().into(),
            name: format!("{} process priority", self.game_name).into(),
            summary: format!(
                "Asks Windows to start {} at High CPU priority, using its own per-program setting.",
                self.game_name
            )
            .into(),
            target: format!(r"HKLM\{}\{VALUE}", self.key()).into(),
            category: "scheduling".into(),
            tier: Tier::Pro,
            safety: SafetyTier::Moderate,
            impact: Impact::Moderate,
            tradeoff: Some(
                "Background programs get less CPU while the game runs. Not enabled for any game until it has \
                 been tested against that game's anti-cheat."
                    .into(),
            ),
            requires_reboot: false,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Service
    }

    fn touches(&self) -> Vec<RegTarget> {
        vec![RegTarget::new(RegRoot::LocalMachine, self.key(), &[VALUE])]
    }

    fn evaluate_predicate(&self, _env: &SystemEnv) -> PredicateOutcome {
        if self.cleared {
            return PredicateOutcome::Allow;
        }
        PredicateOutcome::Block(
            BlockedReason::new(
                BlockedCode::AntiCheatEligibility,
                format!(
                    "Not available for {} yet: it has not been tested against the game's anti-cheat.",
                    self.game_name
                ),
            )
            .with_trigger(self.game_id),
        )
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        Ok(match res.read_dword(RegRoot::LocalMachine, &self.key(), VALUE)? {
            None => TweakState::Default,
            Some(HIGH) if has_journal_entry => TweakState::Applied,
            // Something set a priority for this exe and it was not us.
            Some(_) => TweakState::Foreign,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        tx.set_dword(RegRoot::LocalMachine, &self.key(), VALUE, HIGH)
    }
}
