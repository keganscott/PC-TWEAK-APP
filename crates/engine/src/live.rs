//! Live readings for Home: how busy the processor is, memory in use, and each
//! NVIDIA GPU's load, temperature and memory, read when asked. Real readings
//! only (plan section 7): what cannot be read is Unknown with the reason, and
//! nothing here is an estimate. AMD and Intel graphics are not read yet.
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
    /// The same reading as the standby-list action's (`memory.rs`).
    pub memory: Probe<MemoryUse>,
    /// Every NVIDIA GPU. No when there is none, Unknown when NVML could not
    /// be read (for example, no NVIDIA driver).
    pub gpus: Probe<Vec<GpuLive>>,
    #[ts(type = "number")]
    pub unix_ms: u64,
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

/// Read everything once. Takes `CPU_WINDOW`.
pub fn read() -> LiveReadings {
    let unix_ms = crate::journal::now_ms();
    LiveReadings {
        cpu_busy_percent: imp::cpu_busy(),
        memory: match crate::memory::system().usage() {
            Ok(m) => Probe::yes(m),
            Err(e) => Probe::unknown(e.to_string()),
        },
        gpus: imp::gpus(),
        unix_ms,
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;

    use super::*;

    fn times() -> windows::core::Result<(u64, u64, u64)> {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user))? };
        let n = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        Ok((n(idle), n(kernel), n(user)))
    }

    pub fn cpu_busy() -> Probe<u32> {
        let before = match times() {
            Ok(t) => t,
            Err(e) => return Probe::unknown(format!("GetSystemTimes failed: {e}")),
        };
        std::thread::sleep(CPU_WINDOW);
        match times() {
            Ok(after) => match busy_percent(before, after) {
                Some(p) => Probe::yes(p),
                None => Probe::unknown("the processor times did not move forward"),
            },
            Err(e) => Probe::unknown(format!("GetSystemTimes failed: {e}")),
        }
    }

    pub fn gpus() -> Probe<Vec<GpuLive>> {
        crate::proof::nvml::shared().live()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn cpu_busy() -> Probe<u32> {
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

    /// On Windows CI: every reading answers, and the processor and memory
    /// readings are real numbers in range.
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
    }
}
