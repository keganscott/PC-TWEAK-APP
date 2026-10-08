//! Game traffic priority (CATALOGUE H26, and E3's game half): a Windows
//! Policy-based QoS policy per game that marks the game's network traffic
//! with DSCP 46 (Expedited Forwarding), the tag for traffic to send first.
//!
//! The policies are the ones Group Policy's "Policy-based QoS" writes, under
//! `HKLM\SOFTWARE\Policies\Microsoft\Windows\QoS\<name>`, all text values,
//! matched by the game's program file name, so a game whose folder moves on
//! update (Roblox) is still matched. Windows applies such policies only on a
//! domain network unless `Tcpip\QoS` `Do not use NLA` is "1", so that is set
//! too. Windows reads them on a policy refresh, which runs after the change
//! and after Undo.
//!
//! The tag only matters to routers and networks that honour it; many home
//! routers and internet providers ignore or clear it. Nothing in the game is
//! touched. Still a per-game tool, so it covers only games whose anti-cheat
//! is cleared (`games::anti_cheat_block`, NOTES N75), and only games found
//! on this PC.
//!
//! One tool for every game, not one per game: the `Do not use NLA` value is
//! shared, and per-game tools would undo it under each other.
//!
//! VERIFY (NOTES N91): the value names and `Version` "1.0" against a policy
//! made in the Group Policy editor; `Do not use NLA` on Windows 10 and 11;
//! that a program file name alone matches.

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::env::KNOWN_GAMES;
use crate::error::{EngineError, Result};
use crate::games;
use crate::system::SideEffect;
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RegRoot, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata,
    TweakState,
};

pub const ID: &str = "network.qos.games";
pub const POLICIES: &str = r"SOFTWARE\Policies\Microsoft\Windows\QoS";
pub const TCPIP_QOS: &str = r"SYSTEM\CurrentControlSet\Services\Tcpip\QoS";
pub const NO_NLA: &str = "Do not use NLA";
/// Expedited Forwarding (RFC 3246).
pub const DSCP: &str = "46";

/// Every value of one policy, the program name aside.
pub const FIXED: [(&str, &str); 10] = [
    ("Version", "1.0"),
    ("Protocol", "*"),
    ("Local Port", "*"),
    ("Local IP", "*"),
    ("Local IP Prefix Length", "*"),
    ("Remote Port", "*"),
    ("Remote IP", "*"),
    ("Remote IP Prefix Length", "*"),
    ("DSCP Value", DSCP),
    ("Throttle Rate", "-1"),
];
pub const APPLICATION: &str = "Application Name";

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

/// The policy key for one program: `PeakTweaks <program without .exe>`.
pub fn policy_key(program: &str) -> String {
    let stem = program.strip_suffix(".exe").unwrap_or(program);
    format!(r"{POLICIES}\PeakTweaks {stem}")
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

    /// Does this program's policy hold exactly our values?
    fn has_policy(res: &ContextResolver, program: &str) -> Result<bool> {
        let key = policy_key(program);
        let read = |name: &str| res.read_string(RegRoot::LocalMachine, &key, name);
        if !read(APPLICATION)?.is_some_and(|v| v.eq_ignore_ascii_case(program)) {
            return Ok(false);
        }
        for (name, value) in FIXED {
            if read(name)?.as_deref() != Some(value) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn nla_off(res: &ContextResolver) -> Result<bool> {
        Ok(res.read_string(RegRoot::LocalMachine, TCPIP_QOS, NO_NLA)?.as_deref() == Some("1"))
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
                r"HKLM\{POLICIES}\PeakTweaks <game> (DSCP Value = {DSCP}); HKLM\{TCPIP_QOS}\{NO_NLA} = 1"
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
        let mut names: Vec<&str> = FIXED.iter().map(|(n, _)| *n).collect();
        names.push(APPLICATION);
        let mut out: Vec<RegTarget> = games_with_programs()
            .iter()
            .flat_map(|g| g.programs.iter())
            .map(|p| RegTarget::new(RegRoot::LocalMachine, policy_key(p), &names))
            .collect();
        out.push(RegTarget::new(RegRoot::LocalMachine, TCPIP_QOS, &[NO_NLA]));
        out
    }

    fn effect_targets(&self) -> Vec<SideEffect> {
        vec![SideEffect::RefreshPolicy]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let games = match self.covered(res) {
            Ok(g) => g,
            Err(EngineError::Blocked { reason }) => return Ok(TweakState::Blocked { reason }),
            Err(e) => return Err(e),
        };
        for p in games.iter().flat_map(|g| g.programs.iter()) {
            if !Self::has_policy(res, p)? {
                return Ok(TweakState::Default);
            }
        }
        if !Self::nla_off(res)? {
            return Ok(TweakState::Default);
        }
        Ok(if has_journal_entry {
            TweakState::Applied
        } else {
            TweakState::Foreign
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let games = self.covered(tx.resolver())?;
        let mut changed = false;
        for p in games.iter().flat_map(|g| g.programs.iter()) {
            if Self::has_policy(tx.resolver(), p)? {
                continue;
            }
            let key = policy_key(p);
            tx.set_string(RegRoot::LocalMachine, &key, APPLICATION, p)?;
            for (name, value) in FIXED {
                tx.set_string(RegRoot::LocalMachine, &key, name, value)?;
            }
            changed = true;
        }
        if !Self::nla_off(tx.resolver())? {
            tx.set_string(RegRoot::LocalMachine, TCPIP_QOS, NO_NLA, "1")?;
            changed = true;
        }
        if changed {
            tx.after_commit(SideEffect::RefreshPolicy)?;
        }
        Ok(())
    }

    fn revert(&self, tx: &mut Transaction) -> Result<()> {
        tx.restore_journalled()?;
        tx.after_commit(SideEffect::RefreshPolicy)
    }
}
