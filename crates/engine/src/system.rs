//! Changes that are not registry values: power plans, services, scheduled
//! tasks, DNS servers, `netsh` TCP settings, NVIDIA profile settings and whole
//! files (a game's settings file).
//!
//! Same rules as the registry (`transaction.rs`): a tweak declares what it may
//! change (`Tweak::system_targets`), `Transaction` reads the state before,
//! journals it durably, then changes it, and revert puts the recorded state
//! back. The `SystemBackend` trait is the only thing that touches Windows, so
//! all of this runs in tests against `FakeSystem`.
//!
//! **Side effects** (`SideEffect`) are things done after a change commits, such
//! as restarting a network adapter so it picks up new settings. They change no
//! state of their own, so there is nothing to undo; each run is journalled
//! with its outcome.

#[cfg(any(test, feature = "test-support"))]
use std::collections::BTreeMap;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::{EngineError, Result};

/// What a non-registry change is about. String fields may be `*` in a declared
/// target (`Tweak::system_targets`) for names that differ per PC, such as a
/// network adapter's interface GUID; never for files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SysItem {
    /// The active power plan. State: `Text(guid)`.
    ActivePowerScheme,
    /// A power plan PeakTweaks made as a copy of another. State: `Scheme` when
    /// it exists, `Absent` when not.
    PowerScheme { guid: String },
    /// One setting of a power plan, plugged in (`ac`) or on battery. State:
    /// `Dword(index)`.
    PowerSetting {
        scheme: String,
        subgroup: String,
        setting: String,
        ac: bool,
    },
    /// Hibernation (`powercfg /hibernate`). State: `Bool`.
    Hibernation,
    /// A Windows service: start type and whether it runs. State: `Service`.
    Service { name: String },
    /// A scheduled task's enabled state. State: `Bool`.
    ScheduledTask { path: String },
    /// DNS servers of one network adapter, by interface GUID. State:
    /// `List` (empty means automatic, from the router).
    DnsServers { interface: String },
    /// One `netsh interface tcp global` setting. State: `Text`.
    TcpGlobal { name: String },
    /// One NVIDIA driver profile setting (NvAPI DRS). `profile` is empty for
    /// the base (global) profile. State: `Dword`, or `Absent` for the driver's
    /// default.
    NvidiaSetting { profile: String, setting: u32 },
    /// A whole file. State: `File` (a copy kept with the backups) or `Absent`.
    File { path: String },
}

impl SysItem {
    /// Does this concrete item fall under the declared `pattern`? Same kind,
    /// and every field equal (text ignoring case), where a declared text field
    /// of exactly `*` matches any one value. A file path never matches by `*`.
    pub fn matches(&self, pattern: &SysItem) -> bool {
        if matches!(pattern, SysItem::File { path } if path.contains('*')) {
            return false;
        }
        let (Ok(serde_json::Value::Object(c)), Ok(serde_json::Value::Object(p))) =
            (serde_json::to_value(self), serde_json::to_value(pattern))
        else {
            return false;
        };
        c.len() == p.len()
            && p.iter().all(|(k, pv)| match (pv, c.get(k)) {
                (serde_json::Value::String(ps), Some(serde_json::Value::String(cs))) => {
                    ps == "*" || ps.eq_ignore_ascii_case(cs)
                }
                (pv, Some(cv)) => pv == cv,
                (_, None) => false,
            })
    }

    /// One line for the Backups list and error text.
    pub fn describe(&self) -> String {
        match self {
            Self::ActivePowerScheme => "active power plan".into(),
            Self::PowerScheme { guid } => format!("power plan {guid}"),
            Self::PowerSetting {
                scheme,
                subgroup,
                setting,
                ac,
            } => format!(
                "power plan {scheme} setting {subgroup}/{setting} ({})",
                if *ac { "plugged in" } else { "on battery" }
            ),
            Self::Hibernation => "hibernation".into(),
            Self::Service { name } => format!("service {name}"),
            Self::ScheduledTask { path } => format!("scheduled task {path}"),
            Self::DnsServers { interface } => format!("DNS servers of adapter {interface}"),
            Self::TcpGlobal { name } => format!("TCP setting {name}"),
            Self::NvidiaSetting { profile, setting } => {
                let p = if profile.is_empty() { "global" } else { profile };
                format!("NVIDIA setting 0x{setting:08X} ({p} profile)")
            }
            Self::File { path } => format!("file {path}"),
        }
    }
}

/// How a service starts (`Services\<name>\Start`, as `sc config` names it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStart {
    Boot,
    System,
    Automatic,
    /// Automatic, but some minutes after start-up.
    DelayedAutomatic,
    Manual,
    Disabled,
}

/// The state of a `SysItem`, before or after a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SysState {
    /// The thing does not exist (a plan we have not made, a file, a setting at
    /// the driver default).
    Absent,
    Bool {
        on: bool,
    },
    Text {
        text: String,
    },
    Dword {
        value: u32,
    },
    List {
        items: Vec<String>,
    },
    Service {
        start: ServiceStart,
        running: bool,
    },
    /// A power plan copied from `source`.
    Scheme {
        source: String,
    },
    /// A file's whole content, kept as a copy with the backups (`backup`,
    /// relative to the journal directory) and checked by its SHA-256.
    File {
        backup: String,
        sha256: String,
    },
}

/// Something done after a change commits, which changes nothing that needs
/// undoing. Run after apply and after revert, and journalled with its outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum SideEffect {
    /// Turn a network adapter off and on so it reads its new settings. The
    /// connection drops for a few seconds.
    RestartAdapter { interface: String },
    /// Have Windows re-read policy, so new QoS policies take effect.
    RefreshPolicy,
    /// Stop and start a service so it reads its new settings.
    RestartService { name: String },
}

impl SideEffect {
    pub fn describe(&self) -> String {
        match self {
            Self::RestartAdapter { interface } => format!("restart network adapter {interface}"),
            Self::RefreshPolicy => "refresh Windows policy".into(),
            Self::RestartService { name } => format!("restart service {name}"),
        }
    }
}

/// A physical network adapter, as Windows lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct NetAdapter {
    /// Lower-case interface GUID without braces; the name of its
    /// `Tcpip\Parameters\Interfaces\{guid}` key.
    pub guid: String,
    /// The name Windows shows ("Ethernet", "Wi-Fi").
    pub name: String,
    /// Connected now.
    pub up: bool,
    pub wireless: bool,
}

/// The operations on Windows for non-registry changes, and no more.
pub trait SystemBackend: Send + Sync {
    /// Physical network adapters (no virtual switches or VPNs).
    fn network_adapters(&self) -> Result<Vec<NetAdapter>>;
    /// The current state of `item`. File items are read with `read_file`.
    fn read(&self, item: &SysItem) -> Result<SysState>;
    /// Make `item` be `state`. File items are written with `write_file`.
    fn write(&self, item: &SysItem, state: &SysState) -> Result<()>;
    /// A file's bytes, or `None` when it does not exist.
    fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>>;
    /// Replace a file's bytes, or delete it with `None`.
    fn write_file(&self, path: &str, bytes: Option<&[u8]>) -> Result<()>;
    /// Run a side effect.
    fn run(&self, effect: &SideEffect) -> Result<()>;
}

/// A backend that refuses everything, for builds and tests that do not
/// provide one. A tweak that needs it fails with a plain message, before
/// anything is changed (the read comes first).
pub struct Unavailable;

impl Unavailable {
    fn refuse(what: String) -> EngineError {
        EngineError::Internal {
            detail: format!("this build cannot change the {what}"),
        }
    }
}

impl SystemBackend for Unavailable {
    fn network_adapters(&self) -> Result<Vec<NetAdapter>> {
        Err(Self::refuse("network adapters".into()))
    }
    fn read(&self, item: &SysItem) -> Result<SysState> {
        Err(Self::refuse(item.describe()))
    }
    fn write(&self, item: &SysItem, _: &SysState) -> Result<()> {
        Err(Self::refuse(item.describe()))
    }
    fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>> {
        Err(Self::refuse(format!("file {path}")))
    }
    fn write_file(&self, path: &str, _: Option<&[u8]>) -> Result<()> {
        Err(Self::refuse(format!("file {path}")))
    }
    fn run(&self, effect: &SideEffect) -> Result<()> {
        Err(Self::refuse(effect.describe()))
    }
}

/// In-memory backend for tests. Items not set read as `Absent`.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct FakeSystem {
    inner: Mutex<FakeInner>,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct FakeInner {
    states: BTreeMap<String, SysState>,
    files: BTreeMap<String, Vec<u8>>,
    effects: Vec<SideEffect>,
    adapters: Vec<NetAdapter>,
    fail_writes: bool,
    fail_effects: bool,
}

#[cfg(any(test, feature = "test-support"))]
fn fake_key(item: &SysItem) -> String {
    serde_json::to_string(item).unwrap_or_default().to_lowercase()
}

#[cfg(any(test, feature = "test-support"))]
impl FakeSystem {
    pub fn new() -> Self {
        Self::default()
    }
    /// Set a state as if something outside PeakTweaks did.
    pub fn set(&self, item: &SysItem, state: SysState) {
        self.inner.lock().unwrap().states.insert(fake_key(item), state);
    }
    pub fn get(&self, item: &SysItem) -> SysState {
        self.inner
            .lock()
            .unwrap()
            .states
            .get(&fake_key(item))
            .cloned()
            .unwrap_or(SysState::Absent)
    }
    pub fn set_file(&self, path: &str, bytes: &[u8]) {
        self.inner
            .lock()
            .unwrap()
            .files
            .insert(path.to_lowercase(), bytes.to_vec());
    }
    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().files.get(&path.to_lowercase()).cloned()
    }
    /// Side effects run so far, oldest first.
    pub fn effects(&self) -> Vec<SideEffect> {
        self.inner.lock().unwrap().effects.clone()
    }
    pub fn set_adapters(&self, adapters: Vec<NetAdapter>) {
        self.inner.lock().unwrap().adapters = adapters;
    }
    pub fn fail_writes(&self, fail: bool) {
        self.inner.lock().unwrap().fail_writes = fail;
    }
    pub fn fail_effects(&self, fail: bool) {
        self.inner.lock().unwrap().fail_effects = fail;
    }
}

#[cfg(any(test, feature = "test-support"))]
impl SystemBackend for FakeSystem {
    fn network_adapters(&self) -> Result<Vec<NetAdapter>> {
        Ok(self.inner.lock().unwrap().adapters.clone())
    }
    fn read(&self, item: &SysItem) -> Result<SysState> {
        Ok(self.get(item))
    }
    fn write(&self, item: &SysItem, state: &SysState) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if g.fail_writes {
            return Err(EngineError::Internal {
                detail: format!("test: cannot change the {}", item.describe()),
            });
        }
        match state {
            SysState::Absent => g.states.remove(&fake_key(item)),
            s => g.states.insert(fake_key(item), s.clone()),
        };
        Ok(())
    }
    fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.file(path))
    }
    fn write_file(&self, path: &str, bytes: Option<&[u8]>) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if g.fail_writes {
            return Err(EngineError::Internal {
                detail: format!("test: cannot write {path}"),
            });
        }
        match bytes {
            Some(b) => g.files.insert(path.to_lowercase(), b.to_vec()),
            None => g.files.remove(&path.to_lowercase()),
        };
        Ok(())
    }
    fn run(&self, effect: &SideEffect) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        g.effects.push(effect.clone());
        if g.fail_effects {
            return Err(EngineError::Internal {
                detail: format!("test: could not {}", effect.describe()),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Parsing tool output (pure, so tested on any OS)
// ---------------------------------------------------------------------------

/// A lower-case GUID without braces, if `s` is one.
pub(crate) fn guid(s: &str) -> Option<String> {
    let g = s.trim().trim_matches(|c| c == '{' || c == '}').to_ascii_lowercase();
    let parts: Vec<&str> = g.split('-').collect();
    let ok = parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(n, p)| p.len() == *n && p.bytes().all(|b| b.is_ascii_hexdigit()));
    ok.then_some(g)
}

/// Every GUID that `powercfg /list` or `/getactivescheme` prints, in order.
/// Labels are translated on other Windows languages; the GUIDs are not.
pub(crate) fn guids_in(output: &str) -> Vec<String> {
    output
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .filter_map(guid)
        .collect()
}

/// The AC and DC indexes from `powercfg /query <scheme> <sub> <setting>`: the
/// last two lines ending in a `0x` number. Labels are translated; the layout
/// is not. `None` when the setting printed nothing (hidden settings).
pub(crate) fn setting_indexes(output: &str) -> Option<(u32, u32)> {
    let hex: Vec<u32> = output
        .lines()
        .filter_map(|l| l.trim().rsplit(' ').next())
        .filter_map(|w| w.strip_prefix("0x"))
        .filter_map(|h| u32::from_str_radix(h, 16).ok())
        .collect();
    // Possible-value lines come first; the current AC and DC are the last two.
    (hex.len() >= 2).then(|| (hex[hex.len() - 2], hex[hex.len() - 1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_scheme_guids_are_read_whatever_the_language() {
        let active = "Power Scheme GUID: a42b1691-948d-4dd6-8212-9523b6693a8d  (Ultimate Performance (ExitLag))";
        assert_eq!(guids_in(active), vec!["a42b1691-948d-4dd6-8212-9523b6693a8d"]);
        let german = "GUID des Energieschemas: 381B4222-F694-41F0-9685-FF5BB260DF2E  (Ausbalanciert) *";
        assert_eq!(guids_in(german), vec!["381b4222-f694-41f0-9685-ff5bb260df2e"]);
        assert!(guids_in("no guid here").is_empty());
        assert_eq!(
            guid("{8C5E7FDA-E8BF-4A96-9A85-A6E23A8C635C}").as_deref(),
            Some("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c")
        );
        assert_eq!(guid("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635"), None);
    }

    /// Real output from Kegan's PC (2026-10-07), `powercfg /q SCHEME_CURRENT
    /// SUB_PROCESSOR PROCTHROTTLEMIN`.
    #[test]
    fn setting_indexes_are_the_last_two_hex_lines() {
        let out = "Power Scheme GUID: a42b1691-948d-4dd6-8212-9523b6693a8d  (Ultimate Performance (ExitLag))
  Subgroup GUID: 54533251-82be-4824-96c1-47b60b740d00  (Processor power management)
    GUID Alias: SUB_PROCESSOR
    Power Setting GUID: 893dee8e-2bef-41e0-89c6-b55d0929964c  (Minimum processor state)
      GUID Alias: PROCTHROTTLEMIN
      Minimum Possible Setting: 0x00000000
      Maximum Possible Setting: 0x00000064
      Possible Settings increment: 0x00000001
      Possible Settings units: %
    Current AC Power Setting Index: 0x00000000
    Current DC Power Setting Index: 0x00000005
";
        assert_eq!(setting_indexes(out), Some((0, 5)));
        let hidden = "Power Scheme GUID: a42b1691-948d-4dd6-8212-9523b6693a8d  (x)\n";
        assert_eq!(setting_indexes(hidden), None);
    }

    fn dns(i: &str) -> SysItem {
        SysItem::DnsServers { interface: i.into() }
    }

    #[test]
    fn a_star_field_matches_any_one_value_and_nothing_else_changes() {
        assert!(dns("{AB-12}").matches(&dns("*")));
        assert!(dns("{ab-12}").matches(&dns("{AB-12}")));
        assert!(!dns("{AB-12}").matches(&dns("{AB-13}")));
        assert!(
            !SysItem::TcpGlobal { name: "x".into() }.matches(&dns("*")),
            "other kind"
        );
        let ac = SysItem::PowerSetting {
            scheme: "g".into(),
            subgroup: "s".into(),
            setting: "t".into(),
            ac: true,
        };
        let dc_pattern = SysItem::PowerSetting {
            scheme: "*".into(),
            subgroup: "s".into(),
            setting: "t".into(),
            ac: false,
        };
        assert!(!ac.matches(&dc_pattern), "non-text fields must be equal");
    }

    #[test]
    fn a_file_is_never_declared_by_wildcard() {
        let f = SysItem::File {
            path: r"C:\Games\x.ini".into(),
        };
        assert!(!f.matches(&SysItem::File { path: "*".into() }));
        assert!(f.matches(&SysItem::File {
            path: r"c:\games\X.ini".into()
        }));
    }
}
