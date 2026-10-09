//! The scanner: what is on this PC, what is worth a look, and what is already
//! fine. Phase 5.
//!
//! A pure function of the engine's own `SystemEnv`. It reads nothing itself and
//! changes nothing, so it runs unelevated and its results are reproducible from
//! a saved `SystemEnv`. A finding names the tool that fixes it in one click
//! (`fix_tweak_id`) only when that tool is in the catalogue: so far the power
//! plan (the PeakTweaks power plan, CATALOGUE H7) and the refresh rate
//! (`tweaks::refresh`, NOTES N95). The other one-click fixes the plan lists
//! (per-app GPU choice) are not built, so those findings are guided only
//! (NOTES.md N41, N42).
//!
//! Rules this file keeps, and `tests` below enforces:
//! - A probe that could not tell produces a `Status::Unknown` finding that says
//!   why. It is never rounded to "fine" or to "a problem".
//! - No finding promises a result. It states a reading and points at a remedy;
//!   only a stored proof run may say a change helped (plan section 5).
//! - Nothing here recommends turning off a security feature.

use serde::Serialize;
use ts_rs::TS;

use super::background::BackgroundLoad;
use super::game_installs::{on_hard_drive, GameInstall};
use super::gpu_choice::{GameGpuChoice, GpuPreference};
use super::gpu_driver::{nvidia_branch, NvidiaBranch, VENDOR_NVIDIA};
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

/// Who can act on a finding, for the plain-language Starter scan (plan 6.4:
/// "fixed by us / fixable by you / needs hardware").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum FixBy {
    /// PeakTweaks has a one-click fix (`fix_tweak_id`).
    Us,
    /// A setting the user changes: Windows, the display, the BIOS.
    You,
    /// Only different or extra hardware changes it.
    Hardware,
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
    /// Set on every `Attention` finding, `None` otherwise.
    pub fix_by: Option<FixBy>,
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
        findings.extend(gpu_driver(hw));
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
    if let Some(load) = &env.background {
        findings.extend(background_load(load));
    }
    if let Some(installs) = &env.game_installs {
        findings.extend(game_drives(installs));
    }
    if let (Some(hw), Some(choices)) = (&env.hardware, &env.gpu_choices) {
        findings.extend(gpu_choice(hw, choices));
    }
    // An SSD to move a game onto: the Windows drive is one.
    let ssd_available = env
        .hardware
        .as_ref()
        .is_some_and(|hw| matches!(&hw.boot_disk, Probe::Yes { value } if value.media != DiskMedia::Hdd));
    let sticks = env
        .hardware
        .as_ref()
        .and_then(|hw| hw.memory.value())
        .map_or(0, |m| m.sticks.len());
    for f in findings.iter_mut().filter(|f| f.status == Status::Attention) {
        f.fix_by = Some(fix_by(f, sticks, ssd_available));
    }
    // Stable sort: rule order survives inside each status group.
    findings.sort_by_key(|f| match f.status {
        Status::Attention => 0,
        Status::Unknown => 1,
        Status::Fine => 2,
    });
    ScanReport { findings }
}

/// Who can act on an `Attention` finding. Kept in one place so the Starter
/// grouping cannot drift from the remedies the rules give.
fn fix_by(f: &Finding, sticks: usize, ssd_available: bool) -> FixBy {
    if f.fix_tweak_id.is_some() {
        return FixBy::Us;
    }
    match f.id.as_str() {
        // Two or more modules on one channel can be moved; one module cannot.
        "memory.channels" if sticks >= 2 => FixBy::You,
        // A game on a hard drive can be moved to the SSD Windows is on.
        "games.drive" if ssd_available => FixBy::You,
        "memory.channels" | "storage.boot_disk" | "gpu.driver_branch" | "games.drive" => FixBy::Hardware,
        // XMP/EXPO, refresh rate, Windows version, power plan, Memory Integrity:
        // settings the user changes.
        "memory.speed"
        | "display.refresh_rate"
        | "os.support"
        | "power.plan"
        | "security.memory_integrity"
        | "background.load"
        | "gpu.choice" => FixBy::You,
        // `every_rule_says_who_can_fix_it` keeps this unreachable; a release
        // build must not lose the whole scan over a missing entry.
        other => {
            debug_assert!(false, "scanner rule {other} has no fix_by entry; add one");
            FixBy::You
        }
    }
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
        fix_by: None,
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
        Probe::Yes { value } if value.current_hz + 1 < value.max_hz_at_current_resolution => {
            let mut f = finding(
                ID,
                Status::Attention,
                "The display is set below its highest refresh rate",
                format!(
                    "Running at {} Hz; this display offers {} Hz at {}x{}.",
                    value.current_hz, value.max_hz_at_current_resolution, value.width, value.height
                ),
                Some("Windows Settings > System > Display > Advanced display lets you choose the refresh rate."),
                false,
            );
            f.fix_tweak_id = Some(crate::tweaks::refresh::ID.to_owned());
            f
        }
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

/// Plan 6.2 item 8: NVIDIA's older generations no longer get new game drivers.
/// Only NVIDIA cards are judged; the plan names no rule for other makers, so
/// they get no finding rather than an invented one. The generation table is
/// partly from memory (VERIFY, NOTES.md N47); an unknown name is reported as
/// unknown. Which generations still get security updates is NVIDIA's own
/// (`gpu_driver::security_updates`).
fn gpu_driver(hw: &HardwareReport) -> Option<Finding> {
    const ID: &str = "gpu.driver_branch";
    const TITLE: &str = "Graphics driver";
    let drivers = match &hw.gpu_drivers {
        Probe::Unknown { reason } => return Some(unknown(ID, TITLE, reason)),
        Probe::No { .. } => return None,
        Probe::Yes { value } => value,
    };
    let card = drivers.iter().find(|d| d.vendor_id == Some(VENDOR_NVIDIA))?;
    let version = card
        .nvidia_version
        .as_deref()
        .or(card.driver_version.as_deref())
        .unwrap_or("version not reported");
    let installed = match &card.driver_date {
        Some(date) => format!("Installed driver: {version}, dated {date}."),
        None => format!("Installed driver: {version}."),
    };
    Some(match nvidia_branch(&card.name) {
        NvidiaBranch::Legacy { generation } => Finding {
            remedy: Some(match crate::gpu_driver::security_updates(generation) {
                Some(true) => format!(
                    "NVIDIA no longer releases new Game Ready drivers for {generation} cards. It publishes \
                     security updates for them until October 2028; keep installing those. Getting new game \
                     drivers needs a newer card."
                ),
                Some(false) => format!(
                    "NVIDIA no longer releases any drivers for {generation} cards: their security updates ended \
                     in September 2024. Getting new drivers needs a newer card."
                ),
                None => format!(
                    "NVIDIA no longer releases new Game Ready drivers for {generation} cards. It publishes \
                     security updates for Maxwell cards until October 2028, not for older ones. Getting new game \
                     drivers needs a newer card."
                ),
            }),
            ..finding(
                ID,
                Status::Attention,
                "NVIDIA has stopped new game drivers for this card",
                format!("{} is a {generation} card. {installed}", card.name),
                None,
                true,
            )
        },
        NvidiaBranch::Current => finding(
            ID,
            Status::Fine,
            "This NVIDIA card still gets new game drivers",
            format!("{}. {installed}", card.name),
            None,
            false,
        ),
        NvidiaBranch::Unrecognised => unknown(
            ID,
            TITLE,
            &format!(
                "the card name \"{}\" is not one PeakTweaks knows, so it cannot tell whether NVIDIA still \
                 releases new game drivers for it",
                card.name
            ),
        ),
    })
}

/// Plan 6.2 item 7. Which games are looked for, and where, is in
/// `game_installs.rs` (VERIFY, NOTES.md N54). No game found: no finding.
fn game_drives(installs: &[GameInstall]) -> Option<Finding> {
    const ID: &str = "games.drive";
    const TITLE: &str = "Where your games are installed";
    if installs.is_empty() {
        return None;
    }
    let describe = |g: &GameInstall| match &g.disk {
        Probe::Yes { value } => {
            let kind = match value.media {
                DiskMedia::Hdd => "a hard drive",
                DiskMedia::Ssd | DiskMedia::Scm => "a solid-state drive",
            };
            format!("{} is on {} ({}), {kind}.", g.name, g.drive, value.name)
        }
        _ => format!(
            "{} is on {}.",
            g.name,
            if g.drive.is_empty() {
                "a drive PeakTweaks cannot name"
            } else {
                &g.drive
            }
        ),
    };
    let on_hdd: Vec<&GameInstall> = installs.iter().filter(|g| on_hard_drive(g)).collect();
    if !on_hdd.is_empty() {
        return Some(finding(
            ID,
            Status::Attention,
            "A game is installed on a hard drive",
            on_hdd.iter().map(|g| describe(g)).collect::<Vec<_>>().join(" "),
            Some(
                "Games are usually installed on an SSD when the PC has one. If this PC has an SSD with room, \
                 the game's launcher can move or reinstall the game there; otherwise a hard drive can be \
                 replaced with an SSD. PeakTweaks does not move games and does not push purchases.",
            ),
            true,
        ));
    }
    let unknown_disks: Vec<String> = installs
        .iter()
        .filter_map(|g| match &g.disk {
            Probe::Unknown { reason } => Some(format!("{}: {reason}", g.name)),
            _ => None,
        })
        .collect();
    if !unknown_disks.is_empty() {
        return Some(unknown(ID, TITLE, &unknown_disks.join("; ")));
    }
    Some(finding(
        ID,
        Status::Fine,
        "Your games are on solid-state drives",
        installs.iter().map(describe).collect::<Vec<_>>().join(" "),
        None,
        false,
    ))
}

/// Plan 6.2 item 4, read-only half: on a laptop with two graphics chips, which
/// one Windows is told to run each found game on (`gpu_choice.rs`, VERIFY,
/// NOTES.md N56). Desktops and single-chip laptops get no finding. "Not set" is
/// flagged although the graphics driver may already pick the high-performance
/// chip for a known game: PeakTweaks cannot see the driver's choice.
fn gpu_choice(hw: &HardwareReport, choices: &[GameGpuChoice]) -> Option<Finding> {
    const ID: &str = "gpu.choice";
    const TITLE: &str = "Graphics chip for your games";
    if choices.is_empty() {
        return None;
    }
    let chips = match &hw.gpus {
        Probe::Yes { value } if value.len() < 2 => return None,
        Probe::No { .. } => return None,
        Probe::Unknown { reason } => {
            return match hw.is_laptop {
                Probe::No { .. } | Probe::Yes { value: false } => None,
                _ => Some(unknown(ID, TITLE, reason)),
            }
        }
        Probe::Yes { value } => value.iter().map(|g| g.name.as_str()).collect::<Vec<_>>().join(", "),
    };
    match &hw.is_laptop {
        Probe::Yes { value: true } => {}
        Probe::Unknown { reason } => {
            return Some(unknown(
                ID,
                TITLE,
                &format!("this PC has more than one graphics chip, but whether it is a laptop is unknown: {reason}"),
            ))
        }
        _ => return None,
    }
    let unknowns: Vec<String> = choices
        .iter()
        .filter_map(|c| match &c.preference {
            Probe::Unknown { reason } => Some(format!("{}: {reason}", c.name)),
            _ => None,
        })
        .collect();
    let say = |c: &GameGpuChoice| -> Option<String> {
        let what = match c.preference.value()? {
            GpuPreference::NotSet => "no choice saved, so Windows or the graphics driver picks the chip",
            GpuPreference::LetWindowsDecide => "set to let Windows decide",
            GpuPreference::PowerSaving => "set to Power saving",
            GpuPreference::HighPerformance => "set to High performance",
        };
        Some(format!("{}: {what}.", c.name))
    };
    let lead = format!("This laptop has more than one graphics chip ({chips}).");
    let not_high: Vec<&GameGpuChoice> = choices
        .iter()
        .filter(|c| matches!(c.preference.value(), Some(p) if *p != GpuPreference::HighPerformance))
        .collect();
    if !not_high.is_empty() {
        let mut lines: Vec<String> = not_high.iter().filter_map(|c| say(c)).collect();
        if !unknowns.is_empty() {
            lines.push(format!("Could not be read: {}.", unknowns.join("; ")));
        }
        let mut f = finding(
            ID,
            Status::Attention,
            "A game is not set to the high-performance graphics chip",
            format!("{lead} {}", lines.join(" ")),
            Some(
                "In Windows Settings > System > Display > Graphics, find the game (add it with Browse if it is \
                 not listed), open its options and choose High performance. PeakTweaks does not change this \
                 setting yet.",
            ),
            true,
        );
        // VERIFY (N56): each Roblox update installs to a new folder.
        if let (Some(remedy), true) = (f.remedy.as_mut(), not_high.iter().any(|c| c.game_id == "roblox")) {
            remedy.push_str(
                " Roblox installs each update in a new folder, so its choice may need setting again after an update.",
            );
        }
        return Some(f);
    }
    if !unknowns.is_empty() {
        return Some(unknown(ID, TITLE, &unknowns.join("; ")));
    }
    Some(finding(
        ID,
        Status::Fine,
        "Your games are set to the high-performance graphics chip",
        format!(
            "{lead} {}",
            choices.iter().filter_map(say).collect::<Vec<_>>().join(" ")
        ),
        None,
        false,
    ))
}

/// Plan 6.2 item 9, read-only half. VERIFY/ASSUMED (NOTES.md N53): the
/// thresholds are a first guess. The sample runs while PeakTweaks itself is
/// open, which is as close to "idle" as a scan can get; PeakTweaks and its
/// WebView2 processes are left out of it.
const BUSY_TOTAL_PERCENT: f64 = 15.0;
const BUSY_ONE_PERCENT: f64 = 10.0;

fn background_load(load: &Probe<BackgroundLoad>) -> Option<Finding> {
    const ID: &str = "background.load";
    const TITLE: &str = "Other programs";
    let value = match load {
        Probe::Unknown { reason } => return Some(unknown(ID, TITLE, reason)),
        Probe::No { .. } => return None,
        Probe::Yes { value } => value,
    };
    let secs = (value.sample_ms as f64 / 1000.0).round().max(1.0);
    let named: Vec<String> = value
        .top
        .iter()
        .filter(|p| p.cpu_percent >= 1.0)
        .take(3)
        .map(|p| {
            let copies = if p.processes > 1 {
                format!(", {} processes", p.processes)
            } else {
                String::new()
            };
            format!("{} ({}%{copies})", p.name, p.cpu_percent)
        })
        .collect();
    let reading = format!(
        "Over {secs} seconds while PeakTweaks was checking this PC, other programs used {} percent of the processor.{}",
        value.cpu_percent,
        if named.is_empty() {
            String::new()
        } else {
            format!(" Busiest: {}.", named.join(", "))
        }
    );
    let busiest = value.top.first().map_or(0.0, |p| p.cpu_percent);
    Some(
        if value.cpu_percent >= BUSY_TOTAL_PERCENT || busiest >= BUSY_ONE_PERCENT {
            finding(
                ID,
                Status::Attention,
                "Other programs are using the processor",
                reading,
                Some(
                    "If you do not need them while you play, close them before starting a game, or stop them \
                 starting with Windows in Task Manager > Startup apps. PeakTweaks does not close or change them.",
                ),
                true,
            )
        } else {
            finding(
                ID,
                Status::Fine,
                "Little else is using the processor",
                reading,
                None,
                false,
            )
        },
    )
}

/// Reads the power *plan*. Windows 11 also has a separate "power mode" setting
/// that is not read here, so the copy names the plan and points at both places
/// (NOTES.md N43). The PeakTweaks power plan tool is the one-click fix; Windows'
/// own settings are named as well, for anyone who would rather pick a plan.
fn power_plan(plan: &Probe<PowerPlan>, laptop: bool) -> Finding {
    const ID: &str = "power.plan";
    const TITLE: &str = "Power plan";
    let remedy = if laptop {
        "The PeakTweaks power plan below copies this plan and, while plugged in, keeps the processor and \
         devices out of power saving, which uses more electricity and makes more heat; battery settings stay \
         as they were. Undo switches back. Or pick a plan yourself in Windows Settings > System > Power & \
         battery (Power mode) or Control Panel > Power Options; on battery a performance plan uses more \
         battery."
    } else {
        "The PeakTweaks power plan below copies this plan and keeps the processor and devices out of power \
         saving. It uses more electricity. Undo switches back. Or pick a plan yourself in Windows Settings > \
         System > Power & battery (Power mode) or Control Panel > Power Options."
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
            PowerPlanKind::PeakTweaks => finding(
                ID,
                Status::Fine,
                "The PeakTweaks power plan is active",
                "The active power plan is the one PeakTweaks made from your previous plan. Undo in Backups \
                 switches back."
                    .to_owned(),
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
                let mut f = finding(
                    ID,
                    Status::Attention,
                    "The power plan is not a performance plan",
                    format!("The active power plan is {name}."),
                    Some(remedy),
                    false,
                );
                f.fix_tweak_id = Some(crate::tweaks::power::PLAN_ID.to_owned());
                f
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
            gpu_drivers: Probe::no("no PCI graphics card"),
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
        let f = get(&r, "display.refresh_rate");
        assert_eq!(f.status, Status::Attention);
        assert_eq!(f.fix_tweak_id.as_deref(), Some(crate::tweaks::refresh::ID));
        assert!(!f.guided_only && f.remedy.is_some());
        for id in ["memory.channels", "memory.speed", "storage.boot_disk", "os.support"] {
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
            (PowerPlanKind::PeakTweaks, Status::Fine),
        ] {
            let r = scan(&env_with_plan(plan(kind), false));
            let f = get(&r, "power.plan");
            assert_eq!(f.status, status, "{kind:?}");
            assert_eq!(f.remedy.is_some(), status == Status::Attention, "{kind:?}");
            let fix = (status == Status::Attention).then_some(crate::tweaks::power::PLAN_ID);
            assert_eq!(f.fix_tweak_id.as_deref(), fix, "{kind:?}");
            assert!(!f.guided_only, "{kind:?}");
            assert_eq!(f.fix_by, (status == Status::Attention).then_some(FixBy::Us), "{kind:?}");
        }
        let unknown = scan(&env_with_plan(Probe::unknown("no value"), false));
        assert_eq!(get(&unknown, "power.plan").status, Status::Unknown);
    }

    /// A finding may only name a tool the app ships; Home shows that tool's
    /// card under the finding.
    #[test]
    fn a_fix_a_finding_names_is_a_tool_in_the_catalogue() {
        let ids: Vec<String> = crate::tweaks::catalogue().iter().map(|t| t.id().to_owned()).collect();
        let r = scan(&env_with_plan(plan(PowerPlanKind::Balanced), true));
        let named: Vec<&str> = r.findings.iter().filter_map(|f| f.fix_tweak_id.as_deref()).collect();
        assert!(!named.is_empty());
        for id in named {
            assert!(ids.iter().any(|t| t == id), "{id} is not in the catalogue");
        }
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
    fn nvidia(name: &str, version: Option<&str>, date: Option<&str>) -> crate::gpu_driver::GpuDriver {
        crate::gpu_driver::GpuDriver {
            name: name.into(),
            vendor_id: Some(VENDOR_NVIDIA),
            driver_version: Some("32.0.15.8180".into()),
            nvidia_version: version.map(Into::into),
            driver_date: date.map(Into::into),
        }
    }

    #[test]
    fn an_old_nvidia_card_is_told_it_gets_security_updates_only() {
        let mut hw = hardware();
        hw.gpu_drivers = Probe::yes(vec![nvidia(
            "NVIDIA GeForce GTX 1060 6GB",
            Some("581.80"),
            Some("2025-08-20"),
        )]);
        let f = get(&scan(&env_with(hw)), "gpu.driver_branch").clone();
        assert_eq!(f.status, Status::Attention);
        assert!(f.guided_only && f.fix_tweak_id.is_none());
        assert!(
            f.reading.contains("GTX 1060") && f.reading.contains("Pascal"),
            "{}",
            f.reading
        );
        assert!(f.reading.contains("581.80, dated 2025-08-20"), "{}", f.reading);
        assert!(f.remedy.unwrap().contains("security updates"));
    }

    #[test]
    fn a_current_nvidia_card_is_fine_and_an_unknown_name_is_unknown() {
        let mut hw = hardware();
        hw.gpu_drivers = Probe::yes(vec![nvidia("NVIDIA GeForce RTX 4060", Some("581.80"), None)]);
        let f = get(&scan(&env_with(hw.clone())), "gpu.driver_branch").clone();
        assert_eq!(f.status, Status::Fine);
        assert!(f.remedy.is_none());
        assert!(f.reading.ends_with("Installed driver: 581.80."), "{}", f.reading);

        hw.gpu_drivers = Probe::yes(vec![nvidia("NVIDIA Mystery 9000", None, None)]);
        let f = get(&scan(&env_with(hw)), "gpu.driver_branch").clone();
        assert_eq!(f.status, Status::Unknown);
        assert!(f.reading.contains("Mystery 9000"), "{}", f.reading);
    }

    #[test]
    fn other_makers_get_no_driver_finding_and_a_failed_read_is_unknown() {
        let mut hw = hardware();
        hw.gpu_drivers = Probe::yes(vec![crate::gpu_driver::GpuDriver {
            name: "AMD Radeon RX 580".into(),
            vendor_id: Some(0x1002),
            driver_version: Some("31.0.21001.45002".into()),
            nvidia_version: None,
            driver_date: Some("2024-01-01".into()),
        }]);
        assert!(scan(&env_with(hw.clone()))
            .findings
            .iter()
            .all(|f| f.id != "gpu.driver_branch"));

        hw.gpu_drivers = Probe::unknown("WMI timed out");
        let f = get(&scan(&env_with(hw)), "gpu.driver_branch").clone();
        assert_eq!(f.status, Status::Unknown);
        assert!(f.reading.contains("WMI timed out"));
    }

    #[test]
    fn every_rule_says_who_can_fix_it() {
        let src = include_str!("scanner.rs");
        let production = src.split("#[cfg(test)]").next().unwrap();
        let ids: Vec<&str> = production
            .split("const ID: &str = \"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap())
            .collect();
        assert!(ids.len() >= 8, "found only {ids:?}");
        for id in ids {
            let f = finding(id, Status::Attention, "t", String::new(), None, true);
            // Panics through debug_assert for an id fix_by does not name.
            let _ = fix_by(&f, 1, false);
            let _ = fix_by(&f, 2, true);
        }
    }

    #[test]
    fn the_starter_grouping_follows_the_remedy() {
        let mut hw = hardware();
        hw.memory = Probe::yes(MemoryInfo {
            installed_bytes: 8 << 30,
            sticks: vec![stick(Some(3200), Some(2133))],
            channels: Probe::yes(ChannelLayout::Single),
        });
        hw.boot_disk = Probe::yes(BootDisk {
            media: DiskMedia::Hdd,
            name: "WDC".into(),
        });
        hw.display = Probe::yes(DisplayInfo {
            width: 1920,
            height: 1080,
            current_hz: 60,
            max_hz_at_current_resolution: 144,
        });
        let r = scan(&env_with(hw.clone()));
        assert_eq!(
            get(&r, "memory.channels").fix_by,
            Some(FixBy::Hardware),
            "one module: buy another"
        );
        assert_eq!(get(&r, "storage.boot_disk").fix_by, Some(FixBy::Hardware));
        assert_eq!(get(&r, "memory.speed").fix_by, Some(FixBy::You));
        assert_eq!(get(&r, "display.refresh_rate").fix_by, Some(FixBy::Us));
        for f in &r.findings {
            assert_eq!(f.fix_by.is_some(), f.status == Status::Attention, "{}", f.id);
            assert_eq!(
                f.fix_by == Some(FixBy::Us),
                f.fix_tweak_id.is_some(),
                "{}: ours to fix exactly when it names our tool",
                f.id
            );
        }

        hw.memory = Probe::yes(MemoryInfo {
            installed_bytes: 16 << 30,
            sticks: vec![stick(Some(3200), Some(3200)), stick(Some(3200), Some(3200))],
            channels: Probe::yes(ChannelLayout::Single),
        });
        let r = scan(&env_with(hw));
        assert_eq!(
            get(&r, "memory.channels").fix_by,
            Some(FixBy::You),
            "two modules: move them"
        );
    }

    fn load(total: f64, top: &[(&str, u32, f64)]) -> Probe<BackgroundLoad> {
        Probe::yes(BackgroundLoad {
            sample_ms: 2000,
            cpu_percent: total,
            top: top
                .iter()
                .map(|(name, processes, cpu)| crate::background::ProgramLoad {
                    name: (*name).into(),
                    processes: *processes,
                    cpu_percent: *cpu,
                    memory_bytes: 0,
                })
                .collect(),
            unreadable: 0,
        })
    }

    #[test]
    fn background_load_names_the_busiest_programs_and_says_who_can_act() {
        let mut env = env_with(hardware());
        env.background = Some(load(
            23.4,
            &[("Updater", 1, 14.2), ("chrome", 12, 6.9), ("idle-ish", 1, 0.4)],
        ));
        let f = get(&scan(&env), "background.load").clone();
        assert_eq!(f.status, Status::Attention);
        assert_eq!(f.fix_by, Some(FixBy::You));
        assert!(f.reading.contains("23.4 percent"), "{}", f.reading);
        assert!(
            f.reading.contains("Updater (14.2%), chrome (6.9%, 12 processes)."),
            "{}",
            f.reading
        );
        assert!(!f.reading.contains("idle-ish"), "under 1% is not worth naming");
        assert!(f.remedy.unwrap().contains("does not close or change them"));

        env.background = Some(load(11.0, &[("OneBusy", 1, 10.5)]));
        assert_eq!(
            get(&scan(&env), "background.load").status,
            Status::Attention,
            "one program over 10%"
        );

        env.background = Some(load(4.0, &[("a", 1, 3.0), ("b", 1, 1.0)]));
        let f = get(&scan(&env), "background.load").clone();
        assert_eq!((f.status, f.fix_by), (Status::Fine, None));

        env.background = Some(Probe::unknown("WMI timed out"));
        assert_eq!(get(&scan(&env), "background.load").status, Status::Unknown);
    }

    fn install(name: &str, drive: &str, disk: Probe<BootDisk>) -> GameInstall {
        GameInstall {
            game_id: name.to_lowercase(),
            name: name.into(),
            path: format!(r"{drive}\Games\{name}"),
            drive: drive.into(),
            disk,
            exe: None,
        }
    }

    fn media(m: DiskMedia, name: &str) -> Probe<BootDisk> {
        Probe::yes(BootDisk {
            media: m,
            name: name.into(),
        })
    }

    #[test]
    fn a_game_on_a_hard_drive_says_who_can_move_it() {
        let mut env = env_with(hardware()); // Windows on an SSD
        env.game_installs = Some(vec![
            install("Fortnite", "D:", media(DiskMedia::Hdd, "WDC 2TB")),
            install("Roblox", "C:", media(DiskMedia::Ssd, "Samsung")),
        ]);
        let f = get(&scan(&env), "games.drive").clone();
        assert_eq!(f.status, Status::Attention);
        assert_eq!(f.reading, "Fortnite is on D: (WDC 2TB), a hard drive.");
        assert_eq!(f.fix_by, Some(FixBy::You), "Windows' own drive is an SSD to move it to");

        let mut hdd_only = hardware();
        hdd_only.boot_disk = media(DiskMedia::Hdd, "WDC");
        let mut env2 = env_with(hdd_only);
        env2.game_installs = env.game_installs.clone();
        assert_eq!(
            get(&scan(&env2), "games.drive").fix_by,
            Some(FixBy::Hardware),
            "no SSD to move it to"
        );
    }

    #[test]
    fn games_on_ssds_are_fine_unknown_disks_say_why_and_no_games_say_nothing() {
        let mut env = env_with(hardware());
        env.game_installs = Some(vec![install("Roblox", "C:", media(DiskMedia::Ssd, "Samsung"))]);
        let f = get(&scan(&env), "games.drive").clone();
        assert_eq!(
            (f.status, f.reading.as_str()),
            (Status::Fine, "Roblox is on C: (Samsung), a solid-state drive.")
        );

        env.game_installs = Some(vec![install(
            "Fortnite",
            "E:",
            Probe::unknown("no physical disk record"),
        )]);
        let f = get(&scan(&env), "games.drive").clone();
        assert_eq!(f.status, Status::Unknown);
        assert!(f.reading.contains("Fortnite: no physical disk record"), "{}", f.reading);

        env.game_installs = Some(vec![]);
        assert!(scan(&env).findings.iter().all(|f| f.id != "games.drive"));
    }

    fn adapter(name: &str, vram_gib: u64) -> crate::hardware::GpuAdapter {
        crate::hardware::GpuAdapter {
            name: name.into(),
            vendor_id: 0,
            dedicated_vram_bytes: vram_gib << 30,
            shared_memory_bytes: 8 << 30,
            is_software: false,
        }
    }

    fn hybrid_laptop() -> HardwareReport {
        let mut hw = hardware();
        hw.is_laptop = Probe::yes(true);
        hw.gpus = Probe::yes(vec![
            adapter("NVIDIA GeForce RTX 4060 Laptop GPU", 8),
            adapter("Intel UHD", 0),
        ]);
        hw
    }

    fn choice(name: &str, p: Probe<GpuPreference>) -> GameGpuChoice {
        GameGpuChoice {
            game_id: name.to_lowercase(),
            name: name.into(),
            exe: Some(format!(r"C:\Games\{name}.exe")),
            preference: p,
        }
    }

    fn gpu_finding(hw: HardwareReport, choices: Vec<GameGpuChoice>) -> Option<Finding> {
        let mut env = env_with(hw);
        env.gpu_choices = Some(choices);
        scan(&env).findings.into_iter().find(|f| f.id == "gpu.choice")
    }

    #[test]
    fn a_hybrid_laptop_game_without_the_high_performance_choice_is_flagged_for_the_user() {
        let f = gpu_finding(
            hybrid_laptop(),
            vec![
                choice("Fortnite", Probe::yes(GpuPreference::NotSet)),
                choice("Roblox", Probe::yes(GpuPreference::HighPerformance)),
            ],
        )
        .unwrap();
        assert_eq!((f.status, f.fix_by), (Status::Attention, Some(FixBy::You)));
        assert_eq!(
            f.reading,
            "This laptop has more than one graphics chip (NVIDIA GeForce RTX 4060 Laptop GPU, Intel UHD). \
             Fortnite: no choice saved, so Windows or the graphics driver picks the chip."
        );
        assert!(f.remedy.unwrap().contains("High performance"));

        let f = gpu_finding(
            hybrid_laptop(),
            vec![
                choice("Fortnite", Probe::yes(GpuPreference::PowerSaving)),
                choice("Roblox", Probe::unknown("program file was not found")),
            ],
        )
        .unwrap();
        assert_eq!(f.status, Status::Attention);
        assert!(f.reading.contains("Fortnite: set to Power saving."), "{}", f.reading);
        assert!(
            f.reading
                .contains("Could not be read: Roblox: program file was not found."),
            "{}",
            f.reading
        );
        assert!(
            !f.remedy.unwrap().contains("Roblox"),
            "Roblox was not read, so no Roblox advice"
        );

        let f = gpu_finding(
            hybrid_laptop(),
            vec![choice("Roblox", Probe::yes(GpuPreference::NotSet))],
        )
        .unwrap();
        assert!(f
            .remedy
            .unwrap()
            .contains("Roblox installs each update in a new folder"));
    }

    #[test]
    fn games_on_the_high_performance_chip_are_fine_and_unread_ones_are_unknown() {
        let f = gpu_finding(
            hybrid_laptop(),
            vec![choice("Fortnite", Probe::yes(GpuPreference::HighPerformance))],
        )
        .unwrap();
        assert_eq!(f.status, Status::Fine);
        assert!(
            f.reading.ends_with("Fortnite: set to High performance."),
            "{}",
            f.reading
        );

        let f = gpu_finding(
            hybrid_laptop(),
            vec![
                choice("Fortnite", Probe::yes(GpuPreference::HighPerformance)),
                choice("Roblox", Probe::unknown("the setting could not be read: denied")),
            ],
        )
        .unwrap();
        assert_eq!(f.status, Status::Unknown, "one unread game is never rounded to fine");
        assert!(f.reading.contains("Roblox: the setting could not be read: denied"));
    }

    #[test]
    fn desktops_single_chip_laptops_and_no_games_get_no_graphics_choice_finding() {
        let not_set = || vec![choice("Fortnite", Probe::yes(GpuPreference::NotSet))];
        let mut desktop = hybrid_laptop();
        desktop.is_laptop = Probe::no("desktop");
        assert_eq!(gpu_finding(desktop, not_set()), None);

        let mut one_chip = hybrid_laptop();
        one_chip.gpus = Probe::yes(vec![adapter("Intel Iris Xe", 0)]);
        assert_eq!(gpu_finding(one_chip, not_set()), None);

        assert_eq!(gpu_finding(hybrid_laptop(), vec![]), None);

        let mut unsure = hybrid_laptop();
        unsure.is_laptop = Probe::unknown("chassis not reported");
        let f = gpu_finding(unsure, not_set()).unwrap();
        assert_eq!(f.status, Status::Unknown, "a laptop we cannot confirm is not assumed");
        assert!(f.reading.contains("chassis not reported"));

        let mut no_list = hybrid_laptop();
        no_list.gpus = Probe::unknown("DXGI failed");
        assert_eq!(gpu_finding(no_list, not_set()).unwrap().status, Status::Unknown);
    }

    #[test]
    fn scanner_copy_makes_no_efficacy_claims() {
        let words = crate::copy_lint::claim_words();
        let src = include_str!("scanner.rs");
        let production = src.split("#[cfg(test)]").next().unwrap();
        let literals = crate::copy_lint::string_literals(production);
        for literal in &literals {
            if let Some(word) = crate::copy_lint::find_claim(literal, &words) {
                panic!("copy contains the claim word {word:?}: {literal}");
            }
        }
        let scanned = literals.len();
        assert!(
            scanned > 30,
            "expected to scan the scanner's copy, saw {scanned} literals"
        );
    }
}
