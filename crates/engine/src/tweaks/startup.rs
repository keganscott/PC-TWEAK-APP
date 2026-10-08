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
//! One `StartupToggle` per entry. Its id names the entry (`startup.<source>:
//! <name>`), so a change can be undone from the id alone, even after the
//! program was uninstalled (`Engine::slot_of`).
//!
//! Never turned off: Windows Security's icon, and anti-cheat programs
//! (Kegan's brief: never touch Defender or anti-cheat).
//!
//! VERIFY: the `StartupApproved` key names, which key each source's switch
//! is in, and the byte layout are Task Manager's as recalled (NOTES N82).

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
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

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
}

pub const SOURCES: &[StartupSource] = &[
    StartupSource::UserRun,
    StartupSource::MachineRun,
    StartupSource::MachineRun32,
    StartupSource::UserFolder,
    StartupSource::MachineFolder,
];

impl StartupSource {
    pub fn tag(self) -> &'static str {
        match self {
            StartupSource::UserRun => "user_run",
            StartupSource::MachineRun => "machine_run",
            StartupSource::MachineRun32 => "machine_run32",
            StartupSource::UserFolder => "user_folder",
            StartupSource::MachineFolder => "machine_folder",
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
            StartupSource::UserFolder | StartupSource::MachineFolder => None,
        }
    }

    /// The key holding each entry's on/off switch.
    pub fn approved_key(self) -> (RegRoot, String) {
        let (root, leaf) = match self {
            StartupSource::UserRun => (RegRoot::InteractiveUser, "Run"),
            StartupSource::MachineRun => (RegRoot::LocalMachine, "Run"),
            StartupSource::MachineRun32 => (RegRoot::LocalMachine, "Run32"),
            StartupSource::UserFolder => (RegRoot::InteractiveUser, "StartupFolder"),
            StartupSource::MachineFolder => (RegRoot::LocalMachine, "StartupFolder"),
        };
        (root, format!(r"{APPROVED}\{leaf}"))
    }
}

/// Is a switch value "turned off"? `None` when it is not one Windows writes.
pub fn switched_off(raw: &RawValue) -> Option<bool> {
    (raw.vtype == 3 && !raw.bytes.is_empty()).then(|| raw.bytes[0] & 1 == 1)
}

/// The switch value Task Manager writes when it turns an entry off.
pub fn off_value(filetime: u64) -> RawValue {
    let mut bytes = vec![3, 0, 0, 0];
    bytes.extend(filetime.to_le_bytes());
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
];

/// Why this entry is never turned off, if it is one of `PROTECTED`.
pub fn protected(name: &str, command: Option<&str>) -> Option<BlockedReason> {
    let text = format!("{} {}", name, command.unwrap_or_default()).to_ascii_lowercase();
    PROTECTED
        .iter()
        .find(|(needle, _)| text.contains(needle))
        .map(|(_, what)| BlockedReason::new(BlockedCode::ProtectedProgram, format!("This starts {what}.")))
}

/// The name shown for an entry: a shortcut without its `.lnk`.
pub fn display_name(source: StartupSource, name: &str) -> String {
    match source {
        StartupSource::UserFolder | StartupSource::MachineFolder => name
            .strip_suffix(".lnk")
            .or_else(|| name.strip_suffix(".LNK"))
            .unwrap_or(name)
            .to_owned(),
        _ => name.to_owned(),
    }
}

/// One startup entry's on/off switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupToggle {
    id: String,
    pub source: StartupSource,
    /// The entry's own name: the `Run` value's name, or the file name in the
    /// Startup folder.
    pub name: String,
}

impl StartupToggle {
    pub fn new(source: StartupSource, name: &str) -> Self {
        Self {
            id: format!("{ID_PREFIX}{}:{name}", source.tag()),
            source,
            name: name.to_owned(),
        }
    }

    /// The toggle an id names, if it names one.
    pub fn from_id(id: &str) -> Option<Self> {
        let (tag, name) = id.strip_prefix(ID_PREFIX)?.split_once(':')?;
        let source = StartupSource::from_tag(tag)?;
        (!name.is_empty()).then(|| Self::new(source, name))
    }

    /// The command a `Run` entry starts, if it has one.
    pub fn command(&self, res: &ContextResolver) -> Result<Option<String>> {
        let Some((root, key)) = self.source.run_key() else {
            return Ok(None);
        };
        Ok(res.read_raw(root, key, &self.name)?.and_then(|v| v.as_sz()))
    }

    fn blocked(&self, res: &ContextResolver) -> Result<Option<BlockedReason>> {
        Ok(protected(&self.name, self.command(res)?.as_deref()))
    }
}

impl Tweak for StartupToggle {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> TweakMetadata {
        let shown = display_name(self.source, &self.name);
        let (root, key) = self.source.approved_key();
        let root = match root {
            RegRoot::InteractiveUser => r"HKEY_USERS\<sid>",
            _ => "HKLM",
        };
        TweakMetadata {
            id: Cow::Owned(self.id.clone()),
            name: Cow::Owned(format!("{shown} at sign-in")),
            summary: Cow::Owned(format!(
                "Stops {shown} from starting when you sign in. It stays installed and starts when you open it."
            )),
            target: Cow::Owned(format!(r"{root}\{key}\{}", self.name)),
            category: Cow::Borrowed("startup"),
            tier: Tier::Pro,
            safety: SafetyTier::Safe,
            impact: Impact::Moderate,
            tradeoff: None,
            requires_reboot: false,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        self.source.approved_key().0.required_context()
    }

    fn touches(&self) -> Vec<RegTarget> {
        let (root, key) = self.source.approved_key();
        vec![RegTarget::new(root, &key, &[self.name.as_str()])]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        // Our own change stays undoable whatever the entry is.
        if !has_journal_entry {
            if let Some(reason) = self.blocked(res)? {
                return Ok(TweakState::Blocked { reason });
            }
        }
        let (root, key) = self.source.approved_key();
        Ok(match res.read_raw(root, &key, &self.name)? {
            None => TweakState::Default,
            Some(raw) => match switched_off(&raw) {
                Some(false) => TweakState::Default,
                Some(true) if has_journal_entry => TweakState::Applied,
                Some(true) => TweakState::Foreign,
                None => TweakState::Unknown {
                    detail: format!(
                        "its startup switch has data of type {} PeakTweaks does not know",
                        raw.vtype
                    ),
                },
            },
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
        let (root, key) = self.source.approved_key();
        tx.set_raw(root, &key, &self.name, off_value(filetime_now()))
    }
}
