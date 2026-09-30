//! The scanner: what is on this PC, what is worth a look, and what is already
//! fine. Phase 5.
//!
//! A pure function of the engine's own `SystemEnv`. It reads nothing itself and
//! changes nothing, so it runs unelevated and its results are reproducible from
//! a saved `SystemEnv`. Every finding is `guided_only` for now: the one-click
//! fixes the plan lists (refresh rate, power mode, per-app GPU choice) need
//! tweaks that are not built yet, so a finding never names a fix that does not
//! exist (`fix_tweak_id` is `None` until one does; NOTES.md N41).
//!
//! Rules this file keeps, and `tests` below enforces:
//! - A probe that could not tell produces a `Status::Unknown` finding that says
//!   why. It is never rounded to "fine" or to "a problem".
//! - No finding promises a result. It states a reading and points at a remedy;
//!   only a stored proof run may say a change helped (plan section 5).
//! - Nothing here recommends turning off a security feature.

use serde::Serialize;
use ts_rs::TS;

use super::hardware::{ChannelLayout, DiskMedia, HardwareReport};
use super::power::{PowerPlan, PowerPlanKind};
use super::probe::Probe;
use super::types::SystemEnv;

/// Windows 11 starts at build 22000. Anything older that still reports a build
/// number is Windows 10 (or older) for the purposes of the support finding.
const FIRST_WINDOWS_11_BUILD: u32 = 22000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Something to look at.
    Attention,
    /// Nothing to do. Listed so the user sees what is already right.
    Fine,
    /// We could not tell. The finding says why.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// Stable slug, safe to key UI state and saved dismissals on.
    pub id: String,
    pub status: Status,
    pub title: String,
    /// What we read, in plain words. Always present.
    pub reading: String,
    /// What the user can do about it. `None` when there is nothing to do.
    pub remedy: Option<String>,
    /// True when the fix is something the user does outside PeakTweaks
    /// (BIOS setting, buying hardware). No fix here touches the BIOS.
    pub guided_only: bool,
    /// The tweak that fixes it in one click, when one exists.
    pub fix_tweak_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    /// Attention first (in the fixed order of `RULES`), then Unknown, then Fine.
    /// The order is a judgement about what a user should read first, not a
    /// measurement of what gains most (NOTES.md N41).
    pub findings: Vec<Finding>,
}

impl ScanReport {
    pub fn attention_count(&self) -> usize {
        self.findings.iter().filter(|f| f.status == Status::Attention).count()
    }
}

/// Run every rule. A machine that has not been probed yet yields no findings
/// (there is nothing honest to say), not a list of "unknown".
pub fn scan(env: &SystemEnv) -> ScanReport {
    let mut findings: Vec<Finding> = Vec::new();
    if let Some(hw) = &env.hardware {
        findings.extend(memory_channels(hw));
        findings.extend(memory_speed(hw));
        findings.extend(refresh_rate(hw));
        findings.extend(boot_disk(hw));
        findings.extend(windows_support(hw));
    }
    if let Some(plan) = &env.power_plan {
        let laptop = env
            .hardware
            .as_ref()
            .is_some_and(|hw| matches!(hw.is_laptop, Probe::Yes { value: true }));
        findings.push(power_plan(plan, laptop));
    }
    if let Some(sec) = &env.security {
        findings.push(memory_integrity(&sec.memory_integrity));
    }
    // Stable sort: rule order survives inside each status group.
    findings.sort_by_key(|f| match f.status {
        Status::Attention => 0,
        Status::Unknown => 1,
        Status::Fine => 2,
    });
    ScanReport { findings }
}

fn finding(id: &str, status: Status, title: &str, reading: String, remedy: Option<&str>, guided_only: bool) -> Finding {
    Finding {
        id: id.to_owned(),
        status,
        title: title.to_owned(),
        reading,
        remedy: remedy.map(str::to_owned),
        guided_only,
        fix_tweak_id: None,
    }
}

fn unknown(id: &str, title: &str, reason: &str) -> Finding {
    finding(
        id,
        Status::Unknown,
        title,
        format!("Could not be read: {reason}"),
        None,
        false,
    )
}

fn memory_channels(hw: &HardwareReport) -> Option<Finding> {
    const ID: &str = "memory.channels";
    const TITLE: &str = "Memory channels";
    match &hw.memory {
        // No memory reading at all: the sibling rule reports it once is enough.
        Probe::Unknown { reason } => Some(unknown(ID, TITLE, reason)),
        Probe::No { .. } => None,
        Probe::Yes { value } => Some(match &value.channels {
            Probe::Yes {
                value: ChannelLayout::Single,
            } => finding(
                ID,
                Status::Attention,
                "Memory is on a single channel",
                format!("{} memory module(s) found, all on one channel.", value.sticks.len()),
                Some(
                    "A matched pair of modules in the slots your motherboard manual names for two channels \
                     lets the CPU use both. This is a hardware change; PeakTweaks does not push purchases \
                     and changes nothing here.",
                ),
                true,
            ),
            Probe::Yes {
                value: ChannelLayout::Multi,
            } => finding(
                ID,
                Status::Fine,
                "Memory uses more than one channel",
                format!(
                    "{} memory modules found across more than one channel.",
                    value.sticks.len()
                ),
                None,
                false,
            ),
            Probe::No { reason } | Probe::Unknown { reason } => unknown(ID, TITLE, reason),
        }),
    }
}

fn memory_speed(hw: &HardwareReport) -> Option<Finding> {
    const ID: &str = "memory.speed";
    let Probe::Yes { value } = &hw.memory else {
        return None; // `memory_channels` already reported why memory is unreadable.
    };
    // Only modules that report both numbers can be compared.
    let comparable: Vec<(u32, u32)> = value
        .sticks
        .iter()
        .filter_map(|s| Some((s.rated_mhz?, s.configured_mhz?)))
        .collect();
    if comparable.is_empty() {
        return Some(unknown(
            ID,
            "Memory speed",
            "the memory modules did not report both a rated and a running speed",
        ));
    }
    let slow: Vec<(u32, u32)> = comparable
        .iter()
        .copied()
        .filter(|(rated, running)| running < rated)
        .collect();
    Some(
        if let Some(&(rated, running)) = slow.iter().min_by_key(|(_, running)| *running) {
            finding(
                ID,
                Status::Attention,
                "Memory runs below its rated speed",
                format!(
                    "{} of {} module(s) run below their rated speed (for example {running} of {rated} MT/s).",
                    slow.len(),
                    comparable.len()
                ),
                Some(
                    "Many motherboards ship with the memory profile (XMP or EXPO) switched off. It is a BIOS \
                 setting you change yourself; PeakTweaks does not write to the BIOS. If the PC does not \
                 start after changing it, your motherboard manual explains how to reset the BIOS. Some \
                 modules report the same number for both, in which case this check cannot see the difference.",
                ),
                true,
            )
        } else {
            finding(
                ID,
                Status::Fine,
                "Memory runs at its rated speed",
                format!("{} module(s) report running at their rated speed.", comparable.len()),
                None,
                false,
            )
        },
    )
}

fn refresh_rate(hw: &HardwareReport) -> Option<Finding> {
    const ID: &str = "display.refresh_rate";
    const TITLE: &str = "Display refresh rate";
    Some(match &hw.display {
        Probe::No { .. } => return None,
        Probe::Unknown { reason } => unknown(ID, TITLE, reason),
        Probe::Yes { value } if value.max_hz_at_current_resolution == 0 || value.current_hz == 0 => {
            unknown(ID, TITLE, "Windows did not report a refresh rate")
        }
        // 1 Hz of slack: 59.94 Hz shows up as 59 or 60 depending on the driver.
        Probe::Yes { value } if value.current_hz + 1 < value.max_hz_at_current_resolution => finding(
            ID,
            Status::Attention,
            "The display is set below its highest refresh rate",
            format!(
                "Running at {} Hz; this display offers {} Hz at {}x{}.",
                value.current_hz, value.max_hz_at_current_resolution, value.width, value.height
            ),
            Some("Windows Settings > System > Display > Advanced display lets you choose the refresh rate."),
            true,
        ),
        Probe::Yes { value } => finding(
            ID,
            Status::Fine,
            "The display is at its highest refresh rate",
            format!(
                "Running at {} Hz, the most it offers at {}x{}.",
                value.current_hz, value.width, value.height
            ),
            None,
            false,
        ),
    })
}

fn boot_disk(hw: &HardwareReport) -> Option<Finding> {
    const ID: &str = "storage.boot_disk";
    const TITLE: &str = "Windows drive";
    Some(match &hw.boot_disk {
        Probe::No { .. } => return None,
        Probe::Unknown { reason } => unknown(ID, TITLE, reason),
        Probe::Yes { value } if value.media == DiskMedia::Hdd => finding(
            ID,
            Status::Attention,
            "Windows is on a hard drive",
            format!("The drive Windows runs from ({}) is a hard disk drive.", value.name),
            Some(
                "A solid-state drive is the usual upgrade for a PC that starts and loads slowly. It is a \
                 hardware change; PeakTweaks changes nothing here.",
            ),
            true,
        ),
        Probe::Yes { value } => finding(
            ID,
            Status::Fine,
            "Windows is on a solid-state drive",
            format!("The drive Windows runs from ({}) is not a hard disk drive.", value.name),
            None,
            false,
        ),
    })
}

/// VERIFY (NOTES.md N41): the plan gives an ESU end date for consumers
/// (2027-10-12). It is a single source and is deliberately left out of the copy
/// below until someone re-checks it against Microsoft's page.
fn windows_support(hw: &HardwareReport) -> Option<Finding> {
    const ID: &str = "os.support";
    const TITLE: &str = "Windows version";
    Some(match &hw.os {
        Probe::No { .. } => return None,
        Probe::Unknown { reason } => unknown(ID, TITLE, reason),
        // Server has its own lifecycle; not something a gamer's scan judges.
        Probe::Yes { value } if value.is_server => return None,
        Probe::Yes { value } if value.build < FIRST_WINDOWS_11_BUILD => finding(
            ID,
            Status::Attention,
            "This is Windows 10 or older",
            format!("{} (build {}).", value.caption, value.build),
            Some(
                "Windows 10 stopped receiving free security updates in October 2025 unless the PC is \
                 enrolled in Microsoft's Extended Security Updates. Check Windows Update to see what applies \
                 to this PC.",
            ),
            true,
        ),
        Probe::Yes { value } => finding(
            ID,
            Status::Fine,
            "Windows 11",
            format!("{} (build {}).", value.caption, value.build),
            None,
            false,
        ),
    })
}

/// Reads the power *plan*. Windows 11 also has a separate "power mode" setting
/// that is not read here, so the copy names the plan and points at both places
/// (NOTES.md N43). No one-click fix exists yet, so this is guided only.
fn power_plan(plan: &Probe<PowerPlan>, laptop: bool) -> Finding {
    const ID: &str = "power.plan";
    const TITLE: &str = "Power plan";
    let remedy = if laptop {
        "In Windows Settings > System > Power & battery, the Power mode setting, or Control Panel > Power \
         Options, you can pick a plan that favours performance. On a laptop that uses more battery and \
         makes more heat, so many people only do this while plugged in."
    } else {
        "In Windows Settings > System > Power & battery, the Power mode setting, or Control Panel > Power \
         Options, you can pick a plan that favours performance. It uses more electricity."
    };
    match plan {
        Probe::Unknown { reason } | Probe::No { reason } => unknown(ID, TITLE, reason),
        Probe::Yes { value } => match value.kind {
            PowerPlanKind::HighPerformance | PowerPlanKind::UltimatePerformance => finding(
                ID,
                Status::Fine,
                "A performance power plan is active",
                "The active power plan is one of Windows' performance plans.".to_owned(),
                None,
                false,
            ),
            PowerPlanKind::Custom => finding(
                ID,
                Status::Fine,
                "A custom power plan is active",
                "The active power plan is not one of the four Windows ships, so it was left alone and not judged."
                    .to_owned(),
                None,
                false,
            ),
            PowerPlanKind::Balanced | PowerPlanKind::PowerSaver => {
                let name = if value.kind == PowerPlanKind::Balanced {
                    "Balanced"
                } else {
                    "Power saver"
                };
                finding(
                    ID,
                    Status::Attention,
                    "The power plan is not a performance plan",
                    format!("The active power plan is {name}."),
                    Some(remedy),
                    true,
                )
            }
        },
    }
}

/// Reading only. Memory Integrity is a security feature and this scanner never
/// suggests turning it off. The plan's optional A/B proof is offered by the
/// Proof tab, and only as something the user chooses to do themselves.
fn memory_integrity(mi: &Probe<()>) -> Finding {
    const ID: &str = "security.memory_integrity";
    const TITLE: &str = "Memory Integrity";
    match mi {
        Probe::Yes { .. } => finding(
            ID,
            Status::Fine,
            "Memory Integrity is on",
            "Windows reports Memory Integrity (core isolation) is enabled.".to_owned(),
            None,
            false,
        ),
        Probe::No { reason } => finding(
            ID,
            Status::Attention,
            "Memory Integrity is off",
            format!("Windows reports it is off ({reason})."),
            Some(
                "This is a Windows security feature. PeakTweaks never changes it. If you turned it off on \
                 purpose, leave it as it is.",
            ),
            true,
        ),
        Probe::Unknown { reason } => unknown(ID, TITLE, reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{BootDisk, DisplayInfo, MemoryInfo, MemoryStick, OsInfo};
    use crate::security::SecurityReport;

    fn stick(rated: Option<u32>, running: Option<u32>) -> MemoryStick {
        MemoryStick {
            capacity_bytes: 8 << 30,
            rated_mhz: rated,
            configured_mhz: running,
            bank_label: None,
            device_locator: None,
            kind: Some("DDR4".into()),
        }
    }

    fn hardware() -> HardwareReport {
        HardwareReport {
            os: Probe::yes(OsInfo {
                build: 22631,
                caption: "Microsoft Windows 11 Home".into(),
                is_server: false,
            }),
            cpu: Probe::unknown("not needed here"),
            memory: Probe::yes(MemoryInfo {
                installed_bytes: 16 << 30,
                sticks: vec![stick(Some(3200), Some(3200)), stick(Some(3200), Some(3200))],
                channels: Probe::yes(ChannelLayout::Multi),
            }),
            gpus: Probe::unknown("not needed here"),
            boot_disk: Probe::yes(BootDisk {
                media: DiskMedia::Ssd,
                name: "Samsung SSD".into(),
            }),
            display: Probe::yes(DisplayInfo {
                width: 1920,
                height: 1080,
                current_hz: 144,
                max_hz_at_current_resolution: 144,
            }),
            is_laptop: Probe::no("desktop"),
            rig_class: Probe::unknown("not needed here"),
        }
    }

    fn env_with(hw: HardwareReport) -> SystemEnv {
        SystemEnv {
            hardware: Some(hw),
            ..SystemEnv::default()
        }
    }

    fn get<'a>(r: &'a ScanReport, id: &str) -> &'a Finding {
        r.findings
            .iter()
            .find(|f| f.id == id)
            .unwrap_or_else(|| panic!("no finding {id}: {r:?}"))
    }

    #[test]
    fn an_unprobed_machine_has_no_findings() {
        assert!(scan(&SystemEnv::default()).findings.is_empty());
    }

    #[test]
    fn a_healthy_machine_lists_what_is_already_right() {
        let r = scan(&env_with(hardware()));
        assert_eq!(r.attention_count(), 0, "{r:?}");
        for id in [
            "memory.channels",
            "memory.speed",
            "display.refresh_rate",
            "storage.boot_disk",
            "os.support",
        ] {
            assert_eq!(get(&r, id).status, Status::Fine, "{id}");
            assert!(get(&r, id).remedy.is_none(), "{id}: nothing to do, so no remedy");
        }
    }

    #[test]
    fn a_budget_machine_gets_each_problem_once_with_a_guided_remedy() {
        let mut hw = hardware();
        hw.memory = Probe::yes(MemoryInfo {
            installed_bytes: 8 << 30,
            sticks: vec![stick(Some(3200), Some(2133))],
            channels: Probe::yes(ChannelLayout::Single),
        });
        hw.display = Probe::yes(DisplayInfo {
            width: 1920,
            height: 1080,
            current_hz: 60,
            max_hz_at_current_resolution: 144,
        });
        hw.boot_disk = Probe::yes(BootDisk {
            media: DiskMedia::Hdd,
            name: "WDC WD10".into(),
        });
        hw.os = Probe::yes(OsInfo {
            build: 19045,
            caption: "Microsoft Windows 10 Home".into(),
            is_server: false,
        });
        let r = scan(&env_with(hw));
        for id in [
            "memory.channels",
            "memory.speed",
            "display.refresh_rate",
            "storage.boot_disk",
            "os.support",
        ] {
            let f = get(&r, id);
            assert_eq!(f.status, Status::Attention, "{id}");
            assert!(f.guided_only, "{id}: no one-click fix exists yet");
            assert!(
                f.fix_tweak_id.is_none(),
                "{id}: must not name a fix that does not exist"
            );
            assert!(f.remedy.is_some(), "{id}");
        }
        assert_eq!(r.attention_count(), 5);
        let mut ids: Vec<&str> = r.findings.iter().map(|f| f.id.as_str()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "one finding per check");
    }

    #[test]
    fn attention_comes_first_then_unknown_then_fine() {
        let mut hw = hardware();
        hw.boot_disk = Probe::yes(BootDisk {
            media: DiskMedia::Hdd,
            name: "x".into(),
        });
        hw.display = Probe::unknown("no display attached");
        let r = scan(&env_with(hw));
        let order: Vec<Status> = r.findings.iter().map(|f| f.status).collect();
        let mut sorted = order.clone();
        sorted.sort_by_key(|s| match s {
            Status::Attention => 0,
            Status::Unknown => 1,
            Status::Fine => 2,
        });
        assert_eq!(order, sorted);
        assert_eq!(order[0], Status::Attention);
        assert_eq!(get(&r, "display.refresh_rate").status, Status::Unknown);
    }

    #[test]
    fn a_probe_that_could_not_tell_is_never_rounded_to_fine_or_a_problem() {
        let mut hw = hardware();
        hw.memory = Probe::yes(MemoryInfo {
            installed_bytes: 16 << 30,
            sticks: vec![stick(None, None)],
            channels: Probe::unknown("no bank labels"),
        });
        hw.boot_disk = Probe::unknown("no MediaType");
        hw.os = Probe::unknown("WMI timed out");
        let r = scan(&env_with(hw));
        for id in ["memory.channels", "memory.speed", "storage.boot_disk", "os.support"] {
            let f = get(&r, id);
            assert_eq!(f.status, Status::Unknown, "{id}");
            assert!(f.remedy.is_none(), "{id}: no advice on a reading we do not have");
            assert!(f.reading.contains("Could not be read"), "{id}: {}", f.reading);
        }
    }

    #[test]
    fn refresh_rate_tolerates_one_hz_of_rounding_but_not_a_real_gap() {
        let mut hw = hardware();
        hw.display = Probe::yes(DisplayInfo {
            width: 1920,
            height: 1080,
            current_hz: 59,
            max_hz_at_current_resolution: 60,
        });
        assert_eq!(
            get(&scan(&env_with(hw.clone())), "display.refresh_rate").status,
            Status::Fine
        );
        hw.display = Probe::yes(DisplayInfo {
            width: 1920,
            height: 1080,
            current_hz: 60,
            max_hz_at_current_resolution: 144,
        });
        assert_eq!(
            get(&scan(&env_with(hw)), "display.refresh_rate").status,
            Status::Attention
        );
    }

    #[test]
    fn windows_server_is_not_judged_as_a_gaming_pc() {
        let mut hw = hardware();
        hw.os = Probe::yes(OsInfo {
            build: 20348,
            caption: "Microsoft Windows Server 2022".into(),
            is_server: true,
        });
        assert!(scan(&env_with(hw)).findings.iter().all(|f| f.id != "os.support"));
    }

    #[test]
    fn memory_integrity_is_reported_and_never_recommended_off() {
        let sec = |mi: Probe<()>| SystemEnv {
            security: Some(SecurityReport {
                secure_boot: Probe::unknown("x"),
                memory_integrity: mi,
                tpm: Probe::unknown("x"),
                iommu: Probe::unknown("x"),
            }),
            ..SystemEnv::default()
        };
        let on = scan(&sec(Probe::yes(())));
        assert_eq!(get(&on, "security.memory_integrity").status, Status::Fine);
        let off = scan(&sec(Probe::no("service not running")));
        let f = get(&off, "security.memory_integrity");
        assert_eq!(f.status, Status::Attention);
        let remedy = f.remedy.as_deref().unwrap().to_ascii_lowercase();
        assert!(remedy.contains("never changes"), "{remedy}");
        assert!(!remedy.contains("turn off") && !remedy.contains("disable"), "{remedy}");
        let unknown = scan(&sec(Probe::unknown("no signal")));
        assert_eq!(get(&unknown, "security.memory_integrity").status, Status::Unknown);
    }

    fn env_with_plan(plan: Probe<PowerPlan>, laptop: bool) -> SystemEnv {
        let mut hw = hardware();
        hw.is_laptop = if laptop { Probe::yes(true) } else { Probe::no("desktop") };
        SystemEnv {
            hardware: Some(hw),
            power_plan: Some(plan),
            ..SystemEnv::default()
        }
    }

    fn plan(kind: PowerPlanKind) -> Probe<PowerPlan> {
        Probe::yes(PowerPlan { kind, guid: "x".into() })
    }

    #[test]
    fn only_balanced_and_power_saver_are_flagged_and_customs_are_left_alone() {
        for (kind, status) in [
            (PowerPlanKind::Balanced, Status::Attention),
            (PowerPlanKind::PowerSaver, Status::Attention),
            (PowerPlanKind::HighPerformance, Status::Fine),
            (PowerPlanKind::UltimatePerformance, Status::Fine),
            (PowerPlanKind::Custom, Status::Fine),
        ] {
            let r = scan(&env_with_plan(plan(kind), false));
            let f = get(&r, "power.plan");
            assert_eq!(f.status, status, "{kind:?}");
            assert_eq!(f.remedy.is_some(), status == Status::Attention, "{kind:?}");
            assert!(f.fix_tweak_id.is_none());
        }
        let unknown = scan(&env_with_plan(Probe::unknown("no value"), false));
        assert_eq!(get(&unknown, "power.plan").status, Status::Unknown);
    }

    #[test]
    fn the_laptop_remedy_mentions_battery_and_heat() {
        let laptop = scan(&env_with_plan(plan(PowerPlanKind::Balanced), true));
        let desktop = scan(&env_with_plan(plan(PowerPlanKind::Balanced), false));
        let laptop_text = get(&laptop, "power.plan").remedy.clone().unwrap();
        let desktop_text = get(&desktop, "power.plan").remedy.clone().unwrap();
        assert!(laptop_text.contains("more battery") && laptop_text.contains("heat"));
        assert!(!desktop_text.contains("more battery") && !desktop_text.contains("heat"));
    }

    /// Findings say what was read and what to do. They never promise a result;
    /// only a stored proof run may say a change helped. Same banned words as the
    /// catalogue lint in `tests.rs`, applied to this file's non-test code.
    #[test]
    fn scanner_copy_makes_no_efficacy_claims() {
        let banned = [
            "boost",
            "faster",
            "fps",
            "smoother",
            "smoothness",
            "lag",
            "improv",
            "measured",
            "benchmark",
            "speed up",
            "higher performance",
            "more performance",
            "% ",
            "reduces latency",
            "lower latency",
        ];
        let src = include_str!("scanner.rs");
        let production = src.split("#[cfg(test)]").next().unwrap();
        let mut scanned = 0;
        for line in production.lines().filter(|l| !l.trim_start().starts_with("//")) {
            let mut rest = line;
            while let Some(start) = rest.find('"') {
                let after = &rest[start + 1..];
                let Some(end) = after.find('"') else { break };
                let literal = after[..end].to_ascii_lowercase();
                for word in banned {
                    assert!(
                        !literal.contains(word),
                        "copy contains the claim word {word:?}: {literal}"
                    );
                }
                scanned += 1;
                rest = &after[end + 1..];
            }
        }
        assert!(
            scanned > 30,
            "expected to scan the scanner's copy, saw {scanned} literals"
        );
    }
}
