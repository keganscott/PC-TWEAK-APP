//! MSI mode for the graphics card and network adapters (CATALOGUE H6, Hone's
//! "Optimize Message Signal Interrupts").
//!
//! A PCI device can signal the processor through a shared interrupt line or
//! with messages (MSI). Most current drivers ask for messages already; where a
//! driver does not, Windows uses MSI for the device after a restart once its
//! `Device Parameters\Interrupt Management\MessageSignaledInterruptProperties`
//! key has `MSISupported = 1`. That value is the only thing written, and Undo
//! removes it (and the keys it made), or puts back what was there.
//!
//! One `MsiMode` per device. Its id names the device instance (`msi.<instance
//! id>`), so a change can be undone from the id alone, as with startup apps.
//! Only graphics cards and network adapters on the PCI bus are offered, never
//! the disk controller the PC starts from. Advanced, with a restart.
//!
//! VERIFY (NOTES N85): the key and value as the MSI tools and Microsoft's
//! driver documentation ("Enabling Message-Signaled Interrupts in the
//! Registry") name them, as recalled.

use std::borrow::Cow;

use serde::Serialize;
use ts_rs::TS;

use crate::context::ContextResolver;
use crate::engine::TweakView;
use crate::error::{EngineError, Result};
use crate::system::DeviceClass;
use crate::transaction::Transaction;
use crate::types::{ExecutionContext, Impact, RegRoot, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState};

pub const ID_PREFIX: &str = "msi.";
const ENUM_PCI: &str = r"SYSTEM\CurrentControlSet\Enum\PCI";
const MSI_KEY: &str = r"Device Parameters\Interrupt Management\MessageSignaledInterruptProperties";
pub const VALUE: &str = "MSISupported";

/// Letters, digits and `& _ - .` only: nothing that could step out of the
/// device's own key.
fn plain(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 200
        && part.chars().all(|c| c.is_ascii_alphanumeric() || "&_-.".contains(c))
        && !part.contains("..")
}

/// The hardware id and instance of a PCI device instance id
/// (`PCI\VEN_10DE&DEV_2484&...\4&2b0b1f0c&0&0008`), if it is one.
pub fn pci_instance(instance_id: &str) -> Option<(&str, &str)> {
    let mut parts = instance_id.split('\\');
    let (bus, device, instance) = (parts.next()?, parts.next()?, parts.next()?);
    let ok = parts.next().is_none()
        && bus.eq_ignore_ascii_case("PCI")
        && device.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("VEN_"))
        && plain(device)
        && plain(instance);
    ok.then_some((device, instance))
}

/// A maker's name from the PCI vendor id, for devices named by id alone.
fn vendor(device: &str) -> &'static str {
    match device.get(4..8).map(str::to_ascii_uppercase).as_deref() {
        Some("10DE") => "NVIDIA",
        Some("1002") => "AMD",
        Some("8086") => "Intel",
        Some("10EC") => "Realtek",
        Some("14E4") => "Broadcom",
        Some("168C") | Some("17CB") => "Qualcomm",
        Some("14C3") => "MediaTek",
        _ => "PCI",
    }
}

/// MSI mode for one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsiMode {
    id: String,
    device: String,
    instance: String,
    /// What the user knows it as: Device Manager's name, or the maker and the
    /// device id when only the id is known (an Undo after it was removed).
    pub name: String,
}

impl MsiMode {
    pub fn new(instance_id: &str, name: &str) -> Option<Self> {
        let (device, instance) = pci_instance(instance_id)?;
        let name = if name.trim().is_empty() {
            let dev = device.split('&').nth(1).unwrap_or(device);
            format!("{} device {dev}", vendor(device))
        } else {
            name.trim().to_owned()
        };
        Some(Self {
            id: format!("{ID_PREFIX}{instance_id}"),
            device: device.to_owned(),
            instance: instance.to_owned(),
            name,
        })
    }

    /// The device an id names, if it names one.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::new(id.strip_prefix(ID_PREFIX)?, "")
    }

    fn device_key(&self) -> String {
        format!(r"{ENUM_PCI}\{}\{}", self.device, self.instance)
    }

    fn msi_key(&self) -> String {
        format!(r"{}\{MSI_KEY}", self.device_key())
    }
}

impl Tweak for MsiMode {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> TweakMetadata {
        let name = &self.name;
        TweakMetadata {
            id: Cow::Owned(self.id.clone()),
            name: Cow::Owned(format!("MSI mode: {name}")),
            summary: Cow::Owned(format!(
                "Has {name} signal the processor with messages (MSI) instead of a shared interrupt line. Most \
                 current drivers do this already; this turns it on where the driver left it off."
            )),
            target: Cow::Owned(format!(r"HKLM\{}\{VALUE} = 1", self.msi_key())),
            category: Cow::Borrowed("devices"),
            tier: Tier::Pro,
            safety: SafetyTier::Moderate,
            impact: Impact::Moderate,
            tradeoff: Some(Cow::Owned(format!(
                "If {name} stops working properly after the restart, undo this here and restart again."
            ))),
            requires_reboot: true,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Service
    }

    fn touches(&self) -> Vec<RegTarget> {
        // The instance as `*`, so an Undo after the device was removed is
        // noted rather than recreating its key (`Transaction::revert`).
        vec![RegTarget::new(
            RegRoot::LocalMachine,
            format!(r"{ENUM_PCI}\{}\*\{MSI_KEY}", self.device),
            &[VALUE],
        )]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        Ok(match res.read_dword(RegRoot::LocalMachine, &self.msi_key(), VALUE)? {
            Some(1) if has_journal_entry => TweakState::Applied,
            Some(1) => TweakState::Foreign,
            _ => TweakState::Default,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        if !tx.resolver().key_exists(RegRoot::LocalMachine, &self.device_key())? {
            return Err(EngineError::UnknownTweak {
                tweak_id: self.id.clone(),
            });
        }
        tx.set_dword(RegRoot::LocalMachine, &self.msi_key(), VALUE, 1)
    }
}

/// One device as the app shows it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MsiDevice {
    pub tweak: TweakView,
    pub class: DeviceClass,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MsiDeviceList {
    pub devices: Vec<MsiDevice>,
    /// Why the devices could not be listed, in plain words.
    pub problem: Option<String>,
}
