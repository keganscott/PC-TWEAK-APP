//! DNS servers: Cloudflare's public resolvers on every connected adapter
//! (Hone's DNS option, CATALOGUE H25).
//!
//! Windows' own setting (Settings > Network > adapter > DNS server
//! assignment, manual). What each adapter had before is journalled, including
//! "automatic" (an empty list), and Undo puts it back exactly. IPv4 only;
//! IPv6 DNS is left as it is.
//!
//! The addresses are Cloudflare's published resolvers (1.1.1.1 and 1.0.0.1,
//! from cloudflare.com/learning/dns/what-is-1.1.1.1). Changing the servers
//! sends nothing anywhere by itself; Windows then asks these servers for names
//! instead of the ones it used before.

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::system::{NetAdapter, SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState,
};

pub const ID: &str = "network.dns.cloudflare";
pub const SERVERS: [&str; 2] = ["1.1.1.1", "1.0.0.1"];

pub struct CloudflareDns;

fn item(a: &NetAdapter) -> SysItem {
    SysItem::DnsServers {
        interface: a.guid.clone(),
    }
}

fn wanted() -> SysState {
    SysState::List {
        items: SERVERS.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn connected(res: &ContextResolver) -> Result<Vec<NetAdapter>> {
    Ok(res.network_adapters()?.into_iter().filter(|a| a.up).collect())
}

fn none_connected() -> BlockedReason {
    BlockedReason::new(BlockedCode::HardwareUnsupported, "No network adapter is connected.")
}

impl Tweak for CloudflareDns {
    fn id(&self) -> &str {
        ID
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(ID),
            name: Cow::Borrowed("DNS servers: Cloudflare"),
            summary: Cow::Borrowed(
                "Sets the DNS servers of every connected network adapter to Cloudflare's 1.1.1.1 and 1.0.0.1, which \
                 Windows then uses to look up website and game server names.",
            ),
            target: Cow::Borrowed("DNS servers (IPv4) of each connected adapter = 1.1.1.1, 1.0.0.1"),
            category: Cow::Borrowed("network"),
            tier: Tier::Pro,
            safety: SafetyTier::Safe,
            impact: Impact::Moderate,
            tradeoff: Some(Cow::Borrowed(
                "Name lookups go to Cloudflare instead of your internet provider or router. Networks that need \
                 their own DNS (some work or school networks) may stop resolving their own sites until you undo \
                 this.",
            )),
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
        vec![SysItem::DnsServers { interface: "*".into() }]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let adapters = connected(res)?;
        if adapters.is_empty() {
            return Ok(TweakState::Blocked {
                reason: none_connected(),
            });
        }
        for a in &adapters {
            if res.read_system(&item(a))? != wanted() {
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
        let adapters = connected(tx.resolver())?;
        if adapters.is_empty() {
            return Err(EngineError::Blocked {
                reason: none_connected(),
            });
        }
        for a in adapters {
            tx.set_system(item(&a), wanted())?;
        }
        Ok(())
    }
}
