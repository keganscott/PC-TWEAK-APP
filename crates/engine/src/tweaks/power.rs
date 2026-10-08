//! Power: the PeakTweaks power plan (CATALOGUE H7) and hibernation off (H10).
//!
//! **Power plan.** Apply copies the active plan to a plan of PeakTweaks' own
//! (fixed GUID `power::PEAKTWEAKS`, named "PeakTweaks"), sets four of its
//! plugged-in settings, and makes it the active plan. Battery settings are
//! not touched, so a laptop on battery behaves as before. Undo switches back
//! to the plan that was active and deletes the copy. Copying the user's own
//! plan, rather than High performance as Hone does, keeps every setting this
//! tool does not name as the user had it.
//!
//! The plugged-in settings, all on the copy only:
//! - minimum processor state 100 (the processor does not clock down at idle);
//! - core parking minimum cores 100 (no cores parked; hidden in Control Panel);
//! - USB selective suspend off;
//! - PCI Express link state power management off.
//!
//! A setting whose definition Windows does not have (`Control\Power\
//! PowerSettings\<subgroup>\<setting>`, absent on some editions and virtual
//! machines) is skipped. State is "our plan is the active plan".
//!
//! VERIFY: the subgroup and setting GUIDs are Windows' documented `powercfg`
//! aliases as recalled (SUB_PROCESSOR PROCTHROTTLEMIN, CPMINCORES; SUB_USB
//! USBSELECTIVESUSPEND; SUB_PCIEXPRESS ASPM). The Windows CI evidence step
//! makes the plan on a real runner (NOTES N77).
//!
//! **Hibernation.** `powercfg /hibernate off`, which also deletes
//! `hiberfil.sys`; Undo turns it back on.

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::power::PEAKTWEAKS;
use crate::system::{SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{ExecutionContext, Impact, RegRoot, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState};

const SETTINGS_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Power\PowerSettings";

const SUB_PROCESSOR: &str = "54533251-82be-4824-96c1-47b60b740d00";
const SUB_USB: &str = "2a737441-1930-4402-8d77-b2bebba308a3";
const SUB_PCIEXPRESS: &str = "501a4d13-42af-4429-9fd1-a8218c268e20";

/// (subgroup, setting, plugged-in value, what it is).
pub const PLAN_SETTINGS: &[(&str, &str, u32, &str)] = &[
    (
        SUB_PROCESSOR,
        "893dee8e-2bef-41e0-89c6-b55d0929964c",
        100,
        "minimum processor state 100",
    ),
    (
        SUB_PROCESSOR,
        "0cc5b647-c1df-4637-891a-dec35c318583",
        100,
        "core parking minimum cores 100",
    ),
    (
        SUB_USB,
        "48e6b7a6-50f5-4782-a5d4-53bb8f07e226",
        0,
        "USB selective suspend off",
    ),
    (
        SUB_PCIEXPRESS,
        "ee12f906-d277-404b-b6da-e5fa1a576df5",
        0,
        "PCI Express link state power management off",
    ),
];

fn scheme() -> SysItem {
    SysItem::PowerScheme {
        guid: PEAKTWEAKS.to_owned(),
    }
}

fn setting(subgroup: &str, setting: &str) -> SysItem {
    SysItem::PowerSetting {
        scheme: PEAKTWEAKS.to_owned(),
        subgroup: subgroup.to_owned(),
        setting: setting.to_owned(),
        ac: true,
    }
}

fn active(res: &ContextResolver) -> Result<String> {
    match res.read_system(&SysItem::ActivePowerScheme)? {
        SysState::Text { text } => Ok(text.to_ascii_lowercase()),
        other => Err(EngineError::Internal {
            detail: format!("the active power plan read as {other:?}"),
        }),
    }
}

pub struct PowerPlan;

pub const PLAN_ID: &str = "power.plan";

impl Tweak for PowerPlan {
    fn id(&self) -> &str {
        PLAN_ID
    }

    fn metadata(&self) -> TweakMetadata {
        let shown: Vec<&str> = PLAN_SETTINGS.iter().map(|s| s.3).collect();
        TweakMetadata {
            id: Cow::Borrowed(PLAN_ID),
            name: Cow::Borrowed("PeakTweaks power plan"),
            summary: Cow::Borrowed(
                "Makes a copy of your current power plan called PeakTweaks and switches to it. While plugged in, \
                 the copy keeps the processor from clocking down or parking cores, and keeps USB devices and PCI \
                 Express links out of power saving. Battery settings stay as they were.",
            ),
            target: Cow::Owned(format!(
                "Power plan {PEAKTWEAKS} (a copy of the active plan), plugged in: {}; made the active plan",
                shown.join(", ")
            )),
            category: Cow::Borrowed("power"),
            tier: Tier::Pro,
            safety: SafetyTier::Moderate,
            impact: Impact::Moderate,
            tradeoff: Some(Cow::Borrowed("Uses more electricity while plugged in.")),
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
        let mut out = vec![scheme(), SysItem::ActivePowerScheme];
        out.extend(PLAN_SETTINGS.iter().map(|(sub, set, _, _)| setting(sub, set)));
        out
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        Ok(if active(res)? != PEAKTWEAKS {
            TweakState::Default
        } else if has_journal_entry {
            TweakState::Applied
        } else {
            TweakState::Foreign
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let current = active(tx.resolver())?;
        let exists = !matches!(tx.resolver().read_system(&scheme())?, SysState::Absent);
        if !exists {
            // The active plan cannot be ours when ours does not exist.
            tx.set_system(scheme(), SysState::Scheme { source: current })?;
        }
        for (sub, set, value, _) in PLAN_SETTINGS {
            let defined = tx
                .resolver()
                .key_exists(RegRoot::LocalMachine, &format!(r"{SETTINGS_KEY}\{sub}\{set}"))?;
            if defined {
                tx.set_system(setting(sub, set), SysState::Dword { value: *value })?;
            }
        }
        tx.set_system(
            SysItem::ActivePowerScheme,
            SysState::Text {
                text: PEAKTWEAKS.to_owned(),
            },
        )
    }
}

pub struct Hibernation;

pub const HIBERNATION_ID: &str = "power.hibernation";

impl Tweak for Hibernation {
    fn id(&self) -> &str {
        HIBERNATION_ID
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(HIBERNATION_ID),
            name: Cow::Borrowed("Hibernation"),
            summary: Cow::Borrowed(
                "Turns hibernation off, which also deletes the hibernation file (hiberfil.sys) Windows keeps on \
                 the system drive.",
            ),
            target: Cow::Borrowed("powercfg /hibernate off"),
            category: Cow::Borrowed("power"),
            tier: Tier::Pro,
            safety: SafetyTier::Moderate,
            impact: Impact::Moderate,
            tradeoff: Some(Cow::Borrowed(
                "Fast startup stops working, and a laptop can no longer hibernate when its battery runs low.",
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
        vec![SysItem::Hibernation]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        Ok(match res.read_system(&SysItem::Hibernation)? {
            SysState::Bool { on: false } if has_journal_entry => TweakState::Applied,
            SysState::Bool { on: false } => TweakState::Foreign,
            _ => TweakState::Default,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        tx.set_system(SysItem::Hibernation, SysState::Bool { on: false })
    }
}

pub fn all() -> Vec<Box<dyn Tweak>> {
    vec![Box::new(PowerPlan), Box::new(Hibernation)]
}
