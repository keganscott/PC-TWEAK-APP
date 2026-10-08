//! Process priority through IFEO PerfOptions: each known game (H31) and
//! `csrss.exe` (H3), `Service` (HKLM) tweaks.
//!
//! `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution
//! Options\<exe>\PerfOptions\CpuPriorityClass` makes Windows start that
//! executable at the given CPU priority class. Values: 1 Idle, 2 Normal,
//! 3 High, 4 Realtime, 5 Below normal, 6 Above normal. For a game we only ever
//! write 3 (High): Realtime can starve the input and audio threads and hang
//! the machine. `IoPriority` takes 0 Very low to 3 High (VERIFY, NOTES N79).
//!
//! The csrss tool writes what Hone's does (CATALOGUE H3): `CpuPriorityClass=4`
//! and `IoPriority=3`. That is Realtime for a Windows process that already
//! runs at a high priority, so it is an Advanced change and needs a restart
//! (Windows starts csrss at boot). NOTES N79 records the difference from the
//! game rule.
//!
//! This is not the `Debugger` or `MonitorProcess` IFEO mechanism that security
//! tools flag, but that says nothing about any particular game's anti-cheat,
//! so a game with one stays blocked until Kegan clears it
//! (`games::anti_cheat_block`, NOTES N75). Nothing here touches a running
//! process; Windows reads the setting the next time it starts the program.
//!
//! Neither key exists on a stock machine, so applying creates them; the
//! journal records that (`created_keys`) and revert removes them again if they
//! are empty.

use crate::context::ContextResolver;
use crate::env::KNOWN_GAMES;
use crate::error::Result;
use crate::games;
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, PredicateOutcome, RegRoot, RegTarget, SafetyTier, SystemEnv,
    Tier, Tweak, TweakMetadata, TweakState,
};

const IFEO: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options";
const CPU: &str = "CpuPriorityClass";
const IO: &str = "IoPriority";
const HIGH: u32 = 3;
const REALTIME: u32 = 4;
const IO_HIGH: u32 = 3;

fn perf_options(exe: &str) -> String {
    format!(r"{IFEO}\{exe}\PerfOptions")
}

/// Why a per-game tool is not offered for this game on this PC, if it is not:
/// its anti-cheat is not cleared (`games::anti_cheat_block`), or the games
/// were looked for and this one was not found. When the games could not be
/// looked for, that alone blocks nothing.
pub(crate) fn per_game_block(game_id: &str, env: &SystemEnv, cleared: bool) -> Option<BlockedReason> {
    if !cleared {
        if let Some(reason) = games::anti_cheat_block(game_id) {
            return Some(reason);
        }
    }
    let installs = env.game_installs.as_ref()?;
    (!installs.iter().any(|g| g.game_id == game_id)).then(|| {
        BlockedReason::new(
            BlockedCode::GameNotInstalled,
            format!("PeakTweaks did not find {} on this PC.", games::name(game_id)),
        )
        .with_trigger(game_id)
    })
}

/// H31: start one game at High CPU priority.
pub struct IfeoPriority {
    id: String,
    game_id: &'static str,
    game_name: &'static str,
    /// The program file names Windows matches against (`GameFacts::programs`).
    programs: &'static [&'static str],
    /// Tests only: as if the game's anti-cheat were cleared.
    cleared: bool,
}

impl IfeoPriority {
    /// The tool for a game whose program files are known; `None` for an
    /// unknown game or one without its own program (Minecraft).
    pub fn for_game(game_id: &str) -> Option<Self> {
        let facts = games::facts(game_id)?;
        (!facts.programs.is_empty()).then(|| Self {
            id: format!("priority.ifeo.{}", facts.id),
            game_id: facts.id,
            game_name: games::name(facts.id),
            programs: facts.programs,
            cleared: false,
        })
    }

    /// One tool per offered game that has its own program.
    pub fn offered() -> Vec<Self> {
        KNOWN_GAMES.iter().filter_map(|g| Self::for_game(g.id)).collect()
    }

    pub fn fortnite() -> Self {
        Self::for_game("fortnite").expect("Fortnite has a program file")
    }

    #[cfg(test)]
    pub fn cleared_for_tests(mut self) -> Self {
        self.cleared = true;
        self
    }
}

impl Tweak for IfeoPriority {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> TweakMetadata {
        let targets: Vec<String> = self
            .programs
            .iter()
            .map(|exe| format!(r"HKLM\{}\{CPU}", perf_options(exe)))
            .collect();
        TweakMetadata {
            id: self.id.clone().into(),
            name: format!("{} process priority", self.game_name).into(),
            summary: format!(
                "Asks Windows to start {} at High CPU priority, using its own per-program setting.",
                self.game_name
            )
            .into(),
            target: targets.join("; ").into(),
            category: "gaming".into(),
            tier: Tier::Pro,
            safety: SafetyTier::Moderate,
            impact: Impact::Moderate,
            tradeoff: Some(
                "Background programs get less CPU while the game runs. Takes effect the next time the game starts."
                    .into(),
            ),
            requires_reboot: false,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Service
    }

    fn touches(&self) -> Vec<RegTarget> {
        self.programs
            .iter()
            .map(|exe| RegTarget::new(RegRoot::LocalMachine, perf_options(exe), &[CPU]))
            .collect()
    }

    fn evaluate_predicate(&self, env: &SystemEnv) -> PredicateOutcome {
        match per_game_block(self.game_id, env, self.cleared) {
            Some(reason) => PredicateOutcome::Block(reason),
            None => PredicateOutcome::Allow,
        }
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        for exe in self.programs {
            // Any other priority, ours missing on one program, or none at all:
            // not what this tool sets.
            if res.read_dword(RegRoot::LocalMachine, &perf_options(exe), CPU)? != Some(HIGH) {
                return Ok(TweakState::Default);
            }
        }
        Ok(if has_journal_entry {
            TweakState::Applied
        } else {
            TweakState::Foreign
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        for exe in self.programs {
            tx.set_dword(RegRoot::LocalMachine, &perf_options(exe), CPU, HIGH)?;
        }
        Ok(())
    }
}

/// H3: start `csrss.exe` (Client Server Runtime) at Realtime CPU and High I/O
/// priority, as Hone does.
pub struct CsrssPriority;

impl CsrssPriority {
    const ID: &'static str = "scheduling.csrss";

    fn key() -> String {
        perf_options("csrss.exe")
    }
}

impl Tweak for CsrssPriority {
    fn id(&self) -> &str {
        Self::ID
    }

    fn metadata(&self) -> TweakMetadata {
        let key = Self::key();
        TweakMetadata {
            id: Self::ID.into(),
            name: "Client Server Runtime priority".into(),
            // VERIFY the description of csrss.exe (NOTES N79).
            summary: "Asks Windows to start csrss.exe (Client Server Runtime, a core Windows process that takes part \
                      in handling mouse and keyboard input) at Realtime CPU priority, the highest there is, and High \
                      disk priority. Windows already runs it at High CPU priority."
                .into(),
            target: format!(r"HKLM\{key}\{CPU}; HKLM\{key}\{IO}").into(),
            category: "scheduling".into(),
            tier: Tier::Pro,
            safety: SafetyTier::Extreme,
            impact: Impact::Moderate,
            tradeoff: Some("Other programs may get less processor time. Needs a restart.".into()),
            requires_reboot: true,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Service
    }

    fn touches(&self) -> Vec<RegTarget> {
        vec![RegTarget::new(RegRoot::LocalMachine, Self::key(), &[CPU, IO])]
    }

    fn evaluate_predicate(&self, _env: &SystemEnv) -> PredicateOutcome {
        PredicateOutcome::Allow
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let key = Self::key();
        let ours = res.read_dword(RegRoot::LocalMachine, &key, CPU)? == Some(REALTIME)
            && res.read_dword(RegRoot::LocalMachine, &key, IO)? == Some(IO_HIGH);
        Ok(match (ours, has_journal_entry) {
            (false, _) => TweakState::Default,
            (true, true) => TweakState::Applied,
            (true, false) => TweakState::Foreign,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let key = Self::key();
        tx.set_dword(RegRoot::LocalMachine, &key, CPU, REALTIME)?;
        tx.set_dword(RegRoot::LocalMachine, &key, IO, IO_HIGH)
    }
}
