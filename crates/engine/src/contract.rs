//! The Rust <-> TypeScript contract.
//!
//! `cargo test` regenerates the bindings in `src/generated/` (ts-rs) and
//! `src/generated/fixtures.ts`: real serialized values wrapped in `satisfies`
//! against the generated types. `tsc` therefore fails when the wire format and
//! the types drift apart, and CI fails when either file is stale.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::cleanup::{AreaCleanup, AreaSize, CleanupArea, CleanupReport};
use crate::context::UserResolution;
use crate::drive_optimize::DriveOptimization;
use crate::engine::{ContextInfo, JournalView, Progress, RevertResult};
use crate::env::EnvProbe;
use crate::error::EngineError;
use crate::hardware::{DisplayInfo, GpuAdapter, OsFacts, GIB};
use crate::journal::{
    ActionRecord, CommitAction, CommitRecord, JournalAction, JournalEntry, JournalWarning, JournalWarningKind,
    OneTimeAction, Record, RestoreMethod, RestorePointRecord,
};
use crate::memory::{MemoryUse, StandbyPurge};
use crate::netcheck::{read, summarize, NetworkCheck, PingTarget};
use crate::play::PlayStatus;
use crate::proof::metrics::compute_stats;
use crate::proof::nvml::{ThrottleReason, ThrottleSeen, ThrottleSummary};
use crate::proof::store::{ProofRun, ProofSession, ProofSessionSummary, Side};
use crate::proof::verdict::{compare, RunSummary};
use crate::registry::fake::FakeRegistry;
use crate::registry::Hive;
use crate::restore::{FakeRestoreOps, FixedClock, RestoreOutcome, RestoreService};
use crate::startup::{StartupFolders, StartupList};
use crate::sysprobe::{SystemAudit, SystemProbe};
use crate::testutil::*;
use crate::tweaks::startup::StartupToggle;
use crate::types::{BlockedCode, BlockedReason, ExecutionContext, RawValue, RegRoot, Tweak};
use crate::wmi::{FakeWmi, WmiValue, NS_CIMV2, NS_DEVICEGUARD, NS_STORAGE, NS_TPM};

fn generated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../src/generated")
}

fn ts_const<T: Serialize>(out: &mut String, name: &str, ty: &str, value: &T) {
    let json = serde_json::to_string_pretty(value).unwrap();
    writeln!(out, "export const {name} = {json} satisfies {ty};\n").unwrap();
}

fn views() -> Vec<crate::engine::TweakView> {
    let plain = TestTweak::new("fixture.default", "K", &[("A", 1)]);
    let applied = TestTweak::new("fixture.applied", "K2", &[("A", 1)]);
    let foreign = TestTweak::new("fixture.foreign", "K3", &[("A", 1)]);
    let mut blocked = TestTweak::new("fixture.blocked", "K4", &[("A", 1)]);
    blocked.block = true;
    let mut unknown = TestTweak::new("fixture.unknown", "K5", &[("A", 1)]);
    unknown.fail_read = true;
    let drifted = TestTweak::new("fixture.drifted", "K6", &[("A", 1)]);

    let tweaks: Vec<Box<dyn Tweak>> = vec![
        Box::new(plain),
        Box::new(applied),
        Box::new(foreign),
        Box::new(blocked),
        Box::new(unknown),
        Box::new(drifted),
    ];
    let mut h = Harness::new(tweaks);
    h.engine.apply("fixture.applied").unwrap();
    h.engine.apply("fixture.drifted").unwrap();
    h.fake
        .set_external(crate::registry::Hive::LocalMachine, "K6", "A", RawValue::dword(9));
    h.fake
        .set_external(crate::registry::Hive::LocalMachine, "K3", "A", RawValue::dword(1));
    h.engine.list().unwrap()
}

const MIB: u64 = 1024 * 1024;

/// A check as `netcheck` reads it, from illustrative round trips: the router
/// answers all 20 echoes, Cloudflare loses one (SAMPLE in the UI).
fn network_check_fixture() -> NetworkCheck {
    let rtts = |base: u32, lost: Option<usize>| -> Vec<Option<u32>> {
        (0..20)
            .map(|i| (Some(i) != lost).then_some(base + [0, 1, 0, 2, 1][i % 5]))
            .collect()
    };
    let mut router = summarize(PingTarget::Router, &rtts(1, None));
    router.address = Some("192.168.1.1".into());
    router.via = Some("Wi-Fi".into());
    let results = vec![
        router,
        summarize(PingTarget::Cloudflare, &rtts(14, Some(7))),
        summarize(PingTarget::Google, &rtts(17, None)),
    ];
    NetworkCheck {
        reading: read(&results),
        results,
        unix_ms: 1_791_332_400_000,
    }
}

/// The shape of real results; the numbers are illustrative (SAMPLE in the UI).
fn cleanup_sizes() -> Vec<AreaSize> {
    let size = |area, bytes, files| AreaSize {
        area,
        bytes,
        files,
        skipped: Vec::new(),
    };
    vec![
        size(CleanupArea::UserTemp, 2 * GIB + 300 * MIB, 4_812),
        size(CleanupArea::WindowsTemp, 640 * MIB, 233),
        size(CleanupArea::Thumbnails, 180 * MIB, 9),
        size(CleanupArea::ShaderCaches, GIB + 100 * MIB, 1_506),
        AreaSize {
            skipped: vec![LINKED_FOLDER.into()],
            ..size(CleanupArea::CrashDumps, 3 * GIB, 4)
        },
    ]
}

const LINKED_FOLDER: &str =
    r"C:\Users\Kegan\AppData\Local\CrashDumps is a link, junction or other redirect; left alone";

fn cleanup_report() -> CleanupReport {
    let done = |area, removed_bytes, removed_files| AreaCleanup {
        area,
        removed_bytes,
        removed_files,
        left_bytes: 0,
        left_files: 0,
        left_examples: Vec::new(),
        skipped: Vec::new(),
    };
    CleanupReport {
        areas: vec![
            AreaCleanup {
                left_bytes: 100 * MIB,
                left_files: 22,
                left_examples: vec![r"C:\Users\Kegan\AppData\Local\Temp\setup.log: The process cannot access the file because it is being used by another process. (os error 32)".into()],
                ..done(CleanupArea::UserTemp, 2 * GIB + 200 * MIB, 4_790)
            },
            done(CleanupArea::WindowsTemp, 640 * MIB, 233),
            AreaCleanup {
                skipped: vec![LINKED_FOLDER.into()],
                ..done(CleanupArea::CrashDumps, 3 * GIB, 4)
            },
        ],
        unix_ms: 1_791_331_500_000,
    }
}

fn journal_view() -> JournalView {
    let entry = |seq: u64, prev: Option<RawValue>, created: Vec<String>| JournalEntry {
        seq,
        tx_id: 1,
        unix_ms: 1_700_000_000_000,
        tweak_id: "input.mouseaccel".into(),
        action: JournalAction::Apply,
        context: ExecutionContext::User,
        root: RegRoot::InteractiveUser,
        key_path: r"Control Panel\Mouse".into(),
        display_path: r"HKEY_USERS\S-1-5-21-1-2-3-1001\Control Panel\Mouse".into(),
        value_name: "MouseSpeed".into(),
        previous: prev,
        written: Some(RawValue::sz("0")),
        backup_file: "backups/2026-09-29/2_input_mouseaccel_MouseSpeed.reg".into(),
        created_keys: created,
    };
    JournalView {
        applied: vec![
            crate::engine::AppliedChange {
                tweak_id: "input.mouseaccel".into(),
                name: "Pointer precision".into(),
                kind: crate::engine::ChangeKind::Catalogue,
            },
            crate::engine::AppliedChange {
                tweak_id: "system.restore.frequency".into(),
                name: "Allow a restore point on demand".into(),
                kind: crate::engine::ChangeKind::Internal,
            },
        ],
        records: vec![
            Record::Write(entry(2, Some(RawValue::sz("1")), vec![])),
            Record::Write(entry(3, None, vec![r"Control Panel\Mouse".into()])),
            Record::Commit(CommitRecord {
                seq: 4,
                tx_id: 1,
                unix_ms: 1_700_000_000_500,
                tweak_id: "input.mouseaccel".into(),
                action: CommitAction::Apply,
            }),
            Record::RestorePoint(RestorePointRecord {
                seq: 5,
                unix_ms: 1_700_000_001_000,
                sequence_number: 42,
                description: "PeakTweaks: before changes".into(),
                method: RestoreMethod::PowerShell,
                protection_enabled_by_us: Some(true),
            }),
            Record::Action(ActionRecord {
                seq: 6,
                unix_ms: 1_700_000_002_000,
                action: OneTimeAction::Cleanup,
                done: Some(cleanup_report().done()),
                error: None,
            }),
        ],
        warnings: vec![JournalWarning {
            line: 7,
            kind: JournalWarningKind::UnparsableLine,
            detail: "skipped: expected value at line 1 column 1".into(),
        }],
        offline_error: Some(
            r"C:\ProgramData\PeakTweaks\offline\001_input.mouseaccel.reg: Access is denied. (os error 5)".into(),
        ),
    }
}

struct FixtureFacts;

impl OsFacts for FixtureFacts {
    fn system_drive(&self) -> String {
        "C:".into()
    }
    fn display(&self) -> crate::error::Result<DisplayInfo> {
        Ok(DisplayInfo {
            width: 1920,
            height: 1080,
            current_hz: 60,
            max_hz_at_current_resolution: 144,
        })
    }
    fn gpu_adapters(&self) -> crate::error::Result<Vec<GpuAdapter>> {
        Ok(vec![GpuAdapter {
            name: "Example GPU".into(),
            vendor_id: 4318,
            dedicated_vram_bytes: 12 * GIB,
            shared_memory_bytes: 8 * GIB,
            is_software: false,
        }])
    }
}

/// A plausible mid-range PC as WMI would describe it, with one deliberately
/// unreadable probe (the TPM) so the fixture contains Yes, No and Unknown.
fn audit() -> SystemAudit {
    let u = |n: u64| WmiValue::UInt(n);
    let s = |t: &str| WmiValue::Str(t.into());
    let arr = |v: &[u64]| WmiValue::Array(v.iter().map(|n| WmiValue::UInt(*n)).collect());
    let wmi = Arc::new(
        FakeWmi::new()
            .with_rows(
                NS_CIMV2,
                crate::hardware::WQL_OS,
                vec![vec![
                    ("BuildNumber", s("26100")),
                    ("Caption", s("Microsoft Windows 11 Pro")),
                    ("ProductType", u(1)),
                ]],
            )
            .with_rows(
                NS_CIMV2,
                crate::hardware::WQL_CPU,
                vec![vec![
                    ("Name", s("Example CPU")),
                    ("Manufacturer", s("ExampleVendor")),
                    ("NumberOfCores", u(8)),
                    ("NumberOfLogicalProcessors", u(16)),
                ]],
            )
            .with_rows(
                NS_CIMV2,
                crate::hardware::WQL_MEMORY,
                vec![
                    vec![
                        ("Capacity", s("8589934592")),
                        ("Speed", u(3200)),
                        ("ConfiguredClockSpeed", u(2400)),
                        ("DeviceLocator", s("DIMM_A1")),
                        ("BankLabel", s("BANK 0")),
                        ("SMBIOSMemoryType", u(26)),
                    ],
                    vec![
                        ("Capacity", s("8589934592")),
                        ("Speed", u(3200)),
                        ("ConfiguredClockSpeed", u(2400)),
                        ("DeviceLocator", s("DIMM_B1")),
                        ("BankLabel", s("BANK 2")),
                        ("SMBIOSMemoryType", u(26)),
                    ],
                ],
            )
            .with_rows(
                NS_CIMV2,
                crate::gpu_driver::WQL_VIDEO,
                vec![
                    vec![
                        ("Name", s("Microsoft Basic Display Adapter")),
                        ("PNPDeviceID", s("ROOT\\BASICDISPLAY\\0000")),
                        ("DriverVersion", s("10.0.26100.1")),
                    ],
                    vec![
                        ("Name", s("Example GPU")),
                        ("AdapterCompatibility", s("NVIDIA")),
                        ("PNPDeviceID", s("PCI\\VEN_10DE&DEV_0000&SUBSYS_00000000")),
                        ("DriverVersion", s("32.0.15.8180")),
                        ("DriverDate", s("20250820000000.000000-000")),
                    ],
                ],
            )
            .with_rows(
                NS_CIMV2,
                crate::hardware::WQL_COMPUTER,
                vec![vec![("PCSystemType", u(1))]],
            )
            .with_rows(
                NS_CIMV2,
                crate::restore::WQL_PRODUCT_TYPE,
                vec![vec![("ProductType", u(1))]],
            )
            .with_rows(
                NS_CIMV2,
                "ASSOCIATORS OF {Win32_LogicalDisk.DeviceID='C:'} WHERE AssocClass = Win32_LogicalDiskToPartition",
                vec![vec![("DiskIndex", u(0))]],
            )
            .with_rows(
                NS_STORAGE,
                "SELECT MediaType, FriendlyName FROM MSFT_PhysicalDisk WHERE DeviceId = '0'",
                vec![vec![("MediaType", u(4)), ("FriendlyName", s("Example NVMe"))]],
            )
            .with_rows(
                NS_DEVICEGUARD,
                crate::security::WQL_DEVICE_GUARD,
                vec![vec![
                    ("SecurityServicesRunning", arr(&[1, 2])),
                    ("SecurityServicesConfigured", arr(&[1, 2])),
                    ("AvailableSecurityProperties", arr(&[1, 2, 3])),
                ]],
            )
            .with_error(
                NS_TPM,
                crate::security::WQL_TPM,
                "query failed: HRESULT Call failed with: 0x80041003",
            ),
    );
    let reg = Arc::new(FakeRegistry::new());
    reg.set_external(
        crate::registry::Hive::LocalMachine,
        r"SYSTEM\CurrentControlSet\Control\SecureBoot\State",
        "UEFISecureBootEnabled",
        RawValue::dword(0),
    );
    let now = 1_800_000_000_000u64;
    let ops = Arc::new(FakeRestoreOps::new().with_point(41, "Windows Update", Some(now - 3 * 3_600_000)));
    let restore = Arc::new(
        RestoreService::new(ops, reg.clone(), wmi.clone())
            .with_clock(Arc::new(FixedClock(now)))
            .without_waiting(),
    );
    let mut env = SystemProbe::new(wmi, reg, Arc::new(FixtureFacts), restore)
        .with_background_wait(std::time::Duration::ZERO)
        .with_game_folders(std::path::PathBuf::from("no-such-program-data"), None)
        .probe(true);
    // A game on a hard drive, so the sample shows that finding.
    env.game_installs = Some(vec![
        crate::game_installs::GameInstall {
            game_id: "fortnite".into(),
            name: "Fortnite".into(),
            path: r"D:\Epic Games\Fortnite".into(),
            drive: "D:".into(),
            disk: crate::probe::Probe::yes(crate::hardware::BootDisk {
                media: crate::hardware::DiskMedia::Hdd,
                name: "Example HDD".into(),
            }),
            exe: Some(r"D:\Epic Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe".into()),
            steam_app: None,
        },
        // And one found in a Steam library, so the sample shows its Play button.
        crate::game_installs::GameInstall {
            game_id: "cs2".into(),
            name: "Counter-Strike 2".into(),
            path: r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive".into(),
            drive: "C:".into(),
            disk: crate::probe::Probe::yes(crate::hardware::BootDisk {
                media: crate::hardware::DiskMedia::Ssd,
                name: "Example SSD".into(),
            }),
            exe: Some(
                r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe"
                    .into(),
            ),
            steam_app: Some(730),
        },
    ]);
    env.gpu_choices = Some(vec![crate::gpu_choice::GameGpuChoice {
        game_id: "fortnite".into(),
        name: "Fortnite".into(),
        exe: Some(r"D:\Epic Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe".into()),
        preference: crate::probe::Probe::yes(crate::gpu_choice::GpuPreference::NotSet),
    }]);
    // The fake answers both process samples with the same rows (no time
    // passes), which reads as unknown; the sample shows a measured case.
    env.background = Some(crate::probe::Probe::yes(crate::background::BackgroundLoad {
        sample_ms: 2000,
        cpu_percent: 23.4,
        top: vec![
            crate::background::ProgramLoad {
                name: "ExampleUpdater".into(),
                processes: 1,
                cpu_percent: 14.2,
                memory_bytes: 210 << 20,
            },
            crate::background::ProgramLoad {
                name: "ExampleBrowser".into(),
                processes: 12,
                cpu_percent: 6.9,
                memory_bytes: 1_800 << 20,
            },
        ],
        unreadable: 0,
    }));
    env.target_game = Some("fortnite".into());
    SystemAudit::from_env(
        env,
        crate::settings::Settings::default(),
        Some(crate::hardware::RigClass::Mid),
    )
}

fn proof_run(
    id: &str,
    side: Side,
    index: u32,
    frame_ms: f64,
    throttle: crate::probe::Probe<ThrottleSummary>,
) -> ProofRun {
    ProofRun {
        run_id: id.into(),
        session_id: "session-1700000000000".into(),
        side,
        index,
        started_unix_ms: 1_700_000_000_000 + u64::from(index),
        seconds: 30,
        delay_seconds: 5,
        applied_tweaks: vec!["input.mouseaccel".into()],
        stats: compute_stats(&vec![frame_ms; 300]).unwrap(),
        csv_file: "capture.csv".into(),
        ignored_rows: 2,
        unusable_rows: 1,
        gpu_throttle: throttle,
    }
}

fn proof_run_fixture() -> ProofRun {
    proof_run(
        "run-1700000000001",
        Side::Before,
        1,
        10.0,
        crate::probe::Probe::yes(ThrottleSummary {
            samples: 31,
            seen: vec![ThrottleSeen {
                reason: ThrottleReason::SoftwarePowerCap,
                samples: 4,
            }],
        }),
    )
}

fn proof_sessions_fixture() -> Vec<ProofSessionSummary> {
    vec![ProofSessionSummary {
        session: ProofSession {
            session_id: "session-1700000000000".into(),
            created_unix_ms: 1_700_000_000_000,
            exe: "Game.exe".into(),
            game_id: Some("fortnite".into()),
            game_build: None,
            rig_class: Some(crate::hardware::RigClass::Mid),
            tool_version: "2.6.0".into(),
        },
        before_runs: 2,
        after_runs: 2,
    }]
}

fn comparison_fixture() -> crate::proof::verdict::Comparison {
    let run = |id: &str, avg: f64, low: f64| RunSummary {
        run_id: id.into(),
        avg_fps: avg,
        one_percent_low_fps: low,
    };
    let mut c = compare(
        &[
            run("run-1700000000001", 60.0, 40.0),
            run("run-1700000000002", 61.0, 41.0),
        ],
        &[
            run("run-1700000000003", 70.0, 50.0),
            run("run-1700000000004", 71.0, 51.0),
        ],
    );
    c.warnings =
        vec!["run-1700000000001 (before): the driver held clocks down at its power limit during this run.".into()];
    c
}

fn errors() -> Vec<EngineError> {
    vec![
        EngineError::NotElevated,
        EngineError::UserContextUnresolved { detail: "d".into() },
        EngineError::UserHiveNotLoaded { sid: "S-1-5-21".into() },
        EngineError::Registry {
            path: "HKEY_LOCAL_MACHINE\\K".into(),
            value: Some("V".into()),
            detail: "d".into(),
        },
        EngineError::Registry {
            path: "HKEY_LOCAL_MACHINE\\K".into(),
            value: None,
            detail: "d".into(),
        },
        EngineError::UnsupportedValueType {
            path: "p".into(),
            value: "v".into(),
            vtype: 0,
        },
        EngineError::Win32 {
            call: "CreateDirectoryW".into(),
            code: 5,
            detail: "Access is denied.".into(),
        },
        EngineError::Storage {
            path: "p".into(),
            detail: "d".into(),
        },
        EngineError::InsecureStorage {
            path: "p".into(),
            detail: "d".into(),
        },
        EngineError::SettingsFile {
            path: r"C:\Users\Kegan\AppData\Local\FortniteGame\Saved\Config\WindowsClient\GameUserSettings.ini".into(),
            detail: "is read-only, so PeakTweaks leaves it as it is".into(),
        },
        EngineError::NoJournalEntry { tweak_id: "t".into() },
        EngineError::Blocked {
            reason: BlockedReason::new(BlockedCode::AntiCheatEligibility, "m").with_trigger("fortnite"),
        },
        EngineError::UnknownTweak { tweak_id: "t".into() },
        EngineError::UnknownGame { game_id: "g".into() },
        EngineError::ContextViolation {
            tweak_id: "t".into(),
            detail: "d".into(),
        },
        EngineError::Command {
            what: "Checkpoint-Computer".into(),
            exit_code: Some(1),
            detail: "d".into(),
        },
        EngineError::Wmi {
            namespace: r"ROOT\CIMV2".into(),
            detail: "d".into(),
            timed_out: true,
        },
        EngineError::Internal { detail: "d".into() },
        EngineError::AlreadyRunning,
    ]
}

/// A real engine's startup list: one starting at sign-in, one PeakTweaks
/// turned off, one turned off in Task Manager, Windows Security (never
/// offered) and a shortcut in the Startup folder. The names are SAMPLE.
fn startup_list() -> StartupList {
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED_RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    let mut h = Harness::new(Vec::new());
    let set = |hive, key, name: &str, value| h.fake.set_external(hive, key, name, value);
    set(
        Hive::CurrentUser,
        RUN,
        "Sample chat app",
        RawValue::sz(r"C:\Sample\chat.exe --minimized"),
    );
    set(
        Hive::CurrentUser,
        RUN,
        "Sample game launcher",
        RawValue::sz(r"C:\Sample\launcher.exe -silent"),
    );
    set(
        Hive::CurrentUser,
        RUN,
        "Sample updater",
        RawValue::sz(r"C:\Sample\updater.exe"),
    );
    set(
        Hive::CurrentUser,
        APPROVED_RUN,
        "Sample updater",
        StartupToggle::off_for_test(),
    );
    set(
        Hive::LocalMachine,
        RUN,
        "SecurityHealth",
        RawValue::sz(r"%windir%\system32\SecurityHealthSystray.exe"),
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Sample notes.lnk"), b"").unwrap();
    // A Store app with a startup task, installed in a folder of its own.
    let package = dir.path().join("package");
    std::fs::create_dir(&package).unwrap();
    std::fs::write(
        package.join("AppxManifest.xml"),
        r#"<Package><uap5:StartupTask TaskId="SampleStartup" Enabled="true" DisplayName="Sample Store app"/></Package>"#,
    )
    .unwrap();
    h.fake.set_external(
        Hive::CurrentUser,
        &format!(r"{}\Sample.StoreApp_1.0.0.0_x64__sample", crate::startup::PACKAGES),
        "PackageRootFolder",
        RawValue::sz(&package.to_string_lossy()),
    );
    let folders = StartupFolders {
        user: Some(dir.path().to_path_buf()),
        machine: Some(dir.path().join("none")),
    };
    h.engine.startup_apps(&folders);
    h.engine.apply("startup.user_run:Sample game launcher").unwrap();
    h.engine.startup_apps(&folders)
}

/// A real engine's MSI mode list: a graphics card with MSI off and a network
/// adapter whose driver has it on. The device names are SAMPLE.
fn msi_devices() -> crate::tweaks::msi::MsiDeviceList {
    use crate::system::{DeviceClass, PciDevice};
    const GPU: &str = r"PCI\VEN_10DE&DEV_2484&SUBSYS_146710DE&REV_A1\4&2b0b1f0c&0&0008";
    const NIC: &str = r"PCI\VEN_10EC&DEV_8125&SUBSYS_86771043&REV_05\01000000684CE00000";
    let mut h = Harness::new(Vec::new());
    for i in [GPU, NIC] {
        h.fake.set_external(
            Hive::LocalMachine,
            &format!(r"SYSTEM\CurrentControlSet\Enum\{i}"),
            "DeviceDesc",
            RawValue::sz("SAMPLE"),
        );
    }
    h.fake.set_external(
        Hive::LocalMachine,
        &format!(
            r"SYSTEM\CurrentControlSet\Enum\{NIC}\Device Parameters\Interrupt Management\MessageSignaledInterruptProperties"
        ),
        "MSISupported",
        RawValue::dword(1),
    );
    h.sys.set_pci_devices(vec![
        PciDevice {
            instance_id: GPU.into(),
            name: "Sample graphics card".into(),
            class: DeviceClass::Display,
        },
        PciDevice {
            instance_id: NIC.into(),
            name: "Sample network adapter".into(),
            class: DeviceClass::Net,
        },
    ]);
    h.engine.msi_devices()
}

#[test]
fn writes_fixtures_that_typescript_checks_against_the_generated_types() {
    let mut out = String::from(
        "// Generated by the engine test `contract::writes_fixtures_...`. Do not edit.\n\
         // Real serialized values, checked by `tsc` against the generated types.\n\
         import type { AreaSize } from \"./AreaSize\";\n\
         import type { CleanupReport } from \"./CleanupReport\";\n\
         import type { Comparison } from \"./Comparison\";\n\
         import type { ContextInfo } from \"./ContextInfo\";\n\
         import type { DriveOptimization } from \"./DriveOptimization\";\n\
         import type { EngineError } from \"./EngineError\";\n\
         import type { GameInfo } from \"./GameInfo\";\n\
         import type { JournalView } from \"./JournalView\";\n\
         import type { MsiDeviceList } from \"./MsiDeviceList\";\n\
         import type { NetworkCheck } from \"./NetworkCheck\";\n\
         import type { PlayStatus } from \"./PlayStatus\";\n\
         import type { Progress } from \"./Progress\";\n\
         import type { ProofRun } from \"./ProofRun\";\n\
         import type { ProofSessionSummary } from \"./ProofSessionSummary\";\n\
         import type { RestoreOutcome } from \"./RestoreOutcome\";\n\
         import type { RevertResult } from \"./RevertResult\";\n\
         import type { StandbyPurge } from \"./StandbyPurge\";\n\
         import type { StartupList } from \"./StartupList\";\n\
         import type { SystemAudit } from \"./SystemAudit\";\n\
         import type { TweakView } from \"./TweakView\";\n\n",
    );

    ts_const(&mut out, "tweakViews", "TweakView[]", &views());
    ts_const(
        &mut out,
        "contextInfo",
        "ContextInfo",
        &ContextInfo {
            sid: "S-1-5-21-1-2-3-1001".into(),
            resolution: UserResolution::InteractiveShell,
            is_self: false,
            elevated: true,
            tester_build: false,
            booted_unix_ms: Some(1_700_000_100_000),
        },
    );
    ts_const(
        &mut out,
        "revertResults",
        "RevertResult[]",
        &vec![
            RevertResult {
                tweak_id: "a".into(),
                ok: true,
                error: None,
            },
            RevertResult {
                tweak_id: "b".into(),
                ok: false,
                error: Some("registry K: access denied".into()),
            },
        ],
    );
    ts_const(&mut out, "journalView", "JournalView", &journal_view());
    ts_const(&mut out, "engineErrors", "EngineError[]", &errors());
    ts_const(&mut out, "systemAudit", "SystemAudit", &audit());
    ts_const(&mut out, "proofRun", "ProofRun", &proof_run_fixture());
    ts_const(
        &mut out,
        "proofSessions",
        "ProofSessionSummary[]",
        &proof_sessions_fixture(),
    );
    ts_const(&mut out, "comparison", "Comparison", &comparison_fixture());
    ts_const(
        &mut out,
        "restoreOutcome",
        "RestoreOutcome",
        &RestoreOutcome {
            sequence_number: 42,
            description: "PeakTweaks: before changes".into(),
            method: RestoreMethod::Api,
            protection_enabled_by_us: None,
        },
    );
    // The shape of a real result; the numbers are illustrative (SAMPLE in the UI).
    ts_const(
        &mut out,
        "standbyPurge",
        "StandbyPurge",
        &StandbyPurge {
            before: MemoryUse {
                total_bytes: 16 * GIB,
                available_bytes: 9 * GIB,
                cached_bytes: 6 * GIB,
            },
            after: MemoryUse {
                total_bytes: 16 * GIB,
                available_bytes: 9 * GIB + GIB / 2,
                cached_bytes: GIB,
            },
            unix_ms: 1_791_331_200_000,
        },
    );
    // A real check's shape from a scripted network: the router answers every
    // echo, one public server loses one (SAMPLE in the UI).
    ts_const(&mut out, "networkCheck", "NetworkCheck", &network_check_fixture());
    ts_const(&mut out, "cleanupSizes", "AreaSize[]", &cleanup_sizes());
    ts_const(
        &mut out,
        "driveOptimization",
        "DriveOptimization",
        &DriveOptimization {
            drive: "C:".into(),
            unix_ms: 1_791_331_800_000,
            seconds: 41,
            report: vec![
                "Invoking retrim on (C:)...".into(),
                "Retrim:  100% complete.".into(),
                "The operation completed successfully.".into(),
            ],
        },
    );
    ts_const(&mut out, "cleanupReport", "CleanupReport", &cleanup_report());
    ts_const(
        &mut out,
        "progressEvent",
        "Progress",
        &Progress {
            stage: "apply".into(),
            tweak_id: Some("input.mouseaccel".into()),
            message: "Applying".into(),
        },
    );

    // A game running with Gaming Mode on and the timer held at 0.5 ms.
    ts_const(
        &mut out,
        "playStatus",
        "PlayStatus",
        &PlayStatus {
            game: Some("fortnite".into()),
            gaming_mode_active: true,
            timer_held: Some(5_000),
            problem: None,
            ..PlayStatus::watching()
        },
    );

    ts_const(&mut out, "games", "GameInfo[]", &crate::env::KNOWN_GAMES);
    ts_const(&mut out, "startupList", "StartupList", &startup_list());
    ts_const(&mut out, "msiDevices", "MsiDeviceList", &msi_devices());

    let dir = generated_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("fixtures.ts"), out).unwrap();
}

#[test]
fn every_engine_error_variant_has_a_fixture() {
    // If a variant is added, this match stops compiling until `errors()` and the
    // fixture are updated, so a new error can never ship without a TS check.
    for e in errors() {
        match e {
            EngineError::NotElevated
            | EngineError::UserContextUnresolved { .. }
            | EngineError::UserHiveNotLoaded { .. }
            | EngineError::Registry { .. }
            | EngineError::UnsupportedValueType { .. }
            | EngineError::Win32 { .. }
            | EngineError::Storage { .. }
            | EngineError::InsecureStorage { .. }
            | EngineError::SettingsFile { .. }
            | EngineError::NoJournalEntry { .. }
            | EngineError::Blocked { .. }
            | EngineError::UnknownTweak { .. }
            | EngineError::UnknownGame { .. }
            | EngineError::ContextViolation { .. }
            | EngineError::Command { .. }
            | EngineError::Wmi { .. }
            | EngineError::Internal { .. }
            | EngineError::AlreadyRunning => {}
        }
    }
}
