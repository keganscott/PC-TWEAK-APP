//! Reference `Service` (HKLM) tweak: foreground scheduling boost.
//!
//! `Win32PrioritySeparation` packs three fields into six bits: quantum length
//! (short/long), quantum variability (variable/fixed) and the foreground boost
//! ratio. `0x26` is short, variable, 3:1 — the configuration Windows already
//! uses for the "Programs" setting, with the boost pinned rather than left to
//! the kernel's heuristic.
//!
//! Honest scope, which the metadata says out loud: this is measurable on
//! 4-thread parts where the game genuinely contends with background work, and
//! close to nothing on 8 threads or more. It is graded `Moderate` for that
//! reason, not because it is risky.

use crate::engine::context::ContextResolver;
use crate::engine::error::Result;
use crate::engine::journal::Transaction;
use crate::engine::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, PredicateOutcome, RegRoot, SafetyTier, SystemEnv, Tier,
    Tweak, TweakMetadata, TweakState,
};

const KEY: &str = r"SYSTEM\CurrentControlSet\Control\PriorityControl";
const VALUE: &str = "Win32PrioritySeparation";

/// Windows client default. Anything else is someone's tweak — ours or another
/// tool's, which is what the `Foreign` state distinguishes.
const WINDOWS_DEFAULT: u32 = 2;
const TUNED: u32 = 0x26;

pub struct PrioritySeparation;

impl Tweak for PrioritySeparation {
    fn id(&self) -> &'static str {
        "sched.win32prioritysep"
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: self.id(),
            name: "Foreground scheduling boost",
            summary: "Shortens the time slice Windows gives background work, favouring the active window.",
            target: r"HKLM\SYSTEM\CurrentControlSet\Control\PriorityControl\Win32PrioritySeparation",
            category: "scheduling",
            tier: Tier::Pro,
            safety: SafetyTier::Moderate,
            impact: Impact::Moderate,
            tradeoff: Some(
                "Measurable on 4-thread CPUs where the game competes with background work. \
                 Close to nothing on 8 threads or more. Background tasks — encoding, compiling, \
                 large downloads — will run slower while a game is focused.",
            ),
            requires_reboot: true,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Service
    }

    fn evaluate_predicate(&self, env: &SystemEnv) -> PredicateOutcome {
        // On high-thread-count parts the change is unmeasurable. Offering it
        // anyway would be selling a placebo, which is the thing this product
        // is positioned against.
        if env.logical_processors >= 16 {
            return PredicateOutcome::Block(
                BlockedReason::new(
                    BlockedCode::HardwareCounterproductive,
                    "With this many threads the scheduler already keeps your game fed. \
                     We measured no difference on parts like yours, so we are not going to \
                     pretend otherwise.",
                )
                .with_trigger(format!("{} logical processors", env.logical_processors)),
            );
        }
        PredicateOutcome::Allow
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let current = res
            .open_read(RegRoot::LocalMachine, KEY)
            .ok()
            .and_then(|k| k.get_value::<u32, _>(VALUE).ok())
            .unwrap_or(WINDOWS_DEFAULT);

        Ok(match (current, has_journal_entry) {
            (WINDOWS_DEFAULT, _) => TweakState::Default,
            (TUNED, true) => TweakState::Applied,
            // Non-default with no record of us doing it. Could be our tuned
            // value set by another tool, could be something else entirely.
            // Either way the honest label is "not us".
            _ => TweakState::Foreign,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        tx.set_dword(RegRoot::LocalMachine, KEY, VALUE, TUNED)
    }

    // revert() uses the trait default: replay the journal, restoring the exact
    // prior value rather than assuming it was the documented default. If the
    // user had 0x28 set before us, they get 0x28 back.
}
