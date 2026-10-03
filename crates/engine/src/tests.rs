//! Engine-level scenarios against the in-memory registry: journal semantics,
//! revert correctness, rollback, allowlist, gates.

use std::sync::Arc;

use crate::env::{License, StubProbe};
use crate::error::EngineError;
use crate::journal::{CommitAction, Journal, JournalAction, JournalEntry, Record};
use crate::registry::fake::FakeRegistry;
use crate::registry::Hive;
use crate::secure_dir::TrustedDir;
use crate::testutil::*;
use crate::transaction::Transaction;
use crate::types::{BlockedCode, ExecutionContext, RawValue, RegRoot, Tier, Tweak, TweakState};

const KEY: &str = r"SOFTWARE\PeakTest\Sched";

fn one(t: TestTweak) -> Vec<Box<dyn Tweak>> {
    vec![Box::new(t)]
}

fn dw(h: &Harness, name: &str) -> Option<u32> {
    hklm_dword(&h.fake, KEY, name)
}

#[test]
fn apply_then_revert_deletes_values_and_the_key_we_created() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1), ("B", 2)])));
    assert!(h.fake.key_paths().is_empty());

    let written = h.engine.apply("t").unwrap();
    assert_eq!(written.len(), 2);
    assert_eq!(dw(&h, "A"), Some(1));
    assert_eq!(dw(&h, "B"), Some(2));
    // The first write created SOFTWARE\PeakTest\Sched; all three levels are recorded.
    assert_eq!(written[0].created_keys, vec!["SOFTWARE", r"SOFTWARE\PeakTest", KEY]);
    assert!(written[1].created_keys.is_empty());

    h.engine.revert("t").unwrap();
    assert_eq!(dw(&h, "A"), None);
    assert_eq!(dw(&h, "B"), None);
    assert!(
        h.fake.key_paths().is_empty(),
        "created keys removed: {:?}",
        h.fake.key_paths()
    );
}

#[test]
fn revert_restores_the_exact_prior_value_not_a_default() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(0x28));
    h.engine.apply("t").unwrap();
    assert_eq!(dw(&h, "A"), Some(1));
    h.engine.revert("t").unwrap();
    assert_eq!(dw(&h, "A"), Some(0x28));
}

#[test]
fn keys_that_already_existed_are_left_alone() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "Other", dword(9));
    h.engine.apply("t").unwrap();
    h.engine.revert("t").unwrap();
    assert_eq!(dw(&h, "Other"), Some(9));
    assert!(h.fake.key_exists_for_test(Hive::LocalMachine, KEY));
}

// R4: apply -> revert -> external change -> apply -> revert must land on the
// external value, not the oldest one.
#[test]
fn revert_after_an_external_change_lands_on_the_value_before_the_latest_apply() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(0x02));
    h.engine.apply("t").unwrap();
    h.engine.revert("t").unwrap();
    assert_eq!(dw(&h, "A"), Some(0x02));

    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(0x28));
    h.engine.apply("t").unwrap();
    h.engine.revert("t").unwrap();
    assert_eq!(
        dw(&h, "A"),
        Some(0x28),
        "must not fall back to the oldest recorded value"
    );
}

/// Agent brief bug 4: a revert whose writes are all no-ops (the user already put
/// the value back) must still close the apply, or the tweak stays "applied"
/// and every Undo all retries it.
#[test]
fn a_revert_with_nothing_to_write_still_ends_the_apply() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(5));
    h.engine.apply("t").unwrap();
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(5));
    let before = h.fake.mutation_count();
    h.engine.revert("t").unwrap();
    assert_eq!(h.fake.mutation_count(), before, "nothing needed writing");
    assert!(h.engine.applied_tweak_ids().is_empty(), "the apply is closed");
    assert!(h.engine.revert_all().is_empty(), "Undo all has nothing left to retry");
}

/// Agent brief bug 8: a tweak applied before something started blocking it
/// (a new target game, a changed machine) is still applied. The list must say
/// so, with the block alongside, so the UI keeps offering Undo.
#[test]
fn an_applied_tweak_that_becomes_blocked_still_shows_applied_and_the_reason() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.engine.apply("t").unwrap();
    let mut blocked = TestTweak::new("t", KEY, &[("A", 1)]);
    blocked.block = true;
    h.restart(one(blocked));

    let view = h
        .engine
        .list()
        .unwrap()
        .into_iter()
        .find(|v| v.metadata.id == "t")
        .unwrap();
    assert_eq!(view.state, TweakState::Applied);
    assert_eq!(view.blocked.as_ref().map(|r| r.message.as_str()), Some("test block"));

    h.engine.revert("t").unwrap();
    let view = h
        .engine
        .list()
        .unwrap()
        .into_iter()
        .find(|v| v.metadata.id == "t")
        .unwrap();
    assert!(matches!(view.state, TweakState::Blocked { .. }), "{:?}", view.state);
    assert!(view.blocked.is_some());
}

/// Agent brief bug 9: besides one `.reg` per value, each applied change gets
/// one combined file that restores all of it, newest write first, for recovery
/// by hand (Safe Mode) when PeakTweaks itself cannot run.
#[test]
fn each_applied_change_gets_one_session_reg_in_restore_order() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1), ("B", 2)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(5));
    let entries = h.engine.apply("t").unwrap();
    let tx = entries[0].tx_id;

    let dir = std::path::Path::new(&entries[0].backup_file)
        .parent()
        .unwrap()
        .to_owned();
    let path = h.dir.path().join(&dir).join(format!("session_{tx}_t.reg"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(&bytes[..2], &[0xFF, 0xFE], "UTF-16LE with a BOM");
    let units: Vec<u16> = bytes[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let text = String::from_utf16(&units).unwrap();
    assert!(
        text.starts_with("Windows Registry Editor Version 5.00\r\n\r\n"),
        "{text}"
    );
    let b = text.find("\"B\"=-").expect("B did not exist before: deleted");
    let a = text.find("\"A\"=dword:00000005").expect("A goes back to 5");
    assert!(b < a, "newest write first:\n{text}");
    assert_eq!(
        text.matches("[HKEY_LOCAL_MACHINE\\SOFTWARE\\PeakTest\\Sched]").count(),
        2,
        "{text}"
    );

    // A revert is not a change to recover from: no session file for it.
    let before = std::fs::read_dir(h.dir.path().join(&dir)).unwrap().count();
    h.engine.revert("t").unwrap();
    let names: Vec<String> = std::fs::read_dir(h.dir.path().join(&dir))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names.iter().filter(|n| n.starts_with("session_")).count(),
        1,
        "{names:?} (was {before} files)"
    );
}

#[test]
fn the_offline_undo_set_follows_every_apply_and_revert() {
    const KEY2: &str = r"SOFTWARE\PeakTest\Other";
    let mut h = Harness::new(vec![
        Box::new(TestTweak::new("first", KEY, &[("A", 1)])),
        Box::new(TestTweak::new("second", KEY2, &[("B", 2)])),
    ]);
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(5));
    let offline = h.dir.path().join(crate::offline::DIR);
    let reg_files = || -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = std::fs::read_dir(&offline)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "reg"))
            .map(|p| {
                let bytes = std::fs::read(&p).unwrap();
                let units: Vec<u16> = bytes[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    String::from_utf16(&units).unwrap(),
                )
            })
            .collect();
        v.sort();
        v
    };

    h.engine.apply("first").unwrap();
    h.engine.apply("second").unwrap();
    let files = reg_files();
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["001_second.reg", "002_first.reg"], "Undo all order");
    assert!(files[1]
        .1
        .contains("[HKEY_LOCAL_MACHINE\\PT_OFFLINE_SOFTWARE\\PeakTest\\Sched]\r\n\"A\"=dword:00000005"));
    assert!(files[0].1.contains("\"B\"=-"), "B did not exist: deleted");
    assert!(offline.join("recover.cmd").is_file());

    h.engine.revert("second").unwrap();
    let names: Vec<String> = reg_files().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["001_first.reg"], "a reverted change leaves the set");

    h.engine.revert("first").unwrap();
    assert!(reg_files().is_empty());
}

#[test]
fn double_apply_still_reverts_to_the_state_before_the_first() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(5));
    h.engine.apply("t").unwrap();
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(7));
    h.engine.apply("t").unwrap();
    h.engine.revert("t").unwrap();
    assert_eq!(dw(&h, "A"), Some(5));
}

#[test]
fn overlapping_tweaks_revert_in_reverse_apply_order() {
    let a = TestTweak::new("a", KEY, &[("V", 10)]);
    let b = TestTweak::new("b", KEY, &[("V", 20)]);
    let mut h = Harness::new(vec![Box::new(a), Box::new(b)]);
    h.fake.set_external(Hive::LocalMachine, KEY, "V", dword(1));
    // Applied out of catalogue order on purpose: b first, then a.
    h.engine.apply("b").unwrap();
    h.engine.apply("a").unwrap();
    assert_eq!(dw(&h, "V"), Some(10));

    let results = h.engine.revert_all();
    assert_eq!(
        results.iter().map(|r| r.tweak_id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert!(results.iter().all(|r| r.ok), "{results:?}");
    assert_eq!(dw(&h, "V"), Some(1), "back to the original, not an intermediate value");
}

#[test]
fn crash_mid_apply_still_reverts() {
    let tweak = TestTweak::new("t", KEY, &[("A", 1), ("B", 2), ("C", 3)]);
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1), ("B", 2), ("C", 3)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "B", dword(99));

    // Write two of three, then "crash": drop the transaction without a commit.
    {
        let (resolver, journal, _) = h.engine.parts_for_test();
        let mut tx = Transaction::begin(&tweak, resolver, journal, JournalAction::Apply).unwrap();
        tx.set_dword(RegRoot::LocalMachine, KEY, "A", 1).unwrap();
        tx.set_dword(RegRoot::LocalMachine, KEY, "B", 2).unwrap();
        drop(tx);
    }
    assert_eq!(dw(&h, "A"), Some(1));

    // App restarts; the journal has the writes but no commit.
    h.restart(one(TestTweak::new("t", KEY, &[("A", 1), ("B", 2), ("C", 3)])));
    let views = h.engine.list().unwrap();
    assert_eq!(views[0].state, TweakState::Default); // not all three set
    h.engine.revert("t").unwrap();
    assert_eq!(dw(&h, "A"), None);
    assert_eq!(dw(&h, "B"), Some(99));
    assert_eq!(dw(&h, "C"), None);
}

// R6
#[test]
fn a_failed_apply_rolls_back_its_own_writes_and_returns_the_original_error() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1), ("B", 2), ("C", 3)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(7));
    let before = h.fake.snapshot();

    h.fake.fail_mutation_number(2); // second of three writes
    let err = h.engine.apply("t").unwrap_err();
    assert!(
        matches!(err, EngineError::Registry { .. }),
        "original error kept: {err:?}"
    );
    assert_eq!(h.fake.snapshot(), before, "registry back to how it was");

    // Journal: not applied, and the rollback is recorded.
    let view = h.engine.journal_view();
    assert!(matches!(
        view.records.last(),
        Some(Record::Commit(c)) if c.action == CommitAction::Rollback
    ));
    assert!(h.engine.list().unwrap()[0].state != TweakState::Applied);
    assert!(matches!(h.engine.revert("t"), Err(EngineError::NoJournalEntry { .. })));

    // And the tweak still applies cleanly afterwards.
    h.engine.apply("t").unwrap();
    assert_eq!(dw(&h, "C"), Some(3));
}

#[test]
fn rollback_of_a_second_apply_leaves_the_first_reversible() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1), ("B", 2)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(5));
    h.engine.apply("t").unwrap();

    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(6));
    h.fake.fail_mutation_number(1);
    assert!(h.engine.apply("t").is_err());
    assert_eq!(dw(&h, "A"), Some(6), "rollback restored the externally-set value");

    h.engine.revert("t").unwrap();
    assert_eq!(dw(&h, "A"), Some(5), "the first apply is still outstanding and reverts");
}

#[test]
fn a_failed_revert_can_be_retried_to_completion() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1), ("B", 2), ("C", 3)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(10));
    h.fake.set_external(Hive::LocalMachine, KEY, "B", dword(20));
    h.fake.set_external(Hive::LocalMachine, KEY, "C", dword(30));
    h.engine.apply("t").unwrap();

    h.fake.fail_mutation_number(2);
    assert!(h.engine.revert("t").is_err());
    assert!(
        h.engine
            .journal_view()
            .records
            .iter()
            .rev()
            .take(1)
            .all(|r| matches!(r, Record::Write(_))),
        "no commit after a failed revert"
    );

    h.engine.revert("t").unwrap();
    assert_eq!((dw(&h, "A"), dw(&h, "B"), dw(&h, "C")), (Some(10), Some(20), Some(30)));
}

// R1
#[test]
fn a_journal_line_outside_the_allowlist_is_rejected_and_nothing_is_written() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.engine.apply("t").unwrap();

    // An attacker (or a bug) appends a journal line for tweak "t" that names an
    // unrelated key. It must not be replayed.
    let evil = JournalEntry {
        seq: 900,
        tx_id: 900,
        unix_ms: 0,
        tweak_id: "t".into(),
        action: JournalAction::Apply,
        context: ExecutionContext::Service,
        root: RegRoot::LocalMachine,
        key_path: r"SYSTEM\CurrentControlSet\Services\Evil".into(),
        display_path: String::new(),
        value_name: "ImagePath".into(),
        previous: Some(RawValue::sz(r"C:\attacker.exe")),
        written: None,
        backup_file: String::new(),
        created_keys: vec![],
    };
    {
        let (_, journal, _) = h.engine.parts_for_test();
        journal.append_write(evil).unwrap();
    }
    let before = h.fake.snapshot();

    let err = h.engine.revert("t").unwrap_err();
    assert!(matches!(err, EngineError::ContextViolation { .. }), "{err:?}");
    assert_eq!(
        h.fake.snapshot(),
        before,
        "nothing written, not even the legitimate part"
    );
    assert!(!h
        .fake
        .key_exists_for_test(Hive::LocalMachine, r"SYSTEM\CurrentControlSet\Services\Evil"));
}

#[test]
fn created_keys_outside_the_tweaks_path_are_rejected() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.engine.apply("t").unwrap();
    let mut evil = h
        .engine
        .journal_view()
        .records
        .iter()
        .find_map(|r| match r {
            Record::Write(e) => Some(e.clone()),
            _ => None,
        })
        .unwrap();
    evil.seq = 901;
    evil.tx_id = 901;
    evil.created_keys = vec![r"SOFTWARE\Microsoft".into()];
    {
        let (_, journal, _) = h.engine.parts_for_test();
        journal.append_write(evil).unwrap();
    }
    h.fake
        .set_external(Hive::LocalMachine, r"SOFTWARE\Microsoft", "x", dword(1));
    assert!(matches!(
        h.engine.revert("t"),
        Err(EngineError::ContextViolation { .. })
    ));
}

#[test]
fn a_tweak_cannot_write_outside_its_declared_targets() {
    let mut t = TestTweak::new("t", KEY, &[("A", 1), ("Sneaky", 2)]);
    t.allow = vec!["A".into()];
    let mut h = Harness::new(one(t));
    let before = h.fake.snapshot();
    let err = h.engine.apply("t").unwrap_err();
    assert!(matches!(err, EngineError::ContextViolation { .. }), "{err:?}");
    assert_eq!(h.fake.snapshot(), before, "the allowed write was rolled back too");
}

#[test]
fn allowlist_comparison_ignores_case() {
    let mut t = TestTweak::new("t", KEY, &[("Alpha", 1)]);
    t.allow = vec!["ALPHA".into()];
    t.key = KEY.to_owned();
    let mut h = Harness::new(one(t));
    h.engine.apply("t").unwrap();
}

#[test]
fn a_user_context_tweak_cannot_write_hklm() {
    struct Bad(TestTweak);
    impl Tweak for Bad {
        fn id(&self) -> &str {
            self.0.id()
        }
        fn metadata(&self) -> crate::types::TweakMetadata {
            self.0.metadata()
        }
        fn execution_context(&self) -> ExecutionContext {
            ExecutionContext::User
        }
        fn touches(&self) -> Vec<crate::types::RegTarget> {
            self.0.touches()
        }
        fn read_state(&self, r: &crate::context::ContextResolver, j: bool) -> crate::error::Result<TweakState> {
            self.0.read_state(r, j)
        }
        fn apply(&self, tx: &mut Transaction) -> crate::error::Result<()> {
            tx.set_dword(RegRoot::LocalMachine, KEY, "A", 1)
        }
    }
    let mut h = Harness::new(vec![Box::new(Bad(TestTweak::new("t", KEY, &[("A", 1)])))]);
    assert!(matches!(h.engine.apply("t"), Err(EngineError::ContextViolation { .. })));
}

#[test]
fn values_of_types_we_cannot_restore_are_left_alone() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.fake.set_external(
        Hive::LocalMachine,
        KEY,
        "A",
        RawValue {
            vtype: 0,
            bytes: vec![1, 2, 3],
        },
    );
    let before = h.fake.snapshot();
    let err = h.engine.apply("t").unwrap_err();
    assert!(
        matches!(err, EngineError::UnsupportedValueType { vtype: 0, .. }),
        "{err:?}"
    );
    assert_eq!(h.fake.snapshot(), before);
}

// R2 / R12 / gates
#[test]
fn apply_is_refused_without_a_verified_restore_point() {
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::with(one(TestTweak::new("t", KEY, &[("A", 1)])), fake, dir, false);
    let err = h.engine.apply("t").unwrap_err();
    assert!(
        matches!(&err, EngineError::Blocked { reason } if reason.code == BlockedCode::NoRestorePoint),
        "{err:?}"
    );
    assert!(h.fake.snapshot().is_empty());
}

#[test]
fn revert_is_never_gated() {
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::with(one(TestTweak::new("t", KEY, &[("A", 1)])), fake.clone(), dir, true);
    h.engine.apply("t").unwrap();
    // Restart with the gate closed and a Free license.
    let engine = build_engine(
        &fake,
        h.dir.path(),
        one(TestTweak::new("t", KEY, &[("A", 1)])),
        false,
        Tier::Free,
    );
    h.engine = engine;
    h.engine.revert("t").unwrap();
    assert_eq!(dw(&h, "A"), None);
}

#[test]
fn tier_is_enforced_by_the_engine() {
    let mut t = TestTweak::new("t", KEY, &[("A", 1)]);
    t.tier = Tier::Pro;
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let mut engine = build_engine(&fake, dir.path(), one(t), true, Tier::Free);
    let err = engine.apply("t").unwrap_err();
    assert!(
        matches!(&err, EngineError::Blocked { reason } if reason.code == BlockedCode::TierRequired),
        "{err:?}"
    );
    assert!(fake.snapshot().is_empty());
    assert_eq!(License::free().tier(), Tier::Free);
}

#[test]
fn predicates_block_apply() {
    let mut t = TestTweak::new("t", KEY, &[("A", 1)]);
    t.block = true;
    let mut h = Harness::new(one(t));
    assert!(matches!(h.engine.apply("t"), Err(EngineError::Blocked { .. })));
    assert!(matches!(h.engine.list().unwrap()[0].state, TweakState::Blocked { .. }));
}

// R8
#[test]
fn a_state_read_failure_is_unknown_and_blocks_apply() {
    let mut t = TestTweak::new("t", KEY, &[("A", 1)]);
    t.fail_read = true;
    let mut h = Harness::new(one(t));
    assert!(matches!(h.engine.list().unwrap()[0].state, TweakState::Unknown { .. }));
    assert!(h.engine.apply("t").is_err());
    assert!(h.fake.snapshot().is_empty());
}

#[test]
fn unknown_ids_are_rejected() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    assert!(matches!(h.engine.apply("nope"), Err(EngineError::UnknownTweak { .. })));
    assert!(matches!(h.engine.revert("nope"), Err(EngineError::UnknownTweak { .. })));
    assert!(matches!(
        h.engine.select_target_game(Some("nope".into())),
        Err(EngineError::UnknownGame { .. })
    ));
    h.engine.select_target_game(Some("fortnite".into())).unwrap();
    assert_eq!(h.engine.env().target_game.as_deref(), Some("fortnite"));
    h.engine.select_target_game(None).unwrap();
    assert_eq!(h.engine.env().target_game, None);
}

#[test]
fn engine_env_comes_from_the_probe_only() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    assert!(!h.engine.env().restore_gate_open, "nothing is probed at construction");
    h.engine.rescan();
    assert!(h.engine.env().restore_gate_open, "open dev probe");
    let mut closed = build_engine(
        &h.fake,
        h.dir.path(),
        one(TestTweak::new("t", KEY, &[("A", 1)])),
        false,
        Tier::Free,
    );
    closed.rescan();
    assert!(!closed.env().restore_gate_open);
    let _ = StubProbe::closed();
}

// Journal + transaction integration
#[test]
fn seq_stays_monotonic_across_restarts_and_the_journal_survives_reopen() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.engine.apply("t").unwrap();
    h.restart(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.engine.revert("t").unwrap();
    let seqs: Vec<u64> = h.engine.journal_view().records.iter().map(Record::seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
    let j = Journal::open(&TrustedDir::insecure_for_tests(h.dir.path())).unwrap();
    assert!(!j.is_applied("t"));
}

#[test]
fn backups_are_written_before_the_registry_changes() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(0x28));
    let written = h.engine.apply("t").unwrap();
    let path = h.dir.path().join(&written[0].backup_file);
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
    let units = crate::types::utf16le_units(&bytes[2..]);
    let text = String::from_utf16(&units).unwrap();
    assert!(text.contains("\"A\"=dword:00000028"), "{text}");
    assert!(text.contains(r"[HKEY_LOCAL_MACHINE\SOFTWARE\PeakTest\Sched]"), "{text}");
}

#[test]
fn user_hive_writes_route_through_the_resolved_user() {
    let mut t = TestTweak::new("u", r"Control Panel\Test", &[("X", 1)]);
    t.context = ExecutionContext::User;
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    // Not our own SID: must land in HKEY_USERS\<sid>, never in HKCU.
    let resolver = crate::context::ContextResolver::new(user(false), true, fake.clone());
    let journal = Journal::open(&TrustedDir::insecure_for_tests(dir.path())).unwrap();
    let mut engine = crate::engine::Engine::new(
        resolver,
        journal,
        one(t),
        Box::new(StubProbe::open_for_dev()),
        License::dev(Tier::Ultimate),
    );
    engine.apply("u").unwrap();
    assert!(fake
        .read_value_for_test(Hive::Users, &format!(r"{SID}\Control Panel\Test"), "X")
        .is_some());
    assert!(fake
        .read_value_for_test(Hive::CurrentUser, r"Control Panel\Test", "X")
        .is_none());
}

/// "No tweak may write to the registry, run a command, or touch a service
/// except through `Transaction`." Tweaks live in the same crate, so privacy does
/// not stop a determined author; this keeps the honest mistakes out.
#[test]
fn tweak_sources_do_not_bypass_the_transaction() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tweaks");
    let banned = [
        "winreg",
        "RegistryBackend",
        ".backend(",
        "registry::",
        "std::process",
        "std::fs",
        "Command::new",
        "write_value",
        "delete_key",
        "create_key",
    ];
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        // Doc comments may mention these; only code counts.
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for b in banned {
            assert!(!code.contains(b), "{} uses `{b}` outside Transaction", path.display());
        }
        checked += 1;
    }
    assert!(checked >= 3, "expected to scan the tweak files, saw {checked}");
}

#[test]
fn the_shipped_catalogue_is_consistent() {
    let cat = crate::tweaks::catalogue();
    let mut ids: Vec<&str> = cat.iter().map(|t| t.id()).collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n, "duplicate tweak ids");
    for t in &cat {
        assert_eq!(t.metadata().id, t.id(), "metadata id must match id()");
        assert!(!t.touches().is_empty(), "{} declares no registry targets", t.id());
        for target in t.touches() {
            assert_eq!(
                target.root.required_context(),
                t.execution_context(),
                "{}: targets a root outside its declared context",
                t.id()
            );
            assert!(!target.values.is_empty());
        }
    }
}

#[test]
fn the_ifeo_tweak_is_blocked_until_its_game_is_cleared_and_never_writes_realtime() {
    use crate::tweaks::ifeo_priority::IfeoPriority;
    let uncleared = IfeoPriority::fortnite();
    assert!(matches!(
        uncleared.evaluate_predicate(&crate::types::SystemEnv::default()),
        crate::types::PredicateOutcome::Block(r) if r.code == BlockedCode::AntiCheatEligibility
    ));

    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let mut engine = build_engine(
        &fake,
        dir.path(),
        vec![Box::new(IfeoPriority::fortnite().cleared_for_tests())],
        true,
        Tier::Ultimate,
    );
    engine.apply("priority.ifeo.fortnite").unwrap();
    let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\FortniteClient-Win64-Shipping.exe\PerfOptions";
    assert_eq!(
        fake.read_value_for_test(Hive::LocalMachine, key, "CpuPriorityClass")
            .and_then(|v| v.as_dword()),
        Some(3),
        "High, never Realtime (4)"
    );
    assert!(matches!(engine.list().unwrap()[0].state, TweakState::Applied));

    // Revert removes the value and the two keys we created; IFEO itself stays.
    engine.revert("priority.ifeo.fortnite").unwrap();
    assert!(fake.snapshot().is_empty());
    let left: Vec<String> = fake.key_paths().into_iter().map(|(_, p)| p).collect();
    assert!(
        !left
            .iter()
            .any(|p| p.ends_with("perfoptions") || p.ends_with("fortniteclient-win64-shipping.exe")),
        "{left:?}"
    );
}

#[test]
fn the_engine_catalogue_ships_blocked_and_gated_by_default() {
    // Production defaults until Phases 3 and 7 land: closed gate, Free license.
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let resolver = crate::context::ContextResolver::new(user(true), true, fake.clone());
    let journal = Journal::open(&TrustedDir::insecure_for_tests(dir.path())).unwrap();
    let mut engine = crate::engine::Engine::new(
        resolver,
        journal,
        crate::tweaks::catalogue(),
        Box::new(StubProbe::closed()),
        License::free(),
    );
    for id in ["input.mouseaccel", "priority.ifeo.fortnite"] {
        assert!(matches!(engine.apply(id), Err(EngineError::Blocked { .. })), "{id}");
    }
    assert!(fake.snapshot().is_empty());
}

/// Nothing the engine says about a change may promise a result. Only a stored
/// proof run can, and its wording comes from `proof::verdict`. This scans every
/// string literal in the tweak catalogue for the claim words in
/// `scripts/claim-words.json`, the same list the UI copy lint uses.
#[test]
fn catalogue_copy_makes_no_efficacy_claims() {
    let words = crate::copy_lint::claim_words();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tweaks");
    let mut scanned = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        for literal in crate::copy_lint::string_literals(&src) {
            if let Some(word) = crate::copy_lint::find_claim(&literal, &words) {
                panic!("{}: copy contains the claim word {word:?}: {literal}", path.display());
            }
        }
        scanned += 1;
    }
    assert!(scanned >= 3, "expected to scan the tweak files, saw {scanned}");
}

#[test]
fn proof_verdict_text_is_the_only_place_a_result_is_worded() {
    // The headline is built in exactly one function; nothing else in the
    // engine formats "Better"/"Worse" for a user.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, hits);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).unwrap();
                for (n, line) in src.lines().enumerate() {
                    let t = line.trim_start();
                    if !t.starts_with("//") && (line.contains("\"Better\"") || line.contains("\"Worse\"")) {
                        hits.push(format!("{}:{}", p.display(), n + 1));
                    }
                }
            }
        }
    }
    walk(&root, &mut hits);
    assert!(
        hits.iter().all(|h| h.contains("verdict.rs")),
        "verdict wording outside proof/verdict.rs: {hits:?}"
    );
}

#[test]
fn settings_persist_and_the_override_wins_over_the_detected_rig_class() {
    use crate::hardware::RigClass;
    use crate::settings::{Language, Settings};

    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let trusted = TrustedDir::insecure_for_tests(dir.path());
    let mut engine = build_engine(&fake, dir.path(), vec![], true, Tier::Free).with_settings_in(&trusted);
    assert_eq!(engine.settings(), Settings::default());
    assert_eq!(engine.effective_rig_class(), None, "nothing probed and no override");

    engine
        .set_settings(Settings {
            rig_class_override: Some(RigClass::High),
            language: Language::Technical,
        })
        .unwrap();
    assert_eq!(engine.effective_rig_class(), Some(RigClass::High));
    assert_eq!(
        engine.proof_context().0,
        Some(RigClass::High),
        "proof runs record the effective class"
    );
    assert_eq!(engine.audit().effective_rig_class, Some(RigClass::High));

    // A new engine on the same directory sees the same choices.
    let again = build_engine(&fake, dir.path(), vec![], true, Tier::Free).with_settings_in(&trusted);
    assert_eq!(again.settings().language, Language::Technical);
    assert_eq!(again.settings().rig_class_override, Some(RigClass::High));
}

#[test]
fn settings_never_touch_the_gate_the_licence_or_the_environment() {
    use crate::settings::Settings;
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let mut engine = build_engine(&fake, dir.path(), vec![], false, Tier::Free);
    let before = engine.audit().env.restore_gate_open;
    engine.set_settings(Settings::default()).unwrap();
    let after = engine.audit();
    assert_eq!(before, after.env.restore_gate_open);
    assert!(!after.env.restore_gate_open);
}

#[test]
fn a_failed_save_leaves_the_settings_unchanged() {
    use crate::hardware::RigClass;
    use crate::settings::Settings;
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let trusted = TrustedDir::insecure_for_tests(dir.path());
    let mut engine = build_engine(&fake, dir.path(), vec![], true, Tier::Free).with_settings_in(&trusted);
    // Make the target a directory so the rename over it fails.
    std::fs::create_dir(dir.path().join("settings.json")).unwrap();
    let r = engine.set_settings(Settings {
        rig_class_override: Some(RigClass::Low),
        ..Settings::default()
    });
    assert!(r.is_err());
    assert_eq!(engine.settings(), Settings::default(), "memory must match disk");
}

/// Reverting a per-user change while a different account is the interactive user
/// must not write the old values into that other account's hive.
#[test]
fn a_per_user_change_is_not_reverted_into_another_accounts_hive() {
    use crate::context::{ContextResolver, UserContext, UserResolution};

    let mk = || {
        let mut t = TestTweak::new("u", r"Control Panel\Test", &[("X", 1)]);
        t.context = ExecutionContext::User;
        Box::new(t) as Box<dyn Tweak>
    };
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let engine_for = |sid: &str| {
        let user = UserContext {
            sid: sid.into(),
            resolution: UserResolution::InteractiveShell,
            is_self: false,
        };
        let resolver = ContextResolver::new(user, true, fake.clone());
        let journal = Journal::open(&TrustedDir::insecure_for_tests(dir.path())).unwrap();
        crate::engine::Engine::new(
            resolver,
            journal,
            vec![mk()],
            Box::new(StubProbe::open_for_dev()),
            License::dev(Tier::Ultimate),
        )
    };

    let mut alice = engine_for("S-1-5-21-1-1-1-1001");
    alice.apply("u").unwrap();

    let mut bob = engine_for("S-1-5-21-1-1-1-1002");
    let err = bob.revert("u").unwrap_err();
    assert!(
        matches!(&err, EngineError::ContextViolation { detail, .. } if detail.contains("1001") && detail.contains("1002")),
        "{err:?}"
    );
    assert!(fake
        .read_value_for_test(Hive::Users, r"S-1-5-21-1-1-1-1001\Control Panel\Test", "X")
        .is_some());
    assert!(
        fake.read_value_for_test(Hive::Users, r"S-1-5-21-1-1-1-1002\Control Panel\Test", "X")
            .is_none(),
        "the other account's hive was not touched"
    );

    // The account that made the change can still undo it.
    let mut alice_again = engine_for("S-1-5-21-1-1-1-1001");
    alice_again.revert("u").unwrap();
    assert!(fake.snapshot().is_empty());
}

/// After a panic the engine re-reads the journal from disk, so a revert works
/// from what is really recorded even if the in-memory index was left half-updated.
#[test]
fn recovery_after_a_panic_rebuilds_from_disk_and_revert_still_works() {
    let mut h = Harness::new(vec![Box::new(TestTweak::new("a", "SOFTWARE\\T", &[("X", 1)]))]);
    h.engine.apply("a").unwrap();
    assert!(!h.engine.journal_view().records.is_empty());

    // Damage the in-memory copy the way an interrupted append might.
    h.engine.forget_journal_for_test();
    assert!(h.engine.journal_view().records.is_empty());

    h.engine.recover_after_panic().unwrap();
    assert!(!h.engine.journal_view().records.is_empty());
    h.engine.revert("a").unwrap();
    assert!(h.fake.snapshot().is_empty());
}

// ---------------------------------------------------------------------------
// Mouse acceleration state (agent brief, lower-priority item)
// ---------------------------------------------------------------------------

mod mouse_state {
    use std::sync::Arc;

    use crate::context::ContextResolver;
    use crate::registry::fake::FakeRegistry;
    use crate::registry::Hive;
    use crate::testutil::user;
    use crate::tweaks::mouse_accel::MouseAcceleration;
    use crate::types::{RawValue, Tweak, TweakState};

    const KEY: &str = r"Control Panel\Mouse";
    const OFF: [(&str, &str); 3] = [("MouseSpeed", "0"), ("MouseThreshold1", "0"), ("MouseThreshold2", "0")];

    fn state(values: &[(&str, &str)], ours: bool) -> TweakState {
        let fake = Arc::new(FakeRegistry::new());
        for (name, v) in values {
            fake.set_external(Hive::CurrentUser, KEY, name, RawValue::sz(v));
        }
        MouseAcceleration
            .read_state(&ContextResolver::new(user(true), true, fake), ours)
            .unwrap()
    }

    #[test]
    fn off_because_of_us_is_applied() {
        assert_eq!(state(&OFF, true), TweakState::Applied);
    }

    #[test]
    fn off_but_not_by_us_is_foreign() {
        assert_eq!(state(&OFF, false), TweakState::Foreign);
    }

    #[test]
    fn missing_values_are_windows_defaults_which_mean_acceleration_on() {
        assert_eq!(state(&[], false), TweakState::Default);
        assert_eq!(
            state(&[("MouseSpeed", "0")], false),
            TweakState::Default,
            "one of three is not off"
        );
        assert_eq!(
            state(
                &[("MouseSpeed", "1"), ("MouseThreshold1", "6"), ("MouseThreshold2", "10")],
                true
            ),
            TweakState::Default
        );
    }
}
