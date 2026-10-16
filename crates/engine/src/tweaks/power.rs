//! Power: the PeakTweaks power plan (CATALOGUE H7) and hibernation off (H10).
//!
//! **Power plan.** Apply copies the active plan to a plan of PeakTweaks' own
//! (fixed GUID `power::PEAKTWEAKS`, named "PeakTweaks"), sets five of its
//! plugged-in settings, and makes it the active plan. Battery settings are
//! not touched, so a laptop on battery behaves as before. Undo switches back
//! to the plan that was active and deletes the copy. Copying the user's own
//! plan, rather than High performance as Hone does, keeps every setting this
//! tool does not name as the user had it.
//!
//! The plugged-in settings, all on the copy only:
//! - minimum processor state 100 (the lowest performance state Windows may
//!   pick is the processor's maximum);
//! - core parking minimum cores 100 (no cores parked; hidden in Control Panel);
//! - USB selective suspend off;
//! - PCI Express link state power management off.
//! - processor performance boost mode 2, Aggressive (the processor asks for
//!   its boost speeds without waiting; VERIFY the value list against
//!   Microsoft's power-settings reference, NOTES N131).
//!
//! A setting whose definition Windows does not have (`Control\Power\
//! PowerSettings\<subgroup>\<setting>`, absent on some editions and virtual
//! machines) is skipped. State is "our plan is the active plan".
//!
//! The subgroup and setting GUIDs and values match Microsoft's power-settings
//! reference and the Windows SDK's `GUID_PROCESSOR_THROTTLE_MINIMUM`,
//! `GUID_PROCESSOR_CORE_PARKING_MIN_CORES` ("disabled if the value of this
//! setting is 100%"), `GUID_PCIEXPRESS_ASPM_POLICY` and the USB selective
//! suspend setting (checked 2026-10-08). The Windows CI evidence step makes
//! the plan on a real runner (NOTES N77).
//!
//! **Hibernation.** `powercfg /hibernate off`, which also deletes
//! `hiberfil.sys` (VERIFY, below); Undo turns it back on.

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::power::PEAKTWEAKS;
use crate::system::{SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RegRoot, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata,
    TweakState,
};

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
    (
        SUB_PROCESSOR,
        "be337238-0d82-4146-a960-4f3749d470c7",
        2,
        "processor performance boost mode aggressive",
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

/// Processor idle states (the C-states a core drops into between bursts of
/// work), turned off on the PeakTweaks power plan, plugged in only. Waking
/// from a deep idle state takes time a game can feel as uneven frame times,
/// so tuners switch idle off on a desktop and take the heat instead; that is
/// the trade this tool's card states plainly. It lives on PeakTweaks' own
/// plan copy, so Undo of the plan removes it too, and battery behaviour is
/// never touched. VERIFY: the setting GUID and that 1 turns idle off
/// (Microsoft's power-settings reference as recalled; NOTES N132).
const IDLE_DISABLE: &str = "5d76a2ca-e8c0-402f-a133-2158492d58ad";

fn idle_setting() -> SysItem {
    SysItem::PowerSetting {
        scheme: PEAKTWEAKS.to_owned(),
        subgroup: SUB_PROCESSOR.to_owned(),
        setting: IDLE_DISABLE.to_owned(),
        ac: true,
    }
}

pub struct ProcessorIdle;

pub const IDLE_ID: &str = "power.idle";

fn plan_first() -> BlockedReason {
    BlockedReason::new(
        BlockedCode::ConflictingTweak,
        "Make the PeakTweaks power plan first (Tools > Power), then this setting can be added to it.",
    )
}

impl Tweak for ProcessorIdle {
    fn id(&self) -> &str {
        IDLE_ID
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(IDLE_ID),
            name: Cow::Borrowed("Processor idle states"),
            summary: Cow::Borrowed(
                "Stops the processor dropping into its idle states while plugged in, on the PeakTweaks power \
                 plan. A core coming back from a deep idle state is not instant; with idle off it stays ready \
                 instead. Undo sets the plan back to letting the processor idle.",
            ),
            target: Cow::Owned(format!(
                "Power plan {PEAKTWEAKS}, plugged in: processor idle disable ({IDLE_DISABLE}) = 1"
            )),
            category: Cow::Borrowed("power"),
            tier: Tier::Pro,
            safety: SafetyTier::Extreme,
            impact: Impact::Moderate,
            tradeoff: Some(Cow::Borrowed(
                "The processor runs hotter and uses more electricity at all times while plugged in, and fans may run more.",
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
        vec![idle_setting()]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        if matches!(res.read_system(&scheme())?, SysState::Absent) {
            return Ok(TweakState::Blocked { reason: plan_first() });
        }
        Ok(match res.read_system(&idle_setting())? {
            SysState::Dword { value: 1 } if has_journal_entry => TweakState::Applied,
            SysState::Dword { value: 1 } => TweakState::Foreign,
            SysState::Dword { .. } => TweakState::Default,
            _ => TweakState::Blocked {
                reason: BlockedReason::new(
                    BlockedCode::HardwareUnsupported,
                    "Windows on this PC has no processor idle setting to change.",
                ),
            },
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        if matches!(tx.resolver().read_system(&scheme())?, SysState::Absent) {
            return Err(EngineError::Blocked { reason: plan_first() });
        }
        tx.set_system(idle_setting(), SysState::Dword { value: 1 })
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
            // VERIFY (NOTES N77): Microsoft's powercfg page says only that
            // `/hibernate off` disables the feature; that it deletes the file
            // is from memory. Its size and role: Microsoft's "How to disable
            // and re-enable hibernation".
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
    vec![Box::new(PowerPlan), Box::new(ProcessorIdle), Box::new(Hibernation)]
}
