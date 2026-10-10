//! The plan's never-do list (section 12), as tests rather than intentions.
//!
//! Registry: no tweak, shipped or internal, may declare a target under the keys
//! behind Memory Integrity and VBS, Secure Boot, code-integrity policy (which
//! holds the vulnerable-driver blocklist) or the TPM service. Every write is
//! limited to `touches()` (R1), so checking the declarations covers the writes.
//!
//! Non-registry targets (`system_targets()`) are held to the same list: a
//! service's start type lives in its registry key, so a service under a
//! forbidden key is forbidden too, and no declared file may be the blocklist,
//! a driver or a boot file. Kegan's brief adds Windows Update, Defender and
//! anti-cheat services, and Windows Update's and Defender's scheduled tasks.
//!
//! Source: no code for game memory access or injection, CPU affinity, Roblox
//! fast flags, loading drivers, boot configuration or clearing the TPM.

use std::path::PathBuf;

use crate::network_audit::{hits, walk, workspace_root};
use crate::registry::pattern_is_ancestor_or_equal;
use crate::system::SysItem;
use crate::types::{RegRoot, Tweak};

/// `HKLM` keys no tweak may touch, nor anything below them.
const FORBIDDEN_HKLM: &[(&str, &str)] = &[
    (
        r"SYSTEM\CurrentControlSet\Control\DeviceGuard",
        "Memory Integrity / VBS",
    ),
    (r"SYSTEM\CurrentControlSet\Control\SecureBoot", "Secure Boot"),
    (
        r"SYSTEM\CurrentControlSet\Control\CI",
        "code integrity and the vulnerable-driver blocklist",
    ),
    (r"SYSTEM\CurrentControlSet\Services\TPM", "the TPM"),
];

/// Every declared target that falls under a forbidden key.
fn forbidden_targets(tweaks: &[Box<dyn Tweak>]) -> Vec<String> {
    let mut found = Vec::new();
    for t in tweaks {
        for target in t.touches() {
            if target.root != RegRoot::LocalMachine {
                continue;
            }
            for (key, what) in FORBIDDEN_HKLM {
                if pattern_is_ancestor_or_equal(key, &target.key) {
                    found.push(format!("{}: {} ({what})", t.id(), target.key));
                }
            }
        }
        found.extend(forbidden_system_targets(t.id(), &t.system_targets()));
    }
    found
}

/// Services Kegan ruled out (2026-10-06 brief): Windows Update, Defender and
/// the Windows security stack, and anti-cheat. Names as Windows registers
/// them, compared ignoring case. VERIFY each anti-cheat name against the
/// game's own documentation (NOTES N76).
const FORBIDDEN_SERVICES: &[(&str, &str)] = &[
    ("wuauserv", "Windows Update"),
    ("UsoSvc", "Windows Update"),
    ("WaaSMedicSvc", "Windows Update"),
    ("BITS", "Windows Update"),
    ("DoSvc", "Windows Update"),
    ("TrustedInstaller", "Windows Update"),
    ("WinDefend", "Microsoft Defender"),
    ("WdNisSvc", "Microsoft Defender"),
    ("WdNisDrv", "Microsoft Defender"),
    ("WdFilter", "Microsoft Defender"),
    ("WdBoot", "Microsoft Defender"),
    ("Sense", "Microsoft Defender"),
    ("SecurityHealthService", "Windows Security"),
    ("wscsvc", "Windows Security"),
    ("mpssvc", "Windows Defender Firewall"),
    ("vgc", "anti-cheat (Vanguard)"),
    ("vgk", "anti-cheat (Vanguard)"),
    ("EasyAntiCheat", "anti-cheat (Easy Anti-Cheat)"),
    ("EasyAntiCheat_EOS", "anti-cheat (Easy Anti-Cheat)"),
    ("BEService", "anti-cheat (BattlEye)"),
    ("BEDaisy", "anti-cheat (BattlEye)"),
    ("FACEIT", "anti-cheat (FACEIT)"),
    ("FACEITService", "anti-cheat (FACEIT)"),
    ("EAAntiCheatService", "anti-cheat (EA)"),
    ("PnkBstrA", "anti-cheat (PunkBuster)"),
    ("PnkBstrB", "anti-cheat (PunkBuster)"),
];

/// Task Scheduler folders no declared task may be in (Kegan's brief): Windows
/// Update's and Defender's own tasks. Lower case, with the closing `\`.
const FORBIDDEN_TASK_FOLDERS: &[(&str, &str)] = &[
    (r"\microsoft\windows\windowsupdate\", "Windows Update"),
    (r"\microsoft\windows\updateorchestrator\", "Windows Update"),
    (r"\microsoft\windows\waasmedic\", "Windows Update"),
    (r"\microsoft\windows\windows defender\", "Microsoft Defender"),
    (r"\microsoft\windows\exploitguard\", "Microsoft Defender"),
];

/// Folders no declared file may be in, matched without the drive and ignoring
/// case: code-integrity policy (the vulnerable-driver blocklist is a file
/// there), kernel drivers, and the boot files on the EFI partition.
const FORBIDDEN_DIRS: &[(&str, &str)] = &[
    (
        r"\windows\system32\codeintegrity\",
        "code integrity and the vulnerable-driver blocklist",
    ),
    (r"\windows\system32\drivers\", "kernel drivers"),
    (r"\efi\", "boot files"),
];

/// Every declared non-registry item that reaches a never-do. A `*` service
/// name counts as a match, as a `*` segment does in a registry target.
fn forbidden_system_targets(id: &str, items: &[SysItem]) -> Vec<String> {
    let mut found = Vec::new();
    for item in items {
        match item {
            SysItem::Service { name } => {
                let key = format!(r"SYSTEM\CurrentControlSet\Services\{name}");
                for (forbidden, what) in FORBIDDEN_HKLM {
                    if pattern_is_ancestor_or_equal(forbidden, &key) {
                        found.push(format!("{id}: service {name} ({what})"));
                    }
                }
                for (forbidden, what) in FORBIDDEN_SERVICES {
                    if name == "*" || name.eq_ignore_ascii_case(forbidden) {
                        found.push(format!("{id}: service {name} ({what})"));
                    }
                }
            }
            SysItem::ScheduledTask { path } => {
                let p = path.to_ascii_lowercase();
                let hit = FORBIDDEN_TASK_FOLDERS
                    .iter()
                    .find(|(folder, _)| path == "*" || p.starts_with(folder));
                if let Some((_, what)) = hit {
                    found.push(format!("{id}: task {path} ({what})"));
                }
            }
            SysItem::File { path } => {
                let p = path.replace('/', "\\").to_ascii_lowercase();
                let why = FORBIDDEN_DIRS
                    .iter()
                    .find(|(dir, _)| p.contains(dir))
                    .map(|(_, what)| *what)
                    .or_else(|| (p.ends_with(".sys") || p.ends_with(".efi")).then_some("a driver or boot file"));
                if let Some(what) = why {
                    found.push(format!("{id}: file {path} ({what})"));
                }
            }
            _ => {}
        }
    }
    found
}

#[test]
fn no_tweak_targets_a_security_feature() {
    let all: Vec<Box<dyn Tweak>> = crate::tweaks::catalogue()
        .into_iter()
        .chain(crate::tweaks::internal())
        .collect();
    assert!(all.len() >= 3, "expected the catalogue and internal tweaks");
    let found = forbidden_targets(&all);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// Changes made for what a PC has (a startup entry, a device) are not in the
/// catalogue, so each kind is checked here: its target stays within its own
/// key whatever the entry or device is called.
#[test]
fn changes_for_listed_things_target_no_security_feature() {
    use crate::tweaks::{msi::MsiMode, startup::StartupSource, startup::StartupToggle};
    let mut listed: Vec<Box<dyn Tweak>> = crate::tweaks::startup::SOURCES
        .iter()
        .filter(|&&s| s != StartupSource::StoreApp)
        .map(|&s| Box::new(StartupToggle::new(s, r"..\..\Control\CI")) as Box<dyn Tweak>)
        .collect();
    listed.push(Box::new(StartupToggle::new(StartupSource::MachineRun, "SecureBoot")));
    // A Store app's name is two key names under its own key; no other name
    // makes a change at all, whoever asks for it.
    for name in [
        r"..\..\Control\CI",
        r"family\..",
        r"a\b\c",
        r"family",
        r"family\task/x",
        r"family\",
    ] {
        for id in [
            format!("startup.store_app:{name}"),
            format!("startup.store_app.on:{name}"),
        ] {
            assert!(StartupToggle::from_id(&id).is_none(), "{id}");
        }
    }
    listed.push(Box::new(
        StartupToggle::from_id(r"startup.store_app:Microsoft.GamingApp_8wekyb3d8bbwe\GamingAppStartup").unwrap(),
    ));
    listed.push(Box::new(
        MsiMode::new(r"PCI\VEN_8086&DEV_A0F0&SUBSYS_00748086&REV_20\3&11583659&0&A3", "").unwrap(),
    ));
    let found = forbidden_targets(&listed);
    assert!(found.is_empty(), "{}", found.join("\n"));
    for t in &listed {
        for target in t.touches() {
            assert!(
                target
                    .key
                    .starts_with(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\")
                    || target
                        .key
                        .starts_with(&format!(r"{}\", crate::tweaks::startup::STORE_TASKS))
                    || target.key.starts_with(r"SYSTEM\CurrentControlSet\Enum\PCI\"),
                "{}: {}",
                t.id(),
                target.key
            );
        }
    }
}

#[test]
fn the_registry_check_catches_a_planted_violation() {
    let planted: Vec<Box<dyn Tweak>> = vec![
        Box::new(crate::testutil::TestTweak::new(
            "planted.hvci",
            r"system\currentcontrolset\control\deviceguard\Scenarios\HypervisorEnforcedCodeIntegrity",
            &[("Enabled", 0)],
        )),
        Box::new(crate::testutil::TestTweak::new(
            "planted.blocklist",
            r"SYSTEM\CurrentControlSet\Control\CI\Config",
            &[("VulnerableDriverBlocklistEnable", 0)],
        )),
        Box::new(crate::testutil::TestTweak::new(
            "fine",
            r"SYSTEM\CurrentControlSet\Control\PriorityControl",
            &[("Win32PrioritySeparation", 2)],
        )),
    ];
    let found = forbidden_targets(&planted);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].contains("Memory Integrity") && found[1].contains("vulnerable-driver"));
}

/// A `*` segment could stand for a forbidden key, so it counts as one.
#[test]
fn a_wildcard_target_that_could_reach_a_security_key_is_caught() {
    let mut t = crate::testutil::TestTweak::new("planted.wild", r"SYSTEM\CurrentControlSet\Control\X", &[("V", 0)]);
    t.allow_key = r"SYSTEM\CurrentControlSet\Control\*".into();
    let planted: Vec<Box<dyn Tweak>> = vec![Box::new(t)];
    assert_eq!(
        forbidden_targets(&planted).len(),
        3,
        "DeviceGuard, SecureBoot and CI sit under Control"
    );
}

/// A service's start type is its registry key, and a file can be the
/// blocklist, a driver or a boot file, so non-registry targets are caught too.
#[test]
fn a_service_or_file_target_that_reaches_a_never_do_is_caught() {
    let service = |n: &str| SysItem::Service { name: n.into() };
    let file = |p: &str| SysItem::File { path: p.into() };
    let found = forbidden_system_targets(
        "planted",
        &[
            service("tpm"),
            service("*"),
            service("WSearch"),
            file(r"C:\Windows\System32\CodeIntegrity\driversipolicy.p7b"),
            file("c:/windows/system32/drivers/x.sys"),
            file(r"D:\Tools\WinRing0x64.sys"),
            file(r"S:\EFI\Microsoft\Boot\bootmgfw.efi"),
            file(r"C:\Users\KFS\AppData\Local\FortniteGame\Saved\Config\WindowsClient\GameUserSettings.ini"),
        ],
    );
    // `*` matches the TPM key and every named service below.
    assert_eq!(found.len(), 6 + FORBIDDEN_SERVICES.len(), "{found:#?}");
    assert!(
        found[0].contains("service tpm") && found[1].contains("service *"),
        "{found:#?}"
    );
    assert!(!found
        .iter()
        .any(|f| f.contains("WSearch") || f.contains("GameUserSettings")));
}

/// Kegan's brief: never Windows Update, Defender or anti-cheat services.
#[test]
fn update_defender_and_anti_cheat_services_are_caught() {
    let service = |n: &str| SysItem::Service { name: n.into() };
    let found = forbidden_system_targets(
        "planted",
        &[
            service("WUAUSERV"),
            service("WinDefend"),
            service("vgc"),
            service("EasyAntiCheat_EOS"),
            service("SysMain"),
            service("DiagTrack"),
        ],
    );
    assert_eq!(found.len(), 4, "{found:#?}");
    assert!(found[0].contains("Windows Update") && found[1].contains("Defender"));
    assert!(found[2].contains("Vanguard") && found[3].contains("Easy Anti-Cheat"));
}

/// Scheduled tasks are held to the same brief: nothing in Windows Update's or
/// Defender's own task folders.
#[test]
fn update_and_defender_scheduled_tasks_are_caught() {
    let task = |p: &str| SysItem::ScheduledTask { path: p.into() };
    let found = forbidden_system_targets(
        "planted",
        &[
            task(r"\Microsoft\Windows\WindowsUpdate\Scheduled Start"),
            task(r"\microsoft\windows\updateorchestrator\Schedule Scan"),
            task(r"\Microsoft\Windows\Windows Defender\Windows Defender Scheduled Scan"),
            task("*"),
            task(r"\Microsoft\Windows\Autochk\Proxy"),
            task(r"\Microsoft\Windows\WindowsUpdateHelper\Other"),
        ],
    );
    assert_eq!(found.len(), 4, "{found:#?}");
    assert!(found[0].contains("Windows Update") && found[2].contains("Defender"));
    assert!(found[3].contains("task *"));
}

/// Win32 calls and strings that only the forbidden features need.
const SOURCE_DENY: &[&str] = &[
    // Game memory access and injection.
    "WriteProcessMemory",
    "ReadProcessMemory",
    "VirtualAllocEx",
    "CreateRemoteThread",
    "NtCreateThreadEx",
    "SetWindowsHookEx",
    // CPU affinity (games or anything else).
    "SetProcessAffinityMask",
    "SetThreadAffinityMask",
    "SetProcessDefaultCpuSets",
    // Roblox fast flags.
    "ClientAppSettings",
    "FFlag",
    // Drivers and firmware-adjacent state.
    "NtLoadDriver",
    "SERVICE_KERNEL_DRIVER",
    "bcdedit",
    "Clear-Tpm",
    "Disable-Tpm",
    "Initialize-Tpm",
];

#[test]
fn no_source_implements_a_never_do() {
    let root = workspace_root();
    let this_file = root.join("crates/engine/src/never_do_audit.rs");
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);
    walk(&root.join("src-tauri"), &mut files);
    walk(&root.join("src"), &mut files);
    let files: Vec<PathBuf> = files
        .into_iter()
        .filter(|p| *p != this_file)
        .filter(|p| {
            let n = p.to_string_lossy();
            n.ends_with(".rs") || n.ends_with(".ts") || n.ends_with(".tsx") || n.ends_with("Cargo.toml")
        })
        .collect();
    assert!(files.len() > 100, "only {} files scanned", files.len());
    let found = hits(&files, SOURCE_DENY);
    assert!(found.is_empty(), "never-do code in the source:\n{}", found.join("\n"));
}
