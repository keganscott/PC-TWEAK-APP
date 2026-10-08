//! TCP settings (CATALOGUE H23): three of Windows' global TCP settings, the
//! ones `netsh interface tcp set global` changes.
//!
//! Receive window auto-tuning `normal` and Receive Side Scaling `enabled` are
//! Windows' own defaults; tweak tools and old guides often turn them off,
//! which limits how much a connection can receive at once (auto-tuning) or
//! puts all incoming traffic on one processor core (RSS). ECN (Explicit
//! Congestion Notification) `disabled` keeps routers and firewalls that
//! mishandle ECN marks out of the way. Each value before the change is
//! journalled as text and Undo sets it back with `netsh`.
//!
//! VERIFY (NOTES N90): Windows' default for ECN on Windows 10 and 11, and
//! that the values `netsh` sets are the ones `Get-NetTCPSetting -SettingName
//! Internet` and `Get-NetOffloadGlobalSetting` report (the Windows backend
//! reads them there, because `netsh`'s own output is translated).

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::Result;
use crate::system::{SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{ExecutionContext, Impact, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState};

pub const ID: &str = "network.tcp";

/// Each setting by its `netsh` name, with the value this tool sets.
pub const SETTINGS: [(&str, &str); 3] = [
    ("autotuninglevel", "normal"),
    ("rss", "enabled"),
    ("ecncapability", "disabled"),
];

/// Every value `netsh interface tcp set global` takes for `name`; anything
/// else is refused before `netsh` runs, including on Undo.
pub fn allowed(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "autotuninglevel" => Some(&["disabled", "highlyrestricted", "restricted", "normal", "experimental"]),
        "rss" => Some(&["enabled", "disabled"]),
        "ecncapability" => Some(&["enabled", "disabled", "default"]),
        _ => None,
    }
}

fn item(name: &str) -> SysItem {
    SysItem::TcpGlobal { name: name.into() }
}

fn wanted(value: &str) -> SysState {
    SysState::Text { text: value.into() }
}

/// Is `now` the value this tool sets? Text compared ignoring case.
fn is(now: &SysState, value: &str) -> bool {
    matches!(now, SysState::Text { text } if text.eq_ignore_ascii_case(value))
}

pub struct TcpSettings;

impl Tweak for TcpSettings {
    fn id(&self) -> &str {
        ID
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(ID),
            name: Cow::Borrowed("TCP settings: auto-tuning and RSS on, ECN off"),
            summary: Cow::Borrowed(
                "Sets three of Windows' global TCP settings: receive window auto-tuning to normal and Receive Side \
                 Scaling on, both Windows' own defaults that some tweak tools turn off, and Explicit Congestion \
                 Notification (ECN) off, which some routers and firewalls handle badly.",
            ),
            target: Cow::Borrowed(
                "netsh interface tcp set global autotuninglevel=normal rss=enabled ecncapability=disabled",
            ),
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
        SETTINGS.iter().map(|(name, _)| item(name)).collect()
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        for (name, value) in SETTINGS {
            if !is(&res.read_system(&item(name))?, value) {
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
        for (name, value) in SETTINGS {
            if !is(&tx.resolver().read_system(&item(name))?, value) {
                tx.set_system(item(name), wanted(value))?;
            }
        }
        Ok(())
    }
}
