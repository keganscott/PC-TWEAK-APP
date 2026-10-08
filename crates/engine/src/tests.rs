//! Engine-level scenarios against the in-memory registry: journal semantics,
//! revert correctness, rollback, allowlist, gates.

use std::sync::Arc;

use crate::env::{License, StubProbe};
use crate::error::EngineError;
use crate::journal::{ActionDone, CommitAction, Journal, JournalAction, JournalEntry, OneTimeAction, Record};
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
    // Not all three set, but the journal holds the two writes: undoable.
    assert_eq!(views[0].state, TweakState::Drifted);
    h.engine.revert("t").unwrap();
    assert_eq!(dw(&h, "A"), None);
    assert_eq!(dw(&h, "B"), Some(99));
    assert_eq!(dw(&h, "C"), None);
}

#[test]
fn a_crash_mid_apply_reaches_the_offline_undo_set_at_the_next_start() {
    let tweak = TestTweak::new("t", KEY, &[("A", 1), ("B", 2)]);
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1), ("B", 2)])));
    {
        let (resolver, journal, _) = h.engine.parts_for_test();
        let mut tx = Transaction::begin(&tweak, resolver, journal, JournalAction::Apply).unwrap();
        tx.set_dword(RegRoot::LocalMachine, KEY, "A", 1).unwrap();
        drop(tx); // crash: no commit, so no offline file was written
    }
    let offline = h.dir.path().join(crate::offline::DIR);
    let reg_files = || -> Vec<String> {
        std::fs::read_dir(&offline)
            .map(|d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .filter(|n| n.ends_with(".reg"))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(reg_files().is_empty());

    h.restart(one(TestTweak::new("t", KEY, &[("A", 1), ("B", 2)])));
    h.engine.refresh_offline_undo();
    assert_eq!(reg_files(), ["001_t.reg"]);
    assert_eq!(h.engine.journal_view().offline_error, None);
}

#[test]
fn a_failed_offline_refresh_at_start_is_shown_and_cleared_by_the_next_change() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    // A file where the folder should be: the refresh cannot create it.
    std::fs::write(h.dir.path().join(crate::offline::DIR), b"in the way").unwrap();
    h.engine.refresh_offline_undo();
    let err = h.engine.journal_view().offline_error.expect("the failure is kept");
    assert!(err.contains("offline"), "{err}");

    std::fs::remove_file(h.dir.path().join(crate::offline::DIR)).unwrap();
    h.engine.apply("t").unwrap();
    assert_eq!(h.engine.journal_view().offline_error, None);
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
fn a_one_time_action_is_kept_in_the_history_and_undo_all_leaves_it_there() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.engine.apply("t").unwrap();
    let drive = ActionDone::OptimizeDrive {
        drive: "C:".into(),
        seconds: 41,
    };
    h.engine
        .record_action(OneTimeAction::OptimizeDrive, Ok(drive.clone()))
        .unwrap();
    h.engine
        .record_action(OneTimeAction::PurgeStandby, Err("no privilege".into()))
        .unwrap();

    let view = h.engine.journal_view();
    let actions: Vec<_> = view
        .records
        .iter()
        .filter_map(|r| match r {
            Record::Action(a) => Some(a),
            _ => None,
        })
        .collect();
    assert_eq!(actions.len(), 2);
    assert_eq!(
        (actions[0].done.as_ref(), actions[0].error.as_deref()),
        (Some(&drive), None)
    );
    assert_eq!(
        (actions[1].done.as_ref(), actions[1].error.as_deref()),
        (None, Some("no privilege"))
    );
    // Nothing to undo: only the change is listed.
    let applied: Vec<_> = view.applied.iter().map(|c| c.tweak_id.as_str()).collect();
    assert_eq!(applied, ["t"]);

    assert!(h.engine.revert_all().iter().all(|r| r.ok));
    let view = h.engine.journal_view();
    assert!(view.applied.is_empty());
    assert_eq!(
        view.records.iter().filter(|r| matches!(r, Record::Action(_))).count(),
        2
    );
    assert_eq!(dw(&h, "A"), None);
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

/// A per-PC key (an adapter's `{guid}`) is declared with one `*` segment: any
/// one key there may be written and undone, created keys included.
#[test]
fn a_wildcard_target_allows_one_per_pc_key_and_undo_removes_what_it_created() {
    let concrete = r"SOFTWARE\PeakTest\Interfaces\{1234-ABCD}";
    let mut t = TestTweak::new("t", concrete, &[("TcpNoDelay", 1)]);
    t.allow_key = r"SOFTWARE\PeakTest\Interfaces\*".into();
    let mut h = Harness::new(one(t));
    h.engine.apply("t").unwrap();
    assert_eq!(hklm_dword(&h.fake, concrete, "TcpNoDelay"), Some(1));
    h.engine.revert("t").unwrap();
    assert_eq!(hklm_dword(&h.fake, concrete, "TcpNoDelay"), None);
    assert!(h.fake.key_paths().is_empty(), "{:?}", h.fake.key_paths());
}

/// The `*` stands for exactly one key: one level deeper is refused.
#[test]
fn a_wildcard_target_cannot_reach_a_key_two_levels_down() {
    let deeper = r"SOFTWARE\PeakTest\Interfaces\{1234-ABCD}\Sub";
    let mut t = TestTweak::new("t", deeper, &[("TcpNoDelay", 1)]);
    t.allow_key = r"SOFTWARE\PeakTest\Interfaces\*".into();
    let mut h = Harness::new(one(t));
    let before = h.fake.snapshot();
    let err = h.engine.apply("t").unwrap_err();
    assert!(matches!(err, EngineError::ContextViolation { .. }), "{err:?}");
    assert_eq!(h.fake.snapshot(), before);
}

/// A per-PC key that is gone at Undo (the adapter or device was removed) is
/// skipped, not created again, and the history says so.
#[test]
fn undo_skips_a_wildcard_key_that_no_longer_exists_and_notes_it() {
    let concrete = r"SOFTWARE\PeakTest\Interfaces\{1234-ABCD}";
    let mut t = TestTweak::new("t", concrete, &[("TcpNoDelay", 1)]);
    t.allow_key = r"SOFTWARE\PeakTest\Interfaces\*".into();
    let mut h = Harness::new(one(t));
    h.fake.set_external(Hive::LocalMachine, concrete, "Other", dword(7));
    h.engine.apply("t").unwrap();
    h.fake.remove_key_external(Hive::LocalMachine, concrete);

    h.engine.revert("t").unwrap();
    assert!(
        !h.fake.key_exists_for_test(Hive::LocalMachine, concrete),
        "not recreated"
    );
    assert!(h.engine.applied_tweak_ids().is_empty());
    assert!(h
        .engine
        .journal_view()
        .records
        .iter()
        .any(|r| matches!(r, Record::Note(n) if n.text.contains("no longer exists"))));
}

/// `HKEY_CLASSES_ROOT\*` is a real key (every file type), so under that root
/// a `*` segment is only ever that key, never a wildcard.
#[test]
fn a_star_under_classes_root_is_the_literal_key() {
    struct Hkcr(&'static str);
    impl Tweak for Hkcr {
        fn id(&self) -> &str {
            "hkcr"
        }
        fn metadata(&self) -> crate::types::TweakMetadata {
            TestTweak::new("hkcr", KEY, &[]).metadata()
        }
        fn execution_context(&self) -> ExecutionContext {
            ExecutionContext::Service
        }
        fn touches(&self) -> Vec<crate::types::RegTarget> {
            vec![crate::types::RegTarget::new(RegRoot::ClassesRoot, r"*\shell", &["V"])]
        }
        fn read_state(&self, _: &crate::context::ContextResolver, _: bool) -> crate::error::Result<TweakState> {
            Ok(TweakState::Default)
        }
        fn apply(&self, tx: &mut Transaction) -> crate::error::Result<()> {
            tx.set_dword(RegRoot::ClassesRoot, &format!(r"{}\shell", self.0), "V", 1)
        }
    }
    let mut ok = Harness::new(vec![Box::new(Hkcr("*"))]);
    ok.engine.apply("hkcr").unwrap();
    let mut refused = Harness::new(vec![Box::new(Hkcr("txtfile"))]);
    assert!(matches!(
        refused.engine.apply("hkcr"),
        Err(EngineError::ContextViolation { .. })
    ));
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

/// A normal build never says it is a tester build.
#[test]
fn a_normal_build_is_not_labelled_a_tester_build() {
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let engine = build_engine(&fake, dir.path(), vec![], true, Tier::Free);
    assert!(!engine.context_info().tester_build);
    assert!(!License::free().is_tester());
}

/// The tester build (Cargo feature `tester`, docs/TEST-ON-YOUR-PC.md) unlocks
/// every plan so the paid changes can be tried on a real PC before licensing
/// exists. Everything else holds: no restore point, no change.
#[cfg(feature = "tester")]
#[test]
fn a_tester_build_unlocks_every_plan_but_keeps_the_restore_gate() {
    let tester_engine = |fake: &Arc<FakeRegistry>, dir: &std::path::Path, gate_open: bool| {
        let mut t = TestTweak::new("t", KEY, &[("A", 1)]);
        t.tier = Tier::Ultimate;
        let resolver = crate::context::ContextResolver::new(user(true), true, fake.clone());
        let journal = Journal::open(&TrustedDir::insecure_for_tests(dir)).unwrap();
        let probe = if gate_open {
            StubProbe::open_for_dev()
        } else {
            StubProbe::closed()
        };
        crate::engine::Engine::new(resolver, journal, one(t), Box::new(probe), License::tester())
    };

    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let mut closed = tester_engine(&fake, dir.path(), false);
    assert!(closed.context_info().tester_build);
    assert!(
        closed.list().unwrap()[0].blocked.is_none(),
        "no plan lock in a tester build"
    );
    let err = closed.apply("t").unwrap_err();
    assert!(
        matches!(&err, EngineError::Blocked { reason } if reason.code == BlockedCode::NoRestorePoint),
        "{err:?}"
    );
    assert!(fake.snapshot().is_empty(), "nothing written without a restore point");

    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let mut open = tester_engine(&fake, dir.path(), true);
    open.apply("t").unwrap();
    assert_eq!(hklm_dword(&fake, KEY, "A"), Some(1));
    open.revert("t").unwrap();
    assert!(fake.snapshot().is_empty());
}

/// B3 (docs/AUDIT-2026-10-04.md): the list says a change needs a higher plan
/// before anyone clicks Apply, and still reads its real state.
#[test]
fn the_list_shows_the_plan_a_change_needs_before_apply_is_tried() {
    let mut t = TestTweak::new("t", KEY, &[("A", 1)]);
    t.tier = Tier::Pro;
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let engine = build_engine(&fake, dir.path(), one(t), true, Tier::Free);
    let view = &engine.list().unwrap()[0];
    let reason = view.blocked.as_ref().expect("blocked on the free plan");
    assert_eq!(reason.code, BlockedCode::TierRequired);
    assert_eq!(reason.trigger.as_deref(), Some("pro"));
    assert_eq!(view.state, TweakState::Default, "the state is still read, not hidden");

    // The same change on a plan that includes it is not blocked.
    let mut t = TestTweak::new("t", KEY, &[("A", 1)]);
    t.tier = Tier::Pro;
    let engine = build_engine(&fake, dir.path(), one(t), true, Tier::Pro);
    assert!(engine.list().unwrap()[0].blocked.is_none());
}

/// B2 (docs/AUDIT-2026-10-04.md): a change undone or altered outside
/// PeakTweaks while its apply is still outstanding in the journal keeps its
/// Undo, and says so, instead of reading as "not applied".
#[test]
fn a_change_altered_outside_peaktweaks_reads_as_drifted_and_keeps_its_undo() {
    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(5));
    h.engine.apply("t").unwrap();
    assert_eq!(h.engine.list().unwrap()[0].state, TweakState::Applied);

    // Someone sets it back to what it was before (or to anything else).
    h.fake.set_external(Hive::LocalMachine, KEY, "A", dword(7));
    assert_eq!(h.engine.list().unwrap()[0].state, TweakState::Drifted);

    // Undo still restores the value from before the apply.
    h.engine.revert("t").unwrap();
    assert_eq!(dw(&h, "A"), Some(5));
    assert_eq!(h.engine.list().unwrap()[0].state, TweakState::Default);
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
        ".system(",
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
        assert!(
            !t.touches().is_empty() || !t.system_targets().is_empty(),
            "{} declares no targets",
            t.id()
        );
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
    if crate::games::anti_cheat_block("fortnite").is_some() {
        assert!(matches!(
            uncleared.evaluate_predicate(&crate::types::SystemEnv::default()),
            crate::types::PredicateOutcome::Block(r) if r.code == BlockedCode::AntiCheatEligibility
        ));
    }

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
fn windows_folders_come_from_windows_not_the_environment() {
    // Whoever starts PeakTweaks sets its environment, so a planted SystemRoot
    // or windir must never pick which powershell.exe or System32 tool runs
    // elevated (NOTES N72). Paths come from sysdirs (GetSystemDirectoryW).
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut hits = Vec::new();
    fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "target") {
                    walk(&p, hits);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).unwrap();
                for (n, line) in src.lines().enumerate() {
                    let l = line.to_ascii_lowercase();
                    if l.contains("var") && (l.contains("(\"systemroot\")") || l.contains("(\"windir\")")) {
                        hits.push(format!("{}:{}", p.display(), n + 1));
                    }
                }
            }
        }
    }
    walk(&crates, &mut hits);
    assert!(hits.is_empty(), "Windows folder read from the environment: {hits:?}");
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
            ..Settings::default()
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

/// NOTES.md N6: which slot of `SPI_SETMOUSE`'s array is which. Read-only:
/// Windows loads the session's values from `HKCU\Control Panel\Mouse` at
/// sign-in, so `SPI_GETMOUSE` lined up against the three named registry
/// values shows the order whenever the three differ. Prints NOT VERIFIED when
/// they do not, or when the session no longer matches the registry.
#[cfg(windows)]
#[test]
fn spi_getmouse_order_matches_the_named_registry_values() {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETMOUSE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    use winreg::enums::HKEY_CURRENT_USER;

    let key = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Control Panel\Mouse")
        .expect("HKCU\\Control Panel\\Mouse exists on every Windows");
    let read = |name: &str| -> i32 {
        let s: String = key.get_value(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        s.trim().parse().unwrap_or_else(|e| panic!("{name}={s:?}: {e}"))
    };
    let named = [read("MouseThreshold1"), read("MouseThreshold2"), read("MouseSpeed")];

    let mut live = [0i32; 3];
    unsafe {
        SystemParametersInfoW(
            SPI_GETMOUSE,
            0,
            Some(live.as_mut_ptr() as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .expect("SPI_GETMOUSE");
    }
    let distinct = named[0] != named[1] && named[1] != named[2] && named[0] != named[2];
    let mut sorted_live = live;
    let mut sorted_named = named;
    sorted_live.sort();
    sorted_named.sort();
    if !distinct || sorted_live != sorted_named {
        println!(
            "SPI_GETMOUSE order NOT VERIFIED: registry [MouseThreshold1, MouseThreshold2, MouseSpeed] = {named:?}, \
             SPI_GETMOUSE = {live:?} (values not distinct, or the session differs from the registry)"
        );
        return;
    }
    assert_eq!(
        live, named,
        "SPI_GETMOUSE is not [MouseThreshold1, MouseThreshold2, MouseSpeed]: push_live's order is wrong"
    );
    println!(
        "SPI_GETMOUSE = {live:?} = registry [MouseThreshold1, MouseThreshold2, MouseSpeed]: \
         the array is [threshold1, threshold2, acceleration] (VERIFIED on this Windows)"
    );
}

/// B4 (docs/AUDIT-2026-10-04.md): the audit the UI asks for after every change
/// re-reads what a change can affect (security and restore state) but reuses
/// the slow hardware probes. Only the user's explicit rescan forgets them.
#[test]
fn the_audit_after_a_change_reuses_hardware_and_only_an_explicit_rescan_forgets_it() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Default)]
    struct Counts {
        probes: AtomicUsize,
        invalidate: AtomicUsize,
        invalidate_all: AtomicUsize,
    }
    struct Counting(Arc<Counts>);
    impl crate::env::EnvProbe for Counting {
        fn probe(&self, elevated: bool) -> crate::types::SystemEnv {
            self.0.probes.fetch_add(1, Ordering::SeqCst);
            crate::types::SystemEnv {
                elevated,
                ..Default::default()
            }
        }
        fn restore_gate_open(&self) -> bool {
            true
        }
        fn invalidate(&self) {
            self.0.invalidate.fetch_add(1, Ordering::SeqCst);
        }
        fn invalidate_all(&self) {
            self.0.invalidate_all.fetch_add(1, Ordering::SeqCst);
        }
    }

    let counts = Arc::new(Counts::default());
    let fake = Arc::new(crate::registry::fake::FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let mut engine = crate::engine::Engine::new(
        crate::context::ContextResolver::new(crate::testutil::user(true), true, fake),
        crate::journal::Journal::open(&crate::secure_dir::TrustedDir::insecure_for_tests(dir.path())).unwrap(),
        vec![],
        Box::new(Counting(counts.clone())),
        crate::env::License::dev(crate::types::Tier::Ultimate),
    );

    engine.audit();
    engine.audit();
    assert_eq!(
        counts.invalidate_all.load(Ordering::SeqCst),
        0,
        "audit keeps cached hardware"
    );
    assert!(
        counts.invalidate.load(Ordering::SeqCst) >= 2,
        "audit re-reads security and restore state"
    );

    engine.rescan_fresh();
    assert_eq!(
        counts.invalidate_all.load(Ordering::SeqCst),
        1,
        "the user's rescan forgets everything"
    );
}

/// B1 (docs/AUDIT-2026-10-04.md): every change with an outstanding apply is
/// listed for the Backups tab, including the engine's own restore-frequency
/// change, newest first, so the user can see and undo it.
#[test]
fn the_journal_view_lists_every_outstanding_change_including_the_engines_own() {
    use crate::engine::ChangeKind;

    let mut h = Harness::new(one(TestTweak::new("t", KEY, &[("A", 1)])));
    assert!(h.engine.journal_view().applied.is_empty());

    h.engine.ensure_restore_frequency().unwrap();
    h.engine.apply("t").unwrap();
    let applied = h.engine.journal_view().applied;
    let ids: Vec<&str> = applied.iter().map(|c| c.tweak_id.as_str()).collect();
    assert_eq!(ids, ["t", crate::tweaks::system_restore::ID], "newest first");
    assert_eq!(applied[0].kind, ChangeKind::Catalogue);
    assert_eq!(applied[1].kind, ChangeKind::Internal);
    assert_eq!(applied[1].name, "Allow a restore point on demand");

    // Undoing the engine's own change works from the same list.
    h.engine.revert(crate::tweaks::system_restore::ID).unwrap();
    let ids: Vec<String> = h
        .engine
        .journal_view()
        .applied
        .into_iter()
        .map(|c| c.tweak_id)
        .collect();
    assert_eq!(ids, ["t"]);

    // A change this version no longer ships is still listed, by its id.
    h.restart(vec![]);
    let applied = h.engine.journal_view().applied;
    assert_eq!(
        (applied[0].tweak_id.as_str(), applied[0].kind),
        ("t", ChangeKind::Retired)
    );
    assert_eq!(applied[0].name, "t");
}

#[test]
fn a_value_tweak_already_set_on_the_pc_is_listed_as_done_and_round_trips() {
    use crate::registry::RegistryBackend;
    use crate::tweaks::registry_values::{BACKGROUND_CAPTURE, GAME_MODE, NETWORK_THROTTLING};
    let fake = Arc::new(FakeRegistry::new());
    // Windows' stock values for the network limit; nothing written for the others.
    let mmcss = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile";
    fake.create_key(Hive::LocalMachine, mmcss).unwrap();
    fake.write_value(
        Hive::LocalMachine,
        mmcss,
        "NetworkThrottlingIndex",
        &RawValue::dword(10),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut engine = build_engine(
        &fake,
        dir.path(),
        vec![
            Box::new(GAME_MODE),
            Box::new(BACKGROUND_CAPTURE),
            Box::new(NETWORK_THROTTLING),
        ],
        true,
        Tier::Ultimate,
    );
    let state = |e: &mut crate::engine::Engine, id: &str| {
        e.list()
            .unwrap()
            .into_iter()
            .find(|t| t.metadata.id == id)
            .unwrap()
            .state
    };

    // Game Mode is on by default: shown as already done, not hidden.
    assert!(matches!(state(&mut engine, "gaming.gamemode"), TweakState::Foreign));
    assert!(matches!(state(&mut engine, "gaming.capture"), TweakState::Default));

    engine.apply("gaming.capture").unwrap();
    assert!(matches!(state(&mut engine, "gaming.capture"), TweakState::Applied));
    assert_eq!(
        fake.read_value_for_test(Hive::CurrentUser, r"System\GameConfigStore", "GameDVR_Enabled")
            .and_then(|v| v.as_dword()),
        Some(0)
    );
    engine.revert("gaming.capture").unwrap();
    assert!(matches!(state(&mut engine, "gaming.capture"), TweakState::Default));

    engine.apply("network.throttling").unwrap();
    assert_eq!(
        fake.read_value_for_test(Hive::LocalMachine, mmcss, "NetworkThrottlingIndex")
            .and_then(|v| v.as_dword()),
        Some(0xFFFF_FFFF)
    );
    engine.revert("network.throttling").unwrap();
    assert_eq!(
        fake.read_value_for_test(Hive::LocalMachine, mmcss, "NetworkThrottlingIndex")
            .and_then(|v| v.as_dword()),
        Some(10),
        "revert puts Windows' value back"
    );
}

#[test]
fn the_sticky_keys_tool_clears_only_the_shortcut_and_keeps_the_users_own_settings() {
    use crate::registry::RegistryBackend;
    use crate::tweaks::registry_values::ACCESSIBILITY_SHORTCUTS;
    let fake = Arc::new(FakeRegistry::new());
    let sticky = r"Control Panel\Accessibility\StickyKeys";
    // Sticky Keys itself switched on (bit 0x1) on top of Windows' defaults.
    fake.create_key(Hive::CurrentUser, sticky).unwrap();
    fake.write_value(Hive::CurrentUser, sticky, "Flags", &RawValue::sz("511"))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut engine = build_engine(
        &fake,
        dir.path(),
        vec![Box::new(ACCESSIBILITY_SHORTCUTS)],
        true,
        Tier::Ultimate,
    );
    let flags = |key: &str| {
        fake.read_value_for_test(Hive::CurrentUser, key, "Flags")
            .and_then(|v| v.as_sz())
    };
    let state = |e: &mut crate::engine::Engine| e.list().unwrap().remove(0).state;

    assert!(matches!(state(&mut engine), TweakState::Default));
    engine.apply("input.accessibilitykeys").unwrap();
    assert_eq!(
        flags(sticky).as_deref(),
        Some("507"),
        "Sticky Keys stays on, only the shortcut goes"
    );
    // Absent values start from Windows' defaults.
    assert_eq!(
        flags(r"Control Panel\Accessibility\Keyboard Response").as_deref(),
        Some("122")
    );
    assert_eq!(flags(r"Control Panel\Accessibility\ToggleKeys").as_deref(), Some("58"));
    assert!(matches!(state(&mut engine), TweakState::Applied));

    engine.revert("input.accessibilitykeys").unwrap();
    assert_eq!(flags(sticky).as_deref(), Some("511"));
    assert_eq!(flags(r"Control Panel\Accessibility\ToggleKeys"), None);
    assert!(matches!(state(&mut engine), TweakState::Default));
}

// ---------------------------------------------------------------------------
// Changes that are not registry values (system.rs)
// ---------------------------------------------------------------------------

mod system_changes {
    use super::*;
    use crate::system::{ServiceStart, SideEffect, SysItem, SysState};

    /// Sets non-registry items, writes files and queues side effects, all
    /// declared unless `undeclared` names one it must not be allowed.
    struct SysTweak {
        sets: Vec<(SysItem, SysState)>,
        files: Vec<(String, Vec<u8>)>,
        effects: Vec<SideEffect>,
        declared: Vec<SysItem>,
        declared_effects: Vec<SideEffect>,
        fail_after: bool,
    }

    impl SysTweak {
        fn new(sets: Vec<(SysItem, SysState)>) -> Self {
            let declared = sets.iter().map(|(i, _)| i.clone()).collect();
            Self {
                sets,
                files: vec![],
                effects: vec![],
                declared,
                declared_effects: vec![],
                fail_after: false,
            }
        }
    }

    impl Tweak for SysTweak {
        fn id(&self) -> &str {
            "sys"
        }
        fn metadata(&self) -> crate::types::TweakMetadata {
            TestTweak::new("sys", KEY, &[]).metadata()
        }
        fn execution_context(&self) -> ExecutionContext {
            ExecutionContext::Service
        }
        fn touches(&self) -> Vec<crate::types::RegTarget> {
            vec![]
        }
        fn system_targets(&self) -> Vec<SysItem> {
            self.declared.clone()
        }
        fn effect_targets(&self) -> Vec<SideEffect> {
            if self.declared_effects.is_empty() {
                self.effects.clone()
            } else {
                self.declared_effects.clone()
            }
        }
        fn read_state(&self, _: &crate::context::ContextResolver, j: bool) -> crate::error::Result<TweakState> {
            Ok(if j { TweakState::Applied } else { TweakState::Default })
        }
        fn apply(&self, tx: &mut Transaction) -> crate::error::Result<()> {
            for (i, s) in &self.sets {
                tx.set_system(i.clone(), s.clone())?;
            }
            for (p, b) in &self.files {
                tx.write_file(p, b)?;
            }
            for e in &self.effects {
                tx.after_commit(e.clone())?;
            }
            if self.fail_after {
                return Err(EngineError::Internal {
                    detail: "test failure".into(),
                });
            }
            Ok(())
        }
        fn revert(&self, tx: &mut Transaction) -> crate::error::Result<()> {
            tx.restore_journalled()?;
            for e in &self.effects {
                tx.after_commit(e.clone())?;
            }
            Ok(())
        }
    }

    fn svc() -> SysItem {
        SysItem::Service { name: "WSearch".into() }
    }

    fn running(start: ServiceStart, running: bool) -> SysState {
        SysState::Service { start, running }
    }

    #[test]
    fn a_service_change_is_journalled_before_it_is_made_and_undo_puts_it_back() {
        let t = SysTweak::new(vec![(svc(), running(ServiceStart::Disabled, false))]);
        let mut h = Harness::new(vec![Box::new(t)]);
        h.sys.set(&svc(), running(ServiceStart::DelayedAutomatic, true));

        h.engine.apply("sys").unwrap();
        assert_eq!(h.sys.get(&svc()), running(ServiceStart::Disabled, false));
        let change = h
            .engine
            .journal_view()
            .records
            .into_iter()
            .find_map(|r| match r {
                Record::Change(c) => Some(c),
                _ => None,
            })
            .expect("a change record");
        assert_eq!(change.previous, running(ServiceStart::DelayedAutomatic, true));
        assert_eq!(h.engine.applied_tweak_ids(), vec!["sys"]);

        h.engine.revert("sys").unwrap();
        assert_eq!(h.sys.get(&svc()), running(ServiceStart::DelayedAutomatic, true));
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn a_change_outside_the_declared_items_is_refused_and_nothing_changes() {
        let mut t = SysTweak::new(vec![(svc(), running(ServiceStart::Disabled, false))]);
        t.declared = vec![SysItem::Service { name: "SysMain".into() }];
        let mut h = Harness::new(vec![Box::new(t)]);
        h.sys.set(&svc(), running(ServiceStart::Automatic, true));
        let err = h.engine.apply("sys").unwrap_err();
        assert!(matches!(err, EngineError::ContextViolation { .. }), "{err:?}");
        assert_eq!(h.sys.get(&svc()), running(ServiceStart::Automatic, true));
    }

    #[test]
    fn a_per_pc_item_can_be_declared_with_a_star() {
        let dns = SysItem::DnsServers {
            interface: "{9F2-AA}".into(),
        };
        let mut t = SysTweak::new(vec![(
            dns.clone(),
            SysState::List {
                items: vec!["1.1.1.1".into()],
            },
        )]);
        t.declared = vec![SysItem::DnsServers { interface: "*".into() }];
        let mut h = Harness::new(vec![Box::new(t)]);
        h.engine.apply("sys").unwrap();
        h.engine.revert("sys").unwrap();
        assert_eq!(h.sys.get(&dns), SysState::Absent);
    }

    #[test]
    fn a_failed_apply_puts_back_both_registry_and_other_changes() {
        let plan = SysItem::ActivePowerScheme;
        let mut t = SysTweak::new(vec![
            (plan.clone(), SysState::Text { text: "ours".into() }),
            (svc(), running(ServiceStart::Disabled, false)),
        ]);
        t.fail_after = true;
        let mut h = Harness::new(vec![Box::new(t)]);
        h.sys.set(
            &plan,
            SysState::Text {
                text: "balanced".into(),
            },
        );
        h.sys.set(&svc(), running(ServiceStart::Manual, false));
        assert!(h.engine.apply("sys").is_err());
        assert_eq!(
            h.sys.get(&plan),
            SysState::Text {
                text: "balanced".into()
            }
        );
        assert_eq!(h.sys.get(&svc()), running(ServiceStart::Manual, false));
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn a_file_is_kept_whole_and_undo_restores_it_byte_for_byte() {
        let path = r"C:\Users\x\AppData\Local\Game\Saved\GameUserSettings.ini";
        let mut t = SysTweak::new(vec![]);
        t.files = vec![(path.into(), b"[S]\nFrameRateLimit=0\n".to_vec())];
        t.declared = vec![SysItem::File { path: path.into() }];
        let mut h = Harness::new(vec![Box::new(t)]);
        let original = b"[S]\r\nFrameRateLimit=60\r\n\xEF\xBB\xBF".to_vec();
        h.sys.set_file(path, &original);

        h.engine.apply("sys").unwrap();
        assert_eq!(h.sys.file(path).unwrap(), b"[S]\nFrameRateLimit=0\n");
        h.engine.revert("sys").unwrap();
        assert_eq!(h.sys.file(path).unwrap(), original);
    }

    #[test]
    fn a_file_that_did_not_exist_is_removed_again_on_undo() {
        let path = r"C:\Games\new.cfg";
        let mut t = SysTweak::new(vec![]);
        t.files = vec![(path.into(), b"x=1".to_vec())];
        t.declared = vec![SysItem::File { path: path.into() }];
        let mut h = Harness::new(vec![Box::new(t)]);
        h.engine.apply("sys").unwrap();
        assert!(h.sys.file(path).is_some());
        h.engine.revert("sys").unwrap();
        assert!(h.sys.file(path).is_none());
    }

    #[test]
    fn a_tampered_file_copy_is_refused_rather_than_restored() {
        let path = r"C:\Games\x.ini";
        let mut t = SysTweak::new(vec![]);
        t.files = vec![(path.into(), b"new".to_vec())];
        t.declared = vec![SysItem::File { path: path.into() }];
        let mut h = Harness::new(vec![Box::new(t)]);
        h.sys.set_file(path, b"old");
        h.engine.apply("sys").unwrap();
        let backup = h
            .engine
            .journal_view()
            .records
            .into_iter()
            .find_map(|r| match r {
                Record::Change(crate::journal::ChangeEntry {
                    previous: SysState::File { backup, .. },
                    ..
                }) => Some(backup),
                _ => None,
            })
            .unwrap();
        std::fs::write(h.dir.path().join(backup), b"evil").unwrap();
        assert!(h.engine.revert("sys").is_err());
        assert_eq!(h.sys.file(path).unwrap(), b"new", "nothing written from a bad copy");
    }

    #[test]
    fn side_effects_run_after_apply_and_after_revert_and_are_journalled() {
        let mut t = SysTweak::new(vec![(svc(), running(ServiceStart::Manual, false))]);
        t.effects = vec![SideEffect::RestartAdapter {
            interface: "{AA}".into(),
        }];
        let mut h = Harness::new(vec![Box::new(t)]);
        h.engine.apply("sys").unwrap();
        assert_eq!(h.sys.effects().len(), 1);
        h.engine.revert("sys").unwrap();
        assert_eq!(h.sys.effects().len(), 2);
        let effects = h
            .engine
            .journal_view()
            .records
            .into_iter()
            .filter(|r| matches!(r, Record::Effect(e) if e.error.is_none()))
            .count();
        assert_eq!(effects, 2);
    }

    #[test]
    fn a_failed_side_effect_is_recorded_but_does_not_undo_the_committed_change() {
        let mut t = SysTweak::new(vec![(svc(), running(ServiceStart::Manual, false))]);
        t.effects = vec![SideEffect::RefreshPolicy];
        let mut h = Harness::new(vec![Box::new(t)]);
        h.sys.fail_effects(true);
        h.engine.apply("sys").unwrap();
        assert_eq!(h.sys.get(&svc()), running(ServiceStart::Manual, false));
        assert_eq!(h.engine.applied_tweak_ids(), vec!["sys"]);
        assert!(h
            .engine
            .journal_view()
            .records
            .iter()
            .any(|r| matches!(r, Record::Effect(e) if e.error.is_some())));
    }

    #[test]
    fn no_effect_runs_when_an_apply_is_rolled_back() {
        let mut t = SysTweak::new(vec![(svc(), running(ServiceStart::Manual, false))]);
        t.effects = vec![SideEffect::RefreshPolicy];
        t.fail_after = true;
        let mut h = Harness::new(vec![Box::new(t)]);
        assert!(h.engine.apply("sys").is_err());
        assert!(h.sys.effects().is_empty());
    }

    #[test]
    fn change_records_survive_a_restart_and_undo_all_reverts_them() {
        let t = || SysTweak::new(vec![(SysItem::Hibernation, SysState::Bool { on: false })]);
        let mut h = Harness::new(vec![Box::new(t())]);
        h.sys.set(&SysItem::Hibernation, SysState::Bool { on: true });
        h.engine.apply("sys").unwrap();
        h.restart(vec![Box::new(t())]);
        assert_eq!(h.engine.applied_tweak_ids(), vec!["sys"]);
        let results = h.engine.revert_all();
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        assert_eq!(h.sys.get(&SysItem::Hibernation), SysState::Bool { on: true });
    }

    #[test]
    fn an_engine_without_a_system_backend_refuses_before_changing_anything() {
        let fake = Arc::new(FakeRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let t = SysTweak::new(vec![(svc(), running(ServiceStart::Disabled, false))]);
        let mut engine = build_engine(&fake, dir.path(), vec![Box::new(t)], true, Tier::Ultimate);
        assert!(engine.apply("sys").is_err());
        assert!(engine.applied_tweak_ids().is_empty());
    }

    // ---- review fixes (2026-10-07) -------------------------------------

    const SVC_KEY: &str = r"SYSTEM\CurrentControlSet\Services\WSearch";

    /// Plan section 12: a .reg backup before each change. A service's start
    /// type lives in the registry, so it is exported before the change, and
    /// the per-change session file carries it for Safe Mode recovery.
    #[test]
    fn a_service_change_backs_up_its_registry_values_to_reg_files_first() {
        let t = SysTweak::new(vec![(svc(), running(ServiceStart::Disabled, false))]);
        let mut h = Harness::new(vec![Box::new(t)]);
        h.fake.set_external(Hive::LocalMachine, SVC_KEY, "Start", dword(2));
        h.fake
            .set_external(Hive::LocalMachine, SVC_KEY, "DelayedAutostart", dword(1));
        h.sys.set(&svc(), running(ServiceStart::DelayedAutomatic, true));
        h.engine.apply("sys").unwrap();

        let change = h
            .engine
            .journal_view()
            .records
            .into_iter()
            .find_map(|r| match r {
                Record::Change(c) => Some(c),
                _ => None,
            })
            .unwrap();
        assert_eq!(change.reg_backups.len(), 2, "{:?}", change.reg_backups);
        let text = |rel: &str| {
            let bytes = std::fs::read(h.dir.path().join(rel)).unwrap();
            String::from_utf16_lossy(&crate::types::utf16le_units(&bytes))
        };
        let start = change.reg_backups.iter().find(|b| b.value_name == "Start").unwrap();
        let reg = text(&start.backup_file);
        assert!(
            reg.contains(r"Services\WSearch") && reg.contains("dword:00000002"),
            "{reg}"
        );

        let session = walk_files(h.dir.path())
            .into_iter()
            .find(|p| p.file_name().unwrap().to_string_lossy().starts_with("session_"))
            .expect("a session .reg for the change");
        let s = text(session.strip_prefix(h.dir.path()).unwrap().to_str().unwrap());
        assert!(
            s.contains(r"Services\WSearch") && s.contains("\"DelayedAutostart\"=dword:00000001"),
            "{s}"
        );
    }

    fn walk_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.extend(walk_files(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    fn setting(scheme: &str) -> SysItem {
        SysItem::PowerSetting {
            scheme: scheme.into(),
            subgroup: "54533251-82be-4824-96c1-47b60b740d00".into(),
            setting: "0cc5b647-c1df-4637-891a-dec35c318583".into(),
            ac: true,
        }
    }

    /// A setting Windows hides reads as Absent and could not be put back, so
    /// it may only be changed on a plan copy this same tweak created.
    #[test]
    fn a_hidden_power_setting_on_a_plan_we_did_not_make_is_refused() {
        let t = SysTweak::new(vec![(setting("theirs"), SysState::Dword { value: 100 })]);
        let mut h = Harness::new(vec![Box::new(t)]);
        let err = h.engine.apply("sys").unwrap_err();
        assert!(matches!(err, EngineError::ContextViolation { .. }), "{err:?}");
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn a_hidden_power_setting_on_our_own_new_plan_copy_is_allowed() {
        let ours = SysItem::PowerScheme { guid: "ours".into() };
        let t = SysTweak::new(vec![
            (
                ours.clone(),
                SysState::Scheme {
                    source: "e9a42b02".into(),
                },
            ),
            (setting("ours"), SysState::Dword { value: 100 }),
        ]);
        let mut h = Harness::new(vec![Box::new(t)]);
        h.engine.apply("sys").unwrap();
        h.engine.revert("sys").unwrap();
        assert_eq!(h.sys.get(&ours), SysState::Absent);
    }

    #[test]
    fn a_side_effect_the_tweak_did_not_declare_is_refused() {
        let mut t = SysTweak::new(vec![(svc(), running(ServiceStart::Manual, false))]);
        t.effects = vec![SideEffect::RestartService { name: "Spooler".into() }];
        t.declared_effects = vec![SideEffect::RestartService { name: "WSearch".into() }];
        let mut h = Harness::new(vec![Box::new(t)]);
        assert!(h.engine.apply("sys").is_err());
        assert!(h.sys.effects().is_empty());
    }
}

/// Every value tweak, on an empty registry: Apply reaches Applied, Undo leaves
/// the registry exactly as it was (no values, no keys left behind).
#[test]
fn every_value_tweak_applies_and_undoes_to_an_identical_registry() {
    let expected = [
        "privacy.advertisingid",
        "privacy.tips",
        "privacy.tailored",
        "privacy.activityhistory",
        "privacy.telemetry",
        "explorer.fileextensions",
        "explorer.websearch",
        "display.wallpaperquality",
        "display.animations",
        "input.queuesize",
        "system.faststartup",
        "scheduling.timerrequests",
    ];
    let all = crate::tweaks::registry_values::all();
    let ids: Vec<&str> = all.iter().map(|t| t.id()).collect();
    for id in expected {
        assert!(ids.contains(&id), "{id} missing from the value tweaks");
    }
    for tweak in crate::tweaks::registry_values::all() {
        let id = tweak.id().to_owned();
        let fake = Arc::new(FakeRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let mut engine = build_engine(&fake, dir.path(), vec![tweak], true, Tier::Ultimate);
        let before_keys = fake.key_paths();
        engine.apply(&id).unwrap();
        assert!(
            matches!(engine.list().unwrap()[0].state, TweakState::Applied),
            "{id} not Applied after Apply"
        );
        engine.revert(&id).unwrap();
        assert!(fake.snapshot().is_empty(), "{id} left values behind");
        assert_eq!(fake.key_paths(), before_keys, "{id} left keys behind");
    }
}

mod nagle {
    use super::*;
    use crate::system::NetAdapter;
    use crate::tweaks::nagle::{Nagle, ID};

    const ETH: &str = "3f504232-cecb-4118-b4d8-5a5e72d677c3";
    const WIFI: &str = "4d86b570-2994-4eb0-a004-914ef65ff05a";

    fn key(g: &str) -> String {
        format!(r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\{{{g}}}")
    }

    fn adapters(eth_up: bool) -> Vec<NetAdapter> {
        vec![
            NetAdapter {
                guid: ETH.into(),
                name: "Ethernet".into(),
                up: eth_up,
                wireless: false,
            },
            NetAdapter {
                guid: WIFI.into(),
                name: "Wi-Fi".into(),
                up: false,
                wireless: true,
            },
        ]
    }

    fn state(h: &Harness) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|v| v.metadata.id == ID)
            .unwrap()
            .state
    }

    #[test]
    fn sets_both_values_on_connected_adapters_only_and_undo_restores_them() {
        let mut h = Harness::new(vec![Box::new(Nagle)]);
        h.sys.set_adapters(adapters(true));
        h.fake.set_external(
            Hive::LocalMachine,
            &key(ETH),
            "DhcpIPAddress",
            RawValue::sz("192.168.1.68"),
        );
        h.fake
            .set_external(Hive::LocalMachine, &key(ETH), "TcpAckFrequency", dword(2));
        assert_eq!(state(&h), TweakState::Default);

        h.engine.apply(ID).unwrap();
        assert_eq!(hklm_dword(&h.fake, &key(ETH), "TcpAckFrequency"), Some(1));
        assert_eq!(hklm_dword(&h.fake, &key(ETH), "TCPNoDelay"), Some(1));
        assert_eq!(
            hklm_dword(&h.fake, &key(WIFI), "TCPNoDelay"),
            None,
            "a disconnected adapter is left alone"
        );
        assert_eq!(state(&h), TweakState::Applied);

        h.engine.revert(ID).unwrap();
        assert_eq!(hklm_dword(&h.fake, &key(ETH), "TcpAckFrequency"), Some(2));
        assert_eq!(hklm_dword(&h.fake, &key(ETH), "TCPNoDelay"), None);
        assert!(
            h.fake.key_exists_for_test(Hive::LocalMachine, &key(ETH)),
            "the adapter key stays"
        );
    }

    #[test]
    fn values_already_set_read_as_already_optimized() {
        let h = Harness::new(vec![Box::new(Nagle)]);
        h.sys.set_adapters(adapters(true));
        h.fake
            .set_external(Hive::LocalMachine, &key(ETH), "TcpAckFrequency", dword(1));
        h.fake
            .set_external(Hive::LocalMachine, &key(ETH), "TCPNoDelay", dword(1));
        assert_eq!(state(&h), TweakState::Foreign);
    }

    #[test]
    fn with_no_connected_adapter_it_is_blocked_and_apply_writes_nothing() {
        let mut h = Harness::new(vec![Box::new(Nagle)]);
        h.sys.set_adapters(adapters(false));
        assert!(matches!(state(&h), TweakState::Blocked { .. }));
        let before = h.fake.snapshot();
        assert!(h.engine.apply(ID).is_err());
        assert_eq!(h.fake.snapshot(), before);
    }
}

mod dns {
    use super::*;
    use crate::system::{NetAdapter, SysItem, SysState};
    use crate::tweaks::dns::{CloudflareDns, ID};

    fn adapter(g: &str, up: bool) -> NetAdapter {
        NetAdapter {
            guid: g.into(),
            name: g.into(),
            up,
            wireless: false,
        }
    }

    fn dns(g: &str) -> SysItem {
        SysItem::DnsServers { interface: g.into() }
    }

    fn list(items: &[&str]) -> SysState {
        SysState::List {
            items: items.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn state(h: &Harness) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|v| v.metadata.id == ID)
            .unwrap()
            .state
    }

    #[test]
    fn sets_cloudflare_on_connected_adapters_and_undo_restores_each_ones_own_servers() {
        let mut h = Harness::new(vec![Box::new(CloudflareDns)]);
        h.sys
            .set_adapters(vec![adapter("aa", true), adapter("bb", true), adapter("cc", false)]);
        h.sys.set(&dns("aa"), list(&["156.154.70.5", "156.154.71.5"]));
        h.sys.set(&dns("bb"), list(&[]));
        assert_eq!(state(&h), TweakState::Default);

        h.engine.apply(ID).unwrap();
        assert_eq!(h.sys.get(&dns("aa")), list(&["1.1.1.1", "1.0.0.1"]));
        assert_eq!(h.sys.get(&dns("bb")), list(&["1.1.1.1", "1.0.0.1"]));
        assert_eq!(
            h.sys.get(&dns("cc")),
            SysState::Absent,
            "a disconnected adapter is left alone"
        );
        assert_eq!(state(&h), TweakState::Applied);

        h.engine.revert(ID).unwrap();
        assert_eq!(h.sys.get(&dns("aa")), list(&["156.154.70.5", "156.154.71.5"]));
        assert_eq!(h.sys.get(&dns("bb")), list(&[]), "automatic stays automatic");
    }

    #[test]
    fn cloudflare_already_set_reads_as_already_optimized() {
        let h = Harness::new(vec![Box::new(CloudflareDns)]);
        h.sys.set_adapters(vec![adapter("aa", true)]);
        h.sys.set(&dns("aa"), list(&["1.1.1.1", "1.0.0.1"]));
        assert_eq!(state(&h), TweakState::Foreign);
    }

    #[test]
    fn with_no_connected_adapter_it_is_blocked() {
        let mut h = Harness::new(vec![Box::new(CloudflareDns)]);
        h.sys.set_adapters(vec![adapter("aa", false)]);
        assert!(matches!(state(&h), TweakState::Blocked { .. }));
        assert!(h.engine.apply(ID).is_err());
        assert!(h.engine.applied_tweak_ids().is_empty());
    }
}

/// CATALOGUE H14, H16-H18 (tweaks/services.rs).
mod service_tools {
    use std::sync::Arc;

    use crate::context::ContextResolver;
    use crate::error::EngineError;
    use crate::hardware::{BootDisk, DiskMedia, HardwareReport};
    use crate::probe::Probe;
    use crate::registry::{fake::FakeRegistry, Hive};
    use crate::system::{FakeSystem, ServiceStart, SysItem, SysState};
    use crate::testutil::{user, Harness};
    use crate::tweaks::services::*;
    use crate::types::{BlockedCode, PredicateOutcome, RawValue, SystemEnv, Tweak, TweakState};

    const SERVICES_KEY: &str = r"SYSTEM\CurrentControlSet\Services";
    const GAMING_SERVICES: &str = "GamingServices";
    const FAMILIES: &str =
        r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Families";

    fn svc(name: &str) -> SysItem {
        SysItem::Service { name: name.into() }
    }

    fn state(start: ServiceStart, running: bool) -> SysState {
        SysState::Service { start, running }
    }

    /// Windows has the service installed: its key exists, and the fake system
    /// knows its start type.
    fn install(h: &Harness, name: &str, start: ServiceStart, running: bool) {
        h.fake.set_external(
            Hive::LocalMachine,
            &format!(r"{SERVICES_KEY}\{name}"),
            "Start",
            RawValue::dword(2),
        );
        h.sys.set(&svc(name), state(start, running));
    }

    fn status(h: &Harness, id: &str) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|v| v.metadata.id == id)
            .unwrap()
            .state
    }

    #[test]
    fn search_indexing_is_turned_off_and_undo_restores_how_it_started_and_ran() {
        let mut h = Harness::new(vec![Box::new(SEARCH_INDEXING)]);
        install(&h, "WSearch", ServiceStart::DelayedAutomatic, true);
        assert_eq!(status(&h, SEARCH_INDEXING.id), TweakState::Default);

        h.engine.apply(SEARCH_INDEXING.id).unwrap();
        assert_eq!(h.sys.get(&svc("WSearch")), state(ServiceStart::Disabled, false));
        assert_eq!(status(&h, SEARCH_INDEXING.id), TweakState::Applied);

        h.engine.revert(SEARCH_INDEXING.id).unwrap();
        assert_eq!(h.sys.get(&svc("WSearch")), state(ServiceStart::DelayedAutomatic, true));
        assert_eq!(status(&h, SEARCH_INDEXING.id), TweakState::Default);
    }

    #[test]
    fn a_service_already_off_on_this_pc_is_listed_as_done() {
        let h = Harness::new(vec![Box::new(TELEMETRY_SERVICE)]);
        install(&h, "DiagTrack", ServiceStart::Disabled, false);
        assert_eq!(status(&h, TELEMETRY_SERVICE.id), TweakState::Foreign);
    }

    #[test]
    fn a_tool_whose_services_are_not_installed_is_not_offered() {
        let mut h = Harness::new(vec![Box::new(SEARCH_INDEXING)]);
        match status(&h, SEARCH_INDEXING.id) {
            TweakState::Blocked { reason } => assert_eq!(reason.code, BlockedCode::OsVersionUnsupported),
            other => panic!("{other:?}"),
        }
        let err = h.engine.apply(SEARCH_INDEXING.id).unwrap_err();
        assert!(matches!(err, EngineError::Blocked { .. }), "{err:?}");
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn only_installed_xbox_services_are_changed() {
        let mut h = Harness::new(vec![Box::new(XBOX_SERVICES)]);
        install(&h, "XblAuthManager", ServiceStart::Manual, false);
        install(&h, "XblGameSave", ServiceStart::Manual, false);
        h.engine.apply(XBOX_SERVICES.id).unwrap();
        assert_eq!(h.sys.get(&svc("XblAuthManager")), state(ServiceStart::Disabled, false));
        assert_eq!(h.sys.get(&svc("XblGameSave")), state(ServiceStart::Disabled, false));
        assert_eq!(
            h.sys.get(&svc("XboxNetApiSvc")),
            SysState::Absent,
            "not installed, not touched"
        );
        h.engine.revert(XBOX_SERVICES.id).unwrap();
        assert_eq!(h.sys.get(&svc("XblAuthManager")), state(ServiceStart::Manual, false));
    }

    #[test]
    fn xbox_services_are_never_offered_with_game_pass_or_the_xbox_app() {
        // Game Pass: the Gaming Services service is installed.
        let mut h = Harness::new(vec![Box::new(XBOX_SERVICES)]);
        install(&h, "XblAuthManager", ServiceStart::Manual, false);
        install(&h, GAMING_SERVICES, ServiceStart::Automatic, true);
        match status(&h, XBOX_SERVICES.id) {
            TweakState::Blocked { reason } => {
                assert_eq!(reason.code, BlockedCode::NeededByInstalledApp);
                assert!(reason.message.contains("Game Pass"), "{}", reason.message);
            }
            other => panic!("{other:?}"),
        }
        assert!(h.engine.apply(XBOX_SERVICES.id).is_err());
        assert_eq!(h.sys.get(&svc("XblAuthManager")), state(ServiceStart::Manual, false));

        // The Xbox app, installed for the signed-in user.
        let h = Harness::new(vec![Box::new(XBOX_SERVICES)]);
        install(&h, "XblAuthManager", ServiceStart::Manual, false);
        h.fake.set_external(
            Hive::CurrentUser,
            &format!(r"{FAMILIES}\Microsoft.GamingApp_8wekyb3d8bbwe\Microsoft.GamingApp_1_x64__8wekyb3d8bbwe"),
            "x",
            RawValue::dword(1),
        );
        match status(&h, XBOX_SERVICES.id) {
            TweakState::Blocked { reason } => assert!(reason.message.contains("Xbox app"), "{}", reason.message),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_applied_xbox_change_stays_undoable_after_the_xbox_app_arrives() {
        let fake = Arc::new(FakeRegistry::new());
        let sys = Arc::new(FakeSystem::new());
        let res = ContextResolver::new(user(true), true, fake.clone()).with_system(sys.clone());
        fake.set_external(
            Hive::LocalMachine,
            &format!(r"{SERVICES_KEY}\XblAuthManager"),
            "Start",
            RawValue::dword(4),
        );
        fake.set_external(
            Hive::LocalMachine,
            &format!(r"{SERVICES_KEY}\{GAMING_SERVICES}"),
            "Start",
            RawValue::dword(2),
        );
        sys.set(&svc("XblAuthManager"), state(ServiceStart::Disabled, false));
        assert_eq!(XBOX_SERVICES.read_state(&res, true).unwrap(), TweakState::Applied);
        assert!(matches!(
            XBOX_SERVICES.read_state(&res, false).unwrap(),
            TweakState::Blocked { .. }
        ));
    }

    fn env_with_boot(disk: Probe<BootDisk>) -> SystemEnv {
        let hardware = HardwareReport {
            os: Probe::unknown("-"),
            cpu: Probe::unknown("-"),
            memory: Probe::unknown("-"),
            gpus: Probe::unknown("-"),
            gpu_drivers: Probe::unknown("-"),
            boot_disk: disk,
            display: Probe::unknown("-"),
            is_laptop: Probe::unknown("-"),
            rig_class: Probe::unknown("-"),
        };
        SystemEnv {
            hardware: Some(hardware),
            ..SystemEnv::default()
        }
    }

    fn disk(media: DiskMedia) -> Probe<BootDisk> {
        Probe::yes(BootDisk {
            media,
            name: "disk".into(),
        })
    }

    #[test]
    fn sysmain_is_offered_only_when_windows_is_known_to_be_on_an_ssd() {
        let blocked = |env: &SystemEnv| match SYSMAIN.evaluate_predicate(env) {
            PredicateOutcome::Block(r) => Some(r.code),
            PredicateOutcome::Allow => None,
        };
        assert_eq!(blocked(&env_with_boot(disk(DiskMedia::Ssd))), None);
        assert_eq!(
            blocked(&env_with_boot(disk(DiskMedia::Hdd))),
            Some(BlockedCode::HardwareCounterproductive)
        );
        assert!(blocked(&env_with_boot(Probe::unknown("query failed"))).is_some());
        assert!(blocked(&SystemEnv::default()).is_some(), "not probed yet is not an SSD");
        assert!(matches!(
            SEARCH_INDEXING.evaluate_predicate(&SystemEnv::default()),
            PredicateOutcome::Allow
        ));
    }

    #[test]
    fn every_service_tool_declares_exactly_its_services() {
        for t in all() {
            let declared = t.system_targets();
            assert!(!declared.is_empty(), "{}", t.id());
            assert!(declared
                .iter()
                .all(|i| matches!(i, SysItem::Service { name } if name != "*")));
            assert!(t.touches().is_empty(), "{}", t.id());
        }
    }
}

/// CATALOGUE H7 and H10 (tweaks/power.rs).
mod power_tools {
    use crate::power::PEAKTWEAKS;
    use crate::registry::Hive;
    use crate::system::{SysItem, SysState};
    use crate::testutil::Harness;
    use crate::tweaks::power::*;
    use crate::types::{RawValue, TweakState};

    const BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
    const SETTINGS_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Power\PowerSettings";

    fn text(t: &str) -> SysState {
        SysState::Text { text: t.into() }
    }

    fn ours() -> SysItem {
        SysItem::PowerScheme {
            guid: PEAKTWEAKS.into(),
        }
    }

    fn setting(sub: &str, set: &str) -> SysItem {
        SysItem::PowerSetting {
            scheme: PEAKTWEAKS.into(),
            subgroup: sub.into(),
            setting: set.into(),
            ac: true,
        }
    }

    /// A PC on Balanced whose Windows defines the settings in `defined`.
    fn pc(defined: usize) -> Harness {
        let h = Harness::new(all());
        h.sys.set(&SysItem::ActivePowerScheme, text(BALANCED));
        for (sub, set, _, _) in &PLAN_SETTINGS[..defined] {
            h.fake.set_external(
                Hive::LocalMachine,
                &format!(r"{SETTINGS_KEY}\{sub}\{set}"),
                "Attributes",
                RawValue::dword(1),
            );
        }
        h
    }

    fn status(h: &Harness, id: &str) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|v| v.metadata.id == id)
            .unwrap()
            .state
    }

    #[test]
    fn the_plan_is_a_copy_of_the_active_one_and_undo_switches_back_and_deletes_it() {
        let mut h = pc(PLAN_SETTINGS.len());
        assert_eq!(status(&h, PLAN_ID), TweakState::Default);

        h.engine.apply(PLAN_ID).unwrap();
        assert_eq!(h.sys.get(&SysItem::ActivePowerScheme), text(PEAKTWEAKS));
        assert_eq!(
            h.sys.get(&ours()),
            SysState::Scheme {
                source: BALANCED.into()
            }
        );
        for (sub, set, value, _) in PLAN_SETTINGS {
            assert_eq!(h.sys.get(&setting(sub, set)), SysState::Dword { value: *value });
        }
        assert_eq!(status(&h, PLAN_ID), TweakState::Applied);

        h.engine.revert(PLAN_ID).unwrap();
        assert_eq!(h.sys.get(&SysItem::ActivePowerScheme), text(BALANCED));
        assert_eq!(h.sys.get(&ours()), SysState::Absent, "the copy is deleted");
        assert_eq!(status(&h, PLAN_ID), TweakState::Default);
    }

    #[test]
    fn a_setting_windows_does_not_define_is_skipped() {
        let mut h = pc(PLAN_SETTINGS.len() - 1);
        h.engine.apply(PLAN_ID).unwrap();
        let (sub, set, _, _) = PLAN_SETTINGS[PLAN_SETTINGS.len() - 1];
        assert_eq!(h.sys.get(&setting(sub, set)), SysState::Absent);
        assert_eq!(h.sys.get(&SysItem::ActivePowerScheme), text(PEAKTWEAKS));
    }

    #[test]
    fn switching_plans_by_hand_shows_as_changed_and_apply_again_reuses_the_copy() {
        let mut h = pc(PLAN_SETTINGS.len());
        h.engine.apply(PLAN_ID).unwrap();
        h.sys.set(&SysItem::ActivePowerScheme, text(BALANCED));
        assert_eq!(status(&h, PLAN_ID), TweakState::Drifted);

        h.engine.apply(PLAN_ID).unwrap();
        assert_eq!(h.sys.get(&SysItem::ActivePowerScheme), text(PEAKTWEAKS));
        let schemes = h
            .engine
            .journal_view()
            .records
            .iter()
            .filter(|r| matches!(r, crate::journal::Record::Change(c) if c.item == ours()))
            .count();
        assert_eq!(schemes, 1, "the copy is made once");

        h.engine.revert(PLAN_ID).unwrap();
        assert_eq!(h.sys.get(&SysItem::ActivePowerScheme), text(BALANCED));
        assert_eq!(h.sys.get(&ours()), SysState::Absent);
    }

    #[test]
    fn hibernation_is_turned_off_and_undo_turns_it_back_on() {
        let mut h = pc(0);
        h.sys.set(&SysItem::Hibernation, SysState::Bool { on: true });
        assert_eq!(status(&h, HIBERNATION_ID), TweakState::Default);
        h.engine.apply(HIBERNATION_ID).unwrap();
        assert_eq!(h.sys.get(&SysItem::Hibernation), SysState::Bool { on: false });
        assert_eq!(status(&h, HIBERNATION_ID), TweakState::Applied);
        h.engine.revert(HIBERNATION_ID).unwrap();
        assert_eq!(h.sys.get(&SysItem::Hibernation), SysState::Bool { on: true });
    }

    #[test]
    fn hibernation_already_off_is_listed_as_done() {
        let h = pc(0);
        h.sys.set(&SysItem::Hibernation, SysState::Bool { on: false });
        assert_eq!(status(&h, HIBERNATION_ID), TweakState::Foreign);
    }
}

/// CATALOGUE H14, the scheduled-task half (tweaks/tasks.rs).
mod task_tools {
    use crate::error::EngineError;
    use crate::system::{SysItem, SysState};
    use crate::testutil::Harness;
    use crate::tweaks::tasks::TELEMETRY_TASKS;
    use crate::types::{BlockedCode, SafetyTier, Tweak, TweakState};

    fn task(path: &str) -> SysItem {
        SysItem::ScheduledTask { path: path.into() }
    }

    fn on(on: bool) -> SysState {
        SysState::Bool { on }
    }

    fn status(h: &Harness) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|v| v.metadata.id == TELEMETRY_TASKS.id)
            .unwrap()
            .state
    }

    /// Every task but the last exists on this PC, all enabled.
    fn pc() -> Harness {
        let h = Harness::new(vec![Box::new(TELEMETRY_TASKS)]);
        for path in &TELEMETRY_TASKS.tasks[..TELEMETRY_TASKS.tasks.len() - 1] {
            h.sys.set(&task(path), on(true));
        }
        h
    }

    #[test]
    fn the_tasks_on_this_pc_are_disabled_and_undo_enables_them_again() {
        let mut h = pc();
        assert_eq!(status(&h), TweakState::Default);
        h.engine.apply(TELEMETRY_TASKS.id).unwrap();
        let (last, present) = TELEMETRY_TASKS.tasks.split_last().unwrap();
        for path in present {
            assert_eq!(h.sys.get(&task(path)), on(false), "{path}");
        }
        assert_eq!(h.sys.get(&task(last)), SysState::Absent, "a missing task is left alone");
        assert_eq!(status(&h), TweakState::Applied);

        h.engine.revert(TELEMETRY_TASKS.id).unwrap();
        for path in present {
            assert_eq!(h.sys.get(&task(path)), on(true), "{path}");
        }
        assert_eq!(status(&h), TweakState::Default);
    }

    #[test]
    fn a_task_the_user_had_already_disabled_stays_disabled_after_undo() {
        let mut h = pc();
        let first = TELEMETRY_TASKS.tasks[0];
        h.sys.set(&task(first), on(false));
        assert_eq!(status(&h), TweakState::Default);
        h.engine.apply(TELEMETRY_TASKS.id).unwrap();
        h.engine.revert(TELEMETRY_TASKS.id).unwrap();
        assert_eq!(h.sys.get(&task(first)), on(false));
        assert_eq!(h.sys.get(&task(TELEMETRY_TASKS.tasks[1])), on(true));
    }

    #[test]
    fn all_tasks_already_disabled_is_listed_as_done() {
        let h = pc();
        for path in TELEMETRY_TASKS.tasks {
            if h.sys.get(&task(path)) != SysState::Absent {
                h.sys.set(&task(path), on(false));
            }
        }
        assert_eq!(status(&h), TweakState::Foreign);
    }

    #[test]
    fn with_none_of_the_tasks_on_this_pc_it_is_not_offered() {
        let mut h = Harness::new(vec![Box::new(TELEMETRY_TASKS)]);
        match status(&h) {
            TweakState::Blocked { reason } => assert_eq!(reason.code, BlockedCode::OsVersionUnsupported),
            other => panic!("{other:?}"),
        }
        let err = h.engine.apply(TELEMETRY_TASKS.id).unwrap_err();
        assert!(matches!(err, EngineError::Blocked { .. }), "{err:?}");
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn it_declares_every_task_it_may_change_and_is_safe_tier() {
        let targets = TELEMETRY_TASKS.system_targets();
        assert_eq!(targets.len(), TELEMETRY_TASKS.tasks.len());
        for path in TELEMETRY_TASKS.tasks {
            assert!(targets.contains(&task(path)), "{path}");
            assert!(path.starts_with(r"\Microsoft\Windows\"), "{path}");
        }
        let m = TELEMETRY_TASKS.metadata();
        assert_eq!(m.category, "privacy");
        assert_eq!(m.safety, SafetyTier::Safe);
        assert!(m.tradeoff.is_none());
        assert!(TELEMETRY_TASKS.touches().is_empty());
    }
}
