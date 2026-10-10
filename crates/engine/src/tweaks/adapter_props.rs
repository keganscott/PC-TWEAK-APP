//! Network adapter settings (CATALOGUE H24): the driver settings on an
//! adapter's Advanced tab in Device Manager, and its power saving, on every
//! physical adapter whose driver has them.
//!
//! Each adapter's settings live in its driver key,
//! `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}\<nnnn>`,
//! the key Windows' own `Driver` value under the adapter's `Enum` key names;
//! the key's `NetCfgInstanceId` is checked to be the adapter's before
//! anything is read or written. The driver key differs per PC, so the
//! allowlist names it with one `*` segment.
//!
//! Advanced-tab settings are standardized keywords (`*InterruptModeration`,
//! `*FlowControl`) or ones drivers share (`*EEE`). A tool changes an adapter
//! only when the driver lists the keyword under `Ndi\params\<keyword>` with
//! the value it would set among the driver's own choices (`enum`), so a
//! value the driver does not offer is never written. A keyword the driver
//! has but the key does not hold yet is at the driver's `default`. The
//! driver reads these when the adapter starts, so each changed adapter is
//! restarted once the change commits, and again after Undo: the connection
//! drops for a few seconds.
//!
//! Power saving is `PnPCapabilities`, Microsoft's documented switch for
//! Device Manager's "Allow the computer to turn off this device to save
//! power" (24, 0x18, clears it; KB 2740020). Its other bits are kept. It
//! takes effect after a restart of the PC.
//!
//! VERIFY (NOTES N89): the keyword meanings (0 off) against Microsoft's
//! standardized keyword pages and an Intel and a Realtek driver; that
//! `PnPCapabilities` still works on Windows 11.

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::system::{NetAdapter, SideEffect};
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RawValue, RegRoot, RegTarget, SafetyTier, Tier, Tweak,
    TweakMetadata, TweakState,
};

const NET_CLASS: &str = "{4d36e972-e325-11ce-bfc1-08002be10318}";
const ENUM: &str = r"SYSTEM\CurrentControlSet\Enum";

fn class_root() -> String {
    format!(r"SYSTEM\CurrentControlSet\Control\Class\{NET_CLASS}")
}

/// What a tool sets.
#[derive(Clone, Copy)]
pub enum Wanted {
    /// An Advanced-tab keyword, as the driver stores it (text).
    Keyword(&'static str),
    /// These bits on in a DWORD, the others kept.
    Bits(u32),
}

pub struct AdapterSetting {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    /// The value's name in the driver key.
    pub value_name: &'static str,
    pub wanted: Wanted,
    /// For "No network adapter here has …".
    pub what: &'static str,
    pub safety: SafetyTier,
    pub tradeoff: &'static str,
}

/// One adapter this tool can change, with its driver key.
struct Target {
    adapter: NetAdapter,
    key: String,
}

/// The adapter's driver key, when Windows names one and it is this adapter's.
pub(crate) fn driver_key(res: &ContextResolver, a: &NetAdapter) -> Result<Option<String>> {
    if a.pnp_id.is_empty() || a.pnp_id.contains("..") || a.pnp_id.starts_with('\\') {
        return Ok(None);
    }
    let Some(driver) = res.read_string(RegRoot::LocalMachine, &format!(r"{ENUM}\{}", a.pnp_id), "Driver")? else {
        return Ok(None);
    };
    let Some((class, index)) = driver.split_once('\\') else {
        return Ok(None);
    };
    if !class.eq_ignore_ascii_case(NET_CLASS) || index.len() != 4 || !index.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    let key = format!(r"{}\{index}", class_root());
    let id = res.read_string(RegRoot::LocalMachine, &key, "NetCfgInstanceId")?;
    let mine = id.is_some_and(|id| id.trim_matches(['{', '}']).eq_ignore_ascii_case(&a.guid));
    Ok(mine.then_some(key))
}

/// A stored keyword value as text, whichever way the driver stored it.
fn as_text(v: &RawValue) -> Option<String> {
    v.as_sz().or_else(|| v.as_dword().map(|d| d.to_string()))
}

impl AdapterSetting {
    /// The adapters whose driver has this setting, or why there are none.
    fn targets(&self, res: &ContextResolver) -> Result<Vec<Target>> {
        let mut out = Vec::new();
        for a in res.network_adapters()? {
            let Some(key) = driver_key(res, &a)? else {
                continue;
            };
            let has = match self.wanted {
                Wanted::Keyword(value) => {
                    let choices = format!(r"{key}\Ndi\params\{}\enum", self.value_name);
                    res.value_names(RegRoot::LocalMachine, &choices)?
                        .iter()
                        .any(|n| n.eq_ignore_ascii_case(value))
                }
                Wanted::Bits(_) => true,
            };
            if has {
                out.push(Target { adapter: a, key });
            }
        }
        if out.is_empty() {
            return Err(EngineError::Blocked {
                reason: BlockedReason::new(
                    BlockedCode::HardwareUnsupported,
                    format!("No network adapter here has {}.", self.what),
                ),
            });
        }
        Ok(out)
    }

    /// Is this adapter set as the tool sets it?
    fn is_set(&self, res: &ContextResolver, t: &Target) -> Result<bool> {
        let stored = res.read_raw(RegRoot::LocalMachine, &t.key, self.value_name)?;
        Ok(match self.wanted {
            Wanted::Keyword(value) => {
                let now = match stored {
                    Some(v) => as_text(&v),
                    // Not stored: the driver's default.
                    None => res.read_string(
                        RegRoot::LocalMachine,
                        &format!(r"{}\Ndi\params\{}", t.key, self.value_name),
                        "default",
                    )?,
                };
                now.is_some_and(|n| n.trim().eq_ignore_ascii_case(value))
            }
            Wanted::Bits(bits) => stored.and_then(|v| v.as_dword()).is_some_and(|d| d & bits == bits),
        })
    }

    fn restarts_adapter(&self) -> bool {
        matches!(self.wanted, Wanted::Keyword(_))
    }

    fn restart(tx: &mut Transaction, adapter_guid: &str) -> Result<()> {
        tx.after_commit(SideEffect::RestartAdapter {
            interface: adapter_guid.to_owned(),
        })
    }
}

impl Tweak for AdapterSetting {
    fn id(&self) -> &str {
        self.id
    }

    fn metadata(&self) -> TweakMetadata {
        let value = match self.wanted {
            Wanted::Keyword(v) => format!("\"{v}\""),
            Wanted::Bits(b) => format!("{b} (other bits kept)"),
        };
        TweakMetadata {
            id: Cow::Borrowed(self.id),
            name: Cow::Borrowed(self.name),
            summary: Cow::Borrowed(self.summary),
            target: Cow::Owned(format!(
                r"HKLM\{}\{{adapter}}\{} = {value}",
                class_root(),
                self.value_name
            )),
            category: Cow::Borrowed("network"),
            tier: Tier::Pro,
            safety: self.safety,
            impact: Impact::Moderate,
            tradeoff: Some(Cow::Borrowed(self.tradeoff)),
            requires_reboot: !self.restarts_adapter(),
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Service
    }

    fn touches(&self) -> Vec<RegTarget> {
        vec![RegTarget::new(
            RegRoot::LocalMachine,
            format!(r"{}\*", class_root()),
            &[self.value_name],
        )]
    }

    fn effect_targets(&self) -> Vec<SideEffect> {
        if self.restarts_adapter() {
            vec![SideEffect::RestartAdapter { interface: "*".into() }]
        } else {
            Vec::new()
        }
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let targets = match self.targets(res) {
            Ok(t) => t,
            Err(EngineError::Blocked { reason }) => return Ok(TweakState::Blocked { reason }),
            Err(e) => return Err(e),
        };
        for t in &targets {
            if !self.is_set(res, t)? {
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
        for t in self.targets(tx.resolver())? {
            if self.is_set(tx.resolver(), &t)? {
                continue;
            }
            match self.wanted {
                Wanted::Keyword(value) => {
                    // Keep the driver's own type for the value.
                    let stored = tx.resolver().read_raw(RegRoot::LocalMachine, &t.key, self.value_name)?;
                    match (stored.as_ref().map(|v| v.vtype), value.parse::<u32>()) {
                        (Some(4), Ok(n)) => tx.set_dword(RegRoot::LocalMachine, &t.key, self.value_name, n)?,
                        _ => tx.set_string(RegRoot::LocalMachine, &t.key, self.value_name, value)?,
                    }
                    Self::restart(tx, &t.adapter.guid)?;
                }
                Wanted::Bits(bits) => {
                    let now = tx
                        .resolver()
                        .read_raw(RegRoot::LocalMachine, &t.key, self.value_name)?
                        .and_then(|v| v.as_dword())
                        .unwrap_or(0);
                    tx.set_dword(RegRoot::LocalMachine, &t.key, self.value_name, now | bits)?;
                }
            }
        }
        Ok(())
    }

    fn revert(&self, tx: &mut Transaction) -> Result<()> {
        tx.restore_journalled()?;
        if !self.restarts_adapter() {
            return Ok(());
        }
        // Restart each adapter whose setting was just put back, found from
        // the driver key it lives in (gone adapters were skipped above).
        let keys: Vec<String> = tx.written().iter().map(|e| e.key_path.clone()).collect();
        let mut done: Vec<String> = Vec::new();
        for key in keys {
            let Some(id) = tx
                .resolver()
                .read_string(RegRoot::LocalMachine, &key, "NetCfgInstanceId")?
            else {
                continue;
            };
            let guid = id.trim_matches(['{', '}']).to_ascii_lowercase();
            if !done.contains(&guid) {
                Self::restart(tx, &guid)?;
                done.push(guid);
            }
        }
        Ok(())
    }
}

pub const INTERRUPT_MODERATION: AdapterSetting = AdapterSetting {
    id: "network.interruptmoderation",
    name: "Network adapter interrupt moderation: off",
    summary: "Turns Interrupt Moderation off on each network adapter whose driver has it, so the adapter tells the \
              processor about each packet as it arrives instead of grouping several into one interrupt.",
    value_name: "*InterruptModeration",
    wanted: Wanted::Keyword("0"),
    what: "interrupt moderation",
    safety: SafetyTier::Moderate,
    tradeoff: "The connection drops for a few seconds while each adapter restarts, and the processor works harder \
               during large downloads.",
};

pub const FLOW_CONTROL: AdapterSetting = AdapterSetting {
    id: "network.flowcontrol",
    name: "Network adapter flow control: off",
    summary: "Turns Flow Control off on each network adapter whose driver has it, so the adapter no longer pauses \
              sending when the router or switch asks it to.",
    value_name: "*FlowControl",
    wanted: Wanted::Keyword("0"),
    what: "flow control",
    safety: SafetyTier::Moderate,
    tradeoff: "The connection drops for a few seconds while each adapter restarts.",
};

pub const ENERGY_EFFICIENT_ETHERNET: AdapterSetting = AdapterSetting {
    id: "network.eee",
    name: "Energy-Efficient Ethernet: off",
    summary: "Turns Energy-Efficient Ethernet off on each network adapter whose driver has it, so the cable link \
              stays fully awake between packets instead of dozing when traffic is light.",
    value_name: "*EEE",
    wanted: Wanted::Keyword("0"),
    what: "Energy-Efficient Ethernet",
    safety: SafetyTier::Moderate,
    tradeoff: "The connection drops for a few seconds while each adapter restarts, and the adapter uses a little \
               more power.",
};

pub const POWER_SAVING: AdapterSetting = AdapterSetting {
    id: "network.adapterpower",
    name: "Network adapter power saving: off",
    summary: "Clears Device Manager's \"Allow the computer to turn off this device to save power\" for each network \
              adapter, so Windows does not switch an adapter off to save power.",
    value_name: "PnPCapabilities",
    wanted: Wanted::Bits(0x18),
    what: "power saving",
    safety: SafetyTier::Moderate,
    tradeoff: "On a laptop, the adapter uses a little more battery.",
};

pub fn all() -> Vec<Box<dyn Tweak>> {
    vec![
        Box::new(INTERRUPT_MODERATION),
        Box::new(FLOW_CONTROL),
        Box::new(ENERGY_EFFICIENT_ETHERNET),
        Box::new(POWER_SAVING),
    ]
}
