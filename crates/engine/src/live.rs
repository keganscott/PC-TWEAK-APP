//! Live readings for Home: how busy the processor is (in all and per logical
//! processor), its speed, Windows' thermal zones, memory in use, and each
//! NVIDIA GPU's load, temperature, clock, power, fan and memory, read when
//! asked. Real readings only (plan section 7): what cannot be read is Unknown
//! with the reason, and nothing here is an estimate. AMD and Intel graphics
//! are not read yet. The processor's own temperature is not read: Windows
//! does not report it, and reading it needs a kernel driver (plan section 12).
//!
//! Reads only, and holds no engine lock, so a reading never waits on (or
//! delays) a change.

use serde::Serialize;
use ts_rs::TS;

use crate::memory::MemoryUse;
use crate::probe::Probe;
use crate::proof::nvml::GpuLive;

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct LiveReadings {
    /// Share of processor time not idle over the half second the reading
    /// took, across all cores, 0-100.
    pub cpu_busy_percent: Probe<u32>,
    /// The processor's speed over the same half second, in MHz, worked out
    /// as Task Manager does: its base frequency times `% Processor
    /// Performance` (`Processor Information(_Total)`).
    pub cpu_mhz: Probe<u32>,
    /// Each logical processor's `% Processor Utility` over the same half
    /// second, 0-100 (capped: with turbo it can pass 100), in Windows' order.
    pub cpu_cores: Probe<Vec<u32>>,
    /// Windows' ACPI thermal zones (`Thermal Zone Information`). Set by the
    /// motherboard's firmware; on many PCs none is the processor itself.
    pub thermal_zones: Probe<Vec<ThermalZone>>,
    /// The same reading as the standby-list action's (`memory.rs`).
    pub memory: Probe<MemoryUse>,
    /// Every NVIDIA GPU. No when there is none, Unknown when NVML could not
    /// be read (for example, no NVIDIA driver).
    pub gpus: Probe<Vec<GpuLive>>,
    #[ts(type = "number")]
    pub unix_ms: u64,
}

/// One ACPI thermal zone's temperature.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ThermalZone {
    /// Windows' name for the zone, such as `\_TZ.THRM`.
    pub name: String,
    /// Degrees Celsius, to a tenth.
    pub celsius: f64,
}

/// The processor's idle, kernel and user times since Windows started, in
/// 100 ns units (`GetSystemTimes`). Two of them give `busy_percent`.
pub type CpuTimes = (u64, u64, u64);

/// The processor's times now, for a busy share measured between two looks.
pub fn cpu_times() -> Probe<CpuTimes> {
    imp::cpu_times()
}

/// How long the processor is watched for one reading.
pub const CPU_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

/// Busy share from two `(idle, kernel, user)` totals, in any one unit.
/// Windows counts idle time inside kernel time.
pub fn busy_percent(before: (u64, u64, u64), after: (u64, u64, u64)) -> Option<u32> {
    let idle = after.0.checked_sub(before.0)?;
    let total = after.1.checked_sub(before.1)? + after.2.checked_sub(before.2)?;
    if total == 0 || idle > total {
        return None;
    }
    Some((((total - idle) * 100 + total / 2) / total) as u32)
}

/// Speed in MHz from the base frequency and `% Processor Performance`.
pub fn effective_mhz(base_mhz: f64, performance_percent: f64) -> Option<u32> {
    let mhz = base_mhz * performance_percent / 100.0;
    (base_mhz.is_finite() && mhz.is_finite() && base_mhz > 0.0 && mhz > 0.0 && mhz < 100_000.0)
        .then(|| mhz.round() as u32)
}

/// Each logical processor's share from a `Processor Information(*)` array:
/// the `_Total` rows left out, ordered by group then number (`"0,3"`), capped
/// at 100.
pub fn core_shares(items: &[(String, f64)]) -> Vec<u32> {
    let mut cores: Vec<((u32, u32), u32)> = items
        .iter()
        .filter(|(name, _)| !name.contains("_Total"))
        .filter_map(|(name, value)| {
            let (group, number) = name.split_once(',')?;
            let key = (group.trim().parse().ok()?, number.trim().parse().ok()?);
            value.is_finite().then(|| (key, value.clamp(0.0, 100.0).round() as u32))
        })
        .collect();
    cores.sort_by_key(|(key, _)| *key);
    cores.into_iter().map(|(_, share)| share).collect()
}

/// Thermal zones from `High Precision Temperature` (tenths of a kelvin).
/// A zone reading 0 K, or outside -30 to 150 °C, is left out: firmware fills
/// unused zones with such values.
pub fn thermal_zones(items: &[(String, f64)]) -> Vec<ThermalZone> {
    items
        .iter()
        .filter_map(|(name, tenths_kelvin)| {
            let celsius = tenths_kelvin / 10.0 - 273.15;
            (tenths_kelvin.is_finite() && *tenths_kelvin > 0.0 && (-30.0..=150.0).contains(&celsius)).then(|| {
                ThermalZone {
                    name: name.clone(),
                    celsius: (celsius * 10.0).round() / 10.0,
                }
            })
        })
        .collect()
}

/// Read everything once. Takes `CPU_WINDOW`.
pub fn read() -> LiveReadings {
    let unix_ms = crate::journal::now_ms();
    let cpu = imp::cpu();
    LiveReadings {
        cpu_busy_percent: cpu.busy,
        cpu_mhz: cpu.mhz,
        cpu_cores: cpu.cores,
        thermal_zones: cpu.thermal,
        memory: match crate::memory::system().usage() {
            Ok(m) => Probe::yes(m),
            Err(e) => Probe::unknown(e.to_string()),
        },
        gpus: imp::gpus(),
        unix_ms,
    }
}

/// The processor readings taken over one window.
struct Cpu {
    busy: Probe<u32>,
    mhz: Probe<u32>,
    cores: Probe<Vec<u32>>,
    thermal: Probe<Vec<ThermalZone>>,
}

#[cfg(windows)]
mod imp {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_NO_OBJECT,
        PDH_CSTATUS_VALID_DATA, PDH_FMT, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
        PDH_MORE_DATA,
    };
    use windows::Win32::System::Threading::GetSystemTimes;

    use super::*;

    fn times() -> windows::core::Result<(u64, u64, u64)> {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user))? };
        let n = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        Ok((n(idle), n(kernel), n(user)))
    }

    pub fn cpu_times() -> Probe<CpuTimes> {
        match times() {
            Ok(t) => Probe::yes(t),
            Err(e) => Probe::unknown(format!("GetSystemTimes failed: {e}")),
        }
    }

    /// Everything read over the one half-second window.
    pub fn cpu() -> Cpu {
        let mut pdh = match counters().lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let pdh = pdh.get_or_insert_with(Counters::open);
        pdh.collect();
        let before = times();
        std::thread::sleep(CPU_WINDOW);
        let after = times();
        pdh.collect();
        let busy = match (before, after) {
            (Ok(b), Ok(a)) => match busy_percent(b, a) {
                Some(p) => Probe::yes(p),
                None => Probe::unknown("the processor times did not move forward"),
            },
            (Err(e), _) | (_, Err(e)) => Probe::unknown(format!("GetSystemTimes failed: {e}")),
        };
        let mhz = match (pdh.value(&pdh.frequency), pdh.value(&pdh.performance)) {
            (Ok(base), Ok(perf)) => match effective_mhz(base, perf) {
                Some(m) => Probe::yes(m),
                None => Probe::unknown(format!("Windows gave {base} MHz at {perf}% performance")),
            },
            (Err(e), _) | (_, Err(e)) => Probe::unknown(e),
        };
        let cores = match pdh.array(&pdh.utility) {
            Ok(items) => match core_shares(&items) {
                c if c.is_empty() => Probe::unknown("Windows listed no logical processor"),
                c => Probe::yes(c),
            },
            Err(e) => Probe::unknown(e),
        };
        // Every Windows has `Processor Information`; without it the counters
        // as a whole are off (`Disable Performance Counters` under `Perflib`,
        // set by some tuning tools; seen on Kegan's PC, N125), so a missing
        // thermal zone set says nothing about the PC.
        let counters_off = matches!(pdh.utility, Err(Missing::NoObject));
        let thermal = match &pdh.thermal {
            Err(Missing::NoObject) if counters_off => Probe::unknown(COUNTERS_OFF),
            Err(Missing::NoObject) => Probe::no("Windows lists no thermal zone on this PC"),
            Err(Missing::Failed(e)) => Probe::unknown(e.clone()),
            Ok(_) => match pdh.array(&pdh.thermal) {
                Ok(items) if items.is_empty() => Probe::no("Windows lists no thermal zone on this PC"),
                Ok(items) => match thermal_zones(&items) {
                    z if z.is_empty() => Probe::unknown("no thermal zone gave a believable temperature"),
                    z => Probe::yes(z),
                },
                Err(e) => Probe::unknown(e),
            },
        };
        Cpu {
            busy,
            mhz,
            cores,
            thermal,
        }
    }

    const COUNTERS_OFF: &str = "Windows' performance counters gave nothing on this PC; another tuning tool may have \
         turned them off (Disable Performance Counters)";

    /// Why a counter is not there.
    enum Missing {
        /// The counter set does not exist on this PC.
        NoObject,
        Failed(String),
    }

    /// The performance counters, opened once and kept: the first collection
    /// of a rate counter only sets its starting point.
    struct Counters {
        query: isize,
        frequency: std::result::Result<isize, Missing>,
        performance: std::result::Result<isize, Missing>,
        utility: std::result::Result<isize, Missing>,
        thermal: std::result::Result<isize, Missing>,
    }

    // PDH handles may be used from any thread, one at a time (the mutex).
    unsafe impl Send for Counters {}

    fn counters() -> &'static std::sync::Mutex<Option<Counters>> {
        static COUNTERS: std::sync::OnceLock<std::sync::Mutex<Option<Counters>>> = std::sync::OnceLock::new();
        COUNTERS.get_or_init(|| std::sync::Mutex::new(None))
    }

    /// `% Processor Utility` can pass 100 with turbo; capped in `core_shares`.
    const NOCAP100: u32 = 0x8000;

    impl Counters {
        fn open() -> Self {
            let mut query = 0isize;
            // SAFETY: valid out-pointer; no data source means live data.
            let rc = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) };
            let add = |path: &str| -> std::result::Result<isize, Missing> {
                if rc != 0 {
                    return Err(Missing::Failed(format!("PdhOpenQueryW failed with 0x{rc:08X}")));
                }
                let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
                let mut counter = 0isize;
                // SAFETY: the path is NUL-terminated and outlives the call.
                match unsafe { PdhAddEnglishCounterW(query, PCWSTR(wide.as_ptr()), 0, &mut counter) } {
                    0 => Ok(counter),
                    PDH_CSTATUS_NO_OBJECT => Err(Missing::NoObject),
                    e => Err(Missing::Failed(format!(
                        "Windows could not add the counter {path} (0x{e:08X})"
                    ))),
                }
            };
            Counters {
                frequency: add(r"\Processor Information(_Total)\Processor Frequency"),
                performance: add(r"\Processor Information(_Total)\% Processor Performance"),
                utility: add(r"\Processor Information(*)\% Processor Utility"),
                thermal: add(r"\Thermal Zone Information(*)\High Precision Temperature"),
                query,
            }
        }

        fn collect(&self) {
            // SAFETY: the query handle is ours; a failure shows up as invalid data below.
            unsafe { PdhCollectQueryData(self.query) };
        }

        fn value(&self, counter: &std::result::Result<isize, Missing>) -> std::result::Result<f64, String> {
            let counter = match counter {
                Ok(c) => *c,
                Err(Missing::NoObject) => return Err(COUNTERS_OFF.into()),
                Err(Missing::Failed(e)) => return Err(e.clone()),
            };
            let mut value = PDH_FMT_COUNTERVALUE::default();
            // SAFETY: valid counter handle and out-pointer.
            let rc =
                unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT(PDH_FMT_DOUBLE.0 | NOCAP100), None, &mut value) };
            if rc != 0 || !matches!(value.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA) {
                return Err(format!(
                    "the counter gave no value (0x{rc:08X}, status 0x{:08X})",
                    value.CStatus
                ));
            }
            // SAFETY: PDH_FMT_DOUBLE was asked for.
            Ok(unsafe { value.Anonymous.doubleValue })
        }

        /// Every instance of a wildcard counter, as (name, value).
        fn array(
            &self,
            counter: &std::result::Result<isize, Missing>,
        ) -> std::result::Result<Vec<(String, f64)>, String> {
            let counter = match counter {
                Ok(c) => *c,
                Err(Missing::NoObject) => return Err(COUNTERS_OFF.into()),
                Err(Missing::Failed(e)) => return Err(e.clone()),
            };
            let format = PDH_FMT(PDH_FMT_DOUBLE.0 | NOCAP100);
            let (mut bytes, mut count) = (0u32, 0u32);
            // SAFETY: a null buffer asks for the size.
            let rc = unsafe { PdhGetFormattedCounterArrayW(counter, format, &mut bytes, &mut count, None) };
            if rc != PDH_MORE_DATA {
                return if rc == 0 {
                    Ok(Vec::new())
                } else {
                    Err(format!("the counter gave no values (0x{rc:08X})"))
                };
            }
            // u64 storage keeps the items (which hold an f64) aligned.
            let mut buf = vec![0u64; (bytes as usize).div_ceil(8)];
            let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            // SAFETY: the buffer is the size PDH asked for.
            let rc = unsafe { PdhGetFormattedCounterArrayW(counter, format, &mut bytes, &mut count, Some(items)) };
            if rc != 0 {
                return Err(format!("the counter gave no values (0x{rc:08X})"));
            }
            let mut out = Vec::with_capacity(count as usize);
            for i in 0..count as usize {
                // SAFETY: PDH wrote `count` items, whose names point into the same buffer.
                let item = unsafe { &*items.add(i) };
                if !matches!(item.FmtValue.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA) {
                    continue;
                }
                let name = unsafe { item.szName.to_string() }.unwrap_or_default();
                out.push((name, unsafe { item.FmtValue.Anonymous.doubleValue }));
            }
            Ok(out)
        }
    }

    impl Drop for Counters {
        fn drop(&mut self) {
            // SAFETY: closes the query and its counters once.
            unsafe { PdhCloseQuery(self.query) };
        }
    }

    pub fn gpus() -> Probe<Vec<GpuLive>> {
        crate::proof::nvml::shared().live()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    fn off<T>() -> Probe<T> {
        Probe::unknown("live readings are only taken on Windows")
    }

    pub fn cpu() -> Cpu {
        Cpu {
            busy: off(),
            mhz: off(),
            cores: off(),
            thermal: off(),
        }
    }
    pub fn cpu_times() -> Probe<CpuTimes> {
        Probe::unknown("live readings are only taken on Windows")
    }
    pub fn gpus() -> Probe<Vec<GpuLive>> {
        Probe::unknown("live readings are only taken on Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_share_counts_idle_inside_kernel_time() {
        // 100 ticks passed: 60 kernel (of which 50 idle) and 40 user.
        assert_eq!(busy_percent((0, 0, 0), (50, 60, 40)), Some(50));
        assert_eq!(busy_percent((10, 10, 10), (10, 20, 10)), Some(100));
        assert_eq!(busy_percent((0, 0, 0), (100, 100, 0)), Some(0));
    }

    #[test]
    fn times_that_did_not_move_or_went_back_give_no_share() {
        assert_eq!(busy_percent((5, 5, 5), (5, 5, 5)), None);
        assert_eq!(busy_percent((5, 5, 5), (4, 6, 6)), None);
        assert_eq!(busy_percent((0, 0, 0), (9, 5, 0)), None, "idle above total");
    }

    #[test]
    fn speed_is_base_times_performance() {
        assert_eq!(effective_mhz(3600.0, 125.5), Some(4518));
        assert_eq!(effective_mhz(3600.0, 0.0), None);
        assert_eq!(effective_mhz(0.0, 100.0), None);
        assert_eq!(effective_mhz(f64::NAN, 100.0), None);
    }

    #[test]
    fn cores_leave_out_totals_and_sort_by_group_then_number() {
        let items = [
            ("0,10".to_string(), 12.4),
            ("_Total".to_string(), 50.0),
            ("0,_Total".to_string(), 50.0),
            ("0,2".to_string(), 140.0),
            ("1,0".to_string(), 3.0),
            ("0,1".to_string(), -1.0),
        ];
        assert_eq!(core_shares(&items), vec![0, 100, 12, 3]);
    }

    #[test]
    fn thermal_zones_turn_tenths_of_kelvin_into_celsius_and_drop_unused_ones() {
        let items = [
            (r"\_TZ.THRM".to_string(), 3231.5),
            (r"\_TZ.TZ00".to_string(), 0.0),
            (r"\_TZ.TZ01".to_string(), 2731.5 + 2000.0),
            (r"\_TZ.TZ02".to_string(), 2981.5),
        ];
        let zones = thermal_zones(&items);
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].celsius, 50.0);
        assert_eq!(zones[1].celsius, 25.0);
    }

    /// On Windows CI: every reading answers, and the processor, per-core and
    /// memory readings are real numbers in range. Speed and thermal zones are
    /// printed (a virtual machine may report neither).
    #[cfg(windows)]
    #[test]
    fn this_pc_gives_live_readings() {
        let r = read();
        println!("live readings on this runner: {r:?}");
        match r.cpu_busy_percent {
            Probe::Yes { value } => assert!(value <= 100),
            other => panic!("processor not read: {other:?}"),
        }
        match r.memory {
            Probe::Yes { value } => assert!(value.total_bytes > 0 && value.available_bytes <= value.total_bytes),
            other => panic!("memory not read: {other:?}"),
        }
        match r.cpu_cores {
            Probe::Yes { value } => assert!(!value.is_empty() && value.iter().all(|c| *c <= 100)),
            other => panic!("logical processors not read: {other:?}"),
        }
    }
}
