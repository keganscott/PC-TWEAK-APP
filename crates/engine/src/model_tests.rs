//! Property test: random sequences of apply, revert, external writes, crashes,
//! injected failures, restarts and torn journal tails, checked against a small
//! model of what the registry must contain.
//!
//! The model captures the rule revert implements: for every value the tweak has
//! an outstanding write to, revert restores what that value held before the
//! *oldest* outstanding write to it, and leaves other values alone.

use std::io::Write;

use proptest::prelude::*;

use crate::error::EngineError;
use crate::journal::{JournalAction, JOURNAL_FILE};
use crate::registry::Hive;
use crate::testutil::*;
use crate::transaction::Transaction;
use crate::types::{RegRoot, Tweak};

const KEY: &str = r"SOFTWARE\PeakModel";
const NAMES: [&str; 3] = ["A", "B", "C"];
const TARGET: [u32; 3] = [1, 2, 3];

#[derive(Debug, Clone)]
enum Op {
    Apply,
    Revert,
    External(usize, Option<u32>),
    /// Apply, but "crash" after this many changing writes, before the commit.
    CrashApply(usize),
    /// Apply with the nth changing write failing, so the transaction rolls back.
    FailApply(usize),
    Restart,
    /// A half-written journal line is left at the end of the file, then restart.
    TearTailAndRestart,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => Just(Op::Apply),
        3 => Just(Op::Revert),
        3 => (0usize..3, prop::option::of(0u32..6)).prop_map(|(i, v)| Op::External(i, v)),
        1 => (0usize..4).prop_map(Op::CrashApply),
        1 => (1usize..4).prop_map(Op::FailApply),
        1 => Just(Op::Restart),
        1 => Just(Op::TearTailAndRestart),
    ]
}

#[derive(Default, Debug)]
struct Model {
    reg: [Option<u32>; 3],
    /// Per value: `Some(v)` if there is an outstanding write to it, where `v`
    /// is what it held before the oldest such write.
    restore: [Option<Option<u32>>; 3],
}

impl Model {
    fn changing(&self) -> Vec<usize> {
        (0..3).filter(|&i| self.reg[i] != Some(TARGET[i])).collect()
    }

    fn apply_first(&mut self, n: usize) {
        for i in self.changing().into_iter().take(n) {
            if self.restore[i].is_none() {
                self.restore[i] = Some(self.reg[i]);
            }
            self.reg[i] = Some(TARGET[i]);
        }
    }

    fn outstanding(&self) -> bool {
        self.restore.iter().any(Option::is_some)
    }

    fn revert(&mut self) {
        for i in 0..3 {
            if let Some(v) = self.restore[i] {
                self.reg[i] = v;
            }
        }
        self.restore = [None; 3];
    }
}

fn tweak() -> Vec<Box<dyn Tweak>> {
    vec![Box::new(TestTweak::new(
        "t",
        KEY,
        &[("A", TARGET[0]), ("B", TARGET[1]), ("C", TARGET[2])],
    ))]
}

fn real_registry(h: &Harness) -> [Option<u32>; 3] {
    let mut out = [None; 3];
    for (i, n) in NAMES.iter().enumerate() {
        out[i] = hklm_dword(&h.fake, KEY, n);
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    #[test]
    fn registry_always_matches_the_model(ops in prop::collection::vec(op(), 1..30)) {
        let mut h = Harness::new(tweak());
        let mut m = Model::default();

        for op in ops {
            match op {
                Op::Apply => {
                    let r = h.engine.apply("t");
                    prop_assert!(r.is_ok(), "apply failed: {:?}", r);
                    m.apply_first(3);
                }
                Op::Revert => {
                    let r = h.engine.revert("t");
                    if m.outstanding() {
                        prop_assert!(r.is_ok(), "revert failed: {:?}", r);
                        m.revert();
                    } else {
                        prop_assert!(matches!(r, Err(EngineError::NoJournalEntry { .. })), "{:?}", r);
                    }
                }
                Op::External(i, v) => {
                    match v {
                        Some(v) => h.fake.set_external(Hive::LocalMachine, KEY, NAMES[i], dword(v)),
                        None => h.fake.remove_external(Hive::LocalMachine, KEY, NAMES[i]),
                    }
                    m.reg[i] = v;
                }
                Op::CrashApply(k) => {
                    let t = TestTweak::new("t", KEY, &[("A", TARGET[0]), ("B", TARGET[1]), ("C", TARGET[2])]);
                    let todo: Vec<usize> = m.changing().into_iter().take(k).collect();
                    {
                        let (resolver, journal, _) = h.engine.parts_for_test();
                        let mut tx = Transaction::begin(&t, resolver, journal, JournalAction::Apply).unwrap();
                        for &i in &todo {
                            tx.set_dword(RegRoot::LocalMachine, KEY, NAMES[i], TARGET[i]).unwrap();
                        }
                        drop(tx); // crash: no commit
                    }
                    m.apply_first(k);
                }
                Op::FailApply(n) => {
                    let changing = m.changing().len();
                    h.fake.fail_mutation_number(n);
                    let r = h.engine.apply("t");
                    h.fake.clear_faults();
                    if n <= changing {
                        prop_assert!(r.is_err(), "the injected failure should surface");
                        // Rolled back: registry and outstanding writes unchanged.
                    } else {
                        prop_assert!(r.is_ok(), "no write reached the failure: {:?}", r);
                        m.apply_first(3);
                    }
                }
                Op::Restart => h.restart(tweak()),
                Op::TearTailAndRestart => {
                    let mut f = std::fs::OpenOptions::new()
                        .append(true)
                        .create(true)
                        .open(h.dir.path().join(JOURNAL_FILE))
                        .unwrap();
                    f.write_all(b"{\"record\":\"write\",\"seq\":99").unwrap();
                    drop(f);
                    h.restart(tweak());
                }
            }

            prop_assert_eq!(real_registry(&h), m.reg, "registry diverged from the model");
            let (_, journal, _) = h.engine.parts_for_test();
            prop_assert_eq!(journal.is_applied("t"), m.outstanding(), "journal outstanding state diverged");
        }

        // Finally: undo everything and confirm nothing of ours is left over.
        let _ = h.engine.revert("t");
        m.revert();
        prop_assert_eq!(real_registry(&h), m.reg);
    }
}
