//! Engine orchestration. No Tauri types here: the command surface lives in the
//! `peaktweaks` crate and calls into this.

use serde::Serialize;
use ts_rs::TS;

use super::context::{ContextResolver, UserResolution};
use super::env::{EnvProbe, License, KNOWN_GAMES};
use super::error::{EngineError, Result};
use super::journal::{Journal, JournalAction, JournalEntry, JournalWarning, Record};
use super::transaction::Transaction;
use super::types::{
    BlockedCode, BlockedReason, ExecutionContext, PredicateOutcome, SystemEnv, Tweak, TweakMetadata, TweakState,
};

/// A tweak plus its live state, as sent to the frontend.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TweakView {
    #[serde(flatten)]
    pub metadata: TweakMetadata,
    pub context: ExecutionContext,
    pub state: TweakState,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ContextInfo {
    pub sid: String,
    pub resolution: UserResolution,
    pub is_self: bool,
    pub elevated: bool,
}

/// Outcome of one tweak in a `revert_all`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RevertResult {
    pub tweak_id: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// The journal as shown in the Backups tab.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct JournalView {
    pub records: Vec<Record>,
    pub warnings: Vec<JournalWarning>,
}

/// Progress event emitted on `engine://progress` while a command runs.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub stage: String,
    pub tweak_id: Option<String>,
    pub message: String,
}

pub struct Engine {
    resolver: ContextResolver,
    journal: Journal,
    tweaks: Vec<Box<dyn Tweak>>,
    probe: Box<dyn EnvProbe>,
    license: License,
    env: SystemEnv,
    target_game: Option<String>,
}

impl Engine {
    pub fn new(
        resolver: ContextResolver,
        journal: Journal,
        tweaks: Vec<Box<dyn Tweak>>,
        probe: Box<dyn EnvProbe>,
        license: License,
    ) -> Self {
        let mut engine = Self {
            resolver,
            journal,
            tweaks,
            probe,
            license,
            env: SystemEnv::default(),
            target_game: None,
        };
        engine.rescan();
        engine
    }

    pub fn context_info(&self) -> ContextInfo {
        let u = self.resolver.user();
        ContextInfo {
            sid: u.sid.clone(),
            resolution: u.resolution,
            is_self: u.is_self,
            elevated: self.resolver.elevated(),
        }
    }

    /// Re-run the probes. The environment is built here, in Rust, and nowhere
    /// else; there is no way to hand the engine one.
    pub fn rescan(&mut self) {
        let mut env = self.probe.probe(self.resolver.elevated());
        env.target_game = self.target_game.clone();
        self.env = env;
    }

    /// Pick (or clear) the target game. Only ids from `KNOWN_GAMES` are accepted.
    pub fn select_target_game(&mut self, game_id: Option<String>) -> Result<()> {
        if let Some(id) = &game_id {
            if !KNOWN_GAMES.iter().any(|g| g.id == id) {
                return Err(EngineError::UnknownGame { game_id: id.clone() });
            }
        }
        self.target_game = game_id;
        self.rescan();
        Ok(())
    }

    pub fn env(&self) -> &SystemEnv {
        &self.env
    }

    pub fn license(&self) -> License {
        self.license
    }

    fn index_of(&self, id: &str) -> Result<usize> {
        self.tweaks
            .iter()
            .position(|t| t.id() == id)
            .ok_or_else(|| EngineError::UnknownTweak {
                tweak_id: id.to_string(),
            })
    }

    pub fn list(&self) -> Result<Vec<TweakView>> {
        let mut out = Vec::with_capacity(self.tweaks.len());

        for tweak in &self.tweaks {
            // Predicate first: a blocked tweak must not have its state probed,
            // because "blocked" outranks whatever is currently on disk and the
            // probe may not even be meaningful.
            let state = match tweak.evaluate_predicate(&self.env) {
                PredicateOutcome::Block(reason) => TweakState::Blocked { reason },
                PredicateOutcome::Allow => {
                    let applied = self.journal.is_applied(tweak.id());
                    // A read failure is its own state. Reporting it as Default
                    // would invite an Apply on top of something we cannot see.
                    match tweak.read_state(&self.resolver, applied) {
                        Ok(s) => s,
                        Err(e) => TweakState::Unknown { detail: e.to_string() },
                    }
                }
            };

            out.push(TweakView {
                metadata: tweak.metadata(),
                context: tweak.execution_context(),
                state,
            });
        }
        Ok(out)
    }

    pub fn apply(&mut self, id: &str) -> Result<Vec<JournalEntry>> {
        let idx = self.index_of(id)?;

        // Tier is enforced here, in Rust, against a license the UI cannot set.
        let required = self.tweaks[idx].metadata().tier;
        if required > self.license.tier() {
            return Err(EngineError::Blocked {
                reason: BlockedReason::new(
                    BlockedCode::TierRequired,
                    format!("This change needs the {required:?} plan."),
                )
                .with_trigger(format!("{required:?}").to_lowercase()),
            });
        }

        // Fresh environment for this decision, not whatever was cached.
        self.rescan();
        if let PredicateOutcome::Block(reason) = self.tweaks[idx].evaluate_predicate(&self.env) {
            return Err(EngineError::Blocked { reason });
        }

        // Refuse to mutate without a verified rollback point. Checked here,
        // immediately before the write, against the engine's own probe.
        if !self.probe.restore_gate_open() {
            return Err(EngineError::Blocked {
                reason: BlockedReason::new(
                    BlockedCode::NoRestorePoint,
                    "There is no verified restore point, so there is nothing to roll back to.",
                ),
            });
        }

        // If we cannot see the current state we do not write on top of it.
        let applied = self.journal.is_applied(id);
        self.tweaks[idx].read_state(&self.resolver, applied)?;

        let Engine {
            resolver,
            journal,
            tweaks,
            ..
        } = self;
        let tweak = tweaks[idx].as_ref();

        let mut tx = Transaction::begin(tweak, resolver, journal, JournalAction::Apply)?;
        match tweak.apply(&mut tx) {
            Ok(()) => tx.commit(),
            Err(e) => {
                // Undo what this transaction already wrote. The original error
                // is what the caller needs; if the undo itself fails the writes
                // stay outstanding in the journal and Revert can retry.
                let _ = tx.rollback();
                Err(e)
            }
        }
    }

    /// Revert is never blocked by tier, predicates or the restore gate:
    /// getting back to how things were must always be possible.
    pub fn revert(&mut self, id: &str) -> Result<Vec<JournalEntry>> {
        let idx = self.index_of(id)?;
        let Engine {
            resolver,
            journal,
            tweaks,
            ..
        } = self;
        let tweak = tweaks[idx].as_ref();

        let mut tx = Transaction::begin(tweak, resolver, journal, JournalAction::Revert)?;
        // A failed revert leaves the transaction uncommitted on purpose: the
        // applies stay outstanding and a retry finishes the job.
        tweak.revert(&mut tx)?;
        tx.commit()
    }

    /// Revert everything with an outstanding apply, most recently applied
    /// first. A failure on one tweak does not stop the rest: a partial revert
    /// is strictly better than stopping halfway.
    pub fn revert_all(&mut self) -> Vec<RevertResult> {
        let ids = self.journal.applied_tweaks_newest_first();
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            match self.revert(&id) {
                Ok(_) => results.push(RevertResult {
                    tweak_id: id,
                    ok: true,
                    error: None,
                }),
                Err(e) => results.push(RevertResult {
                    tweak_id: id,
                    ok: false,
                    error: Some(e.to_string()),
                }),
            }
        }
        results
    }

    /// Test access to the pieces a `Transaction` needs, so tests can simulate a
    /// crash by dropping a transaction without committing it.
    #[cfg(test)]
    pub(crate) fn parts_for_test(&mut self) -> (&ContextResolver, &mut Journal, &[Box<dyn Tweak>]) {
        (&self.resolver, &mut self.journal, &self.tweaks)
    }

    pub fn journal_view(&self) -> JournalView {
        JournalView {
            records: self.journal.records().to_vec(),
            warnings: self.journal.warnings().to_vec(),
        }
    }
}

#[cfg(windows)]
impl Engine {
    /// Start on a real Windows machine: check elevation, create or verify the
    /// protected ProgramData directory, resolve the interactive user, open the
    /// journal. Any failure here means the app does not start mutating.
    pub fn start_windows(tweaks: Vec<Box<dyn Tweak>>, probe: Box<dyn EnvProbe>, license: License) -> Result<Self> {
        let elevated = super::identity::is_elevated();
        let dir = super::secure_dir::TrustedDir::ensure_program_data()?;
        let resolver = ContextResolver::detect(elevated)?;
        let journal = Journal::open(&dir)?;
        Ok(Self::new(resolver, journal, tweaks, probe, license))
    }
}
