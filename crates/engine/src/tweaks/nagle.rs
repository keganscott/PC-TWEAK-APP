//! Nagle's algorithm and delayed acknowledgements off, per network adapter
//! (Hone's "Low Latency Mode", CATALOGUE H22).
//!
//! Two values under each connected physical adapter's
//! `HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\{guid}`:
//! - `TcpAckFrequency = 1`: acknowledge every TCP segment at once instead of
//!   every second one or after 200 ms. Checked against Microsoft's KB 328890,
//!   "New registry entry for controlling the TCP Acknowledgment (ACK) behavior
//!   in Windows" (per-interface key, REG_DWORD, default 2).
//! - `TCPNoDelay = 1`: send small packets without waiting to batch them.
//!   VERIFY: Microsoft documents this value for older Windows only; current
//!   Windows may ignore it. Kept because Hone writes it (DECISIONS 15.22).
//!
//! Only TCP is affected. Most online shooters use UDP; Minecraft Java Edition
//! and some MMOs use TCP. The adapter key differs per PC, so the allowlist
//! names it with one `*` segment.

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::system::NetAdapter;
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RegRoot, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata,
    TweakState,
};

pub const ID: &str = "network.nagle";
const INTERFACES: &str = r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces";
const VALUES: [&str; 2] = ["TcpAckFrequency", "TCPNoDelay"];

pub struct Nagle;

fn key(a: &NetAdapter) -> String {
    format!(r"{INTERFACES}\{{{}}}", a.guid)
}

/// Connected physical adapters: the ones a game's traffic can use now.
fn connected(res: &ContextResolver) -> Result<Vec<NetAdapter>> {
    Ok(res.network_adapters()?.into_iter().filter(|a| a.up).collect())
}

impl Tweak for Nagle {
    fn id(&self) -> &str {
        ID
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(ID),
            name: Cow::Borrowed("Nagle's algorithm (TCP)"),
            summary: Cow::Borrowed(
                "Makes Windows send small TCP packets straight away and acknowledge each one it receives, on every \
                 connected network adapter, instead of grouping them. TCP only: most online shooters use UDP and \
                 are not affected. Adapters connected later are covered when you apply it again.",
            ),
            target: Cow::Owned(format!(
                r"HKLM\{INTERFACES}\{{adapter}}\TcpAckFrequency = 1; TCPNoDelay = 1"
            )),
            category: Cow::Borrowed("network"),
            tier: Tier::Pro,
            safety: SafetyTier::Moderate,
            impact: Impact::Moderate,
            tradeoff: None,
            requires_reboot: true,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Service
    }

    fn touches(&self) -> Vec<RegTarget> {
        vec![RegTarget::new(
            RegRoot::LocalMachine,
            format!(r"{INTERFACES}\*"),
            &VALUES,
        )]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let adapters = connected(res)?;
        if adapters.is_empty() {
            return Ok(TweakState::Blocked {
                reason: BlockedReason::new(BlockedCode::HardwareUnsupported, "No network adapter is connected."),
            });
        }
        for a in &adapters {
            for v in VALUES {
                if res.read_dword(RegRoot::LocalMachine, &key(a), v)? != Some(1) {
                    return Ok(TweakState::Default);
                }
            }
        }
        Ok(if has_journal_entry {
            TweakState::Applied
        } else {
            TweakState::Foreign
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let adapters = connected(tx.resolver())?;
        if adapters.is_empty() {
            return Err(EngineError::Blocked {
                reason: BlockedReason::new(BlockedCode::HardwareUnsupported, "No network adapter is connected."),
            });
        }
        for a in adapters {
            for v in VALUES {
                tx.set_dword(RegRoot::LocalMachine, &key(&a), v, 1)?;
            }
        }
        Ok(())
    }
}
