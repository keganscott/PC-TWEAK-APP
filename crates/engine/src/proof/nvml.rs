//! NVIDIA GPU throttle reasons, sampled while a capture runs.
//!
//! `nvml.dll` ships with NVIDIA's driver, so it is loaded at run time (never
//! bundled) and its absence is an ordinary answer: "not available". AMD and
//! Intel have no equivalent wired up yet; they report Unknown, they are not
//! guessed at.
//!
//! The bit values are from NVML's `nvmlClocksThrottleReason*` constants, from
//! memory and marked VERIFY in NOTES.md (N33).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::probe::Probe;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ThrottleReason {
    GpuIdle,
    ApplicationClocks,
    SoftwarePowerCap,
    HardwareSlowdown,
    SyncBoost,
    SoftwareThermalSlowdown,
    HardwareThermalSlowdown,
    HardwarePowerBrake,
    DisplayClock,
}

/// (bit, reason). VERIFY against nvml.h.
pub const REASON_BITS: [(u64, ThrottleReason); 9] = [
    (0x1, ThrottleReason::GpuIdle),
    (0x2, ThrottleReason::ApplicationClocks),
    (0x4, ThrottleReason::SoftwarePowerCap),
    (0x8, ThrottleReason::HardwareSlowdown),
    (0x10, ThrottleReason::SyncBoost),
    (0x20, ThrottleReason::SoftwareThermalSlowdown),
    (0x40, ThrottleReason::HardwareThermalSlowdown),
    (0x80, ThrottleReason::HardwarePowerBrake),
    (0x100, ThrottleReason::DisplayClock),
];

pub fn decode_reasons(mask: u64) -> Vec<ThrottleReason> {
    REASON_BITS
        .iter()
        .filter(|(bit, _)| mask & bit != 0)
        .map(|(_, r)| *r)
        .collect()
}

impl ThrottleReason {
    /// Reasons that mean the GPU is running slower than it could because of
    /// power or heat, which makes a run unreliable as a measure of a setting.
    /// Idle, app-set clocks, sync boost and display clocks are not problems.
    pub fn limits_performance(self) -> bool {
        matches!(
            self,
            Self::SoftwarePowerCap
                | Self::HardwareSlowdown
                | Self::SoftwareThermalSlowdown
                | Self::HardwareThermalSlowdown
                | Self::HardwarePowerBrake
        )
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::GpuIdle => "the GPU was idle",
            Self::ApplicationClocks => "clocks were set by an application",
            Self::SoftwarePowerCap => "the driver held clocks down at its power limit",
            Self::HardwareSlowdown => "the GPU hardware slowed itself down",
            Self::SyncBoost => "clocks were synchronised with other GPUs",
            Self::SoftwareThermalSlowdown => "the driver held clocks down because of heat",
            Self::HardwareThermalSlowdown => "the GPU hardware slowed itself down because of heat",
            Self::HardwarePowerBrake => "an external power brake slowed the GPU",
            Self::DisplayClock => "clocks were limited by the display",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ThrottleSeen {
    pub reason: ThrottleReason,
    /// In how many samples this reason was active.
    pub samples: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ThrottleSummary {
    /// How many times the GPU was asked.
    pub samples: u32,
    pub seen: Vec<ThrottleSeen>,
}

impl ThrottleSummary {
    /// Reasons that limit performance and were seen at least once.
    pub fn limiting(&self) -> Vec<ThrottleReason> {
        self.seen
            .iter()
            .filter(|s| s.reason.limits_performance())
            .map(|s| s.reason)
            .collect()
    }
}

/// One instantaneous reading of the reasons active on any NVIDIA GPU.
pub trait ThrottleSampler: Send + Sync {
    fn sample(&self) -> Probe<Vec<ThrottleReason>>;
}

/// Used where no sampler is available or wanted.
pub struct NoSampler(pub String);

impl ThrottleSampler for NoSampler {
    fn sample(&self) -> Probe<Vec<ThrottleReason>> {
        Probe::unknown(self.0.clone())
    }
}

/// Accumulates samples into a summary.
#[derive(Default)]
pub struct ThrottleTally {
    samples: u32,
    counts: BTreeMap<ThrottleReason, u32>,
    unavailable: Option<String>,
}

impl ThrottleTally {
    pub fn add(&mut self, reading: Probe<Vec<ThrottleReason>>) {
        match reading {
            Probe::Yes { value } => {
                self.samples += 1;
                for r in value {
                    *self.counts.entry(r).or_default() += 1;
                }
            }
            Probe::No { reason } | Probe::Unknown { reason } => {
                self.unavailable.get_or_insert(reason);
            }
        }
    }

    pub fn finish(self) -> Probe<ThrottleSummary> {
        if self.samples == 0 {
            return Probe::unknown(
                self.unavailable
                    .unwrap_or_else(|| "no GPU readings were taken during this run".into()),
            );
        }
        Probe::yes(ThrottleSummary {
            samples: self.samples,
            seen: self
                .counts
                .into_iter()
                .map(|(reason, samples)| ThrottleSeen { reason, samples })
                .collect(),
        })
    }
}

#[cfg(windows)]
pub use real::NvmlSampler;

#[cfg(windows)]
mod real {
    use std::ffi::c_void;
    use std::sync::Mutex;

    use windows::core::{s, w, PCWSTR};
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };

    use super::*;

    type NvmlReturn = i32;
    const NVML_SUCCESS: NvmlReturn = 0;

    type Init = unsafe extern "C" fn() -> NvmlReturn;
    type Count = unsafe extern "C" fn(*mut u32) -> NvmlReturn;
    type HandleByIndex = unsafe extern "C" fn(u32, *mut *mut c_void) -> NvmlReturn;
    type Reasons = unsafe extern "C" fn(*mut c_void, *mut u64) -> NvmlReturn;

    struct Lib {
        get_count: Count,
        get_handle: HandleByIndex,
        get_reasons: Reasons,
        // The module stays loaded for the life of the process.
        _module: HMODULE,
    }

    // Raw function pointers into a DLL that stays loaded are safe to move between
    // threads; NVML documents its API as thread safe.
    unsafe impl Send for Lib {}

    /// Reads throttle reasons from NVIDIA's NVML. Loads the library and
    /// initialises NVML on first use and keeps both for the process lifetime.
    pub struct NvmlSampler {
        state: Mutex<Option<std::result::Result<Lib, String>>>,
    }

    impl Default for NvmlSampler {
        fn default() -> Self {
            Self::new()
        }
    }

    impl NvmlSampler {
        pub fn new() -> Self {
            Self {
                state: Mutex::new(None),
            }
        }
    }

    unsafe fn load_module() -> Option<HMODULE> {
        // System32 first (where current drivers put it); NVIDIA's own folder is
        // admin-writable only, so it is an acceptable second place.
        if let Ok(h) = LoadLibraryExW(w!("nvml.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) {
            return Some(h);
        }
        let program_files = std::env::var_os("ProgramFiles")?;
        let path = std::path::Path::new(&program_files).join(r"NVIDIA Corporation\NVSMI\nvml.dll");
        if !path.is_file() {
            return None;
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide_nul();
        LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS).ok()
    }

    trait EncodeWideNul {
        fn encode_wide_nul(&self) -> Vec<u16>;
    }
    impl EncodeWideNul for std::ffi::OsStr {
        fn encode_wide_nul(&self) -> Vec<u16> {
            use std::os::windows::ffi::OsStrExt;
            self.encode_wide().chain(std::iter::once(0)).collect()
        }
    }

    unsafe fn open() -> std::result::Result<Lib, String> {
        let module =
            load_module().ok_or("nvml.dll was not found: this is not an NVIDIA GPU, or its driver is missing")?;
        macro_rules! sym {
            ($name:literal, $ty:ty) => {
                match GetProcAddress(module, s!($name)) {
                    Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(f),
                    None => return Err(concat!("nvml.dll does not export ", $name).into()),
                }
            };
        }
        let init: Init = sym!("nvmlInit_v2", Init);
        let get_count: Count = sym!("nvmlDeviceGetCount_v2", Count);
        let get_handle: HandleByIndex = sym!("nvmlDeviceGetHandleByIndex_v2", HandleByIndex);
        let get_reasons: Reasons = sym!("nvmlDeviceGetCurrentClocksThrottleReasons", Reasons);
        let rc = init();
        if rc != NVML_SUCCESS {
            return Err(format!("nvmlInit_v2 failed with code {rc}"));
        }
        Ok(Lib {
            get_count,
            get_handle,
            get_reasons,
            _module: module,
        })
    }

    impl ThrottleSampler for NvmlSampler {
        fn sample(&self) -> Probe<Vec<ThrottleReason>> {
            let mut guard = match self.state.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let lib = match guard.get_or_insert_with(|| unsafe { open() }) {
                Ok(l) => l,
                Err(e) => return Probe::unknown(e.clone()),
            };
            unsafe {
                let mut count = 0u32;
                let rc = (lib.get_count)(&mut count);
                if rc != NVML_SUCCESS {
                    return Probe::unknown(format!("nvmlDeviceGetCount_v2 failed with code {rc}"));
                }
                if count == 0 {
                    return Probe::no("NVML reports no NVIDIA GPU");
                }
                let mut all = std::collections::BTreeSet::new();
                for i in 0..count {
                    let mut handle: *mut c_void = std::ptr::null_mut();
                    let rc = (lib.get_handle)(i, &mut handle);
                    if rc != NVML_SUCCESS {
                        return Probe::unknown(format!("nvmlDeviceGetHandleByIndex_v2({i}) failed with code {rc}"));
                    }
                    let mut mask = 0u64;
                    let rc = (lib.get_reasons)(handle, &mut mask);
                    if rc != NVML_SUCCESS {
                        return Probe::unknown(format!(
                            "nvmlDeviceGetCurrentClocksThrottleReasons failed with code {rc}"
                        ));
                    }
                    all.extend(decode_reasons(mask));
                }
                Probe::yes(all.into_iter().collect())
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Records what NVML does on this machine. A runner without an NVIDIA
        /// GPU must answer Unknown/No with a reason; one with a GPU answers Yes.
        #[test]
        fn the_real_sampler_answers_without_panicking() {
            let s = NvmlSampler::new();
            let first = s.sample();
            let second = s.sample();
            println!("nvml sample: {first:?}");
            assert_eq!(first.is_yes(), second.is_yes());
            if let Probe::Unknown { reason } | Probe::No { reason } = &first {
                assert!(!reason.is_empty());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_decode_to_reasons_and_unknown_bits_are_ignored() {
        assert_eq!(decode_reasons(0), vec![]);
        assert_eq!(decode_reasons(0x1), vec![ThrottleReason::GpuIdle]);
        assert_eq!(
            decode_reasons(0x4 | 0x40),
            vec![
                ThrottleReason::SoftwarePowerCap,
                ThrottleReason::HardwareThermalSlowdown
            ]
        );
        assert_eq!(
            decode_reasons(0x8000).len(),
            0,
            "a bit we do not know is not invented into a reason"
        );
        assert_eq!(decode_reasons(0x1FF).len(), 9);
    }

    #[test]
    fn only_power_and_heat_reasons_limit_performance() {
        for (_, r) in REASON_BITS {
            let expected = matches!(
                r,
                ThrottleReason::SoftwarePowerCap
                    | ThrottleReason::HardwareSlowdown
                    | ThrottleReason::SoftwareThermalSlowdown
                    | ThrottleReason::HardwareThermalSlowdown
                    | ThrottleReason::HardwarePowerBrake
            );
            assert_eq!(r.limits_performance(), expected, "{r:?}");
            assert!(!r.describe().is_empty());
        }
    }

    #[test]
    fn a_tally_counts_samples_per_reason() {
        let mut t = ThrottleTally::default();
        t.add(Probe::yes(vec![ThrottleReason::SoftwarePowerCap]));
        t.add(Probe::yes(vec![
            ThrottleReason::SoftwarePowerCap,
            ThrottleReason::GpuIdle,
        ]));
        t.add(Probe::yes(vec![]));
        let s = t.finish();
        let sum = s.value().unwrap();
        assert_eq!(sum.samples, 3);
        assert_eq!(
            sum.seen,
            vec![
                ThrottleSeen {
                    reason: ThrottleReason::GpuIdle,
                    samples: 1
                },
                ThrottleSeen {
                    reason: ThrottleReason::SoftwarePowerCap,
                    samples: 2
                },
            ]
        );
        assert_eq!(sum.limiting(), vec![ThrottleReason::SoftwarePowerCap]);
    }

    #[test]
    fn no_readings_is_unknown_with_the_reason_never_an_empty_yes() {
        let mut t = ThrottleTally::default();
        t.add(Probe::unknown("nvml.dll was not found"));
        t.add(Probe::unknown("second reason is ignored"));
        assert!(matches!(t.finish(), Probe::Unknown { reason } if reason.contains("not found")));
        assert!(ThrottleTally::default().finish().is_unknown());
        assert!(NoSampler("x".into()).sample().is_unknown());
    }
}
