//! The Rust <-> TypeScript contract.
//!
//! `cargo test` regenerates the bindings in `src/generated/` (ts-rs) and
//! `src/generated/fixtures.ts`: real serialized values wrapped in `satisfies`
//! against the generated types. `tsc` therefore fails when the wire format and
//! the types drift apart, and CI fails when either file is stale.

use std::fmt::Write as _;
use std::path::PathBuf;

use serde::Serialize;

use crate::context::UserResolution;
use crate::engine::{ContextInfo, JournalView, Progress, RevertResult};
use crate::error::EngineError;
use crate::journal::{
    CommitAction, CommitRecord, JournalAction, JournalEntry, JournalWarning, JournalWarningKind, Record,
};
use crate::testutil::*;
use crate::types::{BlockedCode, BlockedReason, ExecutionContext, RawValue, RegRoot, Tweak};

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
        ],
        warnings: vec![JournalWarning {
            line: 7,
            kind: JournalWarningKind::UnparsableLine,
            detail: "skipped: expected value at line 1 column 1".into(),
        }],
    }
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
        EngineError::Internal { detail: "d".into() },
    ]
}

#[test]
fn writes_fixtures_that_typescript_checks_against_the_generated_types() {
    let mut out = String::from(
        "// Generated by the engine test `contract::writes_fixtures_...`. Do not edit.\n\
         // Real serialized values, checked by `tsc` against the generated types.\n\
         import type { ContextInfo } from \"./ContextInfo\";\n\
         import type { EngineError } from \"./EngineError\";\n\
         import type { JournalView } from \"./JournalView\";\n\
         import type { Progress } from \"./Progress\";\n\
         import type { RevertResult } from \"./RevertResult\";\n\
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
            | EngineError::Internal { .. } => {}
        }
    }
}
