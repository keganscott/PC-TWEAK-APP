//! Prefer the network cable over Wi-Fi (CATALOGUE E5, ExitLag's
//! Multi-Internet without its servers: failover only).
//!
//! Windows picks the route whose route metric plus interface metric is
//! lowest, and picks each interface metric from the link speed unless one is
//! set, so a fast Wi-Fi link can win over a cable (Microsoft, "An explanation
//! of the Automatic Metric feature for IPv4 routes": Wi-Fi of 500 Mb/s or more
//! gets 30, a cable link of 80 to 200 Mb/s gets 35). This sets every wired adapter's
//! metric low and every Wi-Fi adapter's high, IPv4 and IPv6: with both
//! connected, traffic goes over the cable; unplugged, Wi-Fi carries it as
//! before. Each adapter's previous metric (automatic or a number) is
//! journalled and Undo puts it back. Other adapters (virtual, VPN, Bluetooth)
//! are left as they are, and so is a protocol an adapter does not have. With
//! no IP settings on either (both in Hyper-V virtual switches, whose virtual
//! adapters hold the addresses), the tool is not offered.
//!
//! VERIFY (NOTES N83): the metric survives a restart (Microsoft's
//! `Set-NetIPInterface` page says both that the default writes the active and
//! the persistent store and that the default is the active store only).

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::system::{NetAdapter, SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState,
};

pub const ID: &str = "network.prefercable";
/// Every wired adapter gets this and every Wi-Fi adapter `WIFI_METRIC`, so
/// only their order matters. 5 is the lowest automatic metric Windows 10 and
/// later give (a link of 100 Gb/s or more); every other link gets 10 or more.
pub const WIRED_METRIC: u32 = 5;
/// Above the automatic metric of a cable link of 20 Mb/s or more (45 at
/// most); automatic Wi-Fi metrics run from 25 to 85.
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

fn no_ip_settings() -> BlockedReason {
    BlockedReason::new(
        BlockedCode::HardwareUnsupported,
        "Neither the network cable port nor Wi-Fi has IP settings of its own here.",
    )
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
        // Every item matched or was Absent: none present means no IP here.
        Ok(match (seen, has_journal_entry) {
            (false, _) => TweakState::Blocked {
                reason: no_ip_settings(),
            },
            (true, true) => TweakState::Applied,
            (true, false) => TweakState::Foreign,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let adapters = wanted(tx.resolver())?;
        let mut present = false;
        for (a, metric) in &adapters {
            for item in items(a) {
                if tx.resolver().read_system(&item)? != SysState::Absent {
                    tx.set_system(item, SysState::Dword { value: *metric })?;
                    present = true;
                }
            }
        }
        if !present {
            return Err(EngineError::Blocked {
                reason: no_ip_settings(),
            });
        }
        Ok(())
    }
}
