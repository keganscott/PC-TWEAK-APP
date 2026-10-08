//! The real `SystemBackend` on Windows.
//!
//! Nothing here goes through a shell command line. Windows' own tools
//! (`powercfg.exe`, `gpupdate.exe`) are started by absolute path under
//! `%SystemRoot%\System32` with an argument list, and every argument is checked
//! first (GUIDs must be GUIDs, names a short safe character set). The few
//! things only PowerShell exposes (DNS, adapter restart) run as fixed scripts
//! that read their inputs from environment variables, so no input becomes
//! script text. Services use the Win32 service API and scheduled tasks the
//! Task Scheduler COM API directly.
//!
//! NVIDIA settings go through NvAPI (`nvapi.rs`), base profile only; AMD
//! settings through ADLX (`adlx.rs`).
//!
//! Not yet built here, and refused with a plain message: `netsh` TCP globals
//! (its catalogue step adds them; NOTES N66).

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::error::{EngineError, Result};
use super::proc::run_limited;
use super::registry::windows::WinRegistry;
use super::registry::{Hive, RegistryBackend};
use super::system::{
    guid, guids_in, setting_indexes, DeviceClass, NetAdapter, PciDevice, SideEffect, SysItem, SysState, SystemBackend,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const LIMIT: Duration = Duration::from_secs(60);

/// The real backend. The adapter list is cached for a short while: the tool
/// list asks for it on every refresh, and each listing starts PowerShell.
pub struct WinSystem {
    /// Values Windows keeps in the registry (hibernation, DNS) are read here
    /// rather than by starting PowerShell on every refresh.
    reg: WinRegistry,
    adapters: std::sync::Mutex<Option<(std::time::Instant, Vec<NetAdapter>)>>,
    /// Every physical adapter's interface metrics, from one PowerShell run;
    /// dropped on every metric write.
    metrics: std::sync::Mutex<Option<(std::time::Instant, Vec<Metric>)>>,
    /// The global TCP settings, from one PowerShell run; dropped on every
    /// TCP setting write.
    tcp: std::sync::Mutex<Option<(std::time::Instant, TcpGlobals)>>,
    /// The QoS policies in this computer's policy store, from one PowerShell
    /// run; dropped on every QoS policy write.
    qos: std::sync::Mutex<Option<(std::time::Instant, Vec<QosPolicy>)>>,
    nvapi: crate::nvapi::NvApi,
    adlx: crate::adlx::Adlx,
}

impl WinSystem {
    pub fn new() -> Self {
        Self {
            reg: WinRegistry::new(),
            adapters: std::sync::Mutex::new(None),
            metrics: std::sync::Mutex::new(None),
            tcp: std::sync::Mutex::new(None),
            qos: std::sync::Mutex::new(None),
            nvapi: crate::nvapi::NvApi::new(),
            adlx: crate::adlx::Adlx::new(),
        }
    }

    fn metrics(&self) -> Result<Vec<Metric>> {
        let mut cache = self.metrics.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, list)) = cache.as_ref() {
            if at.elapsed() < ADAPTER_CACHE {
                return Ok(list.clone());
            }
        }
        let list = parse_metrics(&powershell("interface metrics", METRICS_SCRIPT, &[])?);
        *cache = Some((std::time::Instant::now(), list.clone()));
        Ok(list)
    }

    /// One global TCP setting by its `netsh` name, lower case.
    fn tcp_global(&self, name: &str) -> Result<String> {
        let mut cache = self.tcp.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let fresh = cache.as_ref().filter(|(at, _)| at.elapsed() < ADAPTER_CACHE);
        let list = match fresh {
            Some((_, list)) => list.clone(),
            None => {
                let list = parse_tcp_globals(&powershell("TCP settings", TCP_SCRIPT, &[])?);
                *cache = Some((std::time::Instant::now(), list.clone()));
                list
            }
        };
        list.into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
            .ok_or_else(|| fail("TCP settings", format!("Windows did not report {name}")))
    }

    /// After a QoS policy write: does Windows now list what was written?
    fn check_qos(&self, item: &SysItem, wanted: &SysState) -> Result<()> {
        *self.qos.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        let now = self.read(item)?;
        let same = match (&now, wanted) {
            (SysState::QosPolicy { program: a, dscp: x }, SysState::QosPolicy { program: b, dscp: y }) => {
                x == y && a.eq_ignore_ascii_case(b)
            }
            (now, wanted) => now == wanted,
        };
        if same {
            Ok(())
        } else {
            Err(fail(
                "QoS policy",
                format!("{} reads {now:?} after it was set to {wanted:?}", item.describe()),
            ))
        }
    }

    /// The QoS policy called `name` in this computer's policy store.
    fn qos_policy(&self, name: &str) -> Result<Option<QosPolicy>> {
        let mut cache = self.qos.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let fresh = cache.as_ref().filter(|(at, _)| at.elapsed() < ADAPTER_CACHE);
        let list = match fresh {
            Some((_, list)) => list.clone(),
            None => {
                let list = parse_qos_policies(&powershell("QoS policies", QOS_SCRIPT, &[])?);
                *cache = Some((std::time::Instant::now(), list.clone()));
                list
            }
        };
        Ok(list.into_iter().find(|q| q.name.eq_ignore_ascii_case(name)))
    }
}

/// Each global TCP setting by its `netsh` name, with its value.
type TcpGlobals = Vec<(String, String)>;

/// One QoS policy as `Get-NetQosPolicy` lists it: its name, the program it
/// matches (empty for none) and its DSCP tag (-1 for none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QosPolicy {
    name: String,
    program: String,
    dscp: i16,
}

/// The policies `New-NetQosPolicy` adds to without `-PolicyStore`, which
/// Windows keeps across restarts. VERIFY (NOTES N91): that this store is the
/// one Windows applies.
const QOS_SCRIPT: &str = "Get-NetQosPolicy -ErrorAction SilentlyContinue | ForEach-Object { '{0}|{1}|{2}' -f \
                          $_.Name, $_.AppPathNameMatchCondition, $_.DSCPAction }; exit 0";

/// `Name|Program|DSCP` lines from `QOS_SCRIPT`. A line that does not parse
/// is skipped.
pub(crate) fn parse_qos_policies(out: &str) -> Vec<QosPolicy> {
    out.lines()
        .filter_map(|l| {
            let mut parts = l.trim().rsplitn(3, '|');
            let dscp = parts.next()?.trim().parse().ok()?;
            let program = parts.next()?.trim().to_owned();
            let name = parts.next()?.trim().to_owned();
            (!name.is_empty()).then_some(QosPolicy { name, program, dscp })
        })
        .collect()
}

/// A program file name a QoS policy may match: a name only, ending `.exe`.
fn need_program(s: &str) -> Result<()> {
    let ok = s.len() <= 260
        && s.len() > 4
        && s.to_ascii_lowercase().ends_with(".exe")
        && !s.starts_with(['-', '.'])
        && s.chars().all(|c| c.is_ascii_alphanumeric() || " _-.".contains(c));
    if ok {
        Ok(())
    } else {
        Err(fail("QoS policy", format!("{s:?} is not a program file name")))
    }
}

/// The settings `netsh interface tcp set global` changes, read where Windows
/// keeps them as names that are never translated (`netsh`'s own output is).
/// CI run 37778720202 showed `netsh` changes these values (NOTES N90).
const TCP_SCRIPT: &str = "$t = Get-NetTCPSetting -SettingName Internet -ErrorAction Stop; $o = \
                          Get-NetOffloadGlobalSetting -ErrorAction Stop; 'autotuninglevel|{0}' -f \
                          $t.AutoTuningLevelLocal; 'rss|{0}' -f $o.ReceiveSideScaling; 'ecncapability|{0}' -f \
                          $t.EcnCapability";

/// `name|Value` lines from `TCP_SCRIPT`, values in lower case. Only a name
/// and value `netsh` takes are kept.
pub(crate) fn parse_tcp_globals(out: &str) -> TcpGlobals {
    out.lines()
        .filter_map(|l| {
            let (name, value) = l.trim().split_once('|')?;
            let value = value.trim().to_ascii_lowercase();
            crate::tweaks::tcp::allowed(name)?
                .contains(&value.as_str())
                .then(|| (name.to_owned(), value))
        })
        .collect()
}

/// `InstanceId|Class|FriendlyName` lines from `Get-PnpDevice`: graphics
/// cards and network adapters on the PCI bus. Anything else is skipped.
pub(crate) fn parse_pci_devices(out: &str) -> Vec<PciDevice> {
    out.lines()
        .filter_map(|l| {
            let mut parts = l.trim().splitn(3, '|');
            let instance_id = parts.next()?.trim().to_owned();
            let class = match parts.next()?.trim() {
                c if c.eq_ignore_ascii_case("Display") => DeviceClass::Display,
                c if c.eq_ignore_ascii_case("Net") => DeviceClass::Net,
                _ => return None,
            };
            let name = parts.next()?.trim().to_owned();
            crate::tweaks::msi::pci_instance(&instance_id)?;
            Some(PciDevice {
                instance_id,
                name,
                class,
            })
        })
        .collect()
}

/// Every physical adapter's IPv4 and IPv6 interface, one line each.
const METRICS_SCRIPT: &str = "Get-NetAdapter -Physical | ForEach-Object { $g = $_.InterfaceGuid; Get-NetIPInterface \
                              -InterfaceIndex $_.ifIndex -ErrorAction SilentlyContinue | ForEach-Object { \
                              '{0}|{1}|{2}|{3}' -f $g, $_.AddressFamily, $_.AutomaticMetric, $_.InterfaceMetric } }";

/// One adapter's interface metric for one protocol; `0` when automatic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Metric {
    pub guid: String,
    pub ipv6: bool,
    pub value: u32,
}

/// `guid|IPv4|Enabled|25` lines (AutomaticMetric, InterfaceMetric) from
/// `Get-NetIPInterface`. An automatic metric reads as 0. The family and the
/// automatic flag are also taken as numbers (2 / 23, 1 / 0), in case the
/// properties come through as their raw values.
pub(crate) fn parse_metrics(out: &str) -> Vec<Metric> {
    out.lines()
        .filter_map(|l| {
            let mut parts = l.trim().splitn(4, '|');
            let g = guid(parts.next()?)?;
            let ipv6 = match parts.next()?.trim() {
                "IPv4" | "2" => false,
                "IPv6" | "23" => true,
                _ => return None,
            };
            let automatic = match parts.next()?.trim() {
                a if a.eq_ignore_ascii_case("Enabled") || a == "1" => true,
                a if a.eq_ignore_ascii_case("Disabled") || a == "0" => false,
                _ => return None,
            };
            let metric: u32 = parts.next()?.trim().parse().ok()?;
            Some(Metric {
                guid: g,
                ipv6,
                value: if automatic { 0 } else { metric },
            })
        })
        .collect()
}

impl Default for WinSystem {
    fn default() -> Self {
        Self::new()
    }
}

const ADAPTER_CACHE: Duration = Duration::from_secs(30);

/// `guid|Status|PhysicalMediaType|PnPDeviceID|Name` lines from
/// `Get-NetAdapter -Physical`.
pub(crate) fn parse_adapters(out: &str) -> Vec<NetAdapter> {
    out.lines()
        .filter_map(|l| {
            let mut parts = l.trim().splitn(5, '|');
            let g = guid(parts.next()?)?;
            let status = parts.next()?;
            let media = parts.next()?;
            let pnp_id = parts.next()?.trim().to_owned();
            let name = parts.next()?.to_owned();
            Some(NetAdapter {
                guid: g,
                name,
                up: status.eq_ignore_ascii_case("Up"),
                wireless: media.contains("802.11"),
                wired: media.trim() == "802.3",
                pnp_id,
            })
        })
        .collect()
}

/// No NVIDIA card is an answer about this PC ("not available"); anything
/// else is a failed command.
fn nvidia(e: crate::nvapi::NvError) -> EngineError {
    match e {
        crate::nvapi::NvError::NoNvidia => EngineError::Blocked {
            reason: crate::types::BlockedReason::new(
                crate::types::BlockedCode::HardwareUnsupported,
                "This PC has no NVIDIA graphics card.",
            ),
        },
        crate::nvapi::NvError::Failed(detail) => fail("NVIDIA settings", detail),
    }
}

/// No AMD card is an answer about this PC; anything else a failed command.
fn amd(e: crate::adlx::AdlxError) -> EngineError {
    match e {
        crate::adlx::AdlxError::NoAmd => EngineError::Blocked {
            reason: crate::types::BlockedReason::new(
                crate::types::BlockedCode::HardwareUnsupported,
                "This PC has no AMD graphics card.",
            ),
        },
        crate::adlx::AdlxError::Failed(detail) => fail("AMD graphics settings", detail),
    }
}

fn amd_setting(key: &str) -> Result<crate::adlx::Setting> {
    crate::adlx::Setting::from_key(key).ok_or_else(|| {
        fail(
            "AMD graphics settings",
            format!("{key:?} is not a setting PeakTweaks changes"),
        )
    })
}

fn no_such_card(gpu: &str) -> EngineError {
    fail("AMD graphics settings", format!("no AMD graphics card {gpu} was found"))
}

/// Only the base profile (Control Panel's global settings) is changed.
fn need_base_profile(profile: &str) -> Result<()> {
    if profile.is_empty() {
        Ok(())
    } else {
        Err(fail("NVIDIA settings", "only the global profile is changed"))
    }
}

fn fail(what: &str, detail: impl Into<String>) -> EngineError {
    EngineError::Command {
        what: what.into(),
        exit_code: None,
        detail: detail.into(),
    }
}

fn system32(exe: &str) -> Result<PathBuf> {
    // From Windows, not %SystemRoot%, which whoever starts PeakTweaks can set.
    let dir = crate::sysdirs::system32().map_err(|e| fail(exe, format!("could not find the System32 folder: {e}")))?;
    let path = dir.join(exe);
    if path.is_file() {
        Ok(path)
    } else {
        Err(fail(exe, format!("{} does not exist", path.display())))
    }
}

/// Run a System32 tool with arguments; a non-zero exit is an error.
fn tool(exe: &str, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new(system32(exe)?);
    cmd.args(args).creation_flags(CREATE_NO_WINDOW);
    let out = run_limited(cmd, exe, LIMIT, Duration::from_millis(50))?;
    if out.exit_code == Some(0) {
        Ok(out.stdout)
    } else {
        Err(EngineError::Command {
            what: format!("{exe} {}", args.join(" ")),
            exit_code: out.exit_code,
            detail: out.failure_detail(),
        })
    }
}

/// Run a fixed PowerShell script with inputs in `PT_*` environment variables.
fn powershell(what: &str, script: &'static str, env: &[(&str, &str)]) -> Result<String> {
    let mut cmd = Command::new(system32(r"WindowsPowerShell\v1.0\powershell.exe")?);
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = run_limited(cmd, what, LIMIT, Duration::from_millis(50))?;
    if out.exit_code == Some(0) {
        Ok(out.stdout)
    } else {
        Err(EngineError::Command {
            what: what.into(),
            exit_code: out.exit_code,
            detail: out.failure_detail(),
        })
    }
}

fn need_guid(s: &str, what: &str) -> Result<String> {
    guid(s).ok_or_else(|| fail(what, format!("{s:?} is not a GUID")))
}

/// Service names, task paths and the like: letters, digits and a few
/// separators, nothing a tool could read as an option or a path escape.
fn need_name(s: &str, what: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s.len() <= 256
        && !s.starts_with('-')
        && !s.contains("..")
        && s.chars().all(|c| c.is_ascii_alphanumeric() || " _-.\\{}".contains(c));
    if ok {
        Ok(())
    } else {
        Err(fail(what, format!("{s:?} is not a name PeakTweaks accepts")))
    }
}

/// A task's full path from the root folder, as Task Scheduler shows it:
/// `\Folder\Sub\Name`. A name as `need_name` allows, plus the leading `\`.
fn need_task_path(s: &str) -> Result<()> {
    need_name(s, "scheduled task")?;
    if s.starts_with('\\') && !s.ends_with('\\') && !s.contains("\\\\") {
        Ok(())
    } else {
        Err(fail("scheduled task", format!("{s:?} is not a full task path")))
    }
}

fn need_ip(s: &str) -> Result<()> {
    // IPv4 only: the before-state read is the IPv4 NameServer value, so an
    // IPv6 server could not be put back. Windows checks the address itself.
    let ok = !s.is_empty() && s.len() <= 15 && s.chars().all(|c| c.is_ascii_digit() || c == '.');
    if ok {
        Ok(())
    } else {
        Err(fail("DNS servers", format!("{s:?} is not an IP address")))
    }
}

fn wrong_state(item: &SysItem, state: &SysState) -> EngineError {
    EngineError::Internal {
        detail: format!("{state:?} is not a state for the {}", item.describe()),
    }
}

impl SystemBackend for WinSystem {
    fn network_adapters(&self) -> Result<Vec<NetAdapter>> {
        let mut cache = self.adapters.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, list)) = cache.as_ref() {
            if at.elapsed() < ADAPTER_CACHE {
                return Ok(list.clone());
            }
        }
        let out = powershell(
            "network adapters",
            "Get-NetAdapter -Physical | ForEach-Object { '{0}|{1}|{2}|{3}|{4}' -f $_.InterfaceGuid, $_.Status, \
             $_.PhysicalMediaType, $_.PnPDeviceID, $_.Name }",
            &[],
        )?;
        let list = parse_adapters(&out);
        *cache = Some((std::time::Instant::now(), list.clone()));
        Ok(list)
    }

    fn amd_gpus(&self) -> Result<Vec<crate::adlx::AmdGpu>> {
        match self.adlx.gpus() {
            Ok(list) => Ok(list),
            Err(crate::adlx::AdlxError::NoAmd) => Ok(Vec::new()),
            Err(e) => Err(amd(e)),
        }
    }

    fn pci_devices(&self) -> Result<Vec<PciDevice>> {
        let out = powershell(
            "devices",
            "Get-PnpDevice -PresentOnly -Class Display,Net -ErrorAction SilentlyContinue | Where-Object { \
             $_.InstanceId -like 'PCI\\*' } | ForEach-Object { '{0}|{1}|{2}' -f $_.InstanceId, $_.Class, \
             $_.FriendlyName }",
            &[],
        )?;
        Ok(parse_pci_devices(&out))
    }

    fn read(&self, item: &SysItem) -> Result<SysState> {
        match item {
            SysItem::ActivePowerScheme => {
                let out = tool("powercfg.exe", &["/getactivescheme"])?;
                let g = guids_in(&out)
                    .into_iter()
                    .next()
                    .ok_or_else(|| fail("powercfg", "no active power plan in its output"))?;
                Ok(SysState::Text { text: g })
            }
            SysItem::PowerScheme { guid: g } => {
                let g = need_guid(g, "power plan")?;
                let out = tool("powercfg.exe", &["/list"])?;
                Ok(if guids_in(&out).contains(&g) {
                    // Which plan it was copied from is not recorded by
                    // Windows; only presence matters for undo.
                    SysState::Scheme { source: String::new() }
                } else {
                    SysState::Absent
                })
            }
            SysItem::PowerSetting {
                scheme,
                subgroup,
                setting,
                ac,
            } => {
                let (s, sub, set) = (
                    need_guid(scheme, "power plan")?,
                    need_guid(subgroup, "power setting")?,
                    need_guid(setting, "power setting")?,
                );
                // `/qh` also prints hidden settings, such as core parking
                // (VERIFY, NOTES N77: run 37717494960 read it as not set with
                // `/query`); a Windows without `/qh` falls back to `/query`.
                let out = tool("powercfg.exe", &["/qh", &s, &sub, &set])
                    .or_else(|_| tool("powercfg.exe", &["/query", &s, &sub, &set]))?;
                // A setting neither prints counts as not set (PeakTweaks only
                // changes such settings on its own copy, which Undo deletes).
                Ok(match setting_indexes(&out) {
                    Some((a, d)) => SysState::Dword {
                        value: if *ac { a } else { d },
                    },
                    None => SysState::Absent,
                })
            }
            SysItem::Hibernation => {
                // No HibernateEnabled value (Windows Server, or a PC where
                // hibernation was never set up) means hibernation is not on.
                let v = self
                    .reg
                    .read_value(
                        Hive::LocalMachine,
                        r"SYSTEM\CurrentControlSet\Control\Power",
                        "HibernateEnabled",
                    )?
                    .and_then(|v| v.as_dword())
                    .unwrap_or(0);
                Ok(SysState::Bool { on: v != 0 })
            }
            SysItem::Service { name } => {
                need_name(name, "service")?;
                services::read(name)
            }
            SysItem::ScheduledTask { path } => {
                need_task_path(path)?;
                tasks::read(path)
            }
            SysItem::DnsServers { interface } => {
                let g = need_guid(interface, "network adapter")?;
                // The servers set by hand for this adapter (IPv4). Empty means
                // automatic, from the router.
                let key = format!(r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\{{{g}}}");
                let out = match self.reg.read_value(Hive::LocalMachine, &key, "NameServer")? {
                    None => String::new(),
                    Some(v) => v
                        .as_sz()
                        .ok_or_else(|| fail("DNS servers", "NameServer is not a string value"))?,
                };
                let items = out
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                Ok(SysState::List { items })
            }
            SysItem::InterfaceMetric { interface, ipv6 } => {
                let g = need_guid(interface, "network adapter")?;
                Ok(self
                    .metrics()?
                    .into_iter()
                    .find(|m| m.guid == g && m.ipv6 == *ipv6)
                    .map_or(SysState::Absent, |m| SysState::Dword { value: m.value }))
            }
            SysItem::NvidiaSetting { profile, setting } => {
                need_base_profile(profile)?;
                Ok(match self.nvapi.get(*setting).map_err(nvidia)? {
                    Some(value) => SysState::Dword { value },
                    None => SysState::Absent,
                })
            }
            SysItem::AmdSetting { gpu, setting } => {
                match self.adlx.get(gpu, amd_setting(setting)?).map_err(amd)? {
                    Some(Some(value)) => Ok(SysState::Dword { value }),
                    // This card does not have the setting.
                    Some(None) => Ok(SysState::Absent),
                    None => Err(no_such_card(gpu)),
                }
            }
            SysItem::TcpGlobal { name } => Ok(SysState::Text {
                text: self.tcp_global(name)?,
            }),
            SysItem::QosPolicy { name } => {
                need_name(name, "QoS policy")?;
                match self.qos_policy(name)? {
                    None => Ok(SysState::Absent),
                    Some(q) => match u8::try_from(q.dscp).ok().filter(|d| *d <= 63) {
                        Some(dscp) if !q.program.is_empty() => Ok(SysState::QosPolicy {
                            program: q.program,
                            dscp,
                        }),
                        // Not one PeakTweaks could have made, and not one it
                        // could put back as it was.
                        _ => Err(fail(
                            "QoS policy",
                            format!("a policy called {name} is there without a program and a DSCP tag"),
                        )),
                    },
                }
            }
            SysItem::File { .. } => Err(EngineError::Internal {
                detail: "files are read with read_file".into(),
            }),
        }
    }

    fn write(&self, item: &SysItem, state: &SysState) -> Result<()> {
        match (item, state) {
            (SysItem::ActivePowerScheme, SysState::Text { text }) => {
                let g = need_guid(text, "power plan")?;
                tool("powercfg.exe", &["/setactive", &g]).map(drop)
            }
            (SysItem::PowerScheme { guid: g }, SysState::Scheme { source }) => {
                // Already there (a retry, or a rollback putting back what a read
                // reported without its source): nothing to do.
                let g = need_guid(g, "power plan")?;
                if guids_in(&tool("powercfg.exe", &["/list"])?).contains(&g) {
                    return Ok(());
                }
                let src = need_guid(source, "power plan")?;
                tool("powercfg.exe", &["/duplicatescheme", &src, &g])?;
                // Every plan this backend makes is PeakTweaks' own; without a
                // name of its own it would show as a second "Balanced".
                tool("powercfg.exe", &["/changename", &g, "PeakTweaks"]).map(drop)
            }
            (SysItem::PowerScheme { guid: g }, SysState::Absent) => {
                let g = need_guid(g, "power plan")?;
                if !guids_in(&tool("powercfg.exe", &["/list"])?).contains(&g) {
                    return Ok(());
                }
                tool("powercfg.exe", &["/delete", &g]).map(drop)
            }
            (
                SysItem::PowerSetting {
                    scheme,
                    subgroup,
                    setting,
                    ac,
                },
                SysState::Dword { value },
            ) => {
                let (s, sub, set) = (
                    need_guid(scheme, "power plan")?,
                    need_guid(subgroup, "power setting")?,
                    need_guid(setting, "power setting")?,
                );
                let verb = if *ac { "/setacvalueindex" } else { "/setdcvalueindex" };
                tool("powercfg.exe", &[verb, &s, &sub, &set, &value.to_string()]).map(drop)
            }
            // A hidden setting read as not set: nothing to put back.
            (SysItem::PowerSetting { .. }, SysState::Absent) => Ok(()),
            (SysItem::Hibernation, SysState::Bool { on }) => {
                tool("powercfg.exe", &["/hibernate", if *on { "on" } else { "off" }]).map(drop)
            }
            (SysItem::Service { name }, SysState::Service { start, running }) => {
                need_name(name, "service")?;
                services::write(name, *start, *running)
            }
            (SysItem::ScheduledTask { path }, SysState::Bool { on }) => {
                need_task_path(path)?;
                tasks::write(path, *on)
            }
            (SysItem::DnsServers { interface }, SysState::List { items }) => {
                let g = need_guid(interface, "network adapter")?;
                for s in items {
                    need_ip(s)?;
                }
                let joined = items.join(",");
                powershell(
                    "DNS servers",
                    "$a = Get-NetAdapter -IncludeHidden | Where-Object { $_.InterfaceGuid -eq ('{' + $env:PT_GUID + \
                     '}') }; if (-not $a) { throw 'adapter not found' }; \
                     if ($env:PT_DNS) { Set-DnsClientServerAddress -InterfaceIndex $a.ifIndex -ServerAddresses \
                     ($env:PT_DNS -split ',') -ErrorAction Stop } else { Set-DnsClientServerAddress -InterfaceIndex \
                     $a.ifIndex -ResetServerAddresses -ErrorAction Stop }",
                    &[("PT_GUID", &g), ("PT_DNS", &joined)],
                )
                .map(drop)
            }
            (SysItem::InterfaceMetric { interface, ipv6 }, SysState::Dword { value }) => {
                let g = need_guid(interface, "network adapter")?;
                if *value > 9999 {
                    return Err(fail("interface metric", format!("{value} is above 9999")));
                }
                *self.metrics.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                // 0 is Windows' automatic metric. VERIFY (NOTES N83): that
                // Set-NetIPInterface keeps the metric after a restart.
                powershell(
                    "interface metric",
                    "$a = Get-NetAdapter -IncludeHidden | Where-Object { $_.InterfaceGuid -eq ('{' + $env:PT_GUID + \
                     '}') }; if (-not $a) { throw 'adapter not found' }; if ($env:PT_METRIC -eq '0') { \
                     Set-NetIPInterface -InterfaceIndex $a.ifIndex -AddressFamily $env:PT_FAMILY -AutomaticMetric \
                     Enabled -ErrorAction Stop } else { Set-NetIPInterface -InterfaceIndex $a.ifIndex -AddressFamily \
                     $env:PT_FAMILY -AutomaticMetric Disabled -InterfaceMetric ([int]$env:PT_METRIC) -ErrorAction Stop }",
                    &[
                        ("PT_GUID", &g),
                        ("PT_FAMILY", if *ipv6 { "IPv6" } else { "IPv4" }),
                        ("PT_METRIC", &value.to_string()),
                    ],
                )
                .map(drop)
            }
            (SysItem::NvidiaSetting { profile, setting }, SysState::Dword { value }) => {
                need_base_profile(profile)?;
                self.nvapi.set(*setting, Some(*value)).map_err(nvidia)
            }
            (SysItem::NvidiaSetting { profile, setting }, SysState::Absent) => {
                need_base_profile(profile)?;
                self.nvapi.set(*setting, None).map_err(nvidia)
            }
            (SysItem::AmdSetting { gpu, setting }, SysState::Dword { value }) => {
                if self.adlx.set(gpu, amd_setting(setting)?, *value).map_err(amd)? {
                    Ok(())
                } else {
                    Err(no_such_card(gpu))
                }
            }
            // A setting the card does not have: nothing to put back.
            (SysItem::AmdSetting { setting, .. }, SysState::Absent) => amd_setting(setting).map(drop),
            (SysItem::TcpGlobal { name }, SysState::Text { text }) => {
                let value = text.to_ascii_lowercase();
                if !crate::tweaks::tcp::allowed(name).is_some_and(|ok| ok.contains(&value.as_str())) {
                    return Err(fail(
                        "TCP settings",
                        format!("{name}={text:?} is not a setting netsh takes"),
                    ));
                }
                *self.tcp.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                tool(
                    "netsh.exe",
                    &["interface", "tcp", "set", "global", &format!("{name}={value}")],
                )?;
                // netsh can report success for a value it did not keep.
                let now = self.tcp_global(name)?;
                if now == value {
                    Ok(())
                } else {
                    Err(fail(
                        "TCP settings",
                        format!("netsh set {name} to {value}, but Windows reports {now}"),
                    ))
                }
            }
            (SysItem::QosPolicy { name }, SysState::QosPolicy { program, dscp }) => {
                need_name(name, "QoS policy")?;
                need_program(program)?;
                if *dscp > 63 {
                    return Err(fail("QoS policy", format!("DSCP {dscp} is above 63")));
                }
                *self.qos.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                // One name, one policy: one there already is replaced.
                powershell(
                    "QoS policy",
                    "Get-NetQosPolicy -Name $env:PT_NAME -ErrorAction SilentlyContinue | Remove-NetQosPolicy \
                     -Confirm:$false -ErrorAction Stop; New-NetQosPolicy -Name $env:PT_NAME \
                     -AppPathNameMatchCondition $env:PT_PROGRAM -DSCPAction ([sbyte]$env:PT_DSCP) -NetworkProfile \
                     All -IPProtocolMatchCondition Both -ErrorAction Stop | Out-Null",
                    &[
                        ("PT_NAME", name),
                        ("PT_PROGRAM", program),
                        ("PT_DSCP", &dscp.to_string()),
                    ],
                )?;
                self.check_qos(item, state)
            }
            (SysItem::QosPolicy { name }, SysState::Absent) => {
                need_name(name, "QoS policy")?;
                *self.qos.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                powershell(
                    "QoS policy",
                    "Get-NetQosPolicy -Name $env:PT_NAME -ErrorAction SilentlyContinue | Remove-NetQosPolicy \
                     -Confirm:$false -ErrorAction Stop",
                    &[("PT_NAME", name)],
                )?;
                self.check_qos(item, state)
            }
            (item, state) => Err(wrong_state(item, state)),
        }
    }

    fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match std::fs::read(path) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(EngineError::storage(path.to_owned(), e)),
        }
    }

    fn write_file(&self, path: &str, bytes: Option<&[u8]>) -> Result<()> {
        match bytes {
            Some(b) => crate::fsutil::write_durable(std::path::Path::new(path), b),
            None => match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(EngineError::storage(path.to_owned(), e)),
            },
        }
    }

    fn run(&self, effect: &SideEffect) -> Result<()> {
        match effect {
            SideEffect::RestartAdapter { interface } => {
                let g = need_guid(interface, "network adapter")?;
                powershell(
                    "restart network adapter",
                    // A disabled adapter stays disabled: Restart-NetAdapter
                    // would turn it on, and it reads its settings when it is.
                    "Get-NetAdapter -IncludeHidden | Where-Object { $_.InterfaceGuid -eq ('{' + $env:PT_GUID + '}') \
                     -and $_.Status -ne 'Disabled' } | Restart-NetAdapter -Confirm:$false -ErrorAction Stop",
                    &[("PT_GUID", &g)],
                )
                .map(drop)
            }
            SideEffect::RefreshPolicy => tool("gpupdate.exe", &["/target:computer", "/force"]).map(drop),
            SideEffect::RestartService { name } => {
                need_name(name, "service")?;
                let now = services::read(name)?;
                if let SysState::Service { start, .. } = now {
                    services::write(name, start, false)?;
                    services::write(name, start, true)?;
                }
                Ok(())
            }
        }
    }
}

mod services {
    //! Services through the Win32 service API: start type and running state.

    use std::time::{Duration, Instant};

    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::System::Services::*;

    use super::super::error::{EngineError, Result};
    use super::super::system::{ServiceStart, SysState};

    struct Handle(SC_HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: a handle we opened and have not closed.
            unsafe {
                let _ = CloseServiceHandle(self.0);
            }
        }
    }

    fn err(name: &str, what: &str, e: windows::core::Error) -> EngineError {
        EngineError::Command {
            what: format!("service {name}"),
            exit_code: None,
            detail: format!("{what}: {e}"),
        }
    }

    fn open(name: &str, access: u32) -> Result<(Handle, Handle)> {
        // SAFETY: plain Win32 calls with owned, NUL-terminated strings.
        unsafe {
            let scm = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
                .map_err(|e| err(name, "could not open the service manager", e))?;
            let scm = Handle(scm);
            let svc = OpenServiceW(scm.0, &HSTRING::from(name), access)
                .map_err(|e| err(name, "could not open the service", e))?;
            Ok((scm, Handle(svc)))
        }
    }

    fn status(svc: &Handle, name: &str) -> Result<SERVICE_STATUS_CURRENT_STATE> {
        let mut st = SERVICE_STATUS::default();
        // SAFETY: valid handle, valid out-pointer.
        unsafe { QueryServiceStatus(svc.0, &mut st) }.map_err(|e| err(name, "could not read its status", e))?;
        Ok(st.dwCurrentState)
    }

    pub fn read(name: &str) -> Result<SysState> {
        let (_scm, svc) = open(name, SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS)?;
        let mut needed = 0u32;
        // SAFETY: the first call asks for the size; the buffer is u64-aligned
        // and at least that size for the second.
        let start = unsafe {
            let _ = QueryServiceConfigW(svc.0, None, 0, &mut needed);
            let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(8)];
            let cfg = buf.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
            QueryServiceConfigW(svc.0, Some(cfg), (buf.len() * 8) as u32, &mut needed)
                .map_err(|e| err(name, "could not read its configuration", e))?;
            (*cfg).dwStartType
        };
        let delayed = if start == SERVICE_AUTO_START {
            let mut buf = [0u8; 16];
            let mut needed = 0u32;
            // SAFETY: the struct is one BOOL; 16 bytes is enough.
            unsafe {
                QueryServiceConfig2W(
                    svc.0,
                    SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                    Some(&mut buf),
                    &mut needed,
                )
                .map_err(|e| err(name, "could not read its delayed start", e))?;
            }
            u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) != 0
        } else {
            false
        };
        let start = match start {
            SERVICE_BOOT_START => ServiceStart::Boot,
            SERVICE_SYSTEM_START => ServiceStart::System,
            SERVICE_AUTO_START if delayed => ServiceStart::DelayedAutomatic,
            SERVICE_AUTO_START => ServiceStart::Automatic,
            SERVICE_DEMAND_START => ServiceStart::Manual,
            _ => ServiceStart::Disabled,
        };
        let running = matches!(status(&svc, name)?, s if s == SERVICE_RUNNING || s == SERVICE_START_PENDING);
        Ok(SysState::Service { start, running })
    }

    pub fn write(name: &str, start: ServiceStart, running: bool) -> Result<()> {
        let (_scm, svc) = open(
            name,
            SERVICE_QUERY_CONFIG | SERVICE_CHANGE_CONFIG | SERVICE_QUERY_STATUS | SERVICE_START | SERVICE_STOP,
        )?;
        let (kind, delayed) = match start {
            ServiceStart::Boot => (SERVICE_BOOT_START, false),
            ServiceStart::System => (SERVICE_SYSTEM_START, false),
            ServiceStart::Automatic => (SERVICE_AUTO_START, false),
            ServiceStart::DelayedAutomatic => (SERVICE_AUTO_START, true),
            ServiceStart::Manual => (SERVICE_DEMAND_START, false),
            ServiceStart::Disabled => (SERVICE_DISABLED, false),
        };
        // SAFETY: valid handle; SERVICE_NO_CHANGE and nulls leave every other
        // field as it is.
        unsafe {
            ChangeServiceConfigW(
                svc.0,
                ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                kind,
                SERVICE_ERROR(SERVICE_NO_CHANGE),
                PCWSTR::null(),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
            )
            .map_err(|e| err(name, "could not change how it starts", e))?;
            if kind == SERVICE_AUTO_START {
                let info = SERVICE_DELAYED_AUTO_START_INFO {
                    fDelayedAutostart: BOOL::from(delayed),
                };
                ChangeServiceConfig2W(
                    svc.0,
                    SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                    Some(std::ptr::from_ref(&info).cast()),
                )
                .map_err(|e| err(name, "could not change its delayed start", e))?;
            }
        }
        let now = status(&svc, name)?;
        let is_running = now == SERVICE_RUNNING || now == SERVICE_START_PENDING;
        if running && !is_running {
            // SAFETY: valid handle, no arguments.
            unsafe { StartServiceW(svc.0, None) }.map_err(|e| err(name, "could not start it", e))?;
            wait_for(&svc, name, SERVICE_RUNNING)?;
        } else if !running && is_running {
            let mut st = SERVICE_STATUS::default();
            // SAFETY: valid handle, valid out-pointer.
            unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st) }
                .map_err(|e| err(name, "could not stop it", e))?;
            wait_for(&svc, name, SERVICE_STOPPED)?;
        }
        Ok(())
    }

    fn wait_for(svc: &Handle, name: &str, want: SERVICE_STATUS_CURRENT_STATE) -> Result<()> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(30) {
            if status(svc, name)? == want {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(EngineError::Command {
            what: format!("service {name}"),
            exit_code: None,
            detail: "did not reach the wanted state within 30 seconds".into(),
        })
    }
}

mod tasks {
    //! Scheduled tasks through the Task Scheduler COM API: whether a task is
    //! enabled. Not localized, and no PowerShell start on every refresh.
    //! Each call runs on a short-lived thread of its own in the multithreaded
    //! apartment, so the caller's COM state (the Tauri main thread is a
    //! single-threaded apartment) never matters.

    use windows::core::{BSTR, VARIANT};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, VARIANT_FALSE, VARIANT_TRUE};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::TaskScheduler::{IRegisteredTask, ITaskService, TaskScheduler};

    use super::super::error::{EngineError, Result};
    use super::super::system::SysState;

    fn err(path: &str, what: &str, e: windows::core::Error) -> EngineError {
        EngineError::Command {
            what: format!("scheduled task {path}"),
            exit_code: None,
            detail: format!("{what}: {e}"),
        }
    }

    fn is_missing(e: &windows::core::Error) -> bool {
        e.code() == ERROR_FILE_NOT_FOUND.to_hresult() || e.code() == ERROR_PATH_NOT_FOUND.to_hresult()
    }

    /// Run `f` with the task at `path`, or `None` when there is no such task.
    fn with_task<T: Send + 'static>(
        path: &str,
        f: impl FnOnce(Option<IRegisteredTask>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let path = path.to_owned();
        std::thread::spawn(move || {
            // SAFETY: COM is started on this new thread, and stopped only after
            // every interface it handed out has been dropped (they live inside
            // the inner closure).
            unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED)
                    .ok()
                    .map_err(|e| err(&path, "could not start COM", e))?;
                let result = (|| {
                    let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)
                        .map_err(|e| err(&path, "could not reach the Task Scheduler", e))?;
                    let local = VARIANT::default();
                    svc.Connect(&local, &local, &local, &local)
                        .map_err(|e| err(&path, "could not connect to the Task Scheduler", e))?;
                    let root = svc
                        .GetFolder(&BSTR::from("\\"))
                        .map_err(|e| err(&path, "could not open the task folders", e))?;
                    let task = match root.GetTask(&BSTR::from(path.as_str())) {
                        Ok(t) => Some(t),
                        Err(e) if is_missing(&e) => None,
                        Err(e) => return Err(err(&path, "could not open it", e)),
                    };
                    f(task)
                })();
                CoUninitialize();
                result
            }
        })
        .join()
        .unwrap_or_else(|_| {
            Err(EngineError::Internal {
                detail: "the scheduled task thread stopped unexpectedly".into(),
            })
        })
    }

    /// `Bool { on }` for a task that exists, `Absent` for one that does not.
    pub fn read(path: &str) -> Result<SysState> {
        let p = path.to_owned();
        with_task(path, move |task| match task {
            None => Ok(SysState::Absent),
            // SAFETY: a valid interface pointer.
            Some(t) => unsafe { t.Enabled() }
                .map(|on| SysState::Bool {
                    on: on != VARIANT_FALSE,
                })
                .map_err(|e| err(&p, "could not read whether it is enabled", e)),
        })
    }

    pub fn write(path: &str, on: bool) -> Result<()> {
        let p = path.to_owned();
        with_task(path, move |task| {
            let t = task.ok_or_else(|| EngineError::Command {
                what: format!("scheduled task {p}"),
                exit_code: None,
                detail: "there is no such task".into(),
            })?;
            // SAFETY: a valid interface pointer.
            unsafe { t.SetEnabled(if on { VARIANT_TRUE } else { VARIANT_FALSE }) }.map_err(|e| {
                err(
                    &p,
                    if on {
                        "could not enable it"
                    } else {
                        "could not disable it"
                    },
                    e,
                )
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_could_be_read_as_options_or_paths_are_refused() {
        assert!(need_name("WSearch", "t").is_ok());
        assert!(need_name(r"\Microsoft\Windows\Application Experience\ProgramDataUpdater", "t").is_ok());
        for bad in ["", "-delete", r"..\x", "a;b", "a&b", "a\"b", "a|b"] {
            assert!(need_name(bad, "t").is_err(), "{bad:?}");
        }
        assert!(need_ip("1.1.1.1").is_ok());
        assert!(
            need_ip("2606:4700:4700::1111").is_err(),
            "IPv6 could not be put back yet"
        );
        assert!(need_ip("1.1.1.1; rm").is_err());
        assert!(need_task_path(r"\Microsoft\Windows\Autochk\Proxy").is_ok());
        for bad in ["Proxy", r"\Microsoft\Windows\", r"\Microsoft\\Proxy", r"\..\x", "*"] {
            assert!(need_task_path(bad).is_err(), "{bad:?}");
        }
    }

    /// Real output from Kegan's PC (2026-10-07), with device ids in the
    /// form Windows gives them added since.
    #[test]
    fn adapters_are_parsed_from_the_listing() {
        let out = "{4D86B570-2994-4EB0-A004-914EF65FF05A}|Disconnected|Native 802.11|\
                   PCI\\VEN_8086&DEV_2725&SUBSYS_00248086&REV_1A\\4&1B2C3D4E&0&00E4|Wi-Fi\r\n\
                   {3F504232-CECB-4118-B4D8-5A5E72D677C3}|Up|802.3|\
                   PCI\\VEN_10EC&DEV_8125&SUBSYS_86771043&REV_05\\01000000684CE00000|Ethernet | home\r\n";
        let a = parse_adapters(out);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].guid, "4d86b570-2994-4eb0-a004-914ef65ff05a");
        assert!(a[0].wireless && !a[0].wired && !a[0].up);
        assert_eq!(
            a[0].pnp_id,
            r"PCI\VEN_8086&DEV_2725&SUBSYS_00248086&REV_1A\4&1B2C3D4E&0&00E4"
        );
        assert!(!a[1].wireless && a[1].wired && a[1].up && a[1].name == "Ethernet | home");
    }

    #[test]
    fn pci_graphics_and_network_devices_are_parsed_and_others_skipped() {
        let out =
            "PCI\\VEN_10DE&DEV_2484&SUBSYS_146710DE&REV_A1\\4&2B0B1F0C&0&0008|Display|NVIDIA GeForce RTX 3060 Ti\r\n\
                   PCI\\VEN_10EC&DEV_8125&SUBSYS_86771043&REV_05\\01000000684CE00000|Net|Realtek Gaming 2.5GbE\r\n\
                   PCI\\VEN_8086&DEV_A0F0&SUBSYS_00748086&REV_20\\3&11583659&0&A3|Net|\r\n\
                   PCI\\VEN_1022&DEV_1483\\3&2411E6FE&0&09|System|PCI bridge\r\n\
                   USB\\VID_0BDA&PID_8153\\000001|Net|USB Ethernet\r\n";
        let d = parse_pci_devices(out);
        assert_eq!(d.len(), 3, "{d:?}");
        assert_eq!(d[0].class, DeviceClass::Display);
        assert_eq!(d[0].name, "NVIDIA GeForce RTX 3060 Ti");
        assert_eq!(d[1].class, DeviceClass::Net);
        assert_eq!(
            d[2].name, "",
            "a device without a name is kept; the app names it by its maker"
        );
    }

    /// Read-only, against this PC: NVIDIA's power management mode in the
    /// global profile, or "no NVIDIA graphics card" as a block, never a failed
    /// command (a runner has no NVIDIA card).
    #[test]
    fn nvidia_settings_read_or_say_there_is_no_nvidia_card() {
        let item = SysItem::NvidiaSetting {
            profile: String::new(),
            setting: crate::tweaks::nvidia::POWER_MANAGEMENT,
        };
        let read = WinSystem::new().read(&item);
        println!("NVIDIA power management mode: {read:?}");
        match read {
            Ok(SysState::Dword { .. } | SysState::Absent) => {}
            Err(EngineError::Blocked { reason }) => {
                assert_eq!(reason.code, crate::types::BlockedCode::HardwareUnsupported)
            }
            other => panic!("{other:?}"),
        }
    }

    /// Read-only, against this PC: the AMD cards and their settings, or an
    /// empty list (a runner has no AMD card), never a failed command.
    #[test]
    fn amd_cards_are_listed_or_there_are_none() {
        let s = WinSystem::new();
        let gpus = s.amd_gpus();
        println!("AMD graphics cards: {gpus:?}");
        for g in gpus.as_ref().unwrap() {
            for setting in crate::adlx::Setting::ALL {
                let item = SysItem::AmdSetting {
                    gpu: g.id.clone(),
                    setting: setting.key().into(),
                };
                println!("{}: {:?}", item.describe(), s.read(&item));
            }
        }
    }

    /// A resolver and an engine on this PC's real registry and system.
    fn real_engine(
        dir: &std::path::Path,
        tweaks: Vec<Box<dyn crate::types::Tweak>>,
    ) -> (crate::context::ContextResolver, crate::engine::Engine) {
        use std::sync::Arc;

        use crate::context::{ContextResolver, UserContext, UserResolution};
        use crate::engine::Engine;
        use crate::env::{License, StubProbe};
        use crate::journal::Journal;
        use crate::secure_dir::TrustedDir;
        use crate::types::Tier;

        let resolver = || {
            let user = UserContext {
                sid: crate::identity::current_process_sid().unwrap(),
                resolution: UserResolution::OwnToken,
                is_self: true,
            };
            ContextResolver::new(user, crate::identity::is_elevated(), Arc::new(WinRegistry::new()))
                .with_system(Arc::new(WinSystem::new()))
        };
        let engine = Engine::new(
            resolver(),
            Journal::open(&TrustedDir::insecure_for_tests(dir)).unwrap(),
            tweaks,
            Box::new(StubProbe::open_for_dev()),
            License::dev(Tier::Ultimate),
        );
        (resolver(), engine)
    }

    /// Read-only, against this PC: the state of every tool in the catalogue,
    /// read the way the Tools screen reads it. A tool whose read fails shows
    /// "could not be read" there, so none may.
    #[test]
    fn every_tool_reads_its_state_on_this_pc() {
        use crate::types::TweakState;

        let dir = tempfile::tempdir().unwrap();
        let (_, engine) = real_engine(dir.path(), crate::tweaks::catalogue());
        let list = engine.list().unwrap();
        let mut unread = Vec::new();
        for v in &list {
            let id = &v.metadata.id;
            match &v.state {
                TweakState::Blocked { reason } => println!("{id}: blocked ({:?}) {}", reason.code, reason.message),
                TweakState::Unknown { detail } => {
                    println!("{id}: COULD NOT BE READ: {detail}");
                    unread.push(id.to_string());
                }
                other => println!("{id}: {other:?}"),
            }
        }
        println!("{} tools, {} could not be read", list.len(), unread.len());
        assert!(unread.is_empty(), "could not be read here: {unread:?}");
    }

    /// What a tool's registry targets hold, one line per key and value. Keys
    /// with a `*` segment are left out: the tools that have them find their
    /// keys at run time (one per network interface or device).
    fn registry_snapshot(res: &crate::context::ContextResolver, targets: &[crate::types::RegTarget]) -> Vec<String> {
        use crate::registry::value_name_matches;

        let mut out = Vec::new();
        for t in targets {
            if t.key.split('\\').any(|c| c == "*") {
                continue;
            }
            let shown = res.display_path(t.root, &t.key);
            match res.key_exists(t.root, &t.key) {
                Ok(true) => {}
                Ok(false) => {
                    out.push(format!("{shown}: no key"));
                    continue;
                }
                Err(e) => {
                    out.push(format!("{shown}: error {e}"));
                    continue;
                }
            }
            let names = res.value_names(t.root, &t.key).unwrap_or_default();
            for pattern in &t.values {
                let mut found = false;
                for n in names.iter().filter(|n| value_name_matches(pattern, n)) {
                    found = true;
                    out.push(format!("{shown}\\{n} = {:?}", res.read_raw(t.root, &t.key, n)));
                }
                if !found {
                    out.push(format!("{shown}\\{pattern}: absent"));
                }
            }
        }
        out
    }

    /// The lines of two registry snapshots that differ.
    fn snapshot_changes(before: &[String], after: &[String]) -> Vec<String> {
        let gone = before
            .iter()
            .filter(|l| !after.contains(l))
            .map(|l| format!("was: {l}"));
        let new = after
            .iter()
            .filter(|l| !before.contains(l))
            .map(|l| format!("now: {l}"));
        gone.chain(new).collect()
    }

    /// Changes this PC's registry, then puts it back: each tool that only
    /// writes registry values (no Windows command, no device list) is applied
    /// and undone through the engine, one at a time, and then all together
    /// with Undo all. After each undo the tool must read as it did before and
    /// every value it may write must hold what it held before, byte for byte,
    /// with any key the apply created removed again. Tools this PC cannot
    /// take (blocked, already set) are named and skipped.
    #[test]
    fn every_registry_tool_applies_and_undoes_on_this_pc() {
        use crate::types::TweakState;

        if std::env::var("PEAKTWEAKS_REAL_SYSTEM_CHANGES").as_deref() != Ok("1") {
            println!("SKIPPED: set PEAKTWEAKS_REAL_SYSTEM_CHANGES=1 to change this PC's registry for real");
            return;
        }

        let registry_only: Vec<Box<dyn crate::types::Tweak>> = crate::tweaks::catalogue()
            .into_iter()
            .filter(|t| t.system_targets().is_empty() && t.effect_targets().is_empty() && !t.touches().is_empty())
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let (res, mut engine) = real_engine(dir.path(), crate::tweaks::catalogue());
        // Read through the tool itself, as the engine's list does, without
        // listing the whole catalogue after every step.
        let state_of = |engine: &crate::engine::Engine, t: &dyn crate::types::Tweak| -> TweakState {
            let journalled = engine.applied_tweak_ids().iter().any(|a| a == t.id());
            t.read_state(&res, journalled)
                .unwrap_or_else(|e| TweakState::Unknown { detail: e.to_string() })
        };
        let listed = engine.list().unwrap();

        let mut tried = Vec::new();
        let mut failed = Vec::new();
        for tweak in &registry_only {
            let id = tweak.id();
            let targets = &tweak.touches();
            let view = listed.iter().find(|v| v.metadata.id == id).expect("listed");
            if let Some(reason) = &view.blocked {
                println!("{id}: skipped, blocked here ({:?}) {}", reason.code, reason.message);
                continue;
            }
            if view.state != TweakState::Default {
                println!("{id}: skipped, reads {:?} here", view.state);
                continue;
            }
            let before = registry_snapshot(&res, targets);
            if let Err(e) = engine.apply(id) {
                println!("{id}: APPLY FAILED: {e}");
                failed.push(format!("{id}: apply: {e}"));
                continue;
            }
            let applied = state_of(&engine, tweak.as_ref());
            let during = registry_snapshot(&res, targets);
            if let Err(e) = engine.revert(id) {
                println!("{id}: UNDO FAILED: {e}");
                failed.push(format!("{id}: undo: {e}"));
                continue;
            }
            let after_state = state_of(&engine, tweak.as_ref());
            let after = registry_snapshot(&res, targets);
            let mut problems = Vec::new();
            if applied != TweakState::Applied {
                problems.push(format!("read {applied:?} after Apply"));
            }
            if before.is_empty() {
                println!("{id}: its keys are found at run time, so only its state is compared");
            } else if during == before {
                problems.push("Apply changed none of its values".to_owned());
            }
            if after_state != TweakState::Default {
                problems.push(format!("read {after_state:?} after Undo"));
            }
            problems.extend(snapshot_changes(&before, &after));
            if problems.is_empty() {
                println!("{id}: applied and undone, {} values as before", before.len());
                tried.push(id.to_owned());
            } else {
                for p in &problems {
                    println!("{id}: {p}");
                }
                failed.push(format!("{id}: {}", problems.join("; ")));
            }
        }

        // All together, then Undo all.
        let all_targets: Vec<crate::types::RegTarget> = registry_only
            .iter()
            .filter(|t| tried.iter().any(|id| id == t.id()))
            .flat_map(|t| t.touches())
            .collect();
        let before = registry_snapshot(&res, &all_targets);
        for id in &tried {
            if let Err(e) = engine.apply(id) {
                failed.push(format!("{id}: apply with the others: {e}"));
            }
        }
        for r in engine.revert_all() {
            println!("Undo all: {r:?}");
        }
        assert!(
            engine.applied_tweak_ids().is_empty(),
            "left applied: {:?}",
            engine.applied_tweak_ids()
        );
        let after = registry_snapshot(&res, &all_targets);
        failed.extend(
            snapshot_changes(&before, &after)
                .into_iter()
                .map(|c| format!("Undo all: {c}")),
        );

        println!(
            "{} registry tools, {} applied and undone here, {} failed",
            registry_only.len(),
            tried.len(),
            failed.len()
        );
        assert!(failed.is_empty(), "failed: {failed:#?}");
        assert!(
            tried.len() >= 10,
            "only {} tools could be tried here: {tried:?}",
            tried.len()
        );
    }

    /// Read-only, against this PC: the graphics cards and network adapters
    /// MSI mode would be offered for, each with its state.
    #[test]
    fn msi_mode_lists_this_pcs_devices() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut engine) = real_engine(dir.path(), Vec::new());
        let list = engine.msi_devices();
        for d in &list.devices {
            println!("{:?} {:?}: {:?}", d.class, d.tweak.metadata.name, d.tweak.state);
        }
        assert_eq!(list.problem, None);
    }

    /// Against this PC: each adapter's driver key and the state of each
    /// network adapter setting (catalogue H24). With
    /// PEAKTWEAKS_REAL_SYSTEM_CHANGES=1, power saving is also turned off and
    /// back through the engine; it restarts no adapter, so the runner keeps
    /// its connection.
    #[test]
    fn network_adapter_settings_read_on_this_pc() {
        use crate::tweaks::adapter_props::{self, driver_key, POWER_SAVING};
        use crate::types::{RawValue, RegRoot, TweakState};

        let dir = tempfile::tempdir().unwrap();
        let (res, mut engine) = real_engine(dir.path(), adapter_props::all());
        let keys: Vec<String> = res
            .network_adapters()
            .unwrap()
            .iter()
            .filter_map(|a| {
                let key = driver_key(&res, a);
                println!("{} ({}): driver key {key:?}", a.name, a.pnp_id);
                key.unwrap()
            })
            .collect();
        let states = |engine: &crate::engine::Engine| -> Vec<(String, TweakState)> {
            engine
                .list()
                .unwrap()
                .into_iter()
                .map(|v| (v.metadata.id.into_owned(), v.state))
                .collect()
        };
        let before = states(&engine);
        println!("states: {before:?}");
        for (id, state) in &before {
            assert!(!matches!(state, TweakState::Unknown { .. }), "{id} could not be read");
        }

        if std::env::var("PEAKTWEAKS_REAL_SYSTEM_CHANGES").as_deref() != Ok("1") {
            println!("SKIPPED: set PEAKTWEAKS_REAL_SYSTEM_CHANGES=1 to turn adapter power saving off and back");
            return;
        }
        let power = |res: &crate::context::ContextResolver| -> Vec<Option<RawValue>> {
            keys.iter()
                .map(|k| res.read_raw(RegRoot::LocalMachine, k, POWER_SAVING.value_name).unwrap())
                .collect()
        };
        let (pc_before, state_before) = (
            power(&res),
            &before.iter().find(|(id, _)| id == POWER_SAVING.id).unwrap().1,
        );
        println!("PnPCapabilities before: {pc_before:?}");
        if !matches!(state_before, TweakState::Default) {
            println!("SKIPPED: power saving reads {state_before:?} here, so there is nothing to turn off");
            return;
        }
        engine.apply(POWER_SAVING.id).unwrap();
        let during = power(&res);
        println!("PnPCapabilities after apply: {during:?}");
        assert!(during.iter().all(|v| v
            .as_ref()
            .and_then(RawValue::as_dword)
            .is_some_and(|d| d & 0x18 == 0x18)));
        engine.revert(POWER_SAVING.id).unwrap();
        let after = power(&res);
        println!("PnPCapabilities after Undo: {after:?}");
        assert_eq!(after, pc_before);
        assert_eq!(states(&engine), before);
    }

    #[test]
    fn tcp_settings_are_parsed_in_lower_case_and_only_values_netsh_takes_are_kept() {
        let out = "autotuninglevel|Normal\r\nrss|Enabled\r\necncapability|Disabled\r\n\
                   chimney|Enabled\r\necncapability|Sometimes\r\nrss\r\n";
        assert_eq!(
            parse_tcp_globals(out),
            [
                ("autotuninglevel".to_owned(), "normal".to_owned()),
                ("rss".to_owned(), "enabled".to_owned()),
                ("ecncapability".to_owned(), "disabled".to_owned()),
            ]
        );
    }

    /// TCP settings (catalogue H23) on this PC: what `netsh` and PowerShell
    /// each report, then (with PEAKTWEAKS_REAL_SYSTEM_CHANGES=1) auto-tuning
    /// and ECN each set to another value and back. RSS is only read: turning
    /// it off can restart the runner's adapters.
    #[test]
    fn tcp_settings_are_set_and_put_back_on_this_pc() {
        let s = WinSystem::new();
        let show = || tool("netsh.exe", &["interface", "tcp", "show", "global"]);
        println!("netsh before: {:?}", show());
        println!(
            "Get-NetTCPSetting: {:?}",
            powershell(
                "TCP templates",
                "Get-NetTCPSetting | ForEach-Object { '{0}|{1}|{2}' -f $_.SettingName, $_.AutoTuningLevelLocal, \
                 $_.EcnCapability }",
                &[]
            )
        );
        let item = |name: &str| SysItem::TcpGlobal { name: name.into() };
        for (name, _) in crate::tweaks::tcp::SETTINGS {
            println!("{name}: {:?}", s.read(&item(name)).unwrap());
        }
        if std::env::var("PEAKTWEAKS_REAL_SYSTEM_CHANGES").as_deref() != Ok("1") {
            println!("SKIPPED: set PEAKTWEAKS_REAL_SYSTEM_CHANGES=1 to change TCP settings for real");
            return;
        }
        for (name, a, b) in [
            ("autotuninglevel", "normal", "restricted"),
            ("ecncapability", "enabled", "disabled"),
        ] {
            let before = s.read(&item(name)).unwrap();
            let other = if before == (SysState::Text { text: a.into() }) {
                b
            } else {
                a
            };
            s.write(&item(name), &SysState::Text { text: other.into() }).unwrap();
            println!("netsh with {name}={other}: {:?}", show());
            assert_eq!(s.read(&item(name)).unwrap(), SysState::Text { text: other.into() });
            s.write(&item(name), &before).unwrap();
            assert_eq!(s.read(&item(name)).unwrap(), before, "{name} put back");
        }
        println!("netsh after: {:?}", show());
    }

    /// Game traffic priority (catalogue H26) on this PC, through this
    /// backend as the tool uses it: a policy for a program that does not
    /// exist is added, must read back and show in Windows' active QoS
    /// policies with DSCP 46, and is removed again. Where Windows keeps it in
    /// the registry is printed (for the offline undo, NOTES N91). Run only
    /// with PEAKTWEAKS_REAL_SYSTEM_CHANGES=1.
    #[test]
    fn a_qos_policy_is_added_and_removed_on_this_pc() {
        if std::env::var("PEAKTWEAKS_REAL_SYSTEM_CHANGES").as_deref() != Ok("1") {
            println!("SKIPPED: set PEAKTWEAKS_REAL_SYSTEM_CHANGES=1 to add a QoS policy for real");
            return;
        }
        const ACTIVE: &str = "Get-NetQosPolicy -PolicyStore ActiveStore -ErrorAction SilentlyContinue | \
             ForEach-Object { '{0}|{1}|{2}|{3}' -f $_.Name, $_.AppPathNameMatchCondition, $_.DSCPAction, \
             $_.NetworkProfile }; exit 0";
        let active = || powershell("active QoS policies", ACTIVE, &[]).unwrap();
        // Every registry key under these whose name, value names or data
        // mention the policy (reg.exe exits 1 on none).
        let find = || -> Vec<String> {
            let mut keys = Vec::new();
            for root in [
                r"HKLM\SOFTWARE\Policies\Microsoft\Windows",
                r"HKLM\SYSTEM\CurrentControlSet\Services",
                r"HKLM\SYSTEM\CurrentControlSet\Control",
            ] {
                match tool("reg.exe", &["query", root, "/f", "PeakTweaks CI check", "/s"]) {
                    Ok(out) => keys.extend(out.lines().filter(|l| l.starts_with("HKEY_")).map(str::to_owned)),
                    Err(EngineError::Command { exit_code: Some(1), .. }) => {}
                    Err(e) => println!("searching {root}: {e}"),
                }
            }
            keys
        };
        let s = WinSystem::new();
        let item = SysItem::QosPolicy {
            name: "PeakTweaks CI check".into(),
        };
        let wanted = SysState::QosPolicy {
            program: "peaktweaks-ci-check.exe".into(),
            dscp: 46,
        };
        let before = s.read(&item);
        println!("before: {before:?}; active: {:?}", active());
        assert_eq!(before.unwrap(), SysState::Absent);

        let added = s.write(&item, &wanted);
        let read = s.read(&item);
        let listed = active();
        println!("added: {added:?}; reads {read:?}");
        println!("active with it: {listed:?}");
        println!("in the registry: {:?}", find());
        // The values Windows keeps for it: each must be one the change's
        // `.reg` backup covers (`SysItem::registry_backing`).
        let values = tool(
            "reg.exe",
            &[
                "query",
                r"HKLM\SOFTWARE\Policies\Microsoft\Windows\QoS\PeakTweaks CI check",
            ],
        );
        println!("its values: {values:?}");
        let removed = s.write(&item, &SysState::Absent);
        let after = s.read(&item);
        let listed_after = active();
        println!("removed: {removed:?}; reads {after:?}; active: {listed_after:?}");
        let left = find();
        println!("in the registry after: {left:?}");

        added.unwrap();
        assert_eq!(read.unwrap(), wanted);
        let backed: Vec<&str> = item.registry_backing().iter().map(|(_, v)| *v).collect();
        for line in values.unwrap().lines().filter(|l| l.contains("    REG_")) {
            let name = line.trim().split("    ").next().unwrap_or_default();
            assert!(backed.contains(&name), "{name} is not in the .reg backup: {line}");
        }
        // The active store lists names in lower case (run 37785699537).
        let ours = |out: &str| {
            out.lines()
                .map(str::to_ascii_lowercase)
                .find(|l| l.starts_with("peaktweaks ci check|"))
        };
        let line = ours(&listed).expect("Windows applies the policy");
        assert!(line.contains("peaktweaks-ci-check.exe|46|"), "{line}");
        removed.unwrap();
        assert_eq!(after.unwrap(), SysState::Absent);
        assert!(ours(&listed_after).is_none(), "{listed_after}");
        assert!(left.is_empty(), "left in the registry: {left:?}");
    }

    #[test]
    fn qos_policies_are_parsed_with_their_program_and_tag() {
        let out = "PeakTweaks RobloxPlayerBeta|RobloxPlayerBeta.exe|46\r\n\
                   Throttle only||-1\r\n\
                   odd line\r\n\
                   A|B|C|x.exe|10\r\n";
        let list = parse_qos_policies(out);
        assert_eq!(
            list,
            [
                QosPolicy {
                    name: "PeakTweaks RobloxPlayerBeta".into(),
                    program: "RobloxPlayerBeta.exe".into(),
                    dscp: 46
                },
                QosPolicy {
                    name: "Throttle only".into(),
                    program: String::new(),
                    dscp: -1
                },
                QosPolicy {
                    name: "A|B|C".into(),
                    program: "x.exe".into(),
                    dscp: 10
                },
            ]
        );
        assert!(need_program("RobloxPlayerBeta.exe").is_ok());
        assert!(need_program("FortniteClient-Win64-Shipping.exe").is_ok());
        for bad in [r"C:\Games\x.exe", "x.bat", "-x.exe", "a;b.exe", "$env:x.exe", ".exe"] {
            assert!(need_program(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn interface_metrics_are_parsed_with_automatic_as_zero() {
        let out = "{3F504232-CECB-4118-B4D8-5A5E72D677C3}|IPv6|Enabled|25\r\n\
                   {3F504232-CECB-4118-B4D8-5A5E72D677C3}|IPv4|Disabled|5\r\n\
                   {4D86B570-2994-4EB0-A004-914EF65FF05A}|IPv4|Enabled|abc\r\n\
                   {4D86B570-2994-4EB0-A004-914EF65FF05A}|IPv4|Sometimes|5\r\n\
                   {4D86B570-2994-4EB0-A004-914EF65FF05A}|23|1|40\r\n\
                   {4D86B570-2994-4EB0-A004-914EF65FF05A}|2|0|9\r\n\
                   garbage\r\n";
        let m = parse_metrics(out);
        assert_eq!(
            m,
            [
                Metric {
                    guid: "3f504232-cecb-4118-b4d8-5a5e72d677c3".into(),
                    ipv6: true,
                    value: 0
                },
                Metric {
                    guid: "3f504232-cecb-4118-b4d8-5a5e72d677c3".into(),
                    ipv6: false,
                    value: 5
                },
                Metric {
                    guid: "4d86b570-2994-4eb0-a004-914ef65ff05a".into(),
                    ipv6: true,
                    value: 0
                },
                Metric {
                    guid: "4d86b570-2994-4eb0-a004-914ef65ff05a".into(),
                    ipv6: false,
                    value: 9
                },
            ]
        );
    }

    /// Read-only, against this PC: the active plan and a service everyone has.
    #[test]
    fn reads_the_active_power_plan_and_a_service_on_this_pc() {
        let s = WinSystem::new();
        match s.read(&SysItem::ActivePowerScheme).unwrap() {
            SysState::Text { text } => assert!(guid(&text).is_some(), "{text}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            s.read(&SysItem::Service { name: "Dhcp".into() }).unwrap(),
            SysState::Service { .. }
        ));
        assert!(matches!(s.read(&SysItem::Hibernation).unwrap(), SysState::Bool { .. }));
        assert!(s.network_adapters().is_ok());
    }

    /// Read-only, against this PC: scheduled tasks through the Task Scheduler
    /// COM API, from a thread that is a single-threaded apartment (as the
    /// Tauri main thread is). A task that does not exist reads as Absent.
    #[test]
    fn reads_scheduled_tasks_through_the_task_scheduler_on_this_pc() {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

        std::thread::spawn(|| {
            // SAFETY: COM on a thread of our own, never uninitialised (the
            // thread ends).
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap() };
            let s = WinSystem::new();
            let read = |p: &str| s.read(&SysItem::ScheduledTask { path: p.into() }).unwrap();
            assert_eq!(
                read(r"\Microsoft\Windows\PeakTweaks Test\No Such Task"),
                SysState::Absent
            );
            assert_eq!(read(r"\No Such Folder PeakTweaks\Task"), SysState::Absent);
            for p in crate::tweaks::tasks::TELEMETRY_TASKS.tasks {
                println!("scheduled task {p}: {:?}", read(p));
            }
            let defrag = read(r"\Microsoft\Windows\Defrag\ScheduledDefrag");
            println!("scheduled task \\Microsoft\\Windows\\Defrag\\ScheduledDefrag: {defrag:?}");
            assert!(matches!(defrag, SysState::Bool { .. } | SysState::Absent), "{defrag:?}");
        })
        .join()
        .unwrap();
    }

    /// Evidence for CATALOGUE step 3 (NOTES N76-N78): the power plan,
    /// hibernation, telemetry service and telemetry task tools applied and
    /// undone through the real engine, and this PC read back the same as before each undo. It
    /// changes real settings, so it runs only where
    /// `PEAKTWEAKS_REAL_SYSTEM_CHANGES=1` (the CI runner sets it); anywhere
    /// else, Kegan's PC included, it prints SKIPPED and changes nothing.
    #[test]
    fn power_and_service_tools_apply_and_undo_on_this_pc() {
        use std::sync::Arc;

        use crate::context::{ContextResolver, UserContext, UserResolution};
        use crate::engine::Engine;
        use crate::env::{License, StubProbe};
        use crate::journal::Journal;
        use crate::power::PEAKTWEAKS;
        use crate::secure_dir::TrustedDir;
        use crate::tweaks::power::{self, PLAN_ID, PLAN_SETTINGS};
        use crate::tweaks::services::TELEMETRY_SERVICE;
        use crate::tweaks::tasks::TELEMETRY_TASKS;
        use crate::types::{Tier, TweakState};

        if std::env::var("PEAKTWEAKS_REAL_SYSTEM_CHANGES").as_deref() != Ok("1") {
            println!(
                "SKIPPED: set PEAKTWEAKS_REAL_SYSTEM_CHANGES=1 to change this PC's power plan, hibernation, \
                 DiagTrack service and telemetry tasks for real"
            );
            return;
        }

        let sys = Arc::new(WinSystem::new());
        let original = match sys.read(&SysItem::ActivePowerScheme).unwrap() {
            SysState::Text { text } => text,
            other => panic!("active plan read as {other:?}"),
        };
        let on = |scheme: &str, sub: &str, set: &str| SysItem::PowerSetting {
            scheme: scheme.to_owned(),
            subgroup: sub.to_owned(),
            setting: set.to_owned(),
            ac: true,
        };
        let mut watched = vec![
            SysItem::ActivePowerScheme,
            SysItem::PowerScheme {
                guid: PEAKTWEAKS.to_owned(),
            },
            SysItem::Hibernation,
            SysItem::Service {
                name: "DiagTrack".into(),
            },
        ];
        watched.extend(PLAN_SETTINGS.iter().map(|(sub, set, _, _)| on(&original, sub, set)));
        watched.extend(
            TELEMETRY_TASKS
                .tasks
                .iter()
                .map(|p| SysItem::ScheduledTask { path: (*p).to_owned() }),
        );
        let snapshot = |s: &WinSystem| -> Vec<(String, String)> {
            watched
                .iter()
                .map(|i| {
                    let state = match s.read(i) {
                        Ok(v) => format!("{v:?}"),
                        Err(e) => format!("error: {e}"),
                    };
                    (i.describe(), state)
                })
                .collect()
        };
        let before = snapshot(&sys);
        println!("before:");
        for (what, state) in &before {
            println!("  {what}: {state}");
        }

        let dir = tempfile::tempdir().unwrap();
        let user = UserContext {
            sid: crate::identity::current_process_sid().unwrap(),
            resolution: UserResolution::OwnToken,
            is_self: true,
        };
        let resolver = ContextResolver::new(user, crate::identity::is_elevated(), Arc::new(WinRegistry::new()))
            .with_system(sys.clone());
        let mut tweaks = power::all();
        tweaks.push(Box::new(TELEMETRY_SERVICE));
        tweaks.push(Box::new(TELEMETRY_TASKS));
        let ids: Vec<String> = tweaks.iter().map(|t| t.id().to_owned()).collect();
        let mut engine = Engine::new(
            resolver,
            Journal::open(&TrustedDir::insecure_for_tests(dir.path())).unwrap(),
            tweaks,
            Box::new(StubProbe::open_for_dev()),
            License::dev(Tier::Ultimate),
        );
        let state_of = |e: &Engine, id: &str| {
            let v = e.list().unwrap().into_iter().find(|v| v.metadata.id == id).unwrap();
            (v.state, v.blocked)
        };
        let reg_files = |p: &std::path::Path| -> usize {
            fn walk(p: &std::path::Path, n: &mut usize) {
                for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                    let path = e.path();
                    if path.is_dir() {
                        walk(&path, n);
                    } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("reg")) {
                        *n += 1;
                    }
                }
            }
            let mut n = 0;
            walk(p, &mut n);
            n
        };

        let mut done = 0;
        for id in &ids {
            let (state, blocked) = state_of(&engine, id);
            if let Some(reason) = blocked {
                println!("{id}: SKIPPED, not offered here: {}", reason.message);
                continue;
            }
            if state != TweakState::Default {
                println!("{id}: SKIPPED, already {state:?} on this PC, so there is nothing to change");
                continue;
            }
            let reg_before = reg_files(dir.path());
            engine.apply(id).unwrap_or_else(|e| panic!("{id}: apply failed: {e}"));
            let (after, _) = state_of(&engine, id);
            println!(
                "{id}: applied, now {after:?}; {} .reg backup files written",
                reg_files(dir.path()) - reg_before
            );
            assert_eq!(after, TweakState::Applied, "{id}");
            for (what, state) in snapshot(&sys) {
                println!("  {what}: {state}");
            }
            if id == PLAN_ID {
                for (sub, set, want, label) in PLAN_SETTINGS {
                    let got = sys.read(&on(PEAKTWEAKS, sub, set)).unwrap();
                    println!("  PeakTweaks plan, plugged in, {label}: {got:?} (wanted {want})");
                }
                let hidden = tool(
                    "powercfg.exe",
                    &["/qh", PEAKTWEAKS, PLAN_SETTINGS[1].0, PLAN_SETTINGS[1].1],
                );
                println!(
                    "  powercfg /qh, {}: {}",
                    PLAN_SETTINGS[1].3,
                    match &hidden {
                        Ok(out) => out.lines().rev().take(3).collect::<Vec<_>>().join(" | "),
                        Err(e) => format!("error: {e}"),
                    }
                );
                let list = tool("powercfg.exe", &["/list"]).unwrap();
                let ours = list.lines().find(|l| l.to_ascii_lowercase().contains(PEAKTWEAKS));
                println!(
                    "  powercfg /list: {}",
                    ours.unwrap_or("(PeakTweaks plan not listed)").trim()
                );
            }
            if id == TELEMETRY_TASKS.id {
                // A second reader, so the change is not only what our own
                // COM code reports.
                for p in TELEMETRY_TASKS.tasks {
                    let state = powershell(
                        "scheduled task",
                        "$p = $env:PT_TASK; $i = $p.LastIndexOf('\\'); \
                         $t = Get-ScheduledTask -TaskPath $p.Substring(0, $i + 1) -TaskName $p.Substring($i + 1) \
                         -ErrorAction SilentlyContinue; if ($t) { $t.State } else { 'not on this PC' }",
                        &[("PT_TASK", p)],
                    )
                    .unwrap_or_else(|e| format!("error: {e}"));
                    println!("  Get-ScheduledTask {p}: {}", state.trim());
                }
            }
            engine.revert(id).unwrap_or_else(|e| panic!("{id}: undo failed: {e}"));
            let (reverted, _) = state_of(&engine, id);
            let now = snapshot(&sys);
            println!("{id}: undone, now {reverted:?}");
            assert_eq!(now, before, "{id}: this PC reads back differently after undo");
            done += 1;
        }
        println!(
            "{done} of {} tools applied and undone; this PC read back the same after each undo",
            ids.len()
        );
    }

    /// Evidence for CATALOGUE step 5 (NOTES N80): Gaming Mode made and put
    /// back through the real engine, as the game watcher does when a game
    /// starts and closes, with this PC read before, during and after. Gated
    /// like the test above: it changes nothing unless
    /// `PEAKTWEAKS_REAL_SYSTEM_CHANGES=1`.
    #[test]
    fn gaming_mode_starts_and_ends_on_this_pc() {
        use std::sync::Arc;

        use crate::context::{ContextResolver, UserContext, UserResolution};
        use crate::engine::Engine;
        use crate::env::{License, StubProbe};
        use crate::journal::Journal;
        use crate::registry::{Hive, RegistryBackend};
        use crate::secure_dir::TrustedDir;
        use crate::tweaks::session::PUSH_NOTIFICATIONS;
        use crate::types::Tier;

        if std::env::var("PEAKTWEAKS_REAL_SYSTEM_CHANGES").as_deref() != Ok("1") {
            println!(
                "SKIPPED: set PEAKTWEAKS_REAL_SYSTEM_CHANGES=1 to turn notifications off and stop Windows Search \
                 on this PC for real"
            );
            return;
        }

        let sys = Arc::new(WinSystem::new());
        let reg = Arc::new(WinRegistry::new());
        let snapshot = || -> Vec<String> {
            vec![
                format!(
                    "ToastEnabled: {:?}",
                    reg.read_value(Hive::CurrentUser, PUSH_NOTIFICATIONS, "ToastEnabled")
                ),
                format!(
                    "Service WSearch: {:?}",
                    sys.read(&SysItem::Service { name: "WSearch".into() })
                ),
            ]
        };
        let before = snapshot();
        println!("before: {before:?}");

        let dir = tempfile::tempdir().unwrap();
        let user = UserContext {
            sid: crate::identity::current_process_sid().unwrap(),
            resolution: UserResolution::OwnToken,
            is_self: true,
        };
        let resolver = ContextResolver::new(user, crate::identity::is_elevated(), reg.clone()).with_system(sys.clone());
        let mut engine = Engine::new(
            resolver,
            Journal::open(&TrustedDir::insecure_for_tests(dir.path())).unwrap(),
            Vec::new(),
            Box::new(StubProbe::open_for_dev()),
            License::dev(Tier::Ultimate),
        );
        let mut settings = engine.settings();
        settings.gaming_mode = true;
        engine.set_settings(settings).unwrap();

        let steps = engine.start_play_session();
        for step in &steps {
            println!("start: {step:?}");
            assert!(step.error.is_none(), "{step:?}");
        }
        println!("during: {:?}", snapshot());
        println!("in Backups: {:?}", engine.journal_view().applied);

        for r in engine.end_play_session() {
            println!("end: {r:?}");
            assert!(r.ok, "{r:?}");
        }
        assert!(!engine.play_session_open());
        let after = snapshot();
        println!("after: {after:?}");
        assert_eq!(after, before, "this PC reads back differently after Gaming Mode ended");
        println!(
            "Gaming Mode made {} of {} changes and put them back",
            steps.iter().filter(|s| s.made).count(),
            steps.len()
        );
    }

    /// Startup apps (catalogue H12) on this PC's real registry and folders:
    /// lists what this runner starts at sign-in, then turns a startup entry
    /// made for the test off and back on through the real engine, reading
    /// the switch Windows keeps for it before, during and after. Gated like
    /// the tests above.
    #[test]
    fn startup_apps_turn_off_and_back_on_on_this_pc() {
        use std::sync::Arc;

        use crate::context::{ContextResolver, UserContext, UserResolution};
        use crate::engine::Engine;
        use crate::env::{License, StubProbe};
        use crate::journal::Journal;
        use crate::registry::{Hive, RegistryBackend};
        use crate::secure_dir::TrustedDir;
        use crate::startup::StartupFolders;
        use crate::tweaks::startup::{switched_off, StartupSource, StartupToggle};
        use crate::types::{RawValue, Tier, Tweak, TweakState};

        if std::env::var("PEAKTWEAKS_REAL_SYSTEM_CHANGES").as_deref() != Ok("1") {
            println!(
                "SKIPPED: set PEAKTWEAKS_REAL_SYSTEM_CHANGES=1 to add and turn off a startup entry on this PC for real"
            );
            return;
        }
        const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
        const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
        const NAME: &str = "PeakTweaks startup check";

        /// Removes the test entry however the test ends.
        struct Entry(Arc<WinRegistry>);
        impl Drop for Entry {
            fn drop(&mut self) {
                let _ = self.0.delete_value(Hive::CurrentUser, RUN, NAME);
            }
        }

        let reg = Arc::new(WinRegistry::new());
        reg.write_value(
            Hive::CurrentUser,
            RUN,
            NAME,
            &RawValue::sz(r"C:\Windows\System32\notepad.exe"),
        )
        .unwrap();
        let _entry = Entry(reg.clone());
        let switch = || reg.read_value(Hive::CurrentUser, APPROVED, NAME).unwrap();
        let before = switch();
        println!("switch before: {before:?}");

        let sid = crate::identity::current_process_sid().unwrap();
        let folders = StartupFolders::from_places(&crate::cleanup::places(&sid));
        println!("Startup folders: {folders:?}");
        let dir = tempfile::tempdir().unwrap();
        let user = UserContext {
            sid,
            resolution: UserResolution::OwnToken,
            is_self: true,
        };
        let resolver = ContextResolver::new(user, crate::identity::is_elevated(), reg.clone());
        let mut engine = Engine::new(
            resolver,
            Journal::open(&TrustedDir::insecure_for_tests(dir.path())).unwrap(),
            Vec::new(),
            Box::new(StubProbe::open_for_dev()),
            License::dev(Tier::Ultimate),
        );

        let list = engine.startup_apps(&folders);
        for app in &list.apps {
            println!(
                "listed: {:?} {:?} {:?} {:?}",
                app.source, app.name, app.tweak.state, app.command
            );
        }
        println!("problems: {:?}", list.problems);
        assert!(list.problems.is_empty(), "{:?}", list.problems);
        let id = StartupToggle::new(StartupSource::UserRun, NAME).id().to_owned();
        let ours = list
            .apps
            .iter()
            .find(|a| a.tweak.metadata.id == id)
            .expect("the test entry is listed");
        assert_eq!(ours.tweak.state, TweakState::Default);

        let result = engine.apply(&id);
        let during = switch();
        println!("apply: {result:?}");
        println!("switch while off: {during:?}");
        let undo = engine.revert(&id);
        println!("undo: {undo:?}");
        let after = switch();
        println!("switch after Undo: {after:?}");

        result.unwrap();
        undo.unwrap();
        let during = during.expect("a switch is written");
        assert_eq!(switched_off(&during), Some(true));
        assert_eq!(during.bytes.len(), 12);
        assert_eq!(after, before, "the switch reads back differently after Undo");
    }

    /// The interface metric (CATALOGUE E5) of this PC's first connected
    /// physical adapter that carries IP, set by hand and put back through the
    /// real backend, IPv4 and IPv6, with the registry values Windows may keep
    /// it in and the settings Windows applies at start-up (its persistent
    /// store) printed alongside. Gated like the tests above. A physical
    /// adapter bound to a virtual switch carries no IP itself (run
    /// 37750100174): its metric reads as absent and the tool leaves it out.
    #[test]
    fn an_interface_metric_is_set_and_put_back_on_this_pc() {
        if std::env::var("PEAKTWEAKS_REAL_SYSTEM_CHANGES").as_deref() != Ok("1") {
            println!("SKIPPED: set PEAKTWEAKS_REAL_SYSTEM_CHANGES=1 to change a network adapter's metric for real");
            return;
        }
        let s = WinSystem::new();
        let adapters = s.network_adapters().unwrap();
        println!("adapters: {adapters:?}");
        println!(
            "metric script output: {:?}",
            powershell("interface metrics", METRICS_SCRIPT, &[])
        );
        println!(
            "Get-NetIPInterface: {:?}",
            powershell(
                "IP interfaces",
                "Get-NetIPInterface | ForEach-Object { '{0}|{1}|{2}|{3}|{4}' -f $_.ifIndex, $_.InterfaceAlias, \
                 $_.AddressFamily, $_.AutomaticMetric, $_.InterfaceMetric }",
                &[]
            )
        );
        let ipv4 = |a: &NetAdapter| {
            s.read(&SysItem::InterfaceMetric {
                interface: a.guid.clone(),
                ipv6: false,
            })
        };
        let a = adapters
            .iter()
            .find(|a| a.up && matches!(ipv4(a), Ok(SysState::Dword { .. })))
            .expect("a connected physical adapter with an IPv4 metric");
        println!("adapter checked: {} ({})", a.name, a.guid);
        let persistent = |ipv6: bool| {
            powershell(
                "persistent interface",
                "$a = Get-NetAdapter -IncludeHidden | Where-Object { $_.InterfaceGuid -eq ('{' + $env:PT_GUID + '}') }; \
                 Get-NetIPInterface -InterfaceIndex $a.ifIndex -AddressFamily $env:PT_FAMILY -PolicyStore \
                 PersistentStore -ErrorAction SilentlyContinue | ForEach-Object { '{0}|{1}' -f $_.AutomaticMetric, \
                 $_.InterfaceMetric }",
                &[("PT_GUID", &a.guid), ("PT_FAMILY", if ipv6 { "IPv6" } else { "IPv4" })],
            )
        };
        for ipv6 in [false, true] {
            let item = SysItem::InterfaceMetric {
                interface: a.guid.clone(),
                ipv6,
            };
            let backing = || {
                item.registry_backing()
                    .iter()
                    .map(|(key, name)| format!("{key}\\{name} = {:?}", s.reg.read_value(Hive::LocalMachine, key, name)))
                    .collect::<Vec<_>>()
            };
            let before = s.read(&item).unwrap();
            println!(
                "{}: before {before:?}; registry {:?}; at start-up {:?}",
                item.describe(),
                backing(),
                persistent(ipv6)
            );
            // IPv6 may be unbound.
            if before == SysState::Absent {
                continue;
            }
            let set = SysState::Dword { value: 7 };
            let wrote = s.write(&item, &set);
            let during = s.read(&item);
            println!(
                "set 7: {wrote:?}; now {during:?}; registry {:?}; at start-up {:?}",
                backing(),
                persistent(ipv6)
            );
            let put_back = s.write(&item, &before);
            let after = s.read(&item);
            println!(
                "put back: {put_back:?}; now {after:?}; registry {:?}; at start-up {:?}",
                backing(),
                persistent(ipv6)
            );
            wrote.unwrap();
            put_back.unwrap();
            assert_eq!(during.unwrap(), set);
            assert_eq!(after.unwrap(), before);
        }
    }
}
