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

use crate::context::UserResolution;
use crate::engine::{ContextInfo, JournalView, Progress, RevertResult};
use crate::env::EnvProbe;
use crate::error::EngineError;
use crate::hardware::{DisplayInfo, GpuAdapter, OsFacts, GIB};
use crate::journal::{
    CommitAction, CommitRecord, JournalAction, JournalEntry, JournalWarning, JournalWarningKind, Record, RestoreMethod,
    RestorePointRecord,
};
use crate::proof::metrics::compute_stats;
use crate::proof::nvml::{ThrottleReason, ThrottleSeen, ThrottleSummary};
use crate::proof::store::{ProofRun, ProofSession, ProofSessionSummary, Side};
use crate::proof::verdict::{compare, RunSummary};
use crate::registry::fake::FakeRegistry;
use crate::restore::{FakeRestoreOps, FixedClock, RestoreOutcome, RestoreService};
use crate::sysprobe::{SystemAudit, SystemProbe};
use crate::testutil::*;
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

    let tweaks: Vec<Box<dyn Tweak>> = vec![
        Box::new(plain),
        Box::new(applied),
        Box::new(foreign),
        Box::new(blocked),
        Box::new(unknown),
    ];
    let mut h = Harness::new(tweaks);
    h.engine.apply("fixture.applied").unwrap();
    h.fake
        .set_external(crate::registry::Hive::LocalMachine, "K3", "A", RawValue::dword(1));
    h.engine.list().unwrap()
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
                protection_enabled_by_us: true,
            }),
        ],
        warnings: vec![JournalWarning {
            line: 7,
            kind: JournalWarningKind::UnparsableLine,
            detail: "skipped: expected value at line 1 column 1".into(),
        }],
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
    let mut env = SystemProbe::new(wmi, reg, Arc::new(FixtureFacts), restore).probe(true);
    env.target_game = Some("fortnite".into());
    SystemAudit::from_env(env)
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
    ]
}

#[test]
fn writes_fixtures_that_typescript_checks_against_the_generated_types() {
    let mut out = String::from(
        "// Generated by the engine test `contract::writes_fixtures_...`. Do not edit.\n\
         // Real serialized values, checked by `tsc` against the generated types.\n\
         import type { Comparison } from \"./Comparison\";\n\
         import type { ContextInfo } from \"./ContextInfo\";\n\
         import type { EngineError } from \"./EngineError\";\n\
         import type { JournalView } from \"./JournalView\";\n\
         import type { Progress } from \"./Progress\";\n\
         import type { ProofRun } from \"./ProofRun\";\n\
         import type { ProofSessionSummary } from \"./ProofSessionSummary\";\n\
         import type { RestoreOutcome } from \"./RestoreOutcome\";\n\
         import type { RevertResult } from \"./RevertResult\";\n\
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
            protection_enabled_by_us: false,
        },
    );
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
            | EngineError::NoJournalEntry { .. }
            | EngineError::Blocked { .. }
            | EngineError::UnknownTweak { .. }
            | EngineError::UnknownGame { .. }
            | EngineError::ContextViolation { .. }
            | EngineError::Command { .. }
            | EngineError::Wmi { .. }
            | EngineError::Internal { .. } => {}
        }
    }
}
