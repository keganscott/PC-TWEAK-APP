//! Game traffic priority (CATALOGUE H26, and E3's game half): a Windows QoS
//! policy per game program that tags the game's network traffic with DSCP 46
//! (Expedited Forwarding), the tag for traffic to send first.
//!
//! Each policy is added with Windows' own `New-NetQosPolicy` to this
//! computer's policy store, matched by the program file name on every network
//! type (`-NetworkProfile All`), so a game whose folder moves on update
//! (Roblox) is still matched and no domain-only switch is needed. Undo
//! removes it with `Remove-NetQosPolicy`, or puts back a policy of the same
//! name that was there before. The first version wrote the values Group
//! Policy keeps under `SOFTWARE\Policies\Microsoft\Windows\QoS` and refreshed
//! policy; CI showed Windows did not apply such a policy (NOTES N91).
//!
//! The tag only matters to routers and networks that honour it; many home
//! routers and internet providers ignore or clear it. Nothing in the game is
//! touched. Still a per-game tool, so it covers only games whose anti-cheat
//! is cleared (`games::anti_cheat_block`, NOTES N75), and only games found
//! on this PC.
//!
//! VERIFY (NOTES N91): that a policy matched by a program file name tags that
//! program's packets on Windows 10 and 11.

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::env::KNOWN_GAMES;
use crate::error::{EngineError, Result};
use crate::games;
use crate::system::{SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState,
};

pub const ID: &str = "network.qos.games";
/// Expedited Forwarding (RFC 3246).
pub const DSCP: u8 = 46;

/// A game with its own program file, one policy for each of its programs.
struct Game {
    id: &'static str,
    programs: &'static [&'static str],
}

/// The offered games that run their own program.
fn games_with_programs() -> Vec<Game> {
    KNOWN_GAMES
        .iter()
        .filter_map(|g| games::facts(g.id))
        .filter(|f| !f.programs.is_empty())
        .map(|f| Game {
            id: f.id,
            programs: f.programs,
        })
        .collect()
}

/// The policy for one program: `PeakTweaks <program without .exe>`.
pub fn policy(program: &str) -> SysItem {
    let stem = program.strip_suffix(".exe").unwrap_or(program);
    SysItem::QosPolicy {
        name: format!("PeakTweaks {stem}"),
    }
}

/// What the policy for `program` holds once this tool has added it.
pub fn wanted(program: &str) -> SysState {
    SysState::QosPolicy {
        program: program.to_owned(),
        dscp: DSCP,
    }
}

fn is_ours(now: &SysState, program: &str) -> bool {
    matches!(now, SysState::QosPolicy { program: p, dscp } if *dscp == DSCP && p.eq_ignore_ascii_case(program))
}

pub struct GameQos {
    /// Tests only: as if every game's anti-cheat were cleared.
    cleared: bool,
}

impl GameQos {
    pub const fn new() -> Self {
        Self { cleared: false }
    }

    #[cfg(test)]
    pub const fn cleared_for_tests() -> Self {
        Self { cleared: true }
    }

    /// The games this covers here: found on this PC, anti-cheat cleared. When
    /// there are none, why not.
    fn covered(&self, res: &ContextResolver) -> Result<Vec<Game>> {
        let found: Vec<Game> = games_with_programs()
            .into_iter()
            .filter(|g| res.game_program(g.id).is_ok())
            .collect();
        if found.is_empty() {
            return Err(blocked(BlockedReason::new(
                BlockedCode::GameNotInstalled,
                "PeakTweaks did not find a game here that this is for.",
            )));
        }
        let (ok, held): (Vec<Game>, Vec<Game>) = found
            .into_iter()
            .partition(|g| self.cleared || games::anti_cheat_block(g.id).is_none());
        if !ok.is_empty() {
            return Ok(ok);
        }
        if let [one] = held.as_slice() {
            return Err(blocked(
                games::anti_cheat_block(one.id).expect("held back by its anti-cheat"),
            ));
        }
        let names: Vec<&str> = held.iter().map(|g| games::name(g.id)).collect();
        Err(blocked(BlockedReason::new(
            BlockedCode::AntiCheatEligibility,
            format!(
                "Not available yet: {} have not been tried with their anti-cheat.",
                names.join(" and ")
            ),
        )))
    }
}

impl Default for GameQos {
    fn default() -> Self {
        Self::new()
    }
}

fn blocked(reason: BlockedReason) -> EngineError {
    EngineError::Blocked { reason }
}

impl Tweak for GameQos {
    fn id(&self) -> &str {
        ID
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(ID),
            name: Cow::Borrowed("Game traffic priority tag (QoS)"),
            summary: Cow::Borrowed(
                "Adds a Windows QoS policy for each game PeakTweaks found here that tags the game's network \
                 traffic with DSCP 46, the tag for traffic to send first. Only routers and networks that honour \
                 the tag treat it differently; many home routers and internet providers ignore it.",
            ),
            target: Cow::Owned(format!(
                "New-NetQosPolicy \"PeakTweaks <game>\" -AppPathNameMatchCondition <game>.exe -DSCPAction {DSCP} \
                 -NetworkProfile All"
            )),
            category: Cow::Borrowed("network"),
            tier: Tier::Pro,
            safety: SafetyTier::Moderate,
            impact: Impact::Moderate,
            tradeoff: None,
            requires_reboot: false,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Service
    }

    fn touches(&self) -> Vec<RegTarget> {
        Vec::new()
    }

    fn system_targets(&self) -> Vec<SysItem> {
        games_with_programs()
            .iter()
            .flat_map(|g| g.programs.iter())
            .map(|p| policy(p))
            .collect()
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let games = match self.covered(res) {
            Ok(g) => g,
            Err(EngineError::Blocked { reason }) => return Ok(TweakState::Blocked { reason }),
            Err(e) => return Err(e),
        };
        for p in games.iter().flat_map(|g| g.programs.iter()) {
            if !is_ours(&res.read_system(&policy(p))?, p) {
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
        let games = self.covered(tx.resolver())?;
        for p in games.iter().flat_map(|g| g.programs.iter()) {
            if !is_ours(&tx.resolver().read_system(&policy(p))?, p) {
                tx.set_system(policy(p), wanted(p))?;
            }
        }
        Ok(())
    }
}
