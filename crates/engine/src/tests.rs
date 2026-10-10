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

/// The driver install (`gpu_install.rs`) starts only with a verified restore
/// point, and only after its journal line is written.
#[test]
fn a_driver_install_needs_a_restore_point_and_is_journalled_before_it_starts() {
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let mut closed = Harness::with(vec![], fake.clone(), dir, false);
    let err = closed
        .engine
        .begin_gpu_driver_install("581.80-desktop.exe", Some("581.80"))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::Blocked { reason } if reason.code == BlockedCode::NoRestorePoint),
        "{err:?}"
    );
    assert!(closed.engine.require_restore_point().is_err());
    assert!(
        !closed
            .engine
            .journal_view()
            .records
            .iter()
            .any(|r| matches!(r, Record::Action(_))),
        "nothing journalled when refused"
    );

    let dir = tempfile::tempdir().unwrap();
    let mut open = Harness::with(vec![], fake, dir, true);
    open.engine.require_restore_point().unwrap();
    open.engine
        .begin_gpu_driver_install("581.80-desktop.exe", Some("581.80"))
        .unwrap();
    let view = open.engine.journal_view();
    let Some(Record::Action(started)) = view.records.last() else {
        panic!("no action record: {:?}", view.records);
    };
    assert_eq!(started.action, OneTimeAction::InstallGpuDriver);
    assert_eq!(
        started.done,
        Some(ActionDone::GpuDriverStarted {
            file: "581.80-desktop.exe".into(),
            version: Some("581.80".into())
        })
    );
    assert!(
        view.applied.is_empty(),
        "nothing for Undo all: System Restore puts a driver back"
    );
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

/// Which slot of `SPI_SETMOUSE`'s array is which. Read-only:
/// Windows loads the session's values from `HKCU\Control Panel\Mouse` at
/// sign-in, so `SPI_GETMOUSE` lined up against the three named registry
/// values shows the order whenever the three differ. Prints NOT VERIFIED when
/// they do not, or when the session no longer matches the registry. Microsoft's
/// SystemParametersInfo page settles the order too (C38, was NOTES N6).
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

    /// Offline recovery (N48) covers a service's start type from the values
    /// exported before the change, and names a kind it cannot put back.
    #[test]
    fn the_offline_undo_set_covers_a_service_and_names_what_it_cannot() {
        let services = r"SYSTEM\CurrentControlSet\Services\WSearch";
        let t = SysTweak::new(vec![(svc(), running(ServiceStart::Disabled, false))]);
        let mut h = Harness::new(vec![Box::new(t)]);
        h.sys.set(&svc(), running(ServiceStart::DelayedAutomatic, true));
        h.fake
            .set_external(Hive::LocalMachine, services, "Start", RawValue::dword(2));
        h.fake
            .set_external(Hive::LocalMachine, services, "DelayedAutostart", RawValue::dword(1));
        h.fake
            .set_external(Hive::LocalMachine, r"SYSTEM\Select", "Current", RawValue::dword(1));
        h.engine.apply("sys").unwrap();
        let offline = h.dir.path().join(crate::offline::DIR);
        let bytes = std::fs::read(offline.join("001_sys.reg")).unwrap();
        let units: Vec<u16> = bytes[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text = String::from_utf16(&units).unwrap();
        assert!(
            text.contains("ControlSet001\\Services\\WSearch]\r\n\"Start\"=dword:00000002"),
            "{text}"
        );
        assert!(text.contains("\"DelayedAutostart\"=dword:00000001"), "{text}");
        h.engine.revert("sys").unwrap();

        let t = SysTweak::new(vec![(SysItem::Hibernation, SysState::Bool { on: false })]);
        let mut h = Harness::new(vec![Box::new(t)]);
        h.sys.set(&SysItem::Hibernation, SysState::Bool { on: true });
        h.engine.apply("sys").unwrap();
        let readme = std::fs::read_to_string(h.dir.path().join(crate::offline::DIR).join("README.txt")).unwrap();
        assert!(readme.contains("- sys: the hibernation"), "{readme}");
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
        h.sys.set_adapters(vec![crate::system::NetAdapter {
            guid: "{9F2-AA}".into(),
            name: "Ethernet".into(),
            up: true,
            wireless: false,
            wired: true,
            pnp_id: String::new(),
        }]);
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
                wired: true,
                pnp_id: String::new(),
            },
            NetAdapter {
                guid: WIFI.into(),
                name: "Wi-Fi".into(),
                up: false,
                wireless: true,
                wired: false,
                pnp_id: String::new(),
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
            wired: true,
            pnp_id: String::new(),
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

    /// Windows CI run 37825562132: Azure's runner lists a connected adapter
    /// with no IPv4 of its own (bonded under another), and Windows refused to
    /// set DNS servers on it, so Apply failed for every adapter. One without
    /// IPv4 reads Absent and is left alone.
    #[test]
    fn a_connected_adapter_without_ipv4_is_left_alone() {
        let mut h = Harness::new(vec![Box::new(CloudflareDns)]);
        h.sys.set_adapters(vec![adapter("aa", true), adapter("bb", true)]);
        h.sys.set(&dns("aa"), list(&[]));
        assert_eq!(state(&h), TweakState::Default);

        h.engine.apply(ID).unwrap();
        assert_eq!(h.sys.get(&dns("aa")), list(&["1.1.1.1", "1.0.0.1"]));
        assert_eq!(h.sys.get(&dns("bb")), SysState::Absent);
        assert_eq!(state(&h), TweakState::Applied);

        h.engine.revert(ID).unwrap();
        assert_eq!(h.sys.get(&dns("aa")), list(&[]));
        assert_eq!(state(&h), TweakState::Default);
    }

    #[test]
    fn with_only_adapters_without_ipv4_it_is_blocked() {
        let mut h = Harness::new(vec![Box::new(CloudflareDns)]);
        h.sys.set_adapters(vec![adapter("bb", true)]);
        assert!(matches!(state(&h), TweakState::Blocked { .. }));
        assert!(h.engine.apply(ID).is_err());
        assert!(h.engine.applied_tweak_ids().is_empty());
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

/// CATALOGUE H6 (tweaks/msi.rs).
mod msi_mode {
    use super::*;
    use crate::error::EngineError;
    use crate::registry::Hive;
    use crate::system::{DeviceClass, PciDevice};
    use crate::tweaks::msi::{pci_instance, MsiMode, VALUE};
    use crate::types::RawValue;

    const GPU: &str = r"PCI\VEN_10DE&DEV_2484&SUBSYS_146710DE&REV_A1\4&2b0b1f0c&0&0008";
    const NIC: &str = r"PCI\VEN_10EC&DEV_8125&SUBSYS_86771043&REV_05\01000000684CE00000";
    const ENUM: &str = r"SYSTEM\CurrentControlSet\Enum";

    fn msi_key(instance: &str) -> String {
        format!(r"{ENUM}\{instance}\Device Parameters\Interrupt Management\MessageSignaledInterruptProperties")
    }

    /// A graphics card whose driver left MSI off, and a network card whose
    /// driver turned it on.
    fn pc() -> Harness {
        let h = Harness::new(Vec::new());
        for (i, name) in [(GPU, "NVIDIA GeForce RTX 3060 Ti"), (NIC, "Realtek Gaming 2.5GbE")] {
            h.fake.set_external(
                Hive::LocalMachine,
                &format!(r"{ENUM}\{i}"),
                "DeviceDesc",
                RawValue::sz(name),
            );
        }
        h.fake
            .set_external(Hive::LocalMachine, &msi_key(NIC), VALUE, RawValue::dword(1));
        h.sys.set_pci_devices(vec![
            PciDevice {
                instance_id: NIC.into(),
                name: "Realtek Gaming 2.5GbE Family Controller".into(),
                class: DeviceClass::Net,
            },
            PciDevice {
                instance_id: GPU.into(),
                name: "NVIDIA GeForce RTX 3060 Ti".into(),
                class: DeviceClass::Display,
            },
        ]);
        h
    }

    fn id(instance: &str) -> String {
        format!("msi.{instance}")
    }

    #[test]
    fn lists_the_graphics_card_first_with_each_devices_state() {
        let mut h = pc();
        let list = h.engine.msi_devices();
        assert_eq!(list.problem, None);
        let shown: Vec<(&str, DeviceClass, &TweakState)> = list
            .devices
            .iter()
            .map(|d| (d.tweak.metadata.id.as_ref(), d.class, &d.tweak.state))
            .collect();
        let (gpu, nic) = (id(GPU), id(NIC));
        assert_eq!(
            shown,
            [
                (gpu.as_str(), DeviceClass::Display, &TweakState::Default),
                (nic.as_str(), DeviceClass::Net, &TweakState::Foreign),
            ]
        );
        assert_eq!(
            list.devices[0].tweak.metadata.name,
            "MSI mode: NVIDIA GeForce RTX 3060 Ti"
        );
        assert!(list.devices[0].tweak.metadata.requires_reboot);
    }

    #[test]
    fn apply_writes_only_msisupported_and_undo_removes_it_and_its_keys() {
        let mut h = pc();
        h.engine.msi_devices();
        let before = h.fake.snapshot();
        h.engine.apply(&id(GPU)).unwrap();
        assert_eq!(
            h.fake.read_value_for_test(Hive::LocalMachine, &msi_key(GPU), VALUE),
            Some(RawValue::dword(1))
        );
        let after = h.fake.snapshot();
        assert_eq!(after.len(), before.len() + 1, "one value written");
        assert_eq!(
            h.engine.journal_view().applied[0].name,
            "MSI mode: NVIDIA GeForce RTX 3060 Ti"
        );

        h.engine.revert(&id(GPU)).unwrap();
        assert_eq!(h.fake.snapshot(), before);
        assert!(!h.fake.key_exists_for_test(
            Hive::LocalMachine,
            &format!(r"{ENUM}\{GPU}\Device Parameters\Interrupt Management")
        ));
    }

    #[test]
    fn undo_after_the_card_is_removed_finishes_without_recreating_its_key() {
        let mut h = pc();
        h.engine.msi_devices();
        h.engine.apply(&id(GPU)).unwrap();
        h.fake
            .remove_key_external(Hive::LocalMachine, &format!(r"{ENUM}\{GPU}"));
        h.restart(Vec::new());
        let results = h.engine.revert_all();
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        assert!(!h
            .fake
            .key_exists_for_test(Hive::LocalMachine, &format!(r"{ENUM}\{GPU}")));
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn a_failed_listing_says_why_and_withdraws_the_last_list() {
        let mut h = pc();
        h.engine.msi_devices();
        h.sys.fail_listing(true);
        let list = h.engine.msi_devices();
        assert!(list.devices.is_empty());
        assert!(list.problem.as_deref().is_some_and(|p| p.contains("did not answer")));
        assert!(
            matches!(h.engine.apply(&id(GPU)), Err(EngineError::UnknownTweak { .. })),
            "nothing is offered that the app no longer shows"
        );
    }

    #[test]
    fn only_listed_pci_devices_can_be_changed() {
        let mut h = pc();
        let before = h.fake.snapshot();
        assert!(
            matches!(h.engine.apply(&id(GPU)), Err(EngineError::UnknownTweak { .. })),
            "not listed yet"
        );
        h.engine.msi_devices();
        for bad in [
            r"msi.PCI\VEN_10DE&DEV_2484\..\..\Control",
            r"msi.PCI\VEN_10DE&DEV_2484\4&2b0b1f0c&0&0008\Device Parameters",
            r"msi.ACPI\PNP0A08\0",
            r"msi.PCI\DEV_2484\x",
            r"msi.PCI\VEN_10DE&DEV_2484\a b",
            "msi.",
        ] {
            assert!(
                matches!(h.engine.apply(bad), Err(EngineError::UnknownTweak { .. })),
                "{bad}"
            );
        }
        assert_eq!(h.fake.snapshot(), before);
        assert_eq!(
            pci_instance(GPU),
            Some(("VEN_10DE&DEV_2484&SUBSYS_146710DE&REV_A1", "4&2b0b1f0c&0&0008"))
        );
        assert_eq!(
            MsiMode::from_id(&id(GPU)).unwrap().name,
            "NVIDIA device DEV_2484",
            "named by its maker when only the id is known"
        );
    }

    #[test]
    fn its_target_stays_under_the_devices_own_key() {
        let t = MsiMode::new(GPU, "x").unwrap();
        let targets = t.touches();
        assert_eq!(targets.len(), 1);
        assert_eq!(
            targets[0].key,
            r"SYSTEM\CurrentControlSet\Enum\PCI\VEN_10DE&DEV_2484&SUBSYS_146710DE&REV_A1\*\Device Parameters\Interrupt Management\MessageSignaledInterruptProperties"
        );
        assert_eq!(targets[0].values, [VALUE]);
    }
}

/// CATALOGUE E5 (tweaks/cable.rs).
mod prefer_cable {
    use super::*;
    use crate::error::EngineError;
    use crate::system::{NetAdapter, SysItem, SysState};
    use crate::tweaks::cable::{PreferCable, ID, WIFI_METRIC, WIRED_METRIC};
    use crate::types::BlockedCode;

    fn adapter(g: &str, kind: &str, up: bool) -> NetAdapter {
        NetAdapter {
            guid: g.into(),
            name: kind.into(),
            up,
            wireless: kind == "wifi",
            wired: kind == "cable",
            pnp_id: String::new(),
        }
    }

    fn metric(g: &str, ipv6: bool) -> SysItem {
        SysItem::InterfaceMetric {
            interface: g.into(),
            ipv6,
        }
    }

    const AUTO: SysState = SysState::Dword { value: 0 };
    const fn m(value: u32) -> SysState {
        SysState::Dword { value }
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

    /// Kegan's PC: a cable (connected) and Wi-Fi (not), plus Bluetooth. The
    /// cable has IPv4 and IPv6, Wi-Fi only IPv4 with a metric set by hand.
    fn pc() -> Harness {
        let h = Harness::new(vec![Box::new(PreferCable)]);
        h.sys.set_adapters(vec![
            adapter("cab", "cable", true),
            adapter("wif", "wifi", false),
            adapter("blu", "bluetooth", false),
        ]);
        h.sys.set(&metric("cab", false), AUTO);
        h.sys.set(&metric("cab", true), AUTO);
        h.sys.set(&metric("wif", false), m(20));
        h.sys.set(&metric("blu", false), AUTO);
        h
    }

    #[test]
    fn sets_the_cable_low_and_wifi_high_and_undo_puts_each_back() {
        let mut h = pc();
        assert_eq!(state(&h), TweakState::Default);

        h.engine.apply(ID).unwrap();
        assert_eq!(h.sys.get(&metric("cab", false)), m(WIRED_METRIC));
        assert_eq!(h.sys.get(&metric("cab", true)), m(WIRED_METRIC));
        assert_eq!(h.sys.get(&metric("wif", false)), m(WIFI_METRIC));
        assert_eq!(
            h.sys.get(&metric("wif", true)),
            SysState::Absent,
            "a protocol that is not on the adapter is left alone"
        );
        assert_eq!(h.sys.get(&metric("blu", false)), AUTO, "Bluetooth is neither");
        assert_eq!(state(&h), TweakState::Applied);

        h.engine.revert(ID).unwrap();
        assert_eq!(h.sys.get(&metric("cab", false)), AUTO, "automatic stays automatic");
        assert_eq!(h.sys.get(&metric("cab", true)), AUTO);
        assert_eq!(
            h.sys.get(&metric("wif", false)),
            m(20),
            "a metric set by hand comes back"
        );
        assert_eq!(state(&h), TweakState::Default);
    }

    #[test]
    fn a_new_adapter_after_apply_reads_as_changed_and_apply_again_covers_it() {
        let mut h = pc();
        h.engine.apply(ID).unwrap();
        let mut adapters = vec![
            adapter("cab", "cable", true),
            adapter("wif", "wifi", false),
            adapter("usb", "wifi", true),
        ];
        h.sys.set_adapters(adapters.clone());
        h.sys.set(&metric("usb", false), AUTO);
        assert_eq!(state(&h), TweakState::Drifted);
        h.engine.apply(ID).unwrap();
        assert_eq!(h.sys.get(&metric("usb", false)), m(WIFI_METRIC));
        assert_eq!(state(&h), TweakState::Applied);

        // The USB Wi-Fi is unplugged: Undo still finishes, and leaves it be.
        adapters.pop();
        h.sys.set_adapters(adapters);
        h.engine.revert(ID).unwrap();
        assert_eq!(h.sys.get(&metric("wif", false)), m(20));
        assert_eq!(h.sys.get(&metric("cab", false)), AUTO);
        assert_eq!(
            h.sys.get(&metric("usb", false)),
            m(WIFI_METRIC),
            "not written for a removed adapter"
        );
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn already_set_this_way_reads_as_already_optimized() {
        let h = pc();
        h.sys.set(&metric("cab", false), m(WIRED_METRIC));
        h.sys.set(&metric("cab", true), m(WIRED_METRIC));
        h.sys.set(&metric("wif", false), m(WIFI_METRIC));
        assert_eq!(state(&h), TweakState::Foreign);
    }

    #[test]
    fn a_pc_without_both_a_cable_port_and_wifi_is_not_offered() {
        for adapters in [
            vec![adapter("cab", "cable", true)],
            vec![adapter("wif", "wifi", true), adapter("blu", "bluetooth", true)],
            Vec::new(),
        ] {
            let mut h = Harness::new(vec![Box::new(PreferCable)]);
            h.sys.set_adapters(adapters);
            match state(&h) {
                TweakState::Blocked { reason } => assert_eq!(reason.code, BlockedCode::HardwareUnsupported),
                other => panic!("{other:?}"),
            }
            assert!(matches!(h.engine.apply(ID), Err(EngineError::Blocked { .. })));
            assert!(h.engine.applied_tweak_ids().is_empty());
        }
    }

    /// A Hyper-V host whose cable and Wi-Fi are both in virtual switches: the
    /// addresses live on the virtual adapters, so neither has an IP interface
    /// of its own and there is no metric to set.
    #[test]
    fn with_no_ip_settings_on_either_it_is_not_offered() {
        let mut h = Harness::new(vec![Box::new(PreferCable)]);
        h.sys
            .set_adapters(vec![adapter("cab", "cable", true), adapter("wif", "wifi", true)]);
        match state(&h) {
            TweakState::Blocked { reason } => assert_eq!(reason.code, BlockedCode::HardwareUnsupported),
            other => panic!("{other:?}"),
        }
        assert!(matches!(h.engine.apply(ID), Err(EngineError::Blocked { .. })));
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn its_registry_backing_names_each_protocols_key() {
        assert_eq!(
            metric("ab", true).registry_backing(),
            [(
                r"SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters\Interfaces\{ab}".to_owned(),
                "InterfaceMetric"
            )]
        );
        assert_eq!(metric("ab", false).describe(), "IPv4 interface metric of adapter ab");
    }
}

/// CATALOGUE H23 (tweaks/tcp.rs).
mod tcp_tools {
    use super::*;
    use crate::system::{SysItem, SysState};
    use crate::tweaks::tcp::{allowed, TcpSettings, ID, SETTINGS};

    fn item(name: &str) -> SysItem {
        SysItem::TcpGlobal { name: name.into() }
    }

    fn text(t: &str) -> SysState {
        SysState::Text { text: t.into() }
    }

    fn pc(autotuning: &str, rss: &str, ecn: &str) -> Harness {
        let h = Harness::new(vec![Box::new(TcpSettings)]);
        h.sys.set(&item("autotuninglevel"), text(autotuning));
        h.sys.set(&item("rss"), text(rss));
        h.sys.set(&item("ecncapability"), text(ecn));
        h
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

    fn changed(h: &Harness) -> Vec<SysItem> {
        h.engine
            .journal_view()
            .records
            .into_iter()
            .filter_map(|r| match r {
                Record::Change(c) => Some(c.item),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn changes_only_what_differs_and_undo_puts_it_back() {
        let mut h = pc("normal", "enabled", "enabled");
        assert_eq!(state(&h), TweakState::Default);

        h.engine.apply(ID).unwrap();
        assert_eq!(h.sys.get(&item("ecncapability")), text("disabled"));
        assert_eq!(
            changed(&h),
            [item("ecncapability")],
            "the two defaults are left as they are"
        );
        assert_eq!(state(&h), TweakState::Applied);

        h.engine.revert(ID).unwrap();
        assert_eq!(h.sys.get(&item("ecncapability")), text("enabled"));
        assert_eq!(h.sys.get(&item("autotuninglevel")), text("normal"));
        assert_eq!(state(&h), TweakState::Default);
    }

    #[test]
    fn auto_tuning_and_rss_turned_off_elsewhere_come_back_on_and_undo_restores_that() {
        let mut h = pc("disabled", "disabled", "disabled");
        h.engine.apply(ID).unwrap();
        assert_eq!(h.sys.get(&item("autotuninglevel")), text("normal"));
        assert_eq!(h.sys.get(&item("rss")), text("enabled"));
        assert_eq!(changed(&h).len(), 2);

        h.engine.revert(ID).unwrap();
        assert_eq!(h.sys.get(&item("autotuninglevel")), text("disabled"));
        assert_eq!(h.sys.get(&item("rss")), text("disabled"));
        assert_eq!(h.sys.get(&item("ecncapability")), text("disabled"));
    }

    #[test]
    fn already_set_this_way_reads_as_already_optimized_in_any_case() {
        let h = pc("Normal", "ENABLED", "disabled");
        assert_eq!(state(&h), TweakState::Foreign);
    }

    #[test]
    fn a_setting_without_a_value_does_not_count_as_set() {
        let h = Harness::new(vec![Box::new(TcpSettings)]);
        h.sys.set(&item("autotuninglevel"), text("normal"));
        h.sys.set(&item("rss"), text("enabled"));
        // ECN not set: the fake reads it as absent, which is not "disabled".
        // (Windows' backend reports an error instead, read as Unknown.)
        assert_eq!(state(&h), TweakState::Default);
    }

    #[test]
    fn it_declares_exactly_its_three_settings_and_sets_only_values_netsh_takes() {
        assert_eq!(
            TcpSettings.system_targets(),
            [item("autotuninglevel"), item("rss"), item("ecncapability")]
        );
        assert!(TcpSettings.touches().is_empty());
        for (name, value) in SETTINGS {
            assert!(allowed(name).unwrap().contains(&value), "{name}={value}");
        }
        assert_eq!(allowed("chimney"), None, "a setting this tool does not list");
        assert!(!TcpSettings.metadata().requires_reboot);
    }
}

/// CATALOGUE H24 (tweaks/adapter_props.rs).
mod adapter_tools {
    use super::*;
    use crate::error::EngineError;
    use crate::registry::Hive;
    use crate::system::{NetAdapter, SideEffect};
    use crate::tweaks::adapter_props::{
        AdapterSetting, ENERGY_EFFICIENT_ETHERNET, FLOW_CONTROL, INTERRUPT_MODERATION, POWER_SAVING,
    };
    use crate::types::{BlockedCode, RawValue, RegRoot};

    const NET: &str = "{4d36e972-e325-11ce-bfc1-08002be10318}";
    const ENUM: &str = r"SYSTEM\CurrentControlSet\Enum";
    const CABLE: &str = "0a1b2c3d-0000-4000-8000-000000000001";
    const WIFI: &str = "0a1b2c3d-0000-4000-8000-000000000002";
    const BLUETOOTH: &str = "0a1b2c3d-0000-4000-8000-000000000003";
    const CABLE_PNP: &str = r"PCI\VEN_8086&DEV_15F3&SUBSYS_86721043&REV_03\6&1f1b5a0&0&00E0";
    const WIFI_PNP: &str = r"PCI\VEN_8086&DEV_2723&SUBSYS_00848086&REV_1A\4&2a3f6c1&0&00E1";
    const BT_PNP: &str = r"BTH\MS_BTHPAN\7&30f1a3d&0&2";
    const IM: &str = "*InterruptModeration";

    fn class_key(index: &str) -> String {
        format!(r"SYSTEM\CurrentControlSet\Control\Class\{NET}\{index}")
    }

    fn adapter(guid: &str, name: &str, pnp: &str) -> NetAdapter {
        NetAdapter {
            guid: guid.into(),
            name: name.into(),
            up: true,
            wireless: name == "Wi-Fi",
            wired: name == "Ethernet",
            pnp_id: pnp.into(),
        }
    }

    /// An adapter's `Enum` and driver keys, as Windows writes them: `Driver`
    /// names the class key, which names the adapter back.
    fn install(h: &Harness, pnp: &str, index: &str, guid: &str) {
        h.fake.set_external(
            Hive::LocalMachine,
            &format!(r"{ENUM}\{pnp}"),
            "Driver",
            RawValue::sz(&format!(r"{NET}\{index}")),
        );
        h.fake.set_external(
            Hive::LocalMachine,
            &class_key(index),
            "NetCfgInstanceId",
            RawValue::sz(&format!("{{{}}}", guid.to_ascii_uppercase())),
        );
    }

    /// The driver offers `keyword` with these choices and this default.
    fn offer(h: &Harness, index: &str, keyword: &str, choices: &[&str], default: &str) {
        let params = format!(r"{}\Ndi\params\{keyword}", class_key(index));
        h.fake
            .set_external(Hive::LocalMachine, &params, "default", RawValue::sz(default));
        for c in choices {
            h.fake.set_external(
                Hive::LocalMachine,
                &format!(r"{params}\enum"),
                c,
                RawValue::sz(&format!("choice {c}")),
            );
        }
    }

    fn store(h: &Harness, index: &str, name: &str, v: RawValue) {
        h.fake.set_external(Hive::LocalMachine, &class_key(index), name, v);
    }

    fn stored(h: &Harness, index: &str, name: &str) -> Option<RawValue> {
        h.fake.read_value_for_test(Hive::LocalMachine, &class_key(index), name)
    }

    fn restart(guid: &str) -> SideEffect {
        SideEffect::RestartAdapter { interface: guid.into() }
    }

    fn state(h: &Harness, t: &AdapterSetting) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|v| v.metadata.id == t.id)
            .unwrap()
            .state
    }

    fn blocked(h: &Harness, t: &AdapterSetting) -> String {
        match state(h, t) {
            TweakState::Blocked { reason } => {
                assert_eq!(reason.code, BlockedCode::HardwareUnsupported);
                reason.message
            }
            other => panic!("{other:?}"),
        }
    }

    /// An Intel cable adapter with interrupt moderation on (stored), an Intel
    /// Wi-Fi adapter with it already off, and a Bluetooth adapter whose
    /// driver has no such setting.
    fn pc(tool: AdapterSetting) -> Harness {
        let h = Harness::new(vec![Box::new(tool)]);
        h.sys.set_adapters(vec![
            adapter(CABLE, "Ethernet", CABLE_PNP),
            adapter(WIFI, "Wi-Fi", WIFI_PNP),
            adapter(BLUETOOTH, "Bluetooth Network Connection", BT_PNP),
        ]);
        install(&h, CABLE_PNP, "0001", CABLE);
        install(&h, WIFI_PNP, "0002", WIFI);
        install(&h, BT_PNP, "0003", BLUETOOTH);
        offer(&h, "0001", IM, &["0", "1"], "1");
        store(&h, "0001", IM, RawValue::sz("1"));
        offer(&h, "0002", IM, &["0", "1"], "1");
        store(&h, "0002", IM, RawValue::sz("0"));
        h
    }

    #[test]
    fn turns_the_setting_off_where_it_is_on_and_restarts_only_that_adapter() {
        let mut h = pc(INTERRUPT_MODERATION);
        let before = h.fake.snapshot();
        assert_eq!(state(&h, &INTERRUPT_MODERATION), TweakState::Default);

        h.engine.apply(INTERRUPT_MODERATION.id).unwrap();
        assert_eq!(stored(&h, "0001", IM), Some(RawValue::sz("0")));
        assert_eq!(stored(&h, "0002", IM), Some(RawValue::sz("0")));
        assert_eq!(stored(&h, "0003", IM), None, "its driver has no such setting");
        assert_eq!(h.sys.effects(), [restart(CABLE)], "Wi-Fi was already off");
        assert_eq!(state(&h, &INTERRUPT_MODERATION), TweakState::Applied);

        h.engine.revert(INTERRUPT_MODERATION.id).unwrap();
        assert_eq!(h.fake.snapshot(), before);
        assert_eq!(h.sys.effects(), [restart(CABLE), restart(CABLE)]);
        assert_eq!(state(&h, &INTERRUPT_MODERATION), TweakState::Default);
    }

    #[test]
    fn a_value_the_driver_stored_as_a_number_stays_a_number() {
        let mut h = pc(INTERRUPT_MODERATION);
        store(&h, "0001", IM, RawValue::dword(1));
        h.engine.apply(INTERRUPT_MODERATION.id).unwrap();
        assert_eq!(stored(&h, "0001", IM), Some(RawValue::dword(0)));
        h.engine.revert(INTERRUPT_MODERATION.id).unwrap();
        assert_eq!(stored(&h, "0001", IM), Some(RawValue::dword(1)));
    }

    #[test]
    fn a_setting_the_key_does_not_hold_is_at_the_drivers_default() {
        let mut h = pc(INTERRUPT_MODERATION);
        h.fake.remove_external(Hive::LocalMachine, &class_key("0001"), IM);
        offer(&h, "0001", IM, &[], "0");
        assert_eq!(
            state(&h, &INTERRUPT_MODERATION),
            TweakState::Foreign,
            "the driver's default is off: already optimized"
        );

        offer(&h, "0001", IM, &[], "1");
        let before = h.fake.snapshot();
        assert_eq!(state(&h, &INTERRUPT_MODERATION), TweakState::Default);
        h.engine.apply(INTERRUPT_MODERATION.id).unwrap();
        assert_eq!(stored(&h, "0001", IM), Some(RawValue::sz("0")));
        h.engine.revert(INTERRUPT_MODERATION.id).unwrap();
        assert_eq!(h.fake.snapshot(), before, "the value is removed again");
    }

    #[test]
    fn a_value_the_driver_does_not_offer_is_never_written() {
        let mut h = Harness::new(vec![Box::new(FLOW_CONTROL)]);
        h.sys.set_adapters(vec![adapter(CABLE, "Ethernet", CABLE_PNP)]);
        install(&h, CABLE_PNP, "0001", CABLE);
        // Only "Rx & Tx Enabled" and "Rx Enabled": no "Disabled" (0).
        offer(&h, "0001", "*FlowControl", &["3", "2"], "3");
        let before = h.fake.snapshot();
        assert_eq!(blocked(&h, &FLOW_CONTROL), "No network adapter here has flow control.");
        assert!(matches!(
            h.engine.apply(FLOW_CONTROL.id),
            Err(EngineError::Blocked { .. })
        ));
        assert_eq!(h.fake.snapshot(), before);
        assert!(h.sys.effects().is_empty());
    }

    #[test]
    fn a_driver_key_that_is_not_this_adapters_is_left_alone() {
        type Spoil = fn(&Harness);
        let cases: [(&str, Spoil); 4] = [
            ("names another adapter", |h| {
                store(
                    h,
                    "0001",
                    "NetCfgInstanceId",
                    RawValue::sz("{0A1B2C3D-0000-4000-8000-0000000000FF}"),
                )
            }),
            ("another class", |h| {
                h.fake.set_external(
                    Hive::LocalMachine,
                    &format!(r"{ENUM}\{CABLE_PNP}"),
                    "Driver",
                    RawValue::sz(r"{4d36e968-e325-11ce-bfc1-08002be10318}\0001"),
                )
            }),
            ("not an index", |h| {
                h.fake.set_external(
                    Hive::LocalMachine,
                    &format!(r"{ENUM}\{CABLE_PNP}"),
                    "Driver",
                    RawValue::sz(&format!(r"{NET}\..\0001")),
                )
            }),
            ("no driver named", |h| {
                h.fake
                    .remove_external(Hive::LocalMachine, &format!(r"{ENUM}\{CABLE_PNP}"), "Driver")
            }),
        ];
        for (why, spoil) in cases {
            let h = Harness::new(vec![Box::new(INTERRUPT_MODERATION)]);
            h.sys.set_adapters(vec![adapter(CABLE, "Ethernet", CABLE_PNP)]);
            install(&h, CABLE_PNP, "0001", CABLE);
            offer(&h, "0001", IM, &["0", "1"], "1");
            spoil(&h);
            assert_eq!(
                blocked(&h, &INTERRUPT_MODERATION),
                "No network adapter here has interrupt moderation.",
                "{why}"
            );
        }

        // Windows gave no device id, or a path out of Enum.
        for pnp in ["", r"..\..\Control", r"\PCI\x"] {
            let h = Harness::new(vec![Box::new(INTERRUPT_MODERATION)]);
            h.sys.set_adapters(vec![adapter(CABLE, "Ethernet", pnp)]);
            assert!(
                matches!(state(&h, &INTERRUPT_MODERATION), TweakState::Blocked { .. }),
                "{pnp}"
            );
        }
    }

    #[test]
    fn power_saving_sets_two_bits_keeps_the_others_and_waits_for_a_restart() {
        let mut h = Harness::new(vec![Box::new(POWER_SAVING)]);
        h.sys.set_adapters(vec![
            adapter(CABLE, "Ethernet", CABLE_PNP),
            adapter(WIFI, "Wi-Fi", WIFI_PNP),
        ]);
        install(&h, CABLE_PNP, "0001", CABLE);
        install(&h, WIFI_PNP, "0002", WIFI);
        store(&h, "0002", "PnPCapabilities", RawValue::dword(0x100));
        let before = h.fake.snapshot();
        assert_eq!(state(&h, &POWER_SAVING), TweakState::Default);

        h.engine.apply(POWER_SAVING.id).unwrap();
        assert_eq!(stored(&h, "0001", "PnPCapabilities"), Some(RawValue::dword(0x18)));
        assert_eq!(stored(&h, "0002", "PnPCapabilities"), Some(RawValue::dword(0x118)));
        assert!(
            h.sys.effects().is_empty(),
            "no adapter restarts: the PC's restart does it"
        );
        assert_eq!(state(&h, &POWER_SAVING), TweakState::Applied);

        h.engine.revert(POWER_SAVING.id).unwrap();
        assert_eq!(h.fake.snapshot(), before);
        assert!(h.sys.effects().is_empty());

        // One bit of the two is not enough.
        store(&h, "0001", "PnPCapabilities", RawValue::dword(0x18));
        store(&h, "0002", "PnPCapabilities", RawValue::dword(0x10));
        assert_eq!(state(&h, &POWER_SAVING), TweakState::Default);
        store(&h, "0002", "PnPCapabilities", RawValue::dword(0x118));
        assert_eq!(state(&h, &POWER_SAVING), TweakState::Foreign);
    }

    #[test]
    fn an_adapter_removed_before_undo_is_noted_and_the_rest_put_back() {
        let mut h = pc(INTERRUPT_MODERATION);
        store(&h, "0002", IM, RawValue::sz("1"));
        h.engine.apply(INTERRUPT_MODERATION.id).unwrap();
        assert_eq!(h.sys.effects(), [restart(CABLE), restart(WIFI)]);

        h.fake.remove_key_external(Hive::LocalMachine, &class_key("0002"));
        h.sys.set_adapters(vec![adapter(CABLE, "Ethernet", CABLE_PNP)]);
        h.engine.revert(INTERRUPT_MODERATION.id).unwrap();
        assert_eq!(stored(&h, "0001", IM), Some(RawValue::sz("1")));
        assert!(
            !h.fake.key_exists_for_test(Hive::LocalMachine, &class_key("0002")),
            "not created again"
        );
        assert_eq!(h.sys.effects(), [restart(CABLE), restart(WIFI), restart(CABLE)]);
        assert!(h.engine.applied_tweak_ids().is_empty());
        assert!(h
            .engine
            .journal_view()
            .records
            .iter()
            .any(|r| matches!(r, Record::Note(n) if n.text.contains("no longer exists"))));
    }

    #[test]
    fn each_tool_may_write_only_its_own_value_in_a_network_driver_key() {
        for t in [
            INTERRUPT_MODERATION,
            FLOW_CONTROL,
            ENERGY_EFFICIENT_ETHERNET,
            POWER_SAVING,
        ] {
            let touches = t.touches();
            assert_eq!(touches.len(), 1, "{}", t.id);
            assert_eq!(touches[0].root, RegRoot::LocalMachine);
            assert_eq!(touches[0].key, class_key("*"));
            assert_eq!(touches[0].values, [t.value_name.to_owned()]);
            let m = t.metadata();
            assert_eq!(m.category, "network");
            let keyword = t.value_name.starts_with('*');
            assert_eq!(m.requires_reboot, !keyword, "{}", t.id);
            assert_eq!(
                t.effect_targets(),
                if keyword { vec![restart("*")] } else { Vec::new() },
                "{}",
                t.id
            );
        }
    }
}

/// CATALOGUE H20 (tweaks/nvidia.rs).
mod nvidia_tools {
    use super::*;
    use crate::error::EngineError;
    use crate::journal::Record;
    use crate::system::{SysItem, SysState};
    use crate::tweaks::nvidia::{
        self, LOW_LATENCY_STATE, LOW_LATENCY_ULTRA, POWER_MANAGEMENT, PRERENDER_LIMIT, SHADER_CACHE, SHADER_CACHE_SIZE,
    };
    use crate::types::BlockedCode;

    fn setting(id: u32) -> SysItem {
        SysItem::NvidiaSetting {
            profile: String::new(),
            setting: id,
        }
    }

    const fn v(value: u32) -> SysState {
        SysState::Dword { value }
    }

    fn state(h: &Harness, id: &str) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|t| t.metadata.id == id)
            .unwrap()
            .state
    }

    fn pc(nvidia: bool) -> Harness {
        let h = Harness::new(nvidia::all());
        h.sys.set_nvidia(nvidia);
        h
    }

    #[test]
    fn low_latency_sets_its_three_values_and_undo_returns_them_to_the_driver_default() {
        let mut h = pc(true);
        assert_eq!(state(&h, "nvidia.lowlatency"), TweakState::Default);
        h.engine.apply("nvidia.lowlatency").unwrap();
        assert_eq!(h.sys.get(&setting(LOW_LATENCY_STATE)), v(2));
        assert_eq!(h.sys.get(&setting(LOW_LATENCY_ULTRA)), v(1));
        assert_eq!(h.sys.get(&setting(PRERENDER_LIMIT)), v(1));
        assert_eq!(state(&h, "nvidia.lowlatency"), TweakState::Applied);

        h.engine.revert("nvidia.lowlatency").unwrap();
        for id in [LOW_LATENCY_STATE, LOW_LATENCY_ULTRA, PRERENDER_LIMIT] {
            assert_eq!(h.sys.get(&setting(id)), SysState::Absent, "0x{id:08X}");
        }
        assert_eq!(state(&h, "nvidia.lowlatency"), TweakState::Default);
    }

    #[test]
    fn a_value_set_in_control_panel_comes_back_on_undo() {
        let mut h = pc(true);
        // Optimal power, chosen by the user in Control Panel.
        h.sys.set(&setting(POWER_MANAGEMENT), v(5));
        h.engine.apply("nvidia.powermax").unwrap();
        assert_eq!(h.sys.get(&setting(POWER_MANAGEMENT)), v(1));
        h.engine.revert_all();
        assert_eq!(h.sys.get(&setting(POWER_MANAGEMENT)), v(5));
    }

    #[test]
    fn shader_cache_unlimited_turns_a_cache_turned_off_back_on_and_undo_turns_it_off_again() {
        let mut h = pc(true);
        h.sys.set(&setting(SHADER_CACHE), v(0));
        h.engine.apply("nvidia.shadercache").unwrap();
        assert_eq!(h.sys.get(&setting(SHADER_CACHE)), v(1));
        assert_eq!(h.sys.get(&setting(SHADER_CACHE_SIZE)), v(0xFFFF_FFFF));
        assert_eq!(state(&h, "nvidia.shadercache"), TweakState::Applied);

        h.engine.revert("nvidia.shadercache").unwrap();
        assert_eq!(h.sys.get(&setting(SHADER_CACHE)), v(0), "off, as the user had it");
        assert_eq!(
            h.sys.get(&setting(SHADER_CACHE_SIZE)),
            SysState::Absent,
            "the driver's default size"
        );
    }

    #[test]
    fn already_set_this_way_reads_as_already_optimized() {
        let h = pc(true);
        h.sys.set(&setting(POWER_MANAGEMENT), v(1));
        assert_eq!(state(&h, "nvidia.powermax"), TweakState::Foreign);
        h.sys.set(&setting(LOW_LATENCY_STATE), v(2));
        assert_eq!(
            state(&h, "nvidia.lowlatency"),
            TweakState::Default,
            "only part of it is set"
        );
    }

    #[test]
    fn a_pc_without_an_nvidia_card_is_not_offered_any_and_nothing_is_written() {
        let mut h = pc(false);
        for t in nvidia::all() {
            match state(&h, t.id()) {
                TweakState::Blocked { reason } => {
                    assert_eq!(reason.code, BlockedCode::HardwareUnsupported);
                    assert_eq!(reason.message, "This PC has no NVIDIA graphics card.");
                }
                other => panic!("{}: {other:?}", t.id()),
            }
            assert!(matches!(h.engine.apply(t.id()), Err(EngineError::Blocked { .. })));
        }
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn undo_after_the_nvidia_driver_is_removed_finishes_and_says_so() {
        let mut h = pc(true);
        h.engine.apply("nvidia.powermax").unwrap();
        h.sys.set_nvidia(false);
        let results = h.engine.revert_all();
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        assert!(h.engine.applied_tweak_ids().is_empty());
        assert!(h
            .engine
            .journal_view()
            .records
            .iter()
            .any(|r| matches!(r, Record::Note(n) if n.text.contains("NVIDIA driver is no longer on this PC"))));
    }

    #[test]
    fn each_tool_changes_only_its_own_settings() {
        for t in nvidia::all() {
            let targets = t.system_targets();
            assert!(!targets.is_empty());
            for target in targets {
                assert!(
                    matches!(&target, SysItem::NvidiaSetting { profile, .. } if profile.is_empty()),
                    "{}: {target:?}",
                    t.id()
                );
            }
            assert!(t.touches().is_empty(), "{} writes no registry", t.id());
        }
    }
}

/// CATALOGUE H21 (tweaks/amd.rs).
mod amd_tools {
    use super::*;
    use crate::adlx::{anti_lag_level, vsync, AmdGpu};
    use crate::error::EngineError;
    use crate::journal::Record;
    use crate::system::{SysItem, SysState};
    use crate::tweaks::amd;
    use crate::types::BlockedCode;

    const IGPU: &str = r"PCI\VEN_1002&DEV_164E&SUBSYS_88771043&REV_C1";
    const RADEON: &str = r"PCI\VEN_1002&DEV_73BF&SUBSYS_23181458&REV_C1";

    fn card(id: &str) -> AmdGpu {
        AmdGpu {
            id: id.into(),
            name: format!("AMD Radeon {id}"),
        }
    }

    fn setting(gpu: &str, key: &str) -> SysItem {
        SysItem::AmdSetting {
            gpu: gpu.into(),
            setting: key.into(),
        }
    }

    const fn v(value: u32) -> SysState {
        SysState::Dword { value }
    }

    fn state(h: &Harness, id: &str) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|t| t.metadata.id == id)
            .unwrap()
            .state
    }

    /// A PC with these AMD cards, each with every setting at AMD's default
    /// (Anti-Lag off at plain Anti-Lag, Chill off, vertical refresh off
    /// unless the game asks).
    fn pc(cards: &[&str]) -> Harness {
        let h = Harness::new(amd::all());
        h.sys.set_amd_gpus(cards.iter().map(|c| card(c)).collect());
        for c in cards {
            h.sys.set(&setting(c, "anti_lag"), v(0));
            h.sys.set(&setting(c, "anti_lag_level"), v(anti_lag_level::ANTI_LAG));
            h.sys.set(&setting(c, "chill"), v(0));
            h.sys.set(
                &setting(c, "wait_for_vertical_refresh"),
                v(vsync::OFF_UNLESS_APP_SPECIFIES),
            );
        }
        h
    }

    /// The AMD settings the journal records changing, in order.
    fn changed(h: &Harness) -> Vec<String> {
        h.engine
            .journal_view()
            .records
            .iter()
            .filter_map(|r| match r {
                Record::Change(c) => match &c.item {
                    SysItem::AmdSetting { gpu, setting } => Some(format!("{setting} {gpu}")),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    #[test]
    fn anti_lag_never_turns_on_anti_lag_next_and_undo_puts_chill_and_the_level_back() {
        let mut h = pc(&[RADEON]);
        h.sys.set(&setting(RADEON, "anti_lag_level"), v(anti_lag_level::NEXT));
        h.sys.set(&setting(RADEON, "chill"), v(1));

        h.engine.apply("amd.antilag").unwrap();
        assert_eq!(
            h.sys.get(&setting(RADEON, "anti_lag_level")),
            v(anti_lag_level::ANTI_LAG)
        );
        assert_eq!(h.sys.get(&setting(RADEON, "chill")), v(0));
        assert_eq!(h.sys.get(&setting(RADEON, "anti_lag")), v(1));
        assert_eq!(
            changed(&h),
            [
                format!("anti_lag_level {RADEON}"),
                format!("chill {RADEON}"),
                format!("anti_lag {RADEON}")
            ],
            "the level and Chill are set before Anti-Lag is turned on"
        );
        assert_eq!(state(&h, "amd.antilag"), TweakState::Applied);

        h.engine.revert("amd.antilag").unwrap();
        assert_eq!(h.sys.get(&setting(RADEON, "anti_lag")), v(0));
        assert_eq!(h.sys.get(&setting(RADEON, "chill")), v(1));
        assert_eq!(h.sys.get(&setting(RADEON, "anti_lag_level")), v(anti_lag_level::NEXT));
    }

    #[test]
    fn anti_lag_next_on_is_not_already_optimized() {
        let h = pc(&[RADEON]);
        h.sys.set(&setting(RADEON, "anti_lag"), v(1));
        h.sys.set(&setting(RADEON, "anti_lag_level"), v(anti_lag_level::NEXT));
        assert_eq!(state(&h, "amd.antilag"), TweakState::Default);
        h.sys
            .set(&setting(RADEON, "anti_lag_level"), v(anti_lag_level::ANTI_LAG));
        assert_eq!(state(&h, "amd.antilag"), TweakState::Foreign);
    }

    #[test]
    fn a_driver_without_levels_or_chill_still_gets_anti_lag() {
        let mut h = pc(&[RADEON]);
        h.sys.set(&setting(RADEON, "anti_lag_level"), SysState::Absent);
        h.sys.set(&setting(RADEON, "chill"), SysState::Absent);
        h.engine.apply("amd.antilag").unwrap();
        assert_eq!(h.sys.get(&setting(RADEON, "anti_lag")), v(1));
        assert_eq!(changed(&h), [format!("anti_lag {RADEON}")]);
        assert_eq!(state(&h, "amd.antilag"), TweakState::Applied);
    }

    #[test]
    fn both_cards_are_set_and_undo_puts_each_back() {
        let mut h = pc(&[IGPU, RADEON]);
        // Chosen by the user in AMD Software for the Radeon card only.
        h.sys
            .set(&setting(RADEON, "wait_for_vertical_refresh"), v(vsync::ALWAYS_ON));
        assert_eq!(state(&h, "amd.vsyncoff"), TweakState::Default);

        h.engine.apply("amd.vsyncoff").unwrap();
        for c in [IGPU, RADEON] {
            assert_eq!(
                h.sys.get(&setting(c, "wait_for_vertical_refresh")),
                v(vsync::ALWAYS_OFF)
            );
            assert_eq!(
                h.sys.get(&setting(c, "anti_lag")),
                v(0),
                "the other setting is left alone"
            );
        }
        assert_eq!(state(&h, "amd.vsyncoff"), TweakState::Applied);

        h.engine.revert("amd.vsyncoff").unwrap();
        assert_eq!(
            h.sys.get(&setting(IGPU, "wait_for_vertical_refresh")),
            v(vsync::OFF_UNLESS_APP_SPECIFIES)
        );
        assert_eq!(
            h.sys.get(&setting(RADEON, "wait_for_vertical_refresh")),
            v(vsync::ALWAYS_ON)
        );
        assert_eq!(state(&h, "amd.vsyncoff"), TweakState::Default);
    }

    #[test]
    fn a_card_without_the_setting_is_left_out() {
        let mut h = pc(&[IGPU, RADEON]);
        h.sys.set(&setting(IGPU, "anti_lag"), SysState::Absent);
        h.engine.apply("amd.antilag").unwrap();
        assert_eq!(h.sys.get(&setting(RADEON, "anti_lag")), v(1));
        assert_eq!(h.sys.get(&setting(IGPU, "anti_lag")), SysState::Absent);
        assert_eq!(
            state(&h, "amd.antilag"),
            TweakState::Applied,
            "every card that has it is on"
        );
    }

    #[test]
    fn no_card_with_the_setting_says_so() {
        let mut h = pc(&[IGPU]);
        h.sys.set(&setting(IGPU, "anti_lag"), SysState::Absent);
        match state(&h, "amd.antilag") {
            TweakState::Blocked { reason } => {
                assert_eq!(reason.code, BlockedCode::HardwareUnsupported);
                assert_eq!(reason.message, "This PC's AMD graphics does not have Radeon Anti-Lag.");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            h.engine.apply("amd.antilag"),
            Err(EngineError::Blocked { .. })
        ));
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn already_set_this_way_reads_as_already_optimized() {
        let h = pc(&[IGPU, RADEON]);
        h.sys.set(&setting(RADEON, "anti_lag"), v(1));
        assert_eq!(
            state(&h, "amd.antilag"),
            TweakState::Default,
            "the other card is still off"
        );
        h.sys.set(&setting(IGPU, "anti_lag"), v(1));
        assert_eq!(state(&h, "amd.antilag"), TweakState::Foreign);
    }

    #[test]
    fn a_pc_without_an_amd_card_is_not_offered_any_and_nothing_is_written() {
        let mut h = pc(&[]);
        for t in amd::all() {
            match state(&h, t.id()) {
                TweakState::Blocked { reason } => {
                    assert_eq!(reason.code, BlockedCode::HardwareUnsupported);
                    assert_eq!(reason.message, "This PC has no AMD graphics card.");
                }
                other => panic!("{}: {other:?}", t.id()),
            }
            assert!(matches!(h.engine.apply(t.id()), Err(EngineError::Blocked { .. })));
        }
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn a_failed_listing_is_an_error_not_a_missing_card() {
        let h = pc(&[RADEON]);
        h.sys.fail_listing(true);
        match state(&h, "amd.antilag") {
            TweakState::Unknown { detail } => assert!(detail.contains("did not answer"), "{detail}"),
            other => panic!("a failed listing must not read as no card: {other:?}"),
        }
    }

    #[test]
    fn undo_after_a_card_is_removed_puts_back_the_others_and_says_so() {
        let mut h = pc(&[IGPU, RADEON]);
        h.engine.apply("amd.antilag").unwrap();
        h.sys.set_amd_gpus(vec![card(IGPU)]);
        let results = h.engine.revert_all();
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        assert!(h.engine.applied_tweak_ids().is_empty());
        assert_eq!(h.sys.get(&setting(IGPU, "anti_lag")), v(0));
        assert!(h.engine.journal_view().records.iter().any(
            |r| matches!(r, Record::Note(n) if n.text.contains(RADEON) && n.text.contains("no longer on this PC"))
        ));
    }

    #[test]
    fn undo_after_amds_driver_is_removed_finishes() {
        let mut h = pc(&[RADEON]);
        h.engine.apply("amd.vsyncoff").unwrap();
        h.sys.set_amd_gpus(Vec::new());
        let results = h.engine.revert_all();
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn each_tool_changes_only_its_own_settings() {
        let keys = |id: &str| -> Vec<String> {
            amd::all()
                .into_iter()
                .find(|t| t.id() == id)
                .unwrap()
                .system_targets()
                .into_iter()
                .map(|t| match t {
                    SysItem::AmdSetting { gpu, setting } if gpu == "*" => setting,
                    other => panic!("{id}: {other:?}"),
                })
                .collect()
        };
        assert_eq!(keys("amd.antilag"), ["anti_lag_level", "chill", "anti_lag"]);
        assert_eq!(keys("amd.vsyncoff"), ["wait_for_vertical_refresh"]);
        assert!(amd::all().iter().all(|t| t.touches().is_empty()), "no registry writes");
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

/// CATALOGUE step 5: Gaming Mode's session changes (tweaks/session.rs),
/// made when a game starts and put back when it closes.
mod play_session {
    use std::sync::Arc;

    use crate::registry::{fake::FakeRegistry, Hive};
    use crate::system::{FakeSystem, ServiceStart, SysItem, SysState};
    use crate::testutil::{build_engine_with_system, Harness};
    use crate::tweaks::session::{NOTIFICATIONS_ID, PUSH_NOTIFICATIONS, SEARCH_PAUSE_ID};
    use crate::types::{RawValue, Tier};

    fn wsearch() -> SysItem {
        SysItem::Service { name: "WSearch".into() }
    }

    fn running(on: bool) -> SysState {
        SysState::Service {
            start: ServiceStart::DelayedAutomatic,
            running: on,
        }
    }

    fn toast(h: &Harness) -> Option<u32> {
        h.fake
            .read_value_for_test(Hive::CurrentUser, PUSH_NOTIFICATIONS, "ToastEnabled")
            .and_then(|v| v.as_dword())
    }

    /// Search indexing installed and running; Gaming Mode on or off.
    fn pc(gaming_mode: bool) -> Harness {
        let mut h = Harness::new(Vec::new());
        h.fake.set_external(
            Hive::LocalMachine,
            r"SYSTEM\CurrentControlSet\Services\WSearch",
            "Start",
            RawValue::dword(2),
        );
        h.sys.set(&wsearch(), running(true));
        let mut s = h.engine.settings();
        s.gaming_mode = gaming_mode;
        h.engine.set_settings(s).unwrap();
        h
    }

    #[test]
    fn with_gaming_mode_off_a_game_starting_changes_nothing() {
        let mut h = pc(false);
        let before = h.fake.snapshot();
        let steps = h.engine.start_play_session();
        assert!(steps.is_empty(), "{steps:?}");
        assert_eq!(h.fake.snapshot(), before);
        assert_eq!(h.sys.get(&wsearch()), running(true));
        assert!(!h.engine.play_session_open());
    }

    #[test]
    fn a_game_starting_quiets_notifications_and_pauses_indexing_and_closing_it_puts_both_back() {
        let mut h = pc(true);
        let steps = h.engine.start_play_session();
        assert!(steps.iter().all(|s| s.made && s.error.is_none()), "{steps:?}");
        assert_eq!(steps.len(), 2);
        assert_eq!(toast(&h), Some(0));
        assert_eq!(h.sys.get(&wsearch()), running(false), "stopped, start type kept");
        assert!(h.engine.play_session_open());
        // Listed in Backups while in effect.
        let backups: Vec<String> = h
            .engine
            .journal_view()
            .applied
            .into_iter()
            .map(|a| a.tweak_id)
            .collect();
        assert!(backups.iter().any(|id| id == NOTIFICATIONS_ID) && backups.iter().any(|id| id == SEARCH_PAUSE_ID));

        let ended = h.engine.end_play_session();
        assert!(ended.iter().all(|r| r.ok), "{ended:?}");
        assert_eq!(toast(&h), None, "the value did not exist before, so it is gone again");
        assert_eq!(h.sys.get(&wsearch()), running(true));
        assert!(!h.engine.play_session_open());
        assert!(h.engine.journal_view().applied.is_empty());
    }

    #[test]
    fn what_is_already_quiet_or_stopped_is_left_alone() {
        let mut h = pc(true);
        h.fake.set_external(
            Hive::CurrentUser,
            PUSH_NOTIFICATIONS,
            "ToastEnabled",
            RawValue::dword(0),
        );
        h.sys.set(&wsearch(), running(false));
        let steps = h.engine.start_play_session();
        assert!(steps.iter().all(|s| !s.made && s.error.is_none()), "{steps:?}");
        assert!(!h.engine.play_session_open());
        assert!(h.engine.end_play_session().is_empty());
        assert_eq!(toast(&h), Some(0));
    }

    #[test]
    fn without_a_restore_point_nothing_is_changed_and_the_reason_is_given() {
        let fake = Arc::new(FakeRegistry::new());
        let sys = Arc::new(FakeSystem::new());
        sys.set(&wsearch(), running(true));
        let dir = tempfile::tempdir().unwrap();
        let mut engine = build_engine_with_system(&fake, &sys, dir.path(), Vec::new(), false, Tier::Ultimate);
        let mut s = engine.settings();
        s.gaming_mode = true;
        engine.set_settings(s).unwrap();
        let before = fake.snapshot();
        let steps = engine.start_play_session();
        assert!(!steps.is_empty());
        assert!(steps
            .iter()
            .all(|s| !s.made && s.error.as_deref().is_some_and(|e| e.contains("restore point"))));
        assert_eq!(fake.snapshot(), before);
        assert_eq!(sys.get(&wsearch()), running(true));
    }

    #[test]
    fn a_plan_without_gaming_mode_changes_nothing() {
        let fake = Arc::new(FakeRegistry::new());
        let sys = Arc::new(FakeSystem::new());
        let dir = tempfile::tempdir().unwrap();
        let mut engine = build_engine_with_system(&fake, &sys, dir.path(), Vec::new(), true, Tier::Free);
        let mut s = engine.settings();
        s.gaming_mode = true;
        engine.set_settings(s).unwrap();
        let steps = engine.start_play_session();
        assert!(steps.iter().all(|s| !s.made && s.error.is_some()), "{steps:?}");
        assert!(fake.snapshot().is_empty());
    }

    #[test]
    fn turning_gaming_mode_off_during_a_game_puts_its_changes_back() {
        let mut h = pc(true);
        h.engine.start_play_session();
        let mut s = h.engine.settings();
        s.gaming_mode = false;
        h.engine.set_settings(s).unwrap();
        assert!(!h.engine.play_session_open());
        assert_eq!(toast(&h), None);
        assert_eq!(h.sys.get(&wsearch()), running(true));
    }

    #[test]
    fn a_session_left_open_by_a_crash_is_put_back_after_a_restart() {
        let mut h = pc(true);
        h.engine.start_play_session();
        h.restart(Vec::new());
        assert!(h.engine.play_session_open(), "the journal still has it");
        let ended = h.engine.end_play_session();
        assert_eq!(ended.len(), 2, "{ended:?}");
        assert_eq!(toast(&h), None);
        assert_eq!(h.sys.get(&wsearch()), running(true));
    }

    #[test]
    fn session_changes_are_not_listed_as_tools_but_undo_all_reaches_them() {
        let mut h = pc(true);
        h.engine.start_play_session();
        assert!(h.engine.list().unwrap().is_empty());
        let results = h.engine.revert_all();
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        assert!(!h.engine.play_session_open());
        assert_eq!(toast(&h), None);
    }
}

mod windows11_tools {
    use crate::hardware::{HardwareReport, OsInfo};
    use crate::probe::Probe;
    use crate::registry::Hive;
    use crate::testutil::Harness;
    use crate::tweaks::registry_values::{list_get, list_set, FULL_CONTEXT_MENU, NEW_MENU_SERVER, WINDOWED_GAMES};
    use crate::types::{BlockedCode, PredicateOutcome, RawValue, SystemEnv, Tweak, TweakState};

    const DIRECTX: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";
    const GLOBAL: &str = "DirectXUserGlobalSettings";

    fn status(h: &Harness, id: &str) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|v| v.metadata.id == id)
            .unwrap()
            .state
    }

    fn global(h: &Harness) -> Option<String> {
        h.fake
            .read_value_for_test(Hive::CurrentUser, DIRECTX, GLOBAL)
            .and_then(|v| v.as_sz())
    }

    fn windowed(existing: Option<&str>) -> Harness {
        let h = Harness::new(vec![Box::new(WINDOWED_GAMES)]);
        if let Some(list) = existing {
            h.fake
                .set_external(Hive::CurrentUser, DIRECTX, GLOBAL, RawValue::sz(list));
        }
        h
    }

    fn unknown<T>() -> Probe<T> {
        Probe::unknown("not needed here")
    }

    fn env(build: Option<u32>) -> SystemEnv {
        SystemEnv {
            hardware: build.map(|build| HardwareReport {
                os: Probe::yes(OsInfo {
                    build,
                    caption: "Windows".into(),
                    is_server: false,
                }),
                cpu: unknown(),
                memory: unknown(),
                gpus: unknown(),
                gpu_drivers: unknown(),
                boot_disk: unknown(),
                display: unknown(),
                is_laptop: unknown(),
                rig_class: unknown(),
            }),
            ..SystemEnv::default()
        }
    }

    #[test]
    fn list_entries_are_set_in_place_and_every_other_entry_is_kept() {
        assert_eq!(
            list_get("A=1;swapeffectupgradeenable=0;", "SwapEffectUpgradeEnable"),
            Some("0")
        );
        assert_eq!(list_get("A=1;", "SwapEffectUpgradeEnable"), None);
        assert_eq!(list_set("", "S", "1"), "S=1;");
        assert_eq!(list_set("A=1;B=2", "S", "1"), "A=1;B=2;S=1;");
        assert_eq!(list_set("A=1;s=0;B=2;", "S", "1"), "A=1;S=1;B=2;");
        // A repeated entry is folded into one; an entry without `=` is kept.
        assert_eq!(list_set("S=0;odd;S=0;", "S", "1"), "S=1;odd;");
    }

    #[test]
    fn windowed_games_adds_its_entry_and_undo_puts_the_list_back_exactly() {
        let mut h = windowed(Some("VRROptimizeEnable=0;AutoHDREnable=1;"));
        assert_eq!(status(&h, WINDOWED_GAMES.0.id), TweakState::Default);
        h.engine.apply(WINDOWED_GAMES.0.id).unwrap();
        assert_eq!(
            global(&h).as_deref(),
            Some("VRROptimizeEnable=0;AutoHDREnable=1;SwapEffectUpgradeEnable=1;")
        );
        assert_eq!(status(&h, WINDOWED_GAMES.0.id), TweakState::Applied);
        h.engine.revert(WINDOWED_GAMES.0.id).unwrap();
        assert_eq!(global(&h).as_deref(), Some("VRROptimizeEnable=0;AutoHDREnable=1;"));
    }

    #[test]
    fn windowed_games_turned_off_by_the_user_is_turned_on_in_place() {
        let mut h = windowed(Some("SwapEffectUpgradeEnable=0;VRROptimizeEnable=1;"));
        h.engine.apply(WINDOWED_GAMES.0.id).unwrap();
        assert_eq!(
            global(&h).as_deref(),
            Some("SwapEffectUpgradeEnable=1;VRROptimizeEnable=1;")
        );
    }

    #[test]
    fn windowed_games_already_on_is_already_optimized_and_none_set_is_not_applied() {
        assert_eq!(
            status(&windowed(Some("SwapEffectUpgradeEnable=1;")), WINDOWED_GAMES.0.id),
            TweakState::Foreign
        );
        let mut h = windowed(None);
        assert_eq!(status(&h, WINDOWED_GAMES.0.id), TweakState::Default);
        h.engine.apply(WINDOWED_GAMES.0.id).unwrap();
        assert_eq!(global(&h).as_deref(), Some("SwapEffectUpgradeEnable=1;"));
        h.engine.revert(WINDOWED_GAMES.0.id).unwrap();
        assert_eq!(global(&h), None);
    }

    #[test]
    fn the_full_menu_registers_an_empty_server_and_undo_removes_the_keys_it_made() {
        let mut h = Harness::new(vec![Box::new(FULL_CONTEXT_MENU)]);
        let other = r"Software\Classes\CLSID\{00000000-0000-0000-0000-000000000001}";
        h.fake
            .set_external(Hive::CurrentUser, other, "", RawValue::sz("someone else's"));
        assert_eq!(status(&h, FULL_CONTEXT_MENU.0.id), TweakState::Default);

        let written = h.engine.apply(FULL_CONTEXT_MENU.0.id).unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(
            h.fake
                .read_value_for_test(Hive::CurrentUser, NEW_MENU_SERVER, "")
                .and_then(|v| v.as_sz())
                .as_deref(),
            Some("")
        );
        assert_eq!(status(&h, FULL_CONTEXT_MENU.0.id), TweakState::Applied);

        h.engine.revert(FULL_CONTEXT_MENU.0.id).unwrap();
        let class = NEW_MENU_SERVER.rsplit_once('\\').unwrap().0;
        assert!(
            !h.fake.key_exists_for_test(Hive::CurrentUser, class),
            "the class key is gone"
        );
        assert!(
            h.fake.key_exists_for_test(Hive::CurrentUser, other),
            "other classes are kept"
        );
        assert_eq!(status(&h, FULL_CONTEXT_MENU.0.id), TweakState::Default);
    }

    #[test]
    fn windows_11_settings_are_not_offered_on_windows_10() {
        for t in [&FULL_CONTEXT_MENU as &dyn Tweak, &WINDOWED_GAMES] {
            match t.evaluate_predicate(&env(Some(19045))) {
                PredicateOutcome::Block(reason) => assert_eq!(reason.code, BlockedCode::OsVersionUnsupported),
                PredicateOutcome::Allow => panic!("{} offered on Windows 10", t.id()),
            }
            assert!(matches!(
                t.evaluate_predicate(&env(Some(22631))),
                PredicateOutcome::Allow
            ));
            assert!(
                matches!(t.evaluate_predicate(&env(None)), PredicateOutcome::Allow),
                "an unknown build is not rounded to Windows 10"
            );
        }
    }
}

mod startup_apps {
    use std::fs;

    use crate::error::EngineError;
    use crate::registry::Hive;
    use crate::startup::{StartupFolders, StartupList, PACKAGES};
    use crate::testutil::Harness;
    use crate::tweaks::startup::{switched_off, StartupSource, StartupToggle, STORE_TASKS};
    use crate::types::{BlockedCode, RawValue, Tweak, TweakState};

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN_32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED_RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    fn on_value() -> RawValue {
        RawValue {
            vtype: 3,
            bytes: vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        }
    }

    /// Discord and Steam (turned off in Task Manager) for the user, Windows
    /// Security's icon for everyone, a 32-bit launcher, and a shortcut in the
    /// user's Startup folder.
    fn pc() -> (Harness, tempfile::TempDir, StartupFolders) {
        let h = Harness::new(Vec::new());
        let set = |hive, key, name: &str, value| h.fake.set_external(hive, key, name, value);
        set(
            Hive::CurrentUser,
            RUN,
            "Discord",
            RawValue::sz(r#""C:\Discord\Update.exe" --processStart Discord.exe"#),
        );
        set(
            Hive::CurrentUser,
            RUN,
            "Steam",
            RawValue::sz(r#""C:\Steam\steam.exe" -silent"#),
        );
        set(Hive::CurrentUser, APPROVED_RUN, "Steam", StartupToggle::off_for_test());
        set(
            Hive::LocalMachine,
            RUN,
            "SecurityHealth",
            RawValue::sz(r"%windir%\system32\SecurityHealthSystray.exe"),
        );
        set(
            Hive::LocalMachine,
            RUN_32,
            "Launcher32",
            RawValue::sz(r"C:\Launcher\launcher.exe"),
        );
        let dir = tempfile::tempdir().unwrap();
        // Store apps: Slack starts by default, Xbox does not until turned on,
        // a work app is set by policy, and a package without a manifest.
        let package = |full: &str, display: &str, task: Option<&str>| {
            let key = format!(r"{PACKAGES}\{full}");
            let root = dir.path().join(full);
            if let Some(task) = task {
                fs::create_dir_all(&root).unwrap();
                fs::write(
                    root.join("AppxManifest.xml"),
                    format!(r#"<Package><Extensions><uap5:Extension Category="windows.startupTask">{task}</uap5:Extension></Extensions></Package>"#),
                )
                .unwrap();
            }
            h.fake.set_external(
                Hive::CurrentUser,
                &key,
                "PackageRootFolder",
                RawValue::sz(&root.to_string_lossy()),
            );
            h.fake
                .set_external(Hive::CurrentUser, &key, "DisplayName", RawValue::sz(display));
        };
        package(
            "91750D7E.Slack_4.41.105.0_x64__8she8kybcnzg4",
            "Slack",
            Some(r#"<uap5:StartupTask TaskId="SlackStartup" Enabled="true" DisplayName="Slack"/>"#),
        );
        package(
            "Microsoft.GamingApp_2410.1001.20.0_x64__8wekyb3d8bbwe",
            "Xbox",
            Some(r#"<uap5:StartupTask TaskId="GamingAppStartup" Enabled="false" DisplayName="ms-resource:Name"/>"#),
        );
        package(
            "Corp.Tool_1.0.0.0_x64__corp",
            "Corp Tool",
            Some(r#"<uap5:StartupTask TaskId="CorpTask" Enabled="true"/>"#),
        );
        h.fake.set_external(
            Hive::CurrentUser,
            &format!(r"{STORE_TASKS}\Corp.Tool_corp\CorpTask"),
            "State",
            RawValue::dword(3),
        );
        package(
            "Microsoft.VCLibs.140.00_14.0.33519.0_x64__8wekyb3d8bbwe",
            "VC Libs",
            None,
        );
        let user = dir.path().join("user");
        fs::create_dir_all(&user).unwrap();
        fs::write(user.join("OneNote.lnk"), b"").unwrap();
        fs::write(user.join("desktop.ini"), b"").unwrap();
        let folders = StartupFolders {
            user: Some(user),
            machine: Some(dir.path().join("no such folder")),
        };
        (h, dir, folders)
    }

    fn app<'a>(list: &'a StartupList, name: &str) -> &'a crate::startup::StartupApp {
        list.apps
            .iter()
            .find(|a| a.name == name)
            .unwrap_or_else(|| panic!("{name} not listed"))
    }

    #[test]
    fn every_entry_is_listed_with_its_switch() {
        let (mut h, _dir, folders) = pc();
        let list = h.engine.startup_apps(&folders);
        assert!(list.problems.is_empty(), "{:?}", list.problems);
        let names: Vec<&str> = list.apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Corp Tool",
                "Discord",
                "Launcher32",
                "OneNote",
                "SecurityHealth",
                "Slack",
                "Steam",
                "Xbox"
            ]
        );

        let discord = app(&list, "Discord");
        assert_eq!(discord.tweak.state, TweakState::Default);
        assert_eq!(discord.source, StartupSource::UserRun);
        assert!(discord.command.as_deref().unwrap().contains("Update.exe"));
        assert_eq!(
            app(&list, "Steam").tweak.state,
            TweakState::Foreign,
            "turned off in Task Manager"
        );
        assert_eq!(app(&list, "Launcher32").source, StartupSource::MachineRun32);
        let onenote = app(&list, "OneNote");
        assert_eq!(
            (onenote.source, onenote.command.as_deref()),
            (StartupSource::UserFolder, None)
        );
        match &app(&list, "SecurityHealth").tweak.state {
            TweakState::Blocked { reason } => assert_eq!(reason.code, BlockedCode::ProtectedProgram),
            other => panic!("Windows Security offered: {other:?}"),
        }
    }

    const SLACK_TASK: &str = r"91750d7e.slack_8she8kybcnzg4\SlackStartup";
    const XBOX_TASK: &str = r"microsoft.gamingapp_8wekyb3d8bbwe\GamingAppStartup";

    fn task_state(h: &Harness, task: &str) -> Option<RawValue> {
        h.fake
            .read_value_for_test(Hive::CurrentUser, &format!(r"{STORE_TASKS}\{task}"), "State")
    }

    #[test]
    fn store_apps_are_listed_from_their_manifests() {
        let (mut h, _dir, folders) = pc();
        let list = h.engine.startup_apps(&folders);
        let slack = app(&list, "Slack");
        assert_eq!(slack.source, StartupSource::StoreApp);
        assert_eq!(slack.tweak.metadata.id, format!("startup.store_app:{SLACK_TASK}"));
        assert_eq!(
            slack.tweak.state,
            TweakState::Default,
            "enabled in its manifest, so it starts"
        );
        assert_eq!(slack.command, None);
        // Its task's name is a resource only Windows can read here, so the
        // package's name is shown.
        let xbox = app(&list, "Xbox");
        assert_eq!(xbox.tweak.state, TweakState::Foreign, "not enabled until turned on");
        assert_eq!(xbox.turn_on.state, TweakState::Default, "offered");
        let corp = app(&list, "Corp Tool");
        for view in [&corp.tweak, &corp.turn_on] {
            match &view.state {
                TweakState::Blocked { reason } => assert_eq!(reason.code, BlockedCode::SetByPolicy),
                other => panic!("a task set by policy offered: {other:?}"),
            }
        }
        assert!(list.apps.iter().all(|a| a.name != "VC Libs"), "no startup task");
    }

    #[test]
    fn a_store_app_is_turned_off_the_users_way_and_undo_puts_it_back() {
        let (mut h, _dir, folders) = pc();
        let id = app(&h.engine.startup_apps(&folders), "Slack")
            .tweak
            .metadata
            .id
            .to_string();
        let before = h.fake.snapshot();
        h.engine.apply(&id).unwrap();
        assert_eq!(task_state(&h, SLACK_TASK), Some(RawValue::dword(1)), "DisabledByUser");
        let slack = app(&h.engine.startup_apps(&folders), "Slack").clone();
        assert_eq!(slack.tweak.state, TweakState::Applied);
        assert!(h
            .engine
            .journal_view()
            .applied
            .iter()
            .any(|c| c.tweak_id == id && c.name == "Slack at sign-in"));
        h.engine.revert(&id).unwrap();
        assert_eq!(h.fake.snapshot(), before, "everything as it was");

        // A state Windows already had is put back as it was.
        h.fake.set_external(
            Hive::CurrentUser,
            &format!(r"{STORE_TASKS}\{SLACK_TASK}"),
            "State",
            RawValue::dword(2),
        );
        h.engine.startup_apps(&folders);
        h.engine.apply(&id).unwrap();
        h.engine.revert(&id).unwrap();
        assert_eq!(task_state(&h, SLACK_TASK), Some(RawValue::dword(2)));
    }

    #[test]
    fn a_store_app_can_be_turned_on_and_undone_after_a_restart() {
        let (mut h, _dir, folders) = pc();
        let id = app(&h.engine.startup_apps(&folders), "Xbox")
            .turn_on
            .metadata
            .id
            .to_string();
        h.engine.apply(&id).unwrap();
        assert_eq!(task_state(&h, XBOX_TASK), Some(RawValue::dword(2)), "Enabled");
        assert_eq!(
            app(&h.engine.startup_apps(&folders), "Xbox").tweak.state,
            TweakState::Default,
            "it starts now"
        );
        h.restart(Vec::new());
        let results = h.engine.revert_all();
        assert!(results.iter().any(|r| r.tweak_id == id && r.ok), "{results:?}");
        assert_eq!(task_state(&h, XBOX_TASK), None);
    }

    #[test]
    fn a_store_app_set_by_policy_or_with_a_strange_state_is_left_alone() {
        let (mut h, _dir, folders) = pc();
        let list = h.engine.startup_apps(&folders);
        let corp = app(&list, "Corp Tool").clone();
        let before = h.fake.snapshot();
        for id in [&corp.tweak.metadata.id, &corp.turn_on.metadata.id] {
            match h.engine.apply(id) {
                Err(EngineError::Blocked { reason }) => assert_eq!(reason.code, BlockedCode::SetByPolicy),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(h.fake.snapshot(), before);

        h.fake.set_external(
            Hive::CurrentUser,
            &format!(r"{STORE_TASKS}\{SLACK_TASK}"),
            "State",
            RawValue::dword(9),
        );
        let slack = app(&h.engine.startup_apps(&folders), "Slack").clone();
        assert!(
            matches!(slack.tweak.state, TweakState::Unknown { .. }),
            "{:?}",
            slack.tweak.state
        );
    }

    #[test]
    fn store_apps_that_cannot_be_found_are_said_not_skipped() {
        let (mut h, _dir, folders) = pc();
        h.fake.remove_key_external(Hive::CurrentUser, PACKAGES);
        let list = h.engine.startup_apps(&folders);
        assert_eq!(list.problems.len(), 1, "{:?}", list.problems);
        assert!(list.problems[0].contains("Store apps"), "{:?}", list.problems);
    }

    #[test]
    fn turning_one_off_writes_only_its_switch_and_undo_removes_it() {
        let (mut h, _dir, folders) = pc();
        let id = app(&h.engine.startup_apps(&folders), "Discord")
            .tweak
            .metadata
            .id
            .to_string();
        let before = h.fake.snapshot();

        h.engine.apply(&id).unwrap();
        let switch = h
            .fake
            .read_value_for_test(Hive::CurrentUser, APPROVED_RUN, "Discord")
            .unwrap();
        assert_eq!(switched_off(&switch), Some(true));
        assert_eq!(switch.bytes.len(), 12);
        assert!(
            h.fake.read_value_for_test(Hive::CurrentUser, RUN, "Discord").is_some(),
            "the entry itself is kept"
        );
        assert_eq!(
            app(&h.engine.startup_apps(&folders), "Discord").tweak.state,
            TweakState::Applied
        );
        assert!(h
            .engine
            .journal_view()
            .applied
            .iter()
            .any(|c| c.tweak_id == id && c.name == "Discord at sign-in"));

        h.engine.revert(&id).unwrap();
        assert_eq!(h.fake.snapshot(), before, "everything as it was");
        assert_eq!(
            app(&h.engine.startup_apps(&folders), "Discord").tweak.state,
            TweakState::Default
        );
    }

    #[test]
    fn a_switch_windows_already_had_is_put_back_byte_for_byte() {
        let (mut h, _dir, folders) = pc();
        h.fake
            .set_external(Hive::CurrentUser, APPROVED_RUN, "Discord", on_value());
        let id = app(&h.engine.startup_apps(&folders), "Discord")
            .tweak
            .metadata
            .id
            .to_string();
        h.engine.apply(&id).unwrap();
        h.engine.revert(&id).unwrap();
        assert_eq!(
            h.fake.read_value_for_test(Hive::CurrentUser, APPROVED_RUN, "Discord"),
            Some(on_value())
        );
    }

    #[test]
    fn one_turned_off_elsewhere_can_be_turned_back_on_and_undo_puts_it_back() {
        let (mut h, _dir, folders) = pc();
        let list = h.engine.startup_apps(&folders);
        let steam = app(&list, "Steam");
        assert_eq!(steam.turn_on.state, TweakState::Default, "offered");
        assert_eq!(app(&list, "Discord").turn_on.state, TweakState::Foreign, "it starts");
        let id = steam.turn_on.metadata.id.to_string();
        assert_eq!(id, "startup.user_run.on:Steam");
        let before = h.fake.snapshot();

        h.engine.apply(&id).unwrap();
        assert_eq!(
            h.fake.read_value_for_test(Hive::CurrentUser, APPROVED_RUN, "Steam"),
            Some(on_value()),
            "Task Manager's own on value"
        );
        let steam = app(&h.engine.startup_apps(&folders), "Steam").clone();
        assert_eq!(steam.turn_on.state, TweakState::Applied);
        assert_eq!(steam.tweak.state, TweakState::Default, "it starts again");
        assert!(h
            .engine
            .journal_view()
            .applied
            .iter()
            .any(|c| c.tweak_id == id && c.name == "Steam at sign-in, turned back on"));

        h.engine.revert(&id).unwrap();
        assert_eq!(h.fake.snapshot(), before, "turned off again, byte for byte");
        assert_eq!(
            app(&h.engine.startup_apps(&folders), "Steam").tweak.state,
            TweakState::Foreign
        );
    }

    #[test]
    fn windows_security_can_be_turned_back_on() {
        let (mut h, _dir, folders) = pc();
        h.fake.set_external(
            Hive::LocalMachine,
            APPROVED_RUN,
            "SecurityHealth",
            StartupToggle::off_for_test(),
        );
        let list = h.engine.startup_apps(&folders);
        let security = app(&list, "SecurityHealth");
        assert!(matches!(security.tweak.state, TweakState::Blocked { .. }));
        assert_eq!(security.turn_on.state, TweakState::Default);
        assert!(security.turn_on.blocked.is_none());
        let id = security.turn_on.metadata.id.to_string();
        h.engine.apply(&id).unwrap();
        assert_eq!(
            h.fake
                .read_value_for_test(Hive::LocalMachine, APPROVED_RUN, "SecurityHealth"),
            Some(on_value())
        );
    }

    #[test]
    fn windows_security_is_never_turned_off() {
        let (mut h, _dir, folders) = pc();
        let id = app(&h.engine.startup_apps(&folders), "SecurityHealth")
            .tweak
            .metadata
            .id
            .to_string();
        let before = h.fake.snapshot();
        match h.engine.apply(&id) {
            Err(EngineError::Blocked { reason }) => assert_eq!(reason.code, BlockedCode::ProtectedProgram),
            other => panic!("{other:?}"),
        }
        assert_eq!(h.fake.snapshot(), before);
    }

    #[test]
    fn undo_still_works_after_the_program_is_uninstalled_and_undo_all_reaches_it() {
        let (mut h, _dir, folders) = pc();
        let id = app(&h.engine.startup_apps(&folders), "Discord")
            .tweak
            .metadata
            .id
            .to_string();
        h.engine.apply(&id).unwrap();
        h.fake.remove_external(Hive::CurrentUser, RUN, "Discord");
        assert!(h.engine.startup_apps(&folders).apps.iter().all(|a| a.name != "Discord"));
        h.restart(Vec::new());
        let results = h.engine.revert_all();
        assert!(results.iter().any(|r| r.tweak_id == id && r.ok), "{results:?}");
        assert!(h
            .fake
            .read_value_for_test(Hive::CurrentUser, APPROVED_RUN, "Discord")
            .is_none());
    }

    #[test]
    fn ids_that_name_no_startup_entry_are_refused() {
        let (mut h, _dir, folders) = pc();
        // Only what the list showed: an id the UI makes up is never a write,
        // even for a real entry before the list was read.
        let before = h.fake.snapshot();
        for id in [
            "startup.user_run:Discord",
            "startup.user_run.on:Steam",
            "startup.user_folder:Made up.lnk",
        ] {
            assert!(
                matches!(h.engine.apply(id), Err(EngineError::UnknownTweak { .. })),
                "{id}"
            );
        }
        h.engine.startup_apps(&folders);
        assert!(matches!(
            h.engine.apply("startup.user_folder:Made up.lnk"),
            Err(EngineError::UnknownTweak { .. })
        ));
        assert_eq!(h.fake.snapshot(), before);
        h.engine.apply("startup.user_run:Discord").unwrap();

        for id in [
            "startup.",
            "startup.nowhere:Discord",
            "startup.user_run:",
            "startup.user_run",
            "startup.user_run.off:Discord",
            "startup.user_run.on:",
            "startup..on:Discord",
        ] {
            assert!(
                matches!(h.engine.apply(id), Err(EngineError::UnknownTweak { .. })),
                "{id}"
            );
        }
        let before = h.fake.snapshot();
        assert!(matches!(
            h.engine.apply("startup.user_run:Not Installed"),
            Err(EngineError::UnknownTweak { .. })
        ));
        assert_eq!(h.fake.snapshot(), before, "no switch for an entry that is not there");
        let t = StartupToggle::from_id("startup.user_folder:OneNote.lnk").unwrap();
        assert_eq!(
            (t.source, t.name.as_str(), t.id()),
            (
                StartupSource::UserFolder,
                "OneNote.lnk",
                "startup.user_folder:OneNote.lnk"
            )
        );
    }

    #[test]
    fn a_folder_that_cannot_be_found_is_said_not_skipped() {
        let (mut h, _dir, _folders) = pc();
        let list = h.engine.startup_apps(&StartupFolders::default());
        assert_eq!(list.problems.len(), 2, "{:?}", list.problems);
        assert!(list.problems[0].contains("Startup folder"), "{:?}", list.problems);
        assert!(
            list.apps.iter().any(|a| a.name == "Discord"),
            "the registry entries are still listed"
        );
    }
}

/// Kegan's PC had a keyboard buffer of 18 events: the buffer tool keeps a
/// size already smaller than its 50 and sets only the other one, and with
/// both already small it reads as Already optimized.
#[test]
fn the_input_buffer_tool_never_raises_a_smaller_size() {
    const KBD: &str = r"SYSTEM\CurrentControlSet\Services\kbdclass\Parameters";
    const MOU: &str = r"SYSTEM\CurrentControlSet\Services\mouclass\Parameters";
    let id = "input.queuesize";
    let tweak = || -> Vec<Box<dyn Tweak>> { vec![Box::new(crate::tweaks::registry_values::INPUT_QUEUE)] };

    let fake = Arc::new(FakeRegistry::new());
    fake.set_external(Hive::LocalMachine, KBD, "KeyboardDataQueueSize", dword(18));
    let dir = tempfile::tempdir().unwrap();
    let mut engine = build_engine(&fake, dir.path(), tweak(), true, Tier::Ultimate);
    assert!(matches!(engine.list().unwrap()[0].state, TweakState::Default));
    engine.apply(id).unwrap();
    assert_eq!(
        fake.read_value_for_test(Hive::LocalMachine, KBD, "KeyboardDataQueueSize"),
        Some(dword(18))
    );
    assert_eq!(
        fake.read_value_for_test(Hive::LocalMachine, MOU, "MouseDataQueueSize"),
        Some(dword(50))
    );
    assert!(matches!(engine.list().unwrap()[0].state, TweakState::Applied));
    engine.revert(id).unwrap();
    assert_eq!(
        fake.read_value_for_test(Hive::LocalMachine, KBD, "KeyboardDataQueueSize"),
        Some(dword(18))
    );
    assert_eq!(
        fake.read_value_for_test(Hive::LocalMachine, MOU, "MouseDataQueueSize"),
        None
    );

    let fake = Arc::new(FakeRegistry::new());
    fake.set_external(Hive::LocalMachine, KBD, "KeyboardDataQueueSize", dword(18));
    fake.set_external(Hive::LocalMachine, MOU, "MouseDataQueueSize", dword(20));
    let dir = tempfile::tempdir().unwrap();
    let engine = build_engine(&fake, dir.path(), tweak(), true, Tier::Ultimate);
    assert!(matches!(engine.list().unwrap()[0].state, TweakState::Foreign));
}

#[test]
fn the_game_history_is_kept_across_restarts_and_capped() {
    use crate::play::PlayReport;
    use crate::probe::Probe;

    let report = |ended: u64| PlayReport {
        game: "fortnite".into(),
        started_unix_ms: ended - 1_000,
        ended_unix_ms: ended,
        gpu_throttle: Probe::unknown("not read in tests"),
        heat_readings: 0,
        hardware_readings: 0,
        gpu_hottest_c: Probe::unknown("not read in tests"),
        temperature_missed: None,
        gpu_busy_average: Probe::unknown("not read in tests"),
        cpu_busy_average: Probe::unknown("not read in tests"),
        memory_peak: Probe::unknown("not read in tests"),
        memory_cleans: 0,
        memory_cleaned_bytes: 0,
    };
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let trusted = TrustedDir::insecure_for_tests(dir.path());
    let mut engine = build_engine(&fake, dir.path(), vec![], true, Tier::Free).with_play_history_in(&trusted);
    assert!(engine.play_history().is_empty());
    for i in 0..(crate::play_history::KEEP as u64 + 2) {
        engine.record_play(report(10_000 + i)).unwrap();
    }
    let kept = engine.play_history();
    assert_eq!(kept.len(), crate::play_history::KEEP);
    assert_eq!(
        kept.last().map(|r| r.ended_unix_ms),
        Some(10_000 + crate::play_history::KEEP as u64 + 1)
    );

    let again = build_engine(&fake, dir.path(), vec![], true, Tier::Free).with_play_history_in(&trusted);
    assert_eq!(
        again.play_history(),
        kept,
        "a new engine on the same directory sees the same history"
    );
}
