//! The real `SystemBackend` on Windows.
//!
//! Nothing here goes through a shell command line. Windows' own tools
//! (`powercfg.exe`, `gpupdate.exe`) are started by absolute path under
//! `%SystemRoot%\System32` with an argument list, and every argument is checked
//! first (GUIDs must be GUIDs, names a short safe character set). The few
//! things only PowerShell exposes (scheduled tasks, DNS, adapter restart) run
//! as fixed scripts that read their inputs from environment variables, so no
//! input becomes script text. Services use the Win32 service API directly.
//!
//! Not yet built here, and refused with a plain message: `netsh` TCP globals
//! and NVIDIA profile settings (their catalogue steps add them; NOTES N66).

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::error::{EngineError, Result};
use super::proc::run_limited;
use super::system::{guid, guids_in, setting_indexes, NetAdapter, SideEffect, SysItem, SysState, SystemBackend};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const LIMIT: Duration = Duration::from_secs(60);

/// The real backend. The adapter list is cached for a short while: the tool
/// list asks for it on every refresh, and each listing starts PowerShell.
pub struct WinSystem {
    adapters: std::sync::Mutex<Option<(std::time::Instant, Vec<NetAdapter>)>>,
}

impl WinSystem {
    pub fn new() -> Self {
        Self {
            adapters: std::sync::Mutex::new(None),
        }
    }
}

impl Default for WinSystem {
    fn default() -> Self {
        Self::new()
    }
}

const ADAPTER_CACHE: Duration = Duration::from_secs(30);

/// `guid|Status|PhysicalMediaType|Name` lines from `Get-NetAdapter -Physical`.
pub(crate) fn parse_adapters(out: &str) -> Vec<NetAdapter> {
    out.lines()
        .filter_map(|l| {
            let mut parts = l.trim().splitn(4, '|');
            let g = guid(parts.next()?)?;
            let status = parts.next()?;
            let media = parts.next()?;
            let name = parts.next()?.to_owned();
            Some(NetAdapter {
                guid: g,
                name,
                up: status.eq_ignore_ascii_case("Up"),
                wireless: media.contains("802.11"),
            })
        })
        .collect()
}

fn fail(what: &str, detail: impl Into<String>) -> EngineError {
    EngineError::Command {
        what: what.into(),
        exit_code: None,
        detail: detail.into(),
    }
}

fn system32(exe: &str) -> Result<PathBuf> {
    let root = std::env::var_os("SystemRoot").ok_or_else(|| fail(exe, "%SystemRoot% is not set"))?;
    let path = PathBuf::from(root).join("System32").join(exe);
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

fn need_ip(s: &str) -> Result<()> {
    // IPv4 or IPv6 characters only; Windows checks the address itself.
    let ok = !s.is_empty() && s.len() <= 45 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':');
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
            "Get-NetAdapter -Physical | ForEach-Object { '{0}|{1}|{2}|{3}' -f $_.InterfaceGuid, $_.Status, \
             $_.PhysicalMediaType, $_.Name }",
            &[],
        )?;
        let list = parse_adapters(&out);
        *cache = Some((std::time::Instant::now(), list.clone()));
        Ok(list)
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
                let out = tool("powercfg.exe", &["/query", &s, &sub, &set])?;
                // Hidden settings print nothing; their index then counts as
                // not set (PeakTweaks only changes settings on its own copy,
                // which Undo deletes).
                Ok(match setting_indexes(&out) {
                    Some((a, d)) => SysState::Dword {
                        value: if *ac { a } else { d },
                    },
                    None => SysState::Absent,
                })
            }
            SysItem::Hibernation => {
                let out = powershell(
                    "hibernation",
                    "(Get-ItemProperty 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\Power' -Name HibernateEnabled \
                     -ErrorAction Stop).HibernateEnabled",
                    &[],
                )?;
                Ok(SysState::Bool { on: out.trim() != "0" })
            }
            SysItem::Service { name } => {
                need_name(name, "service")?;
                services::read(name)
            }
            SysItem::ScheduledTask { path } => {
                need_name(path, "scheduled task")?;
                let out = powershell(
                    "scheduled task",
                    "$p = $env:PT_TASK; $i = $p.LastIndexOf('\\'); \
                     (Get-ScheduledTask -TaskPath $p.Substring(0, $i + 1) -TaskName $p.Substring($i + 1) \
                     -ErrorAction Stop).State",
                    &[("PT_TASK", path)],
                )?;
                Ok(SysState::Bool {
                    on: out.trim() != "Disabled",
                })
            }
            SysItem::DnsServers { interface } => {
                let g = need_guid(interface, "network adapter")?;
                // The servers set by hand for this adapter (IPv4). Empty means
                // automatic, from the router.
                let out = powershell(
                    "DNS servers",
                    "$k = 'HKLM:\\SYSTEM\\CurrentControlSet\\Services\\Tcpip\\Parameters\\Interfaces\\{' + \
                     $env:PT_GUID + '}'; (Get-ItemProperty $k -ErrorAction Stop).NameServer",
                    &[("PT_GUID", &g)],
                )?;
                let items = out
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                Ok(SysState::List { items })
            }
            SysItem::TcpGlobal { .. } | SysItem::NvidiaSetting { .. } => Err(EngineError::Internal {
                detail: format!("this version cannot change the {} yet", item.describe()),
            }),
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
                let (g, src) = (need_guid(g, "power plan")?, need_guid(source, "power plan")?);
                if guids_in(&tool("powercfg.exe", &["/list"])?).contains(&g) {
                    return Ok(());
                }
                tool("powercfg.exe", &["/duplicatescheme", &src, &g]).map(drop)
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
                need_name(path, "scheduled task")?;
                let script = if *on {
                    "$p = $env:PT_TASK; $i = $p.LastIndexOf('\\'); \
                     Enable-ScheduledTask -TaskPath $p.Substring(0, $i + 1) -TaskName $p.Substring($i + 1) \
                     -ErrorAction Stop | Out-Null"
                } else {
                    "$p = $env:PT_TASK; $i = $p.LastIndexOf('\\'); \
                     Disable-ScheduledTask -TaskPath $p.Substring(0, $i + 1) -TaskName $p.Substring($i + 1) \
                     -ErrorAction Stop | Out-Null"
                };
                powershell("scheduled task", script, &[("PT_TASK", path)]).map(drop)
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
                    "Get-NetAdapter -IncludeHidden | Where-Object { $_.InterfaceGuid -eq ('{' + $env:PT_GUID + '}') } \
                     | Restart-NetAdapter -Confirm:$false -ErrorAction Stop",
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
        assert!(need_ip("1.1.1.1").is_ok() && need_ip("2606:4700:4700::1111").is_ok());
        assert!(need_ip("1.1.1.1; rm").is_err());
    }

    /// Real output from Kegan's PC (2026-10-07).
    #[test]
    fn adapters_are_parsed_from_the_listing() {
        let out = "{4D86B570-2994-4EB0-A004-914EF65FF05A}|Disconnected|Native 802.11|Wi-Fi\r\n\
                   {3F504232-CECB-4118-B4D8-5A5E72D677C3}|Up|802.3|Ethernet\r\n";
        let a = parse_adapters(out);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].guid, "4d86b570-2994-4eb0-a004-914ef65ff05a");
        assert!(a[0].wireless && !a[0].up);
        assert!(!a[1].wireless && a[1].up && a[1].name == "Ethernet");
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
}
