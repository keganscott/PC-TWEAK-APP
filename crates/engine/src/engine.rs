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

enum Slot {
    Catalogue(usize),
    Internal(usize),
}

pub struct Engine {
    resolver: ContextResolver,
    journal: Journal,
    tweaks: Vec<Box<dyn Tweak>>,
    /// The engine's own tweaks (see `tweaks::internal`). Not listed, not
    /// applicable over IPC, but revertable.
    internal: Vec<Box<dyn Tweak>>,
    probe: Box<dyn EnvProbe>,
    license: License,
    env: SystemEnv,
    target_game: Option<String>,
    restore: Option<std::sync::Arc<crate::restore::RestoreService>>,
    proof: Option<std::sync::Arc<crate::proof::service::ProofService>>,
    settings: crate::settings::Settings,
    settings_store: crate::settings::SettingsStore,
}

impl Engine {
    pub fn new(
        resolver: ContextResolver,
        journal: Journal,
        tweaks: Vec<Box<dyn Tweak>>,
        probe: Box<dyn EnvProbe>,
        license: License,
    ) -> Self {
        // No probing here: constructing the engine happens on Tauri's main
        // thread, and the probes take seconds. The first command that needs an
        // environment (`rescan`, `audit`, `apply`) probes, on a worker thread.
        let env = SystemEnv {
            elevated: resolver.elevated(),
            ..SystemEnv::default()
        };
        Self {
            resolver,
            journal,
            tweaks,
            internal: crate::tweaks::internal(),
            probe,
            license,
            env,
            target_game: None,
            restore: None,
            proof: None,
            settings: crate::settings::Settings::default(),
            settings_store: crate::settings::SettingsStore::in_memory(),
        }
    }

    /// Attach the restore-point service (the real one on Windows). Without it,
    /// `create_restore_point` reports that this build cannot make one.
    pub fn with_restore_service(mut self, svc: std::sync::Arc<crate::restore::RestoreService>) -> Self {
        self.restore = Some(svc);
        self
    }

    /// Keep settings in the protected data directory and load what is there.
    pub fn with_settings_in(mut self, dir: &super::secure_dir::TrustedDir) -> Self {
        self.settings_store = crate::settings::SettingsStore::in_dir(dir);
        self.settings = self.settings_store.load();
        self
    }

    pub fn settings(&self) -> crate::settings::Settings {
        self.settings.clone()
    }

    /// Replace the settings. Saved first; the in-memory copy changes only if the
    /// save worked, so what the UI shows is what is on disk.
    pub fn set_settings(&mut self, settings: crate::settings::Settings) -> Result<crate::settings::Settings> {
        self.settings_store.save(&settings)?;
        self.settings = settings;
        Ok(self.settings.clone())
    }

    /// The rig class in use: the user's override, else the detected one.
    pub fn effective_rig_class(&self) -> Option<crate::hardware::RigClass> {
        self.settings
            .rig_class_override
            .or_else(|| self.env.hardware.as_ref().and_then(|h| h.rig_class.value().copied()))
    }

    pub fn restore_service(&self) -> Option<std::sync::Arc<crate::restore::RestoreService>> {
        self.restore.clone()
    }

    /// Attach the proof (telemetry) service.
    pub fn with_proof_service(mut self, svc: std::sync::Arc<crate::proof::service::ProofService>) -> Self {
        self.proof = Some(svc);
        self
    }

    pub fn proof_service(&self) -> Option<std::sync::Arc<crate::proof::service::ProofService>> {
        self.proof.clone()
    }

    /// Catalogue tweaks with an outstanding apply, most recent first. Recorded
    /// with every proof run so the result says what it measured. The engine's
    /// own bookkeeping tweaks are not listed.
    pub fn applied_tweak_ids(&self) -> Vec<String> {
        self.journal
            .applied_tweaks_newest_first()
            .into_iter()
            .filter(|id| self.tweaks.iter().any(|t| t.id() == id))
            .collect()
    }

    /// What a proof session needs to know about this PC, from the engine's own
    /// (recently cached) probes.
    pub fn proof_context(&mut self) -> (Option<crate::hardware::RigClass>, super::types::Tier) {
        self.rescan();
        (self.effective_rig_class(), self.license.tier())
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

    /// Forget every cached probe result, then probe again. This is what the
    /// user's "rescan" does; `rescan` alone may reuse recent results.
    pub fn rescan_fresh(&mut self) {
        self.probe.invalidate_all();
        self.rescan();
    }

    /// Everything we know about this machine, freshly probed.
    pub fn audit(&mut self) -> crate::sysprobe::SystemAudit {
        self.rescan_fresh();
        crate::sysprobe::SystemAudit::from_env(self.env.clone(), self.settings.clone(), self.effective_rig_class())
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

    /// Where a tweak id lives: the catalogue or the engine's own list.
    fn slot_of(&self, id: &str) -> Result<Slot> {
        if let Some(i) = self.tweaks.iter().position(|t| t.id() == id) {
            return Ok(Slot::Catalogue(i));
        }
        if let Some(i) = self.internal.iter().position(|t| t.id() == id) {
            return Ok(Slot::Internal(i));
        }
        Err(EngineError::UnknownTweak {
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

        // Fresh state for this decision: the state probes (security, restore) are
        // re-read; hardware, which does not change under a running app, may come
        // from the 10-minute cache.
        self.probe.invalidate();
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
        let result = match tweak.apply(&mut tx) {
            Ok(()) => tx.commit(),
            Err(e) => {
                // Undo what this transaction already wrote. The original error
                // is what the caller needs; if the undo itself fails the writes
                // stay outstanding in the journal and Revert can retry.
                let _ = tx.rollback();
                Err(e)
            }
        };
        self.probe.invalidate();
        result
    }

    /// Revert is never blocked by tier, predicates or the restore gate:
    /// getting back to how things were must always be possible.
    pub fn revert(&mut self, id: &str) -> Result<Vec<JournalEntry>> {
        let slot = self.slot_of(id)?;
        let Engine {
            resolver,
            journal,
            tweaks,
            internal,
            ..
        } = self;
        let tweak = match slot {
            Slot::Catalogue(i) => tweaks[i].as_ref(),
            Slot::Internal(i) => internal[i].as_ref(),
        };

        let mut tx = Transaction::begin(tweak, resolver, journal, JournalAction::Revert)?;
        // A failed revert leaves the transaction uncommitted on purpose: the
        // applies stay outstanding and a retry finishes the job.
        let reverted = tweak.revert(&mut tx);
        // Even a failed revert may have changed things (some values restored):
        // cached probe results must not outlive it.
        let result = reverted.and_then(|()| tx.commit());
        self.probe.invalidate();
        result
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

    /// Lift Windows' 24-hour restore-point limit. The one change allowed before
    /// the restore gate opens: it is what makes the first restore point
    /// possible. Journalled and revertable like any other change.
    pub fn ensure_restore_frequency(&mut self) -> Result<()> {
        let Slot::Internal(idx) = self.slot_of(crate::tweaks::system_restore::ID)? else {
            return Err(EngineError::Internal {
                detail: "the restore-frequency bootstrap tweak is not an internal tweak".into(),
            });
        };
        let Engine {
            resolver,
            journal,
            internal,
            ..
        } = self;
        let tweak = internal[idx].as_ref();
        let mut tx = Transaction::begin(tweak, resolver, journal, JournalAction::Apply)?;
        match tweak.apply(&mut tx) {
            Ok(()) => tx.commit().map(|_| ()),
            Err(e) => {
                let _ = tx.rollback();
                Err(e)
            }
        }
    }

    /// Journal a restore point that was created and verified, and make the next
    /// gate check look at Windows again.
    pub fn record_restore_point(
        &mut self,
        sequence_number: u32,
        description: &str,
        method: crate::journal::RestoreMethod,
        protection_enabled_by_us: bool,
    ) -> Result<()> {
        let seq = self.journal.take_seq();
        self.journal.append_restore_point(crate::journal::RestorePointRecord {
            seq,
            unix_ms: crate::journal::now_ms(),
            sequence_number,
            description: description.to_owned(),
            method,
            protection_enabled_by_us,
        })?;
        self.probe.invalidate();
        Ok(())
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
    /// Start on a real Windows machine with the real probes and restore
    /// service: check elevation, create or verify the protected ProgramData
    /// directory, resolve the interactive user, open the journal. Any failure
    /// here means the app does not start mutating.
    pub fn start_windows(tweaks: Vec<Box<dyn Tweak>>, license: License) -> Result<Self> {
        use std::sync::Arc;

        use super::hardware::OsFacts;
        use super::registry::windows::WinRegistry;
        use super::registry::RegistryBackend;
        use super::restore::RestoreService;
        use super::restore_win::WindowsRestoreOps;
        use super::sysprobe::SystemProbe;
        use super::wmi::{WmiSource, WmiWorker};

        let wmi: Arc<dyn WmiSource> = Arc::new(WmiWorker::start());
        let reg: Arc<dyn RegistryBackend> = Arc::new(WinRegistry::new());
        let facts: Arc<dyn OsFacts> = Arc::new(super::osfacts::WindowsFacts);
        let restore = Arc::new(RestoreService::new(
            Arc::new(WindowsRestoreOps::new(wmi.clone())),
            reg.clone(),
            wmi.clone(),
        ));
        let probe = SystemProbe::new(wmi, reg, facts, restore.clone());
        Ok(Self::start_windows_with_probe(tweaks, Box::new(probe), license)?.with_restore_service(restore))
    }

    /// Like `start_windows` but with a caller-supplied probe. Used by the
    /// `dev-stubs` build to open the gate by hand; never in a release.
    pub fn start_windows_with_probe(
        tweaks: Vec<Box<dyn Tweak>>,
        probe: Box<dyn EnvProbe>,
        license: License,
    ) -> Result<Self> {
        use std::sync::Arc;

        let elevated = super::identity::is_elevated();
        let dir = super::secure_dir::TrustedDir::ensure_program_data()?;
        let resolver = ContextResolver::detect(elevated)?;
        let journal = Journal::open(&dir)?;
        let proof = Arc::new(build_proof_service(&dir));
        Ok(Self::new(resolver, journal, tweaks, probe, license)
            .with_proof_service(proof)
            .with_settings_in(&dir))
    }
}

/// The proof service for a real Windows install: PresentMon from the protected
/// `tools` directory (hash-checked) when this build carries it, NVML for GPU
/// throttle readings. If PresentMon cannot be provided the service still exists
/// and says why when a capture is requested.
#[cfg(windows)]
fn build_proof_service(dir: &super::secure_dir::TrustedDir) -> crate::proof::service::ProofService {
    use std::sync::Arc;

    use crate::proof::capture::{CaptureTool, PresentMonTool, UnavailableTool};
    use crate::proof::nvml::NvmlSampler;

    let tool: Arc<dyn CaptureTool> = match crate::proof::presentmon::provision(dir) {
        Ok(path) => Arc::new(PresentMonTool::new(path)),
        Err(e) => Arc::new(UnavailableTool(e.to_string())),
    };
    crate::proof::service::ProofService::new(dir.path().join("proof"), tool).with_sampler(Arc::new(NvmlSampler::new()))
}
