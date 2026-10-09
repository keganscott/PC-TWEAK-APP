//! Startup apps (CATALOGUE H12): stop a program from starting when the user
//! signs in, the way Task Manager's Startup apps page does.
//!
//! Windows keeps the on/off switch apart from the entry itself, in an
//! `Explorer\StartupApproved` key: one binary value per entry, named like the
//! entry, whose first byte is even while it starts and odd once turned off;
//! the next eight bytes are when it was turned off. The entry (the `Run`
//! value or the shortcut in a Startup folder) is never changed, so the program
//! stays installed and Undo puts the switch back exactly.
//!
//! Two `StartupToggle`s per entry: one turns it off (`startup.<source>:
//! <name>`), one turns back on an entry that was turned off elsewhere, for
//! example in Task Manager (`startup.<source>.on:<name>`). Each id names the
//! entry, so a change can be undone from the id alone, even after the
//! program was uninstalled (`Engine::slot_of`).
//!
//! Never turned off: Windows Security's icon, and anti-cheat programs
//! (Kegan's brief: never touch Defender or anti-cheat). Turning one of them
//! back on is allowed.
//!
//! VERIFY: the `StartupApproved` key names, which key each source's switch
//! is in, and the byte layout are Task Manager's as recalled (NOTES N82).
//!
//! Store apps (packaged apps that declare a `StartupTask` in their manifest)
//! keep their switch elsewhere: a DWORD `State` under the user's
//! `AppModel\SystemAppData\<package family>\<task id>`, holding one of
//! Windows' `StartupTaskState` values (0 Disabled, 1 DisabledByUser,
//! 2 Enabled, 3 DisabledByPolicy, 4 EnabledByPolicy; checked 2026-10-09
//! against Microsoft's Windows.ApplicationModel.StartupTaskState reference).
//! Turning one off writes 1, the user's own "off"; turning one on writes 2.
//! One set by policy is left alone. VERIFY: the key's place, and that Windows
//! honours a value written there (NOTES N100).

use std::borrow::Cow;

use serde::Serialize;
use ts_rs::TS;

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RawValue, RegRoot, RegTarget, SafetyTier, Tier, Tweak,
    TweakMetadata, TweakState,
};

pub const ID_PREFIX: &str = "startup.";

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const TURN_ON: &str = ".on";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
/// Each Store app's startup task switches, under the user's classes
/// (`<package family>\<task id>`, value `State`).
pub const STORE_TASKS: &str =
    r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";
const STORE_STATE: &str = "State";
/// `StartupTaskState` values.
const TASK_DISABLED_BY_USER: u32 = 1;
const TASK_ENABLED: u32 = 2;
const TASK_DISABLED_BY_POLICY: u32 = 3;
const TASK_ENABLED_BY_POLICY: u32 = 4;

/// Where a startup entry comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum StartupSource {
    /// The signed-in user's `Run` key.
    UserRun,
    /// `Run` for everyone on this PC.
    MachineRun,
    /// `Run` for everyone, 32-bit programs.
    MachineRun32,
    /// The signed-in user's Startup folder.
    UserFolder,
    /// The Startup folder for everyone.
    MachineFolder,
    /// A Store app's own startup task, for the signed-in user.
    StoreApp,
}

pub const SOURCES: &[StartupSource] = &[
    StartupSource::UserRun,
    StartupSource::MachineRun,
    StartupSource::MachineRun32,
    StartupSource::UserFolder,
    StartupSource::MachineFolder,
    StartupSource::StoreApp,
];

impl StartupSource {
    pub fn tag(self) -> &'static str {
        match self {
            StartupSource::UserRun => "user_run",
            StartupSource::MachineRun => "machine_run",
            StartupSource::MachineRun32 => "machine_run32",
            StartupSource::UserFolder => "user_folder",
            StartupSource::MachineFolder => "machine_folder",
            StartupSource::StoreApp => "store_app",
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        SOURCES.iter().copied().find(|s| s.tag() == tag)
    }

    /// The registry key holding the entries, for the `Run` sources.
    pub fn run_key(self) -> Option<(RegRoot, &'static str)> {
        match self {
            StartupSource::UserRun => Some((RegRoot::InteractiveUser, RUN)),
            StartupSource::MachineRun => Some((RegRoot::LocalMachine, RUN)),
            StartupSource::MachineRun32 => Some((RegRoot::LocalMachine, RUN_32)),
            StartupSource::UserFolder | StartupSource::MachineFolder | StartupSource::StoreApp => None,
        }
    }

    /// Where an entry's on/off switch is: root, key and value name.
    pub fn switch(self, name: &str) -> (RegRoot, String, String) {
        let (root, leaf) = match self {
            StartupSource::UserRun => (RegRoot::InteractiveUser, "Run"),
            StartupSource::MachineRun => (RegRoot::LocalMachine, "Run"),
            StartupSource::MachineRun32 => (RegRoot::LocalMachine, "Run32"),
            StartupSource::UserFolder => (RegRoot::InteractiveUser, "StartupFolder"),
            StartupSource::MachineFolder => (RegRoot::LocalMachine, "StartupFolder"),
            StartupSource::StoreApp => {
                return (
                    RegRoot::InteractiveUser,
                    format!(r"{STORE_TASKS}\{name}"),
                    STORE_STATE.to_owned(),
                )
            }
        };
        (root, format!(r"{APPROVED}\{leaf}"), name.to_owned())
    }
}

/// Is a switch value "turned off"? `None` when it is not one Windows writes.
pub fn switched_off(raw: &RawValue) -> Option<bool> {
    (raw.vtype == 3 && !raw.bytes.is_empty()).then(|| raw.bytes[0] & 1 == 1)
}

/// The switch value Task Manager writes when it turns an entry off: 12 bytes,
/// 3 and three zero bytes, then the time as a FILETIME.
pub fn off_value(filetime: u64) -> RawValue {
    let mut bytes = vec![3, 0, 0, 0];
    bytes.extend(filetime.to_le_bytes());
    RawValue { vtype: 3, bytes }
}

/// The switch value Task Manager writes when it turns an entry on: 12 bytes,
/// 2 and eleven zero bytes.
pub fn on_value() -> RawValue {
    let mut bytes = vec![0; 12];
    bytes[0] = 2;
    RawValue { vtype: 3, bytes }
}

impl StartupToggle {
    /// A switch turned off at some time, as Task Manager leaves it.
    #[cfg(test)]
    pub fn off_for_test() -> RawValue {
        off_value(133_000_000_000_000_000)
    }
}

/// Now as a Windows FILETIME (100 ns since 1601).
fn filetime_now() -> u64 {
    const UNIX_TO_1601: u64 = 11_644_473_600;
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (since.as_secs() + UNIX_TO_1601) * 10_000_000 + u64::from(since.subsec_nanos() / 100)
}

/// Programs PeakTweaks never stops from starting, by entry name or command
/// (any case): Windows Security, and anti-cheat.
const PROTECTED: &[(&str, &str)] = &[
    ("securityhealth", "Windows Security"),
    ("windowsdefender", "Windows Security"),
    ("msascuil", "Windows Security"),
    ("vgtray", "Riot Vanguard, an anti-cheat"),
    ("riot vanguard", "Riot Vanguard, an anti-cheat"),
    ("easyanticheat", "Easy Anti-Cheat"),
    ("battleye", "BattlEye, an anti-cheat"),
    ("faceit", "FACEIT, an anti-cheat"),
    ("esea", "ESEA, an anti-cheat"),
    ("sechealthui", "Windows Security"),
];

/// Why this entry is never turned off, if it is one of `PROTECTED`.
pub fn protected(name: &str, command: Option<&str>) -> Option<BlockedReason> {
    let text = format!("{} {}", name, command.unwrap_or_default()).to_ascii_lowercase();
    PROTECTED
        .iter()
        .find(|(needle, _)| text.contains(needle))
        .map(|(_, what)| BlockedReason::new(BlockedCode::ProtectedProgram, format!("This starts {what}.")))
}

/// The name shown for an entry: a shortcut without its `.lnk`; for a Store
/// app, its task id when no better name is known.
pub fn display_name(source: StartupSource, name: &str) -> String {
    match source {
        StartupSource::StoreApp => name.rsplit('\\').next().unwrap_or(name).to_owned(),
        StartupSource::UserFolder | StartupSource::MachineFolder => name
            .strip_suffix(".lnk")
            .or_else(|| name.strip_suffix(".LNK"))
            .unwrap_or(name)
            .to_owned(),
        _ => name.to_owned(),
    }
}

/// `<package family>\<task id>`, each made of letters, digits, `.`, `_` and
/// `-` and not only dots: two key names under `STORE_TASKS`, never a way out
/// of it.
pub fn is_store_task_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('\\').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && !p.chars().all(|c| c == '.')
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
}

/// One startup entry's on/off switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupToggle {
    id: String,
    pub source: StartupSource,
    /// The entry's own name: the `Run` value's name, or the file name in the
    /// Startup folder.
    pub name: String,
    /// Turns the entry on, rather than off.
    pub turn_on: bool,
    /// Store apps: the name Windows shows for the task, when known.
    pub label: Option<String>,
    /// Store apps: whether the task starts when no switch has been saved
    /// (its manifest's `Enabled`).
    pub on_by_default: bool,
}

impl StartupToggle {
    /// The switch that turns the entry off.
    pub fn new(source: StartupSource, name: &str) -> Self {
        Self {
            id: format!("{ID_PREFIX}{}:{name}", source.tag()),
            source,
            name: name.to_owned(),
            turn_on: false,
            label: None,
            on_by_default: false,
        }
    }

    /// The switch that turns the entry back on.
    pub fn turning_on(source: StartupSource, name: &str) -> Self {
        Self {
            id: format!("{ID_PREFIX}{}{TURN_ON}:{name}", source.tag()),
            source,
            name: name.to_owned(),
            turn_on: true,
            label: None,
            on_by_default: false,
        }
    }

    /// The same switch, with what the startup list knows about a Store app.
    pub fn with_store_facts(mut self, label: Option<String>, on_by_default: bool) -> Self {
        self.label = label;
        self.on_by_default = on_by_default;
        self
    }

    /// The facts `with_store_facts` takes, as one string kept with the id
    /// between listing and applying (`Engine::relist`).
    pub fn store_facts(&self) -> String {
        format!(
            "{}{}",
            u8::from(self.on_by_default),
            self.label.as_deref().unwrap_or_default()
        )
    }

    /// `store_facts` read back.
    pub fn with_saved_facts(self, facts: &str) -> Self {
        let on = facts.starts_with('1');
        let label = facts.get(1..).filter(|l| !l.is_empty()).map(str::to_owned);
        self.with_store_facts(label, on)
    }

    /// The name shown.
    pub fn shown(&self) -> String {
        self.label
            .clone()
            .unwrap_or_else(|| display_name(self.source, &self.name))
    }

    /// The toggle an id names, if it names one.
    pub fn from_id(id: &str) -> Option<Self> {
        let (tag, name) = id.strip_prefix(ID_PREFIX)?.split_once(':')?;
        if name.is_empty() {
            return None;
        }
        let (source, turn_on) = match tag.strip_suffix(TURN_ON) {
            Some(tag) => (StartupSource::from_tag(tag)?, true),
            None => (StartupSource::from_tag(tag)?, false),
        };
        // A Store app's name is part of a key path, so only one shaped as
        // Windows names them: `<package family>\<task id>`.
        if source == StartupSource::StoreApp && !is_store_task_name(name) {
            return None;
        }
        Some(if turn_on {
            Self::turning_on(source, name)
        } else {
            Self::new(source, name)
        })
    }

    /// The command a `Run` entry starts, if it has one.
    pub fn command(&self, res: &ContextResolver) -> Result<Option<String>> {
        let Some((root, key)) = self.source.run_key() else {
            return Ok(None);
        };
        Ok(res.read_raw(root, key, &self.name)?.and_then(|v| v.as_sz()))
    }

    fn blocked(&self, res: &ContextResolver) -> Result<Option<BlockedReason>> {
        if self.source == StartupSource::StoreApp {
            if let Some(TASK_DISABLED_BY_POLICY | TASK_ENABLED_BY_POLICY) = self.task_state(res)? {
                return Ok(Some(BlockedReason::new(
                    BlockedCode::SetByPolicy,
                    "An administrator or a policy on this PC decides whether this starts.",
                )));
            }
        }
        if self.turn_on {
            return Ok(None);
        }
        let name = format!("{} {}", self.name, self.label.as_deref().unwrap_or_default());
        Ok(protected(&name, self.command(res)?.as_deref()))
    }

    /// A Store app task's saved `State`, if it has one. Not a DWORD: an error,
    /// so the switch reads as unknown and nothing is written over it.
    fn task_state(&self, res: &ContextResolver) -> Result<Option<u32>> {
        let (root, key, value) = self.source.switch(&self.name);
        match res.read_raw(root, &key, &value)? {
            None => Ok(None),
            Some(raw) if raw.vtype == 4 && raw.bytes.len() == 4 => {
                Ok(Some(u32::from_le_bytes(raw.bytes[..4].try_into().unwrap())))
            }
            Some(raw) => Err(EngineError::registry_msg(
                res.display_path(root, &key),
                Some(&value),
                format!(
                    "its startup switch has data of type {} PeakTweaks does not know",
                    raw.vtype
                ),
            )),
        }
    }

    /// Is the entry turned off now? `Err(detail)` when its switch holds
    /// something PeakTweaks does not know.
    fn is_off(&self, res: &ContextResolver) -> Result<std::result::Result<bool, String>> {
        if self.source == StartupSource::StoreApp {
            return Ok(match self.task_state(res)? {
                None => Ok(!self.on_by_default),
                Some(TASK_ENABLED | TASK_ENABLED_BY_POLICY) => Ok(false),
                Some(0 | TASK_DISABLED_BY_USER | TASK_DISABLED_BY_POLICY) => Ok(true),
                Some(other) => Err(format!(
                    "its startup task state is {other}, which PeakTweaks does not know"
                )),
            });
        }
        let (root, key, value) = self.source.switch(&self.name);
        // No switch value: it starts.
        Ok(match res.read_raw(root, &key, &value)? {
            None => Ok(false),
            Some(raw) => switched_off(&raw).ok_or_else(|| {
                format!(
                    "its startup switch has data of type {} PeakTweaks does not know",
                    raw.vtype
                )
            }),
        })
    }
}

impl Tweak for StartupToggle {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> TweakMetadata {
        let shown = self.shown();
        let (root, key, value) = self.source.switch(&self.name);
        let root = match root {
            RegRoot::InteractiveUser => r"HKEY_USERS\<sid>",
            _ => "HKLM",
        };
        let (name, summary) = if self.turn_on {
            (
                format!("{shown} at sign-in, turned back on"),
                format!("Starts {shown} when you sign in again. It was turned off outside PeakTweaks."),
            )
        } else {
            (
                format!("{shown} at sign-in"),
                format!(
                    "Stops {shown} from starting when you sign in. It stays installed and starts when you open it."
                ),
            )
        };
        TweakMetadata {
            id: Cow::Owned(self.id.clone()),
            name: Cow::Owned(name),
            summary: Cow::Owned(summary),
            target: Cow::Owned(format!(r"{root}\{key}\{value}")),
            category: Cow::Borrowed("startup"),
            tier: Tier::Pro,
            safety: SafetyTier::Safe,
            impact: Impact::Moderate,
            tradeoff: None,
            requires_reboot: false,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        self.source.switch(&self.name).0.required_context()
    }

    fn touches(&self) -> Vec<RegTarget> {
        let (root, key, value) = self.source.switch(&self.name);
        vec![RegTarget::new(root, &key, &[value.as_str()])]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        // Our own change stays undoable whatever the entry is.
        if !has_journal_entry {
            if let Some(reason) = self.blocked(res)? {
                return Ok(TweakState::Blocked { reason });
            }
        }
        // Done means off for the turning-off switch, on for the other.
        Ok(match self.is_off(res)?.map(|off| off != self.turn_on) {
            Ok(false) => TweakState::Default,
            Ok(true) if has_journal_entry => TweakState::Applied,
            Ok(true) => TweakState::Foreign,
            Err(detail) => TweakState::Unknown { detail },
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        if let Some(reason) = self.blocked(tx.resolver())? {
            return Err(EngineError::Blocked { reason });
        }
        // A switch is only written for an entry that is there.
        if let Some((root, key)) = self.source.run_key() {
            if tx.resolver().read_raw(root, key, &self.name)?.is_none() {
                return Err(EngineError::UnknownTweak {
                    tweak_id: self.id.clone(),
                });
            }
        }
        let (root, key, name) = self.source.switch(&self.name);
        let value = match (self.source, self.turn_on) {
            (StartupSource::StoreApp, true) => RawValue::dword(TASK_ENABLED),
            (StartupSource::StoreApp, false) => RawValue::dword(TASK_DISABLED_BY_USER),
            (_, true) => on_value(),
            (_, false) => off_value(filetime_now()),
        };
        tx.set_raw(root, &key, &name, value)
    }
}
