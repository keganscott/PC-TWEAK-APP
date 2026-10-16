//! Windows services a player can do without (CATALOGUE H14, H16, H17, H18).
//!
//! Each tool turns one or more services off: start type Disabled, and stopped
//! now. The change goes through `Transaction::set_system`, so the start type
//! and running state from before are journalled first and Undo puts both
//! back. Services that are not installed on this PC are skipped; a tool whose
//! services are all missing is shown as not available.
//!
//! What is never offered here, whatever another app does: Windows Update,
//! Defender and the Windows security stack, and anti-cheat services
//! (`never_do_audit::FORBIDDEN_SERVICES` checks every declared service).
//!
//! The Xbox tool is blocked while the Xbox app, Game Pass (Gaming Services)
//! or Minecraft Bedrock Edition is on the PC, because they sign in through
//! those services (Kegan's brief, 2026-10-06). The Xbox accessory service
//! (`XboxGipSvc`) is left alone: Xbox controllers use it.
//!
//! VERIFY: service names are Windows' own as recalled; the package family
//! names under `AppModel\Repository\Families` are from memory (NOTES N76).

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::hardware::DiskMedia;
use crate::probe::Probe;
use crate::system::{ServiceStart, SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, PredicateOutcome, RegRoot, RegTarget, SafetyTier, SystemEnv,
    Tier, Tweak, TweakMetadata, TweakState,
};

const SERVICES_KEY: &str = r"SYSTEM\CurrentControlSet\Services";

/// When a tool may be offered, beyond its services being installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    None,
    /// Only when Windows is on an SSD.
    SsdBoot,
    /// Never while something that signs in through Xbox services is installed.
    NoXboxSignIn,
}

pub struct ServiceTweak {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub category: &'static str,
    pub services: &'static [&'static str],
    pub safety: SafetyTier,
    pub tradeoff: Option<&'static str>,
    pub guard: Guard,
}

/// Package families (per user) whose apps sign in through Xbox services.
const XBOX_FAMILIES: &[(&str, &str)] = &[
    ("Microsoft.GamingApp_8wekyb3d8bbwe", "the Xbox app"),
    ("Microsoft.XboxApp_8wekyb3d8bbwe", "the Xbox Console Companion app"),
    ("Microsoft.MinecraftUWP_8wekyb3d8bbwe", "Minecraft (Bedrock Edition)"),
];
const FAMILIES: &str =
    r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Families";
/// Installed with the Xbox app; Game Pass games need it.
const GAMING_SERVICES: &str = "GamingServices";

fn disabled() -> SysState {
    SysState::Service {
        start: ServiceStart::Disabled,
        running: false,
    }
}

impl ServiceTweak {
    /// The services of this tool that are installed on this PC.
    fn installed(&self, res: &ContextResolver) -> Result<Vec<&'static str>> {
        let mut out = Vec::new();
        for s in self.services {
            if res.key_exists(RegRoot::LocalMachine, &format!(r"{SERVICES_KEY}\{s}"))? {
                out.push(*s);
            }
        }
        Ok(out)
    }

    /// Why this tool cannot be used on this PC right now, if it cannot.
    fn blocked(&self, res: &ContextResolver) -> Result<Option<BlockedReason>> {
        if self.guard == Guard::NoXboxSignIn {
            if res.key_exists(RegRoot::LocalMachine, &format!(r"{SERVICES_KEY}\{GAMING_SERVICES}"))? {
                return Ok(Some(xbox_block("Game Pass (Gaming Services)")));
            }
            for (family, what) in XBOX_FAMILIES {
                if res.key_exists(RegRoot::InteractiveUser, &format!(r"{FAMILIES}\{family}"))? {
                    return Ok(Some(xbox_block(what)));
                }
            }
        }
        if self.installed(res)?.is_empty() {
            return Ok(Some(BlockedReason::new(
                BlockedCode::OsVersionUnsupported,
                "These services are not part of this edition of Windows.",
            )));
        }
        Ok(None)
    }
}

fn xbox_block(what: &str) -> BlockedReason {
    BlockedReason::new(
        BlockedCode::NeededByInstalledApp,
        format!("Not offered while {what} is installed: it signs in through these services."),
    )
}

impl Tweak for ServiceTweak {
    fn id(&self) -> &str {
        self.id
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(self.id),
            name: Cow::Borrowed(self.name),
            summary: Cow::Borrowed(self.summary),
            target: Cow::Owned(format!("Services {}: Disabled, stopped", self.services.join(", "))),
            category: Cow::Borrowed(self.category),
            tier: Tier::Pro,
            safety: self.safety,
            impact: Impact::Moderate,
            tradeoff: self.tradeoff.map(Cow::Borrowed),
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
        self.services
            .iter()
            .map(|s| SysItem::Service { name: (*s).to_owned() })
            .collect()
    }

    fn evaluate_predicate(&self, env: &SystemEnv) -> PredicateOutcome {
        if self.guard != Guard::SsdBoot {
            return PredicateOutcome::Allow;
        }
        let disk = env.hardware.as_ref().map(|h| &h.boot_disk);
        match disk {
            Some(Probe::Yes { value }) if value.media == DiskMedia::Hdd => PredicateOutcome::Block(BlockedReason::new(
                BlockedCode::HardwareCounterproductive,
                "Windows is on a hard drive, where SysMain's preloading is worth keeping.",
            )),
            Some(Probe::Yes { .. }) => PredicateOutcome::Allow,
            _ => PredicateOutcome::Block(BlockedReason::new(
                BlockedCode::HardwareCounterproductive,
                "PeakTweaks could not tell whether Windows is on an SSD, so this stays off.",
            )),
        }
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        // Our own change is shown as applied even where it would not be
        // offered now, so Undo stays in reach.
        if !has_journal_entry {
            if let Some(reason) = self.blocked(res)? {
                return Ok(TweakState::Blocked { reason });
            }
        }
        for s in self.installed(res)? {
            match res.read_system(&SysItem::Service { name: s.to_owned() })? {
                SysState::Service {
                    start: ServiceStart::Disabled,
                    ..
                } => {}
                _ => return Ok(TweakState::Default),
            }
        }
        Ok(if has_journal_entry {
            TweakState::Applied
        } else {
            TweakState::Foreign
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        if let Some(reason) = self.blocked(tx.resolver())? {
            return Err(EngineError::Blocked { reason });
        }
        for s in self.installed(tx.resolver())? {
            tx.set_system(SysItem::Service { name: s.to_owned() }, disabled())?;
        }
        Ok(())
    }
}

/// H16. Windows Search keeps an index of files and reads the disk in the
/// background to keep it current.
pub const SEARCH_INDEXING: ServiceTweak = ServiceTweak {
    id: "services.searchindexing",
    name: "Search indexing",
    summary: "Turns off the Windows Search service, which keeps an index of your files and reads the disk in the \
              background to keep it up to date.",
    category: "services",
    services: &["WSearch"],
    safety: SafetyTier::Moderate,
    tradeoff: Some(
        "Searching for files in File Explorer and the Start menu takes longer, and Outlook's search stops working.",
    ),
    guard: Guard::None,
};

/// H17. SysMain (formerly Superfetch) preloads often-used programs into
/// memory. Offered on SSD PCs only, as Hone does.
pub const SYSMAIN: ServiceTweak = ServiceTweak {
    id: "services.sysmain",
    name: "SysMain (Superfetch)",
    summary: "Turns off SysMain, which preloads programs you use often into memory and reads the disk in the \
              background to do it. Offered only when Windows is on an SSD.",
    category: "services",
    services: &["SysMain"],
    safety: SafetyTier::Moderate,
    tradeoff: Some("Programs you use often are no longer kept in memory ahead of time."),
    guard: Guard::SsdBoot,
};

/// H18. Xbox Live sign-in, cloud saves and networking for Xbox games on PC.
pub const XBOX_SERVICES: ServiceTweak = ServiceTweak {
    id: "services.xbox",
    name: "Xbox Live services",
    summary: "Turns off the Xbox Live sign-in, game save and networking services. Not offered while the Xbox app, \
              Game Pass or Minecraft Bedrock Edition is installed. Xbox controllers are not affected.",
    category: "services",
    services: &["XblAuthManager", "XblGameSave", "XboxNetApiSvc"],
    safety: SafetyTier::Moderate,
    tradeoff: Some("PC games that sign in to Xbox Live, sync saves through it or use Xbox party chat stop working."),
    guard: Guard::NoXboxSignIn,
};

/// H14, the service half. Connected User Experiences and Telemetry sends
/// Windows' diagnostic data to Microsoft.
pub const TELEMETRY_SERVICE: ServiceTweak = ServiceTweak {
    id: "services.telemetry",
    name: "Telemetry service",
    summary: "Turns off Connected User Experiences and Telemetry, the service that sends Windows' diagnostic data to \
              Microsoft.",
    category: "privacy",
    services: &["DiagTrack"],
    safety: SafetyTier::Safe,
    tradeoff: None,
    guard: Guard::None,
};

pub fn all() -> Vec<Box<dyn Tweak>> {
    vec![
        Box::new(SEARCH_INDEXING),
        Box::new(SYSMAIN),
        Box::new(XBOX_SERVICES),
        Box::new(TELEMETRY_SERVICE),
    ]
}
