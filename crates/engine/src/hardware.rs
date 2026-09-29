//! Hardware probes and the rig class.
//!
//! Everything reads through `WmiSource` and `OsFacts` so the logic runs in tests
//! against canned data. Each field is a `Probe`: a value we could not read is
//! `Unknown` and stays that way. The rig class is a pure function over those
//! probes and says "Unknown" rather than guessing when an input it needs is
//! missing.

use serde::Serialize;
use ts_rs::TS;

use super::error::Result;
use super::probe::Probe;
use super::wmi::{WmiRow, WmiSource, NS_CIMV2, NS_STORAGE};

pub const GIB: u64 = 1024 * 1024 * 1024;

pub(crate) const WQL_MEMORY: &str =
    "SELECT Capacity, Speed, ConfiguredClockSpeed, BankLabel, DeviceLocator, SMBIOSMemoryType FROM Win32_PhysicalMemory";
pub(crate) const WQL_COMPUTER: &str = "SELECT TotalPhysicalMemory, PCSystemType FROM Win32_ComputerSystem";
pub(crate) const WQL_CPU: &str =
    "SELECT Name, Manufacturer, NumberOfCores, NumberOfLogicalProcessors, MaxClockSpeed FROM Win32_Processor";
pub(crate) const WQL_OS: &str = "SELECT BuildNumber, Caption, ProductType FROM Win32_OperatingSystem";

// ---------------------------------------------------------------------------
// OS-level facts that are not WMI
// ---------------------------------------------------------------------------

/// A graphics adapter as DXGI reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GpuAdapter {
    pub name: String,
    pub vendor_id: u32,
    pub dedicated_vram_bytes: u64,
    pub shared_memory_bytes: u64,
    /// Microsoft's software renderer (WARP / Basic Render Driver). Not a GPU.
    pub is_software: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    pub current_hz: u32,
    /// Highest refresh rate the primary display offers at the current
    /// resolution and colour depth.
    pub max_hz_at_current_resolution: u32,
}

/// Win32 facts the probes need. Real implementation: `osfacts.rs`.
pub trait OsFacts: Send + Sync {
    /// The system drive as `"C:"`.
    fn system_drive(&self) -> String;
    fn display(&self) -> Result<DisplayInfo>;
    fn gpu_adapters(&self) -> Result<Vec<GpuAdapter>>;
}

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct OsInfo {
    pub build: u32,
    pub caption: String,
    /// True on Windows Server. System Restore does not exist there.
    pub is_server: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CpuInfo {
    pub name: String,
    pub vendor: String,
    pub cores: u32,
    pub logical_processors: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStick {
    pub capacity_bytes: u64,
    /// Rated speed of the module.
    pub rated_mhz: Option<u32>,
    /// Speed it is actually running at.
    pub configured_mhz: Option<u32>,
    pub bank_label: Option<String>,
    pub device_locator: Option<String>,
    /// "DDR4", "DDR5", ... when the SMBIOS type code is one we recognise.
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ChannelLayout {
    /// All modules on one channel (including a single module).
    Single,
    /// Modules on two or more channels.
    Multi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInfo {
    /// Sum of module capacities, the number to judge "8 GB" by.
    pub installed_bytes: u64,
    pub sticks: Vec<MemoryStick>,
    pub channels: Probe<ChannelLayout>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DiskMedia {
    Hdd,
    Ssd,
    /// Storage class memory.
    Scm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct BootDisk {
    pub media: DiskMedia,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RigClass {
    Low,
    Mid,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct HardwareReport {
    pub os: Probe<OsInfo>,
    pub cpu: Probe<CpuInfo>,
    pub memory: Probe<MemoryInfo>,
    /// Real adapters only (software renderers filtered out), largest VRAM first.
    pub gpus: Probe<Vec<GpuAdapter>>,
    pub boot_disk: Probe<BootDisk>,
    pub display: Probe<DisplayInfo>,
    pub is_laptop: Probe<bool>,
    pub rig_class: Probe<RigClass>,
}

// ---------------------------------------------------------------------------
// Parsers (pure)
// ---------------------------------------------------------------------------

fn smbios_memory_kind(code: u64) -> Option<&'static str> {
    // SMBIOS "Memory Device" type codes. VERIFY against the SMBIOS spec
    // (NOTES.md N22); from memory.
    Some(match code {
        20 => "DDR",
        21 => "DDR2",
        24 => "DDR3",
        26 => "DDR4",
        27 => "LPDDR",
        28 => "LPDDR2",
        29 => "LPDDR3",
        30 => "LPDDR4",
        34 => "DDR5",
        35 => "LPDDR5",
        _ => return None,
    })
}

fn stick_from(row: &WmiRow) -> Option<MemoryStick> {
    Some(MemoryStick {
        capacity_bytes: row.u64("Capacity")?,
        rated_mhz: row.u64("Speed").and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0),
        configured_mhz: row
            .u64("ConfiguredClockSpeed")
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0),
        bank_label: row.str("BankLabel").map(str::to_owned).filter(|s| !s.trim().is_empty()),
        device_locator: row
            .str("DeviceLocator")
            .map(str::to_owned)
            .filter(|s| !s.trim().is_empty()),
        kind: row
            .u64("SMBIOSMemoryType")
            .and_then(smbios_memory_kind)
            .map(str::to_owned),
    })
}

/// A key that identifies which memory channel a slot label refers to, or `None`
/// when the label does not say. Recognised: `...Channel<A|B|0|1>...` (optionally
/// with a `Controller<n>` prefix), and `DIMM_<letter><digit>` where the letter is
/// the channel (`DIMM_A1`, `DIMMB2`). Anything else, for example `DIMM1` or
/// `Bottom-Slot 1(left)`, is deliberately not interpreted.
pub fn channel_key(label: &str) -> Option<String> {
    let lower = label.to_ascii_lowercase();

    if let Some(at) = lower.find("channel") {
        let after = &lower[at + "channel".len()..];
        let id: String = after
            .trim_start_matches([' ', '_', '-'])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .take(1)
            .collect();
        if id.is_empty() {
            return None;
        }
        let controller = lower.find("controller").map(|c| {
            lower[c + "controller".len()..]
                .trim_start_matches([' ', '_', '-'])
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        });
        return Some(match controller {
            Some(c) if !c.is_empty() => format!("{c}:{id}"),
            _ => id,
        });
    }

    if let Some(at) = lower.find("dimm") {
        let mut rest = lower[at + 4..]
            .chars()
            .skip_while(|c| *c == '_' || *c == ' ' || *c == '-');
        if let (Some(letter), Some(digit)) = (rest.next(), rest.next()) {
            if letter.is_ascii_lowercase() && digit.is_ascii_digit() {
                return Some(letter.to_string());
            }
        }
    }
    None
}

/// Channel layout from slot labels. One module is one channel. With several,
/// the labels must all name a channel; otherwise we do not know.
pub fn estimate_channels(sticks: &[MemoryStick]) -> Probe<ChannelLayout> {
    match sticks.len() {
        0 => Probe::unknown("no memory module data was returned"),
        1 => {
            if sticks[0].kind.as_deref().is_some_and(|k| k.starts_with("LPDDR")) {
                Probe::unknown("soldered LPDDR memory: the channel layout is not reported reliably")
            } else {
                Probe::yes(ChannelLayout::Single)
            }
        }
        _ => {
            let keys: Vec<Option<String>> = sticks
                .iter()
                .map(|s| {
                    s.device_locator
                        .as_deref()
                        .and_then(channel_key)
                        .or_else(|| s.bank_label.as_deref().and_then(channel_key))
                })
                .collect();
            if keys.iter().any(Option::is_none) {
                return Probe::unknown("the slot labels do not say which channel each module is on");
            }
            let mut distinct: Vec<&String> = keys.iter().flatten().collect();
            distinct.sort();
            distinct.dedup();
            if distinct.len() >= 2 {
                Probe::yes(ChannelLayout::Multi)
            } else {
                Probe::yes(ChannelLayout::Single)
            }
        }
    }
}

pub fn memory_from(rows: &Result<Vec<WmiRow>>) -> Probe<MemoryInfo> {
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return Probe::unknown(format!("cannot read memory modules: {e}")),
    };
    let sticks: Vec<MemoryStick> = rows.iter().filter_map(stick_from).collect();
    if sticks.is_empty() {
        return Probe::unknown("Windows returned no memory module data");
    }
    let installed_bytes = sticks.iter().map(|s| s.capacity_bytes).sum();
    let channels = estimate_channels(&sticks);
    Probe::yes(MemoryInfo {
        installed_bytes,
        sticks,
        channels,
    })
}

pub fn cpu_from(rows: &Result<Vec<WmiRow>>) -> Probe<CpuInfo> {
    let rows = match rows {
        Ok(r) if !r.is_empty() => r,
        Ok(_) => return Probe::unknown("Windows returned no processor data"),
        Err(e) => return Probe::unknown(format!("cannot read the processor: {e}")),
    };
    let sum = |name: &str| -> Option<u32> {
        rows.iter()
            .map(|r| r.u64(name).and_then(|v| u32::try_from(v).ok()))
            .sum::<Option<u32>>()
    };
    let (Some(cores), Some(logical)) = (sum("NumberOfCores"), sum("NumberOfLogicalProcessors")) else {
        return Probe::unknown("the processor core counts were missing");
    };
    if logical == 0 {
        return Probe::unknown("the processor reported zero logical processors");
    }
    Probe::yes(CpuInfo {
        name: rows[0].str("Name").unwrap_or("").trim().to_owned(),
        vendor: rows[0].str("Manufacturer").unwrap_or("").trim().to_owned(),
        cores,
        logical_processors: logical,
    })
}

pub fn os_from(rows: &Result<Vec<WmiRow>>) -> Probe<OsInfo> {
    let rows = match rows {
        Ok(r) if !r.is_empty() => r,
        Ok(_) => return Probe::unknown("Windows returned no operating system data"),
        Err(e) => return Probe::unknown(format!("cannot read the operating system: {e}")),
    };
    let row = &rows[0];
    let Some(build) = row.u64("BuildNumber").and_then(|b| u32::try_from(b).ok()) else {
        return Probe::unknown("the OS build number was missing");
    };
    Probe::yes(OsInfo {
        build,
        caption: row.str("Caption").unwrap_or("").trim().to_owned(),
        // Win32_OperatingSystem.ProductType: 1 workstation, 2 domain controller, 3 server.
        is_server: row.u64("ProductType").is_some_and(|t| t != 1),
    })
}

pub fn laptop_from(rows: &Result<Vec<WmiRow>>) -> Probe<bool> {
    let row = match rows {
        Ok(r) if !r.is_empty() => &r[0],
        Ok(_) => return Probe::unknown("Windows returned no computer system data"),
        Err(e) => return Probe::unknown(format!("cannot read the computer system: {e}")),
    };
    // PCSystemType: 0 unspecified, 1 desktop, 2 mobile, 3 workstation, ...
    match row.u64("PCSystemType") {
        Some(2) => Probe::yes(true),
        Some(0) | None => Probe::unknown("the machine type is unspecified"),
        Some(_) => Probe::no("not reported as a mobile system"),
    }
}

/// Physical GPUs from the adapters DXGI listed, biggest VRAM first.
pub fn gpus_from(adapters: &Result<Vec<GpuAdapter>>) -> Probe<Vec<GpuAdapter>> {
    let adapters = match adapters {
        Ok(a) => a,
        Err(e) => return Probe::unknown(format!("cannot list graphics adapters: {e}")),
    };
    let mut real: Vec<GpuAdapter> = adapters.iter().filter(|a| !a.is_software).cloned().collect();
    if real.is_empty() {
        return Probe::no("no hardware graphics adapter was found");
    }
    real.sort_by_key(|a| std::cmp::Reverse(a.dedicated_vram_bytes));
    Probe::yes(real)
}

// ---------------------------------------------------------------------------
// Boot disk
// ---------------------------------------------------------------------------

fn valid_drive(drive: &str) -> bool {
    let b = drive.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// `MSFT_PhysicalDisk.MediaType`: 3 HDD, 4 SSD, 5 SCM, 0 unspecified. VERIFY.
fn media_from_code(code: u64) -> Option<DiskMedia> {
    match code {
        3 => Some(DiskMedia::Hdd),
        4 => Some(DiskMedia::Ssd),
        5 => Some(DiskMedia::Scm),
        _ => None,
    }
}

/// Disk holding the system drive: logical disk -> partition -> disk index ->
/// physical disk. The query text is built only from a validated drive letter and
/// a number.
pub fn probe_boot_disk(wmi: &dyn WmiSource, system_drive: &str) -> Probe<BootDisk> {
    if !valid_drive(system_drive) {
        return Probe::unknown(format!("unexpected system drive {system_drive:?}"));
    }
    let assoc = format!(
        "ASSOCIATORS OF {{Win32_LogicalDisk.DeviceID='{system_drive}'}} WHERE AssocClass = Win32_LogicalDiskToPartition"
    );
    let parts = match wmi.query(NS_CIMV2, &assoc) {
        Ok(p) => p,
        Err(e) => return Probe::unknown(format!("cannot map {system_drive} to a partition: {e}")),
    };
    let Some(index) = parts.first().and_then(|p| p.u64("DiskIndex")) else {
        return Probe::unknown(format!("cannot tell which disk holds {system_drive}"));
    };
    let q = format!("SELECT MediaType, FriendlyName FROM MSFT_PhysicalDisk WHERE DeviceId = '{index}'");
    match wmi.query(NS_STORAGE, &q) {
        Err(e) => Probe::unknown(format!("cannot read the storage type of disk {index}: {e}")),
        Ok(rows) => match rows.first() {
            None => Probe::unknown(format!("Windows has no physical disk record for disk {index}")),
            Some(r) => match r.u64("MediaType").and_then(media_from_code) {
                Some(media) => Probe::yes(BootDisk {
                    media,
                    name: r.str("FriendlyName").unwrap_or("").trim().to_owned(),
                }),
                None => Probe::unknown("the disk does not report whether it is an SSD or a hard drive"),
            },
        },
    }
}

// ---------------------------------------------------------------------------
// Rig class (pure)
// ---------------------------------------------------------------------------

/// Low / Mid / High from the weakest of memory, graphics, logical processors and
/// boot disk (plan section 6.1). Thresholds are the plan's suggestions, to be
/// tuned with data: **Low** if RAM <= 8 GiB, or the best GPU has <= 4 GiB of
/// dedicated memory, or <= 4 logical processors, or the boot disk is a hard
/// drive. **High** if the best GPU has >= 12 GiB and >= 16 logical processors and
/// the display does >= 144 Hz. Otherwise **Mid**.
///
/// Any single Low factor is definitive. When no Low factor holds but one of the
/// Low inputs is unknown, the answer could still be Low, so it is Unknown. The
/// same goes for separating Mid from High when a High input is unknown.
pub fn classify(
    memory: &Probe<MemoryInfo>,
    gpus: &Probe<Vec<GpuAdapter>>,
    cpu: &Probe<CpuInfo>,
    disk: &Probe<BootDisk>,
    display: &Probe<DisplayInfo>,
) -> Probe<RigClass> {
    let ram = memory.value().map(|m| m.installed_bytes);
    let vram = gpus.value().and_then(|g| g.first()).map(|g| g.dedicated_vram_bytes);
    let logical = cpu.value().map(|c| c.logical_processors);
    let hdd = disk.value().map(|d| d.media == DiskMedia::Hdd);
    let hz = display
        .value()
        .map(|d| d.max_hz_at_current_resolution.max(d.current_hz));

    // A no-GPU machine (Probe::No) is as low as it gets.
    let no_gpu = gpus.is_no();

    let low_hits: Vec<&str> = [
        ram.filter(|r| *r <= 8 * GIB).map(|_| "8 GB of RAM or less"),
        vram.filter(|v| *v <= 4 * GIB)
            .map(|_| "graphics with 4 GB of memory or less"),
        no_gpu.then_some("no dedicated graphics adapter"),
        logical.filter(|l| *l <= 4).map(|_| "4 logical processors or fewer"),
        hdd.filter(|h| *h).map(|_| "a hard drive as the boot disk"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !low_hits.is_empty() {
        return Probe::yes(RigClass::Low);
    }

    let unknown_low: Vec<&str> = [
        ("memory", ram.is_none()),
        ("graphics", vram.is_none() && !no_gpu),
        ("processor", logical.is_none()),
        ("boot disk", hdd.is_none()),
    ]
    .into_iter()
    .filter_map(|(n, u)| u.then_some(n))
    .collect();
    if !unknown_low.is_empty() {
        return Probe::unknown(format!(
            "cannot rule out a Low rig: the {} could not be read",
            unknown_low.join(", ")
        ));
    }

    // Every Low input is known and none is low. High needs all three High tests.
    let high = [
        vram.map(|v| v >= 12 * GIB),
        logical.map(|l| l >= 16),
        hz.map(|h| h >= 144),
    ];
    if high.contains(&Some(false)) {
        return Probe::yes(RigClass::Mid);
    }
    if high.iter().all(|h| *h == Some(true)) {
        return Probe::yes(RigClass::High);
    }
    Probe::unknown("cannot separate Mid from High: the display refresh rate could not be read")
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

pub fn probe_hardware(wmi: &dyn WmiSource, os_facts: &dyn OsFacts) -> HardwareReport {
    let os = os_from(&wmi.query(NS_CIMV2, WQL_OS));
    let cpu = cpu_from(&wmi.query(NS_CIMV2, WQL_CPU));
    let memory = memory_from(&wmi.query(NS_CIMV2, WQL_MEMORY));
    let gpus = gpus_from(&os_facts.gpu_adapters());
    let boot_disk = probe_boot_disk(wmi, &os_facts.system_drive());
    let display = os_facts.display().into();
    let is_laptop = laptop_from(&wmi.query(NS_CIMV2, WQL_COMPUTER));
    let rig_class = classify(&memory, &gpus, &cpu, &boot_disk, &display);
    HardwareReport {
        os,
        cpu,
        memory,
        gpus,
        boot_disk,
        display,
        is_laptop,
        rig_class,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EngineError;
    use crate::wmi::{FakeWmi, WmiValue};

    fn stick(cap_gib: u64, locator: &str, bank: &str, kind: Option<&str>) -> MemoryStick {
        MemoryStick {
            capacity_bytes: cap_gib * GIB,
            rated_mhz: Some(3200),
            configured_mhz: Some(3200),
            bank_label: Some(bank.into()),
            device_locator: Some(locator.into()),
            kind: kind.map(str::to_owned),
        }
    }

    #[test]
    fn channel_keys_from_real_world_labels() {
        assert_eq!(channel_key("ChannelA-DIMM0").as_deref(), Some("a"));
        assert_eq!(channel_key("ChannelB-DIMM1").as_deref(), Some("b"));
        assert_eq!(channel_key("Controller0-ChannelA").as_deref(), Some("0:a"));
        assert_eq!(channel_key("Controller1-ChannelA").as_deref(), Some("1:a"));
        assert_eq!(channel_key("DIMM_A1").as_deref(), Some("a"));
        assert_eq!(channel_key("DIMM_B2").as_deref(), Some("b"));
        assert_eq!(channel_key("DIMMA1").as_deref(), Some("a"));
        assert_eq!(channel_key("DIMM A2").as_deref(), Some("a"));
        // Labels that do not name a channel are not interpreted.
        assert_eq!(channel_key("DIMM1"), None);
        assert_eq!(channel_key("Bottom-Slot 1(left)"), None);
        assert_eq!(channel_key("BANK 0"), None);
        assert_eq!(channel_key(""), None);
        assert_eq!(channel_key("Channel"), None);
    }

    #[test]
    fn channels_are_only_claimed_when_the_labels_say_so() {
        let one = [stick(8, "DIMM_A1", "BANK 0", Some("DDR4"))];
        assert_eq!(estimate_channels(&one), Probe::yes(ChannelLayout::Single));

        let dual = [
            stick(8, "DIMM_A1", "", Some("DDR4")),
            stick(8, "DIMM_B1", "", Some("DDR4")),
        ];
        assert_eq!(estimate_channels(&dual), Probe::yes(ChannelLayout::Multi));

        let same_channel = [stick(8, "DIMM_A1", "", None), stick(8, "DIMM_A2", "", None)];
        assert_eq!(estimate_channels(&same_channel), Probe::yes(ChannelLayout::Single));

        // Two sticks, labels say nothing: Unknown, never a guess.
        let opaque = [stick(8, "DIMM1", "BANK 0", None), stick(8, "DIMM2", "BANK 1", None)];
        assert!(estimate_channels(&opaque).is_unknown());

        // One stick of soldered LPDDR: not trusted.
        let lp = [stick(16, "Bottom-Slot 1", "", Some("LPDDR4"))];
        assert!(estimate_channels(&lp).is_unknown());

        assert!(estimate_channels(&[]).is_unknown());
    }

    fn mem_rows(sticks: &[(u64, &str)]) -> Result<Vec<WmiRow>> {
        let wmi = FakeWmi::new().with_rows(
            NS_CIMV2,
            WQL_MEMORY,
            sticks
                .iter()
                .map(|(gib, loc)| {
                    vec![
                        ("Capacity", WmiValue::Str((gib * GIB).to_string())),
                        ("Speed", WmiValue::UInt(3200)),
                        ("ConfiguredClockSpeed", WmiValue::UInt(2400)),
                        ("DeviceLocator", WmiValue::Str((*loc).into())),
                        ("BankLabel", WmiValue::Str("BANK 0".into())),
                        ("SMBIOSMemoryType", WmiValue::UInt(26)),
                    ]
                })
                .collect(),
        );
        wmi.query(NS_CIMV2, WQL_MEMORY)
    }

    #[test]
    fn memory_report_sums_capacity_and_reads_speeds_and_kind() {
        let p = memory_from(&mem_rows(&[(8, "DIMM_A1"), (8, "DIMM_B1")]));
        let m = p.value().expect("yes");
        assert_eq!(m.installed_bytes, 16 * GIB);
        assert_eq!(m.sticks[0].kind.as_deref(), Some("DDR4"));
        assert_eq!(m.sticks[0].rated_mhz, Some(3200));
        assert_eq!(m.sticks[0].configured_mhz, Some(2400));
        assert_eq!(m.channels, Probe::yes(ChannelLayout::Multi));

        let failed: Result<Vec<WmiRow>> = Err(EngineError::Internal { detail: "boom".into() });
        assert!(memory_from(&failed).is_unknown());
        assert!(memory_from(&Ok(vec![])).is_unknown());
    }

    fn gpu(name: &str, vram_gib: u64, software: bool) -> GpuAdapter {
        GpuAdapter {
            name: name.into(),
            vendor_id: 0x10de,
            dedicated_vram_bytes: vram_gib * GIB,
            shared_memory_bytes: 8 * GIB,
            is_software: software,
        }
    }

    #[test]
    fn software_renderers_are_not_gpus_and_the_biggest_comes_first() {
        let p = gpus_from(&Ok(vec![
            gpu("Microsoft Basic Render Driver", 0, true),
            gpu("Intel UHD", 0, false),
            gpu("RTX", 8, false),
        ]));
        let v = p.value().unwrap();
        assert_eq!(
            v.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            vec!["RTX", "Intel UHD"]
        );
        assert!(gpus_from(&Ok(vec![gpu("WARP", 0, true)])).is_no());
        assert!(gpus_from(&Err(EngineError::Internal { detail: "x".into() })).is_unknown());
    }

    fn cpu_probe(logical: u32) -> Probe<CpuInfo> {
        Probe::yes(CpuInfo {
            name: "cpu".into(),
            vendor: "v".into(),
            cores: logical / 2,
            logical_processors: logical,
        })
    }

    fn mem_probe(gib: u64) -> Probe<MemoryInfo> {
        Probe::yes(MemoryInfo {
            installed_bytes: gib * GIB,
            sticks: vec![],
            channels: Probe::unknown("-"),
        })
    }

    fn disk_probe(m: DiskMedia) -> Probe<BootDisk> {
        Probe::yes(BootDisk {
            media: m,
            name: "d".into(),
        })
    }

    fn display_probe(hz: u32) -> Probe<DisplayInfo> {
        Probe::yes(DisplayInfo {
            width: 1920,
            height: 1080,
            current_hz: 60,
            max_hz_at_current_resolution: hz,
        })
    }

    fn gpus_probe(vram_gib: u64) -> Probe<Vec<GpuAdapter>> {
        Probe::yes(vec![gpu("g", vram_gib, false)])
    }

    #[test]
    fn rig_class_table() {
        let mid = |mem, vram, logical, disk, hz| {
            classify(
                &mem_probe(mem),
                &gpus_probe(vram),
                &cpu_probe(logical),
                &disk_probe(disk),
                &display_probe(hz),
            )
        };

        // The typical Steam PC: 16 GB, 8 GB card, 8 threads, SSD, 60 Hz.
        assert_eq!(mid(16, 8, 8, DiskMedia::Ssd, 60), Probe::yes(RigClass::Mid));
        // Any single Low factor wins, even with everything else High.
        assert_eq!(mid(8, 16, 24, DiskMedia::Ssd, 240), Probe::yes(RigClass::Low));
        assert_eq!(mid(32, 4, 24, DiskMedia::Ssd, 240), Probe::yes(RigClass::Low));
        assert_eq!(mid(32, 16, 4, DiskMedia::Ssd, 240), Probe::yes(RigClass::Low));
        assert_eq!(mid(32, 16, 24, DiskMedia::Hdd, 240), Probe::yes(RigClass::Low));
        // Boundaries: 12 GB VRAM, 16 threads, 144 Hz is High; one short is Mid.
        assert_eq!(mid(32, 12, 16, DiskMedia::Ssd, 144), Probe::yes(RigClass::High));
        assert_eq!(mid(32, 11, 16, DiskMedia::Ssd, 144), Probe::yes(RigClass::Mid));
        assert_eq!(mid(32, 12, 15, DiskMedia::Ssd, 144), Probe::yes(RigClass::Mid));
        assert_eq!(mid(32, 12, 16, DiskMedia::Ssd, 143), Probe::yes(RigClass::Mid));
        // 12 GB of RAM is not Low.
        assert_eq!(mid(12, 8, 8, DiskMedia::Ssd, 60), Probe::yes(RigClass::Mid));
    }

    #[test]
    fn rig_class_admits_what_it_cannot_tell() {
        let unk = || Probe::<u8>::unknown("no");
        let _ = unk;
        // A known Low factor beats unknowns elsewhere.
        assert_eq!(
            classify(
                &mem_probe(8),
                &Probe::unknown("x"),
                &Probe::unknown("x"),
                &Probe::unknown("x"),
                &Probe::unknown("x")
            ),
            Probe::yes(RigClass::Low)
        );
        // No Low factor but the disk is unreadable: could still be Low.
        let p = classify(
            &mem_probe(16),
            &gpus_probe(8),
            &cpu_probe(8),
            &Probe::unknown("x"),
            &display_probe(60),
        );
        assert!(
            matches!(&p, Probe::Unknown { reason } if reason.contains("boot disk")),
            "{p:?}"
        );
        // Everything High-ish except the display is unreadable: cannot say Mid vs High.
        let p = classify(
            &mem_probe(32),
            &gpus_probe(16),
            &cpu_probe(24),
            &disk_probe(DiskMedia::Ssd),
            &Probe::unknown("x"),
        );
        assert!(p.is_unknown(), "{p:?}");
        // But if a High test already failed, unknown display does not matter: Mid.
        let p = classify(
            &mem_probe(32),
            &gpus_probe(8),
            &cpu_probe(24),
            &disk_probe(DiskMedia::Ssd),
            &Probe::unknown("x"),
        );
        assert_eq!(p, Probe::yes(RigClass::Mid));
        // No hardware GPU at all is Low.
        let p = classify(
            &mem_probe(32),
            &Probe::no("none"),
            &cpu_probe(24),
            &disk_probe(DiskMedia::Ssd),
            &display_probe(60),
        );
        assert_eq!(p, Probe::yes(RigClass::Low));
    }

    #[test]
    fn cpu_os_and_laptop_parsing() {
        let wmi = FakeWmi::new()
            .with_rows(
                NS_CIMV2,
                WQL_CPU,
                vec![vec![
                    ("Name", WmiValue::Str("  Ryzen 5 3600 ".into())),
                    ("Manufacturer", WmiValue::Str("AuthenticAMD".into())),
                    ("NumberOfCores", WmiValue::UInt(6)),
                    ("NumberOfLogicalProcessors", WmiValue::UInt(12)),
                ]],
            )
            .with_rows(
                NS_CIMV2,
                WQL_OS,
                vec![vec![
                    ("BuildNumber", WmiValue::Str("26100".into())),
                    ("Caption", WmiValue::Str("Microsoft Windows 11 Pro".into())),
                    ("ProductType", WmiValue::UInt(1)),
                ]],
            )
            .with_rows(NS_CIMV2, WQL_COMPUTER, vec![vec![("PCSystemType", WmiValue::UInt(2))]]);
        let cpu = cpu_from(&wmi.query(NS_CIMV2, WQL_CPU));
        assert_eq!(cpu.value().unwrap().name, "Ryzen 5 3600");
        assert_eq!(cpu.value().unwrap().logical_processors, 12);
        let os = os_from(&wmi.query(NS_CIMV2, WQL_OS));
        assert_eq!(os.value().unwrap().build, 26100);
        assert!(!os.value().unwrap().is_server);
        assert_eq!(laptop_from(&wmi.query(NS_CIMV2, WQL_COMPUTER)), Probe::yes(true));

        // Two sockets add up.
        let two = Ok(vec![
            WmiRow(
                [
                    ("NumberOfCores".into(), WmiValue::UInt(8)),
                    ("NumberOfLogicalProcessors".into(), WmiValue::UInt(16)),
                ]
                .into_iter()
                .collect(),
            ),
            WmiRow(
                [
                    ("NumberOfCores".into(), WmiValue::UInt(8)),
                    ("NumberOfLogicalProcessors".into(), WmiValue::UInt(16)),
                ]
                .into_iter()
                .collect(),
            ),
        ]);
        assert_eq!(cpu_from(&two).value().unwrap().logical_processors, 32);

        // Server SKU.
        let server = Ok(vec![WmiRow(
            [
                ("BuildNumber".into(), WmiValue::Str("26100".into())),
                ("ProductType".into(), WmiValue::UInt(3)),
            ]
            .into_iter()
            .collect(),
        )]);
        assert!(os_from(&server).value().unwrap().is_server);
    }

    #[test]
    fn boot_disk_walks_partition_to_physical_disk() {
        let assoc = "ASSOCIATORS OF {Win32_LogicalDisk.DeviceID='C:'} WHERE AssocClass = Win32_LogicalDiskToPartition";
        let q = "SELECT MediaType, FriendlyName FROM MSFT_PhysicalDisk WHERE DeviceId = '1'";
        let wmi = FakeWmi::new()
            .with_rows(NS_CIMV2, assoc, vec![vec![("DiskIndex", WmiValue::UInt(1))]])
            .with_rows(
                NS_STORAGE,
                q,
                vec![vec![
                    ("MediaType", WmiValue::UInt(3)),
                    ("FriendlyName", WmiValue::Str("WDC WD10".into())),
                ]],
            );
        let d = probe_boot_disk(&wmi, "C:");
        assert_eq!(d.value().unwrap().media, DiskMedia::Hdd);

        // MediaType 0 (unspecified, common on virtual disks): Unknown.
        let wmi = FakeWmi::new()
            .with_rows(NS_CIMV2, assoc, vec![vec![("DiskIndex", WmiValue::UInt(1))]])
            .with_rows(NS_STORAGE, q, vec![vec![("MediaType", WmiValue::UInt(0))]]);
        assert!(probe_boot_disk(&wmi, "C:").is_unknown());

        // Hostile or malformed drive strings never reach a query.
        let empty = FakeWmi::new();
        assert!(probe_boot_disk(&empty, "C:' OR 1=1 --").is_unknown());
        assert!(empty.calls.lock().unwrap().is_empty());
    }

    struct Facts;
    impl OsFacts for Facts {
        fn system_drive(&self) -> String {
            "C:".into()
        }
        fn display(&self) -> Result<DisplayInfo> {
            Ok(DisplayInfo {
                width: 2560,
                height: 1440,
                current_hz: 60,
                max_hz_at_current_resolution: 165,
            })
        }
        fn gpu_adapters(&self) -> Result<Vec<GpuAdapter>> {
            Ok(vec![gpu("RTX 3070", 8, false)])
        }
    }

    #[test]
    fn a_dead_wmi_degrades_to_unknowns_not_errors() {
        let r = probe_hardware(&FakeWmi::new(), &Facts);
        assert!(r.os.is_unknown() && r.cpu.is_unknown() && r.memory.is_unknown() && r.boot_disk.is_unknown());
        assert!(r.gpus.is_yes() && r.display.is_yes());
        // GPU and display are known, everything else is not: cannot rule out Low.
        assert!(r.rig_class.is_unknown());
    }
}
