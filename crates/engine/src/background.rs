//! What else is running while PeakTweaks scans: plan 6.2 item 9, the read-only
//! half ("list top offenders at idle"). Offering to demote known-safe
//! launchers and updaters needs the known-safe list from `tweak-dictionary.md`
//! (NOTES.md N2) and is not built.
//!
//! Two snapshots of every process's CPU time, a moment apart, give each
//! program's share of the whole machine over that moment.
//!
//! The snapshots come from Windows' own performance counters through WMI
//! (`Win32_PerfRawData_PerfProc_Process`), not from opening each process.
//! Opening a handle to every running process would include a running game,
//! and anti-cheat systems watch for exactly that (plan section 12: no game
//! process access of any kind).

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;
use ts_rs::TS;

use super::error::{EngineError, Result};
use super::probe::Probe;
use super::wmi::WmiRow;

pub(crate) const WQL_PROCESSES: &str = "SELECT Name, IDProcess, CreatingProcessID, PercentProcessorTime, \
     WorkingSetPrivate, Timestamp_Sys100NS FROM Win32_PerfRawData_PerfProc_Process";

/// Used when the performance counters cannot be read. Some tuning tools set
/// `Disable Performance Counters` under `Perflib`, and then every
/// `Win32_PerfRawData` query fails with "invalid query" (0x80041017); seen on
/// Kegan's PC in the field check of 2026-10-15. `Win32_Process` comes from
/// WMI's own process provider, so this still opens no process from PeakTweaks.
pub(crate) const WQL_PROCESSES_FALLBACK: &str = "SELECT Name, ProcessId, ParentProcessId, KernelModeTime, \
     UserModeTime, WorkingSetSize FROM Win32_Process";

/// One process at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcTimes {
    pub pid: u32,
    pub parent_pid: u32,
    /// Executable name, e.g. `chrome.exe`.
    pub name: String,
    /// Kernel + user CPU time so far, in 100 ns units. `None` when the process
    /// could not be opened (protected processes).
    pub cpu_100ns: Option<u64>,
    pub working_set_bytes: Option<u64>,
}

/// Every process at one moment, with the wall clock in 100 ns units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub wall_100ns: u64,
    pub procs: Vec<ProcTimes>,
}

/// One program (all processes with the same executable name).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProgramLoad {
    pub name: String,
    pub processes: u32,
    /// Share of the whole machine's CPU over the sample, 0 to 100.
    pub cpu_percent: f64,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundLoad {
    /// How long the sample lasted.
    pub sample_ms: u32,
    /// Everything except PeakTweaks itself, 0 to 100.
    pub cpu_percent: f64,
    /// The busiest programs, busiest first.
    pub top: Vec<ProgramLoad>,
    /// Processes whose CPU time could not be read (protected), so they are
    /// not counted.
    pub unreadable: u32,
}

/// One snapshot from the performance-counter rows. `PercentProcessorTime` is
/// raw CPU time in 100 ns units (all cores added up); `Timestamp_Sys100NS` is
/// the clock the counters were read at. The `_Total` row is a sum, not a
/// process. Instance names are the executable without `.exe`, with `#n` added
/// for the second and later copies (`chrome#3`), which is stripped.
pub fn snapshot_from(rows: &Result<Vec<WmiRow>>) -> Result<ProcessSnapshot> {
    let rows = rows
        .as_ref()
        .map_err(|e| EngineError::Internal { detail: e.to_string() })?;
    let wall_100ns = rows
        .iter()
        .find_map(|r| r.u64("Timestamp_Sys100NS"))
        .ok_or_else(|| EngineError::Internal {
            detail: "the process counters carried no timestamp".into(),
        })?;
    let procs = rows
        .iter()
        .filter_map(|r| {
            let raw = r.str("Name")?;
            if raw.eq_ignore_ascii_case("_Total") {
                return None;
            }
            let name = raw.split('#').next().unwrap_or(raw).to_owned();
            Some(ProcTimes {
                pid: u32::try_from(r.u64("IDProcess")?).ok()?,
                parent_pid: r
                    .u64("CreatingProcessID")
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or(0),
                name,
                cpu_100ns: r.u64("PercentProcessorTime"),
                working_set_bytes: r.u64("WorkingSetPrivate"),
            })
        })
        .collect();
    Ok(ProcessSnapshot { wall_100ns, procs })
}

/// One snapshot from `Win32_Process` rows (the fallback). CPU time is
/// `KernelModeTime + UserModeTime` (100 ns units); these rows carry no clock,
/// so the caller passes the wall clock. Names lose `.exe` so they read the
/// same as the counters' instance names. Memory is the whole working set,
/// not only the private part the counters give.
pub fn snapshot_from_processes(rows: &Result<Vec<WmiRow>>, wall_100ns: u64) -> Result<ProcessSnapshot> {
    let rows = rows
        .as_ref()
        .map_err(|e| EngineError::Internal { detail: e.to_string() })?;
    let procs = rows
        .iter()
        .filter_map(|r| {
            let raw = r.str("Name")?;
            let name = match raw.len().checked_sub(4) {
                Some(cut) if raw.is_char_boundary(cut) && raw[cut..].eq_ignore_ascii_case(".exe") => &raw[..cut],
                _ => raw,
            };
            Some(ProcTimes {
                pid: u32::try_from(r.u64("ProcessId")?).ok()?,
                parent_pid: r
                    .u64("ParentProcessId")
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or(0),
                name: name.to_owned(),
                cpu_100ns: match (r.u64("KernelModeTime"), r.u64("UserModeTime")) {
                    (Some(k), Some(u)) => Some(k.saturating_add(u)),
                    _ => None,
                },
                working_set_bytes: r.u64("WorkingSetSize"),
            })
        })
        .collect();
    Ok(ProcessSnapshot { wall_100ns, procs })
}

/// A snapshot from the counters, or from `Win32_Process` when the counters
/// cannot be read. Once it has fallen back it stays there (`fell_back`), so
/// the two snapshots of one sample come from the same source.
pub fn snapshot_via(
    query: &dyn Fn(&str) -> Result<Vec<WmiRow>>,
    wall_100ns: &dyn Fn() -> u64,
    fell_back: &std::cell::Cell<bool>,
) -> Result<ProcessSnapshot> {
    if !fell_back.get() {
        match snapshot_from(&query(WQL_PROCESSES)) {
            Ok(s) => return Ok(s),
            Err(counters) => {
                fell_back.set(true);
                return snapshot_from_processes(&query(WQL_PROCESSES_FALLBACK), wall_100ns()).map_err(|e| {
                    EngineError::Internal {
                        detail: format!("performance counters: {counters}; Win32_Process: {e}"),
                    }
                });
            }
        }
    }
    snapshot_from_processes(&query(WQL_PROCESSES_FALLBACK), wall_100ns())
}

/// The wall clock in 100 ns units, for [`snapshot_from_processes`].
pub fn wall_clock_100ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_nanos() / 100).min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

/// How many programs a finding lists.
pub const TOP: usize = 5;

/// Share of the machine between two snapshots. `own_pid` and everything it
/// started (WebView2) are left out: the scan should not report itself.
pub fn load_between(
    a: &ProcessSnapshot,
    b: &ProcessSnapshot,
    logical_processors: u32,
    own_pid: u32,
) -> Probe<BackgroundLoad> {
    let elapsed = b.wall_100ns.saturating_sub(a.wall_100ns);
    if elapsed == 0 || logical_processors == 0 {
        return Probe::unknown("the two process samples were taken at the same moment");
    }
    let capacity = elapsed as f64 * logical_processors as f64;

    // PeakTweaks and its descendants, by the parent links in the later sample.
    let mut ours: HashSet<u32> = HashSet::from([own_pid]);
    loop {
        let before = ours.len();
        for p in &b.procs {
            if ours.contains(&p.parent_pid) && p.pid != 0 {
                ours.insert(p.pid);
            }
        }
        if ours.len() == before {
            break;
        }
    }

    let earlier: HashMap<(u32, &str), Option<u64>> = a
        .procs
        .iter()
        .map(|p| ((p.pid, p.name.as_str()), p.cpu_100ns))
        .collect();
    let mut programs: BTreeMap<String, ProgramLoad> = BTreeMap::new();
    let mut total = 0.0;
    let mut unreadable = 0;
    for p in &b.procs {
        // pid 0 is the System Idle Process: its "CPU time" is idleness.
        if p.pid == 0 || ours.contains(&p.pid) {
            continue;
        }
        // A pid that is new, reused by another program, or unreadable in
        // either sample has no comparable CPU time.
        let delta = match (earlier.get(&(p.pid, p.name.as_str())), p.cpu_100ns) {
            (Some(Some(before)), Some(after)) => after.saturating_sub(*before),
            (Some(_), None) | (Some(None), _) => {
                unreadable += 1;
                continue;
            }
            (None, _) => continue,
        };
        let percent = delta as f64 / capacity * 100.0;
        total += percent;
        let entry = programs
            .entry(p.name.to_ascii_lowercase())
            .or_insert_with(|| ProgramLoad {
                name: p.name.clone(),
                processes: 0,
                cpu_percent: 0.0,
                memory_bytes: 0,
            });
        entry.processes += 1;
        entry.cpu_percent += percent;
        entry.memory_bytes += p.working_set_bytes.unwrap_or(0);
    }

    let mut top: Vec<ProgramLoad> = programs.into_values().collect();
    top.sort_by(|x, y| {
        y.cpu_percent
            .total_cmp(&x.cpu_percent)
            .then_with(|| x.name.cmp(&y.name))
    });
    top.truncate(TOP);
    for t in &mut top {
        t.cpu_percent = round1(t.cpu_percent);
    }
    Probe::yes(BackgroundLoad {
        sample_ms: (elapsed / 10_000).min(u32::MAX as u64) as u32,
        cpu_percent: round1(total.min(100.0)),
        top,
        unreadable,
    })
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Take a snapshot, wait, take another, and compare.
pub fn sample(
    snapshot: &dyn Fn() -> Result<ProcessSnapshot>,
    wait: std::time::Duration,
    logical_processors: Option<u32>,
) -> Probe<BackgroundLoad> {
    let Some(logical) = logical_processors else {
        return Probe::unknown("the number of logical processors is not known");
    };
    let a = match snapshot() {
        Ok(s) => s,
        Err(e) => return Probe::unknown(format!("cannot list running programs: {e}")),
    };
    std::thread::sleep(wait);
    match snapshot() {
        Ok(b) => load_between(&a, &b, logical, std::process::id()),
        Err(e) => Probe::unknown(format!("cannot list running programs: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, name: &str, cpu: Option<u64>, mem: u64) -> ProcTimes {
        ProcTimes {
            pid,
            parent_pid: parent,
            name: name.into(),
            cpu_100ns: cpu,
            working_set_bytes: Some(mem),
        }
    }

    /// One second apart on a 4-thread machine: 4e7 units of CPU capacity.
    fn snaps(before: Vec<ProcTimes>, after: Vec<ProcTimes>) -> (ProcessSnapshot, ProcessSnapshot) {
        (
            ProcessSnapshot {
                wall_100ns: 1_000_000_000,
                procs: before,
            },
            ProcessSnapshot {
                wall_100ns: 1_010_000_000,
                procs: after,
            },
        )
    }

    #[test]
    fn programs_are_summed_by_name_ranked_and_measured_against_the_whole_machine() {
        let (a, b) = snaps(
            vec![
                p(10, 1, "chrome.exe", Some(0), 0),
                p(11, 10, "chrome.exe", Some(0), 0),
                p(20, 1, "Updater.exe", Some(5_000_000), 0),
                p(30, 1, "quiet.exe", Some(7), 0),
            ],
            vec![
                p(10, 1, "chrome.exe", Some(4_000_000), 300),
                p(11, 10, "chrome.exe", Some(2_000_000), 200),
                p(20, 1, "Updater.exe", Some(9_000_000), 100),
                p(30, 1, "quiet.exe", Some(7), 50),
            ],
        );
        let load = load_between(&a, &b, 4, 999).value().cloned().unwrap();
        assert_eq!(load.sample_ms, 1000);
        // chrome 6e6 / 4e7 = 15 %, updater 4e6 / 4e7 = 10 %.
        assert_eq!(load.cpu_percent, 25.0);
        assert_eq!(load.top[0].name, "chrome.exe");
        assert_eq!(
            (load.top[0].processes, load.top[0].cpu_percent, load.top[0].memory_bytes),
            (2, 15.0, 500)
        );
        assert_eq!(
            (load.top[1].name.as_str(), load.top[1].cpu_percent),
            ("Updater.exe", 10.0)
        );
        assert_eq!(load.top[2].cpu_percent, 0.0);
    }

    #[test]
    fn peaktweaks_and_what_it_started_are_left_out_and_idle_is_not_load() {
        let (a, b) = snaps(
            vec![
                p(0, 0, "[System Process]", Some(0), 0),
                p(500, 1, "peaktweaks.exe", Some(0), 0),
                p(501, 500, "msedgewebview2.exe", Some(0), 0),
                p(502, 501, "msedgewebview2.exe", Some(0), 0),
            ],
            vec![
                p(0, 0, "[System Process]", Some(30_000_000), 0),
                p(500, 1, "peaktweaks.exe", Some(5_000_000), 0),
                p(501, 500, "msedgewebview2.exe", Some(5_000_000), 0),
                p(502, 501, "msedgewebview2.exe", Some(5_000_000), 0),
            ],
        );
        let load = load_between(&a, &b, 4, 500).value().cloned().unwrap();
        assert_eq!(load.cpu_percent, 0.0);
        assert!(load.top.is_empty(), "{:?}", load.top);
    }

    #[test]
    fn new_reused_and_unreadable_processes_are_not_guessed() {
        let (a, b) = snaps(
            vec![p(40, 1, "old.exe", Some(0), 0), p(41, 1, "locked.exe", None, 0)],
            vec![
                p(40, 1, "new-with-reused-pid.exe", Some(9_000_000), 0),
                p(41, 1, "locked.exe", None, 0),
                p(42, 1, "started-just-now.exe", Some(9_000_000), 0),
            ],
        );
        let load = load_between(&a, &b, 4, 999).value().cloned().unwrap();
        assert_eq!(load.cpu_percent, 0.0);
        assert!(load.top.is_empty());
        assert_eq!(load.unreadable, 1);
    }

    #[test]
    fn counter_rows_become_a_snapshot() {
        use crate::wmi::WmiValue;
        let row = |pairs: Vec<(&str, WmiValue)>| WmiRow(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
        let s = |t: &str| WmiValue::Str(t.into());
        let rows = vec![
            row(vec![
                ("Name", s("_Total")),
                ("IDProcess", WmiValue::UInt(0)),
                ("PercentProcessorTime", s("999")),
            ]),
            row(vec![
                ("Name", s("Idle")),
                ("IDProcess", WmiValue::UInt(0)),
                ("PercentProcessorTime", s("123")),
                ("Timestamp_Sys100NS", s("133000000000000000")),
            ]),
            row(vec![
                ("Name", s("chrome#3")),
                ("IDProcess", WmiValue::UInt(4321)),
                ("CreatingProcessID", WmiValue::UInt(4000)),
                ("PercentProcessorTime", s("5000000")),
                ("WorkingSetPrivate", s("104857600")),
                ("Timestamp_Sys100NS", s("133000000000000000")),
            ]),
        ];
        let snap = snapshot_from(&Ok(rows)).unwrap();
        assert_eq!(snap.wall_100ns, 133_000_000_000_000_000);
        assert_eq!(snap.procs.len(), 2, "_Total is a sum, not a process");
        assert_eq!(snap.procs[1], p(4321, 4000, "chrome", Some(5_000_000), 104_857_600));
        assert!(snapshot_from(&Ok(vec![])).is_err(), "no timestamp, no snapshot");
        assert!(snapshot_from(&Err(EngineError::Internal { detail: "x".into() })).is_err());
    }

    #[test]
    fn process_rows_become_a_snapshot_with_the_given_clock() {
        use crate::wmi::WmiValue;
        let row = |pairs: Vec<(&str, WmiValue)>| WmiRow(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
        let s = |t: &str| WmiValue::Str(t.into());
        let rows = vec![
            row(vec![
                ("Name", s("chrome.EXE")),
                ("ProcessId", WmiValue::UInt(4321)),
                ("ParentProcessId", WmiValue::UInt(4000)),
                ("KernelModeTime", s("1000000")),
                ("UserModeTime", s("4000000")),
                ("WorkingSetSize", s("104857600")),
            ]),
            // A protected process: WMI leaves the times empty.
            row(vec![
                ("Name", s("System")),
                ("ProcessId", WmiValue::UInt(4)),
                ("WorkingSetSize", s("1")),
            ]),
        ];
        let snap = snapshot_from_processes(&Ok(rows), 42).unwrap();
        assert_eq!(snap.wall_100ns, 42);
        assert_eq!(snap.procs[0], p(4321, 4000, "chrome", Some(5_000_000), 104_857_600));
        assert_eq!(snap.procs[1].name, "System");
        assert_eq!(snap.procs[1].cpu_100ns, None);
    }

    /// Kegan's PC (field check 2026-10-15): the counters are switched off, so
    /// the counter query fails and every later snapshot uses `Win32_Process`.
    #[test]
    fn switched_off_counters_fall_back_to_win32_process_and_stay_there() {
        use crate::wmi::WmiValue;
        use std::cell::{Cell, RefCell};
        let asked = RefCell::new(Vec::new());
        let query = |q: &str| -> Result<Vec<WmiRow>> {
            asked.borrow_mut().push(q.to_owned());
            if q == WQL_PROCESSES {
                return Err(EngineError::Internal {
                    detail: "0x80041017".into(),
                });
            }
            Ok(vec![WmiRow(
                [
                    ("Name".to_owned(), WmiValue::Str("game.exe".into())),
                    ("ProcessId".to_owned(), WmiValue::UInt(7)),
                    ("KernelModeTime".to_owned(), WmiValue::Str("1".into())),
                    ("UserModeTime".to_owned(), WmiValue::Str("2".into())),
                ]
                .into_iter()
                .collect(),
            )])
        };
        let fell_back = Cell::new(false);
        let first = snapshot_via(&query, &|| 10, &fell_back).unwrap();
        let second = snapshot_via(&query, &|| 20, &fell_back).unwrap();
        assert!(fell_back.get());
        assert_eq!((first.wall_100ns, second.wall_100ns), (10, 20));
        assert_eq!(first.procs[0].cpu_100ns, Some(3));
        assert_eq!(
            *asked.borrow(),
            vec![WQL_PROCESSES, WQL_PROCESSES_FALLBACK, WQL_PROCESSES_FALLBACK],
            "the counters are asked once, not on every snapshot"
        );

        // Both failing names both reasons.
        let none = |_: &str| -> Result<Vec<WmiRow>> { Err(EngineError::Internal { detail: "down".into() }) };
        let err = snapshot_via(&none, &|| 0, &Cell::new(false)).unwrap_err().to_string();
        assert!(
            err.contains("performance counters") && err.contains("Win32_Process"),
            "{err}"
        );
    }

    #[test]
    fn only_the_top_five_are_listed_and_a_failed_listing_is_unknown() {
        let names: Vec<String> = (0..8).map(|i| format!("p{i}.exe")).collect();
        let before = names
            .iter()
            .enumerate()
            .map(|(i, n)| p(100 + i as u32, 1, n, Some(0), 0))
            .collect();
        let after = names
            .iter()
            .enumerate()
            .map(|(i, n)| p(100 + i as u32, 1, n, Some(100_000 * (i as u64 + 1)), 0))
            .collect();
        let (a, b) = snaps(before, after);
        let load = load_between(&a, &b, 4, 999).value().cloned().unwrap();
        assert_eq!(load.top.len(), TOP);
        assert_eq!(load.top[0].name, "p7.exe");

        let failing =
            || -> Result<ProcessSnapshot> { Err(crate::error::EngineError::Internal { detail: "no".into() }) };
        assert!(sample(&failing, std::time::Duration::ZERO, Some(4)).is_unknown());
        assert!(sample(&|| Ok(a.clone()), std::time::Duration::ZERO, None).is_unknown());
        assert!(load_between(&a, &a, 4, 999).is_unknown(), "zero elapsed time");
    }
}
