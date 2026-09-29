//! Core engine types.
//!
//! Design note on the trait shape: `apply` and `revert` do not receive a raw
//! registry handle. They receive a `&mut Transaction`, which is the only thing
//! in the engine that can mutate. Every mutation through it captures the prior
//! value, writes a `.reg` backup and appends a journal entry before the write
//! lands. A tweak therefore cannot make an untracked change, by oversight or
//! deliberately. That constraint is the whole safety story, so it lives in the
//! type system rather than in a code review checklist.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::context::ContextResolver;
use super::error::Result;
use super::hardware::HardwareReport;
use super::restore::RestoreStatus;
use super::security::SecurityReport;
use super::transaction::Transaction;

// ---------------------------------------------------------------------------
// Execution context
// ---------------------------------------------------------------------------

/// Which privilege and hive a tweak needs.
///
/// We ship a single elevated binary, not a SYSTEM service, so "Service" here
/// means machine-wide state written by the elevated process, not a separate
/// service account. The distinction that matters is which registry root the
/// write lands in, because an elevated process writing `HKEY_CURRENT_USER` is
/// still writing the *invoking* user's hive, which may not be the interactive
/// user if the app was launched with alternate admin credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionContext {
    /// Machine-wide: HKLM, services, bcdedit, powercfg, netsh.
    Service,
    /// The interactive user's hive: HKEY_USERS\<sid> (or HKCU when they match).
    User,
    /// Read-only. Never mutates; safe to run unelevated.
    Diagnostic,
}

// ---------------------------------------------------------------------------
// Registry addressing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RegRoot {
    LocalMachine,
    /// Resolved at runtime to `HKEY_USERS\<sid>` or `HKEY_CURRENT_USER`.
    InteractiveUser,
    ClassesRoot,
}

impl RegRoot {
    /// The context a write to this root belongs to. Used to enforce that a
    /// tweak declaring `ExecutionContext::User` cannot quietly write HKLM.
    pub fn required_context(self) -> ExecutionContext {
        match self {
            Self::LocalMachine | Self::ClassesRoot => ExecutionContext::Service,
            Self::InteractiveUser => ExecutionContext::User,
        }
    }
}

/// One registry key and the value names a tweak is allowed to change in it.
/// This is the tweak's declared blast radius: `Transaction` refuses anything
/// outside it, on apply and on replay of the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegTarget {
    pub root: RegRoot,
    pub key: String,
    pub values: Vec<String>,
}

impl RegTarget {
    pub fn new(root: RegRoot, key: impl Into<String>, values: &[&str]) -> Self {
        Self {
            root,
            key: key.into(),
            values: values.iter().map(|v| (*v).to_owned()).collect(),
        }
    }
}

/// Registry value types we can back up and restore byte-exact. Anything else
/// (REG_NONE, REG_LINK, resource lists) is refused rather than mis-typed.
pub const SUPPORTED_VALUE_TYPES: [u32; 6] = [
    1,  // REG_SZ
    2,  // REG_EXPAND_SZ
    3,  // REG_BINARY
    4,  // REG_DWORD
    7,  // REG_MULTI_SZ
    11, // REG_QWORD
];

/// A registry value carried as raw bytes plus its type, so the engine never
/// has to understand a value in order to back it up and restore it byte-exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RawValue {
    /// REG_SZ = 1, REG_EXPAND_SZ = 2, REG_BINARY = 3, REG_DWORD = 4,
    /// REG_MULTI_SZ = 7, REG_QWORD = 11.
    pub vtype: u32,
    /// Comma-separated lowercase hex bytes, e.g. `"26,00,00,00"`.
    #[serde(with = "hex_bytes")]
    #[ts(type = "string")]
    pub bytes: Vec<u8>,
}

impl RawValue {
    pub fn dword(v: u32) -> Self {
        Self {
            vtype: 4,
            bytes: v.to_le_bytes().to_vec(),
        }
    }

    pub fn sz(s: &str) -> Self {
        let mut bytes: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
        bytes.extend_from_slice(&[0, 0]); // REG_SZ is NUL-terminated
        Self { vtype: 1, bytes }
    }

    pub fn is_supported_type(&self) -> bool {
        SUPPORTED_VALUE_TYPES.contains(&self.vtype)
    }

    pub fn as_dword(&self) -> Option<u32> {
        (self.vtype == 4 && self.bytes.len() == 4)
            .then(|| u32::from_le_bytes([self.bytes[0], self.bytes[1], self.bytes[2], self.bytes[3]]))
    }

    pub fn as_sz(&self) -> Option<String> {
        if self.vtype != 1 && self.vtype != 2 {
            return None;
        }
        let units: Vec<u16> = utf16le_units(&self.bytes).into_iter().take_while(|&u| u != 0).collect();
        String::from_utf16(&units).ok()
    }
}

/// Little-endian UTF-16 code units of `bytes`; a trailing odd byte is ignored.
pub(crate) fn utf16le_units(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect()
}

/// Hex-string serialisation so journal files stay readable and hand-editable
/// during offline recovery. A recovery operator staring at a JSON line in
/// Notepad from WinPE should be able to see what the value was.
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(",");
        s.serialize_str(&hex)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        if s.is_empty() {
            return Ok(Vec::new());
        }
        s.split(',')
            .map(|p| u8::from_str_radix(p.trim(), 16).map_err(serde::de::Error::custom))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tweak state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TweakState {
    /// Matches the Windows default.
    Default,
    /// Matches our applied value and we hold a journal entry for it.
    Applied,
    /// Non-default, but we have no journal entry: someone else set this.
    /// The UI offers "Undo it" rather than a toggle.
    Foreign,
    /// Predicate failed.
    Blocked { reason: BlockedReason },
    /// We could not read the state. The UI shows this and blocks Apply; a read
    /// failure must never be reported as `Default`.
    Unknown { detail: String },
}

// ---------------------------------------------------------------------------
// Structured blocked reasons
// ---------------------------------------------------------------------------

/// Machine-readable block codes. The frontend maps these to specific modals; it
/// must never parse `message`. Adding a variant is a breaking change for the UI
/// by design: a new block class deserves a considered UI response, not a
/// fallback string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum BlockedCode {
    /// The selected game's anti-cheat requires the state this tweak would change.
    AntiCheatRequirement,
    /// Applying this would break launch eligibility for the target title.
    AntiCheatEligibility,
    /// Hardware does not support it, or it is meaningless on this hardware.
    HardwareUnsupported,
    /// Would be a net loss on this hardware (thermally limited, HDD boot, etc.).
    HardwareCounterproductive,
    /// Windows build too old or too new.
    OsVersionUnsupported,
    /// A conflicting tweak is applied.
    ConflictingTweak,
    /// Needs elevation we do not have.
    InsufficientPrivilege,
    /// System protection is off, or no restore point has been verified, so there
    /// is no rollback point.
    NoRestorePoint,
    /// Device-specific: MSI mode on the boot storage controller, etc.
    UnsafeForDevice,
    /// The license held by the engine does not cover this tweak's tier.
    TierRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BlockedReason {
    pub code: BlockedCode,
    /// What triggered it: a game id, a device instance path, a build number.
    /// Lets the UI say "because you selected Fortnite" without string parsing.
    pub trigger: Option<String>,
    /// Human-readable, for display only. Never parsed.
    pub message: String,
}

impl BlockedReason {
    pub fn new(code: BlockedCode, message: impl Into<String>) -> Self {
        Self {
            code,
            trigger: None,
            message: message.into(),
        }
    }

    pub fn with_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = Some(trigger.into());
        self
    }
}

#[derive(Debug, Clone)]
pub enum PredicateOutcome {
    Allow,
    Block(BlockedReason),
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum SafetyTier {
    Safe,
    Moderate,
    Extreme,
    OfflineRig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum Impact {
    Moderate,
    Extreme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Free,
    Pro,
    Ultimate,
}

/// Text fields are `Cow` so static tweaks use literals and parameterised
/// tweaks (one per game exe) can own theirs. Serialize only: metadata flows out
/// to the UI and is never read back.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TweakMetadata {
    pub id: Cow<'static, str>,
    pub name: Cow<'static, str>,
    pub summary: Cow<'static, str>,
    /// Rendered verbatim in the UI so the user can verify what we touched.
    pub target: Cow<'static, str>,
    pub category: Cow<'static, str>,
    pub tier: Tier,
    pub safety: SafetyTier,
    pub impact: Impact,
    /// Required reading before the toggle engages. `None` for safe-tier tweaks.
    pub tradeoff: Option<Cow<'static, str>>,
    pub requires_reboot: bool,
}

/// Snapshot of the machine that predicates evaluate against. Populated once per
/// refresh so a hundred predicates do not each hit WMI.
///
/// Built only by the engine, from probes. It is deliberately not `Deserialize`:
/// nothing the webview sends can become a `SystemEnv`. A probe that could not
/// run leaves its report `None`; a probe that ran but could not tell says so
/// inside the report (`Probe::Unknown`). Predicates must treat both as "not
/// known", never as "no".
#[derive(Debug, Clone, Default, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SystemEnv {
    pub elevated: bool,
    /// Currently selected target game id, if any. Drives anti-cheat predicates.
    pub target_game: Option<String>,
    /// True while a fresh restore point exists (see `restore.rs`).
    pub restore_gate_open: bool,
    pub hardware: Option<HardwareReport>,
    pub security: Option<SecurityReport>,
    pub restore: Option<RestoreStatus>,
}

impl SystemEnv {
    /// Logical processors, if known.
    pub fn logical_processors(&self) -> Option<u32> {
        self.hardware.as_ref()?.cpu.value().map(|c| c.logical_processors)
    }

    /// Windows build number, if known.
    pub fn os_build(&self) -> Option<u32> {
        self.hardware.as_ref()?.os.value().map(|o| o.build)
    }
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// A single reversible system change.
///
/// Implementors are stateless. All mutation flows through `Transaction`, all
/// reads through `ContextResolver`. Both are supplied by the engine.
pub trait Tweak: Send + Sync {
    fn id(&self) -> &str;

    fn metadata(&self) -> TweakMetadata;

    fn execution_context(&self) -> ExecutionContext;

    /// Every registry key and value this tweak may change. `Transaction`
    /// refuses writes, deletes and journal replays outside this list.
    ///
    /// Revert replays old journal records against *this* list, so when a
    /// release changes what a tweak writes, keep the old targets listed for as
    /// long as any user may still have the old version applied. Dropping one
    /// makes that user's revert fail with `ContextViolation`.
    fn touches(&self) -> Vec<RegTarget>;

    /// Cheap, pure, no I/O. Called on every refresh.
    fn evaluate_predicate(&self, env: &SystemEnv) -> PredicateOutcome {
        let _ = env;
        PredicateOutcome::Allow
    }

    /// Read the current on-disk state. Must not mutate.
    ///
    /// `has_journal_entry` tells the implementor whether *we* applied it, which
    /// is what separates `Applied` from `Foreign`. Return `Err` when the state
    /// cannot be read; the engine reports that as `Unknown`, never `Default`.
    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState>;

    /// Describe the mutation. Everything written through `tx` is captured and
    /// journalled before it lands.
    fn apply(&self, tx: &mut Transaction) -> Result<()>;

    /// Default revert restores the state from before this tweak's outstanding
    /// applies, which is correct for any tweak whose apply is a set of registry
    /// writes. Override only when reverting needs something the journal cannot
    /// express, for example re-enabling a service that also has to be restarted.
    fn revert(&self, tx: &mut Transaction) -> Result<()> {
        tx.restore_journalled()
    }
}
