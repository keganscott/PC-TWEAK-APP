//! Prefer the network cable over Wi-Fi (CATALOGUE E5, ExitLag's
//! Multi-Internet without its servers: failover only).
//!
//! Windows sends traffic over the connected adapter with the lowest interface
//! metric, and picks each metric from the link speed unless one is set, so a
//! fast Wi-Fi link can win over a cable. This sets every wired adapter's
//! metric low and every Wi-Fi adapter's high, IPv4 and IPv6: with both
//! connected, traffic goes over the cable; unplugged, Wi-Fi carries it as
//! before. Each adapter's previous metric (automatic or a number) is
//! journalled and Undo puts it back. Other adapters (virtual, VPN, Bluetooth)
//! are left as they are.
//!
//! VERIFY (NOTES N83): the metric survives a restart; the numbers sit below
//! and above Windows' automatic metrics.

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::system::{NetAdapter, SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState,
};

pub const ID: &str = "network.prefercable";
/// Below every automatic metric Windows gives a link.
pub const WIRED_METRIC: u32 = 5;
/// Above the automatic metric of any cable link.
pub const WIFI_METRIC: u32 = 50;

pub struct PreferCable;

/// Each wired and Wi-Fi adapter with the metric it gets.
fn wanted(res: &ContextResolver) -> Result<Vec<(NetAdapter, u32)>> {
    let adapters = res.network_adapters()?;
    let has = |f: fn(&NetAdapter) -> bool| adapters.iter().any(f);
    if !has(|a| a.wired) || !has(|a| a.wireless) {
        return Err(EngineError::Blocked {
            reason: BlockedReason::new(
                BlockedCode::HardwareUnsupported,
                "This PC does not have both a network cable port and Wi-Fi.",
            ),
        });
    }
    Ok(adapters
        .into_iter()
        .filter_map(|a| {
            let metric = if a.wired {
                WIRED_METRIC
            } else if a.wireless {
                WIFI_METRIC
            } else {
                return None;
            };
            Some((a, metric))
        })
        .collect())
}

fn items(a: &NetAdapter) -> [SysItem; 2] {
    [false, true].map(|ipv6| SysItem::InterfaceMetric {
        interface: a.guid.clone(),
        ipv6,
    })
}

impl Tweak for PreferCable {
    fn id(&self) -> &str {
        ID
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(ID),
            name: Cow::Borrowed("Prefer the network cable over Wi-Fi"),
            summary: Cow::Borrowed(
                "When the cable and Wi-Fi are both connected, Windows sends traffic over the cable, and over Wi-Fi \
                 when the cable is unplugged. Windows otherwise ranks them by link speed, so a fast Wi-Fi link can \
                 win.",
            ),
            target: Cow::Borrowed("Interface metric (IPv4 and IPv6): each wired adapter 5, each Wi-Fi adapter 50"),
            category: Cow::Borrowed("network"),
            tier: Tier::Pro,
            safety: SafetyTier::Safe,
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
        [false, true]
            .map(|ipv6| SysItem::InterfaceMetric {
                interface: "*".into(),
                ipv6,
            })
            .to_vec()
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let adapters = match wanted(res) {
            Ok(a) => a,
            Err(EngineError::Blocked { reason }) => return Ok(TweakState::Blocked { reason }),
            Err(e) => return Err(e),
        };
        let mut seen = false;
        for (a, metric) in &adapters {
            for item in items(a) {
                match res.read_system(&item)? {
                    // The protocol is not on this adapter.
                    SysState::Absent => {}
                    SysState::Dword { value } if value == *metric => seen = true,
                    _ => return Ok(TweakState::Default),
                }
            }
        }
        Ok(match (seen, has_journal_entry) {
            (false, _) => TweakState::Default,
            (true, true) => TweakState::Applied,
            (true, false) => TweakState::Foreign,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let adapters = wanted(tx.resolver())?;
        for (a, metric) in &adapters {
            for item in items(a) {
                if tx.resolver().read_system(&item)? != SysState::Absent {
                    tx.set_system(item, SysState::Dword { value: *metric })?;
                }
            }
        }
        Ok(())
    }
}
