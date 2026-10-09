//! Engine orchestration. No Tauri types here: the command surface lives in the
//! `peaktweaks` crate and calls into this.

use serde::Serialize;
use ts_rs::TS;

use super::context::{ContextResolver, UserResolution};
use super::env::{EnvProbe, License, KNOWN_GAMES};
use super::error::{EngineError, Result};
use super::journal::{
    now_ms, ActionDone, ActionRecord, Journal, JournalAction, JournalEntry, JournalWarning, OneTimeAction, Record,
};
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
    /// Why Apply is refused right now, if it is. Separate from `state` so a
    /// tweak that was applied before the block started still reads as
    /// `Applied` and keeps its Undo.
    pub blocked: Option<BlockedReason>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ContextInfo {
    pub sid: String,
    pub resolution: UserResolution,
    pub is_self: bool,
    pub elevated: bool,
    /// A tester build (`License::tester`): every plan is unlocked for testing,
    /// nothing else differs. The UI says so on every screen.
    pub tester_build: bool,
}

/// One Gaming Mode change when a game started (`Engine::start_play_session`).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SessionStep {
    pub tweak_id: String,
    /// The change was made now (false: already so, or it failed).
    pub made: bool,
    pub error: Option<String>,
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

/// Where a change with an outstanding apply comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// A tweak from the catalogue (Tools).
    Catalogue,
    /// A change PeakTweaks makes for itself (`tweaks::internal`), such as
    /// allowing a restore point on demand.
    Internal,
    /// A tweak this version no longer ships (NOTES.md N49).
    Retired,
}

/// A change whose apply is still outstanding in the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppliedChange {
    pub tweak_id: String,
    /// The tweak's name, or its id when this version does not ship it.
    pub name: String,
    pub kind: ChangeKind,
}

/// The journal as shown in the Backups tab.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct JournalView {
    /// Every change with an outstanding apply, most recent first: what Undo
    /// all reverts, whatever kind it is.
    pub applied: Vec<AppliedChange>,
    pub records: Vec<Record>,
    pub warnings: Vec<JournalWarning>,
    /// Why the files for undoing changes from outside Windows (`offline.rs`)
    /// could not be brought up to date at start-up, if they could not. Cleared
    /// by the next apply or undo, which rewrites them or fails.
    pub offline_error: Option<String>,
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
    /// A change made from its id for something this PC has: a startup
    /// entry's switch (`tweaks::startup`) or a device's MSI mode
    /// (`tweaks::msi`).
    Listed(Box<dyn Tweak>),
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
    offline_error: Option<String>,
    /// The ids the last `startup_apps` and `msi_devices` listed, with the
    /// name each was shown with. Only those can be applied, so an id the UI
    /// makes up never becomes a write.
    listed: std::collections::HashMap<String, String>,
    /// Held while this engine lives, so no second engine writes the same
    /// journal (`instance.rs`). `None` for engines on test directories.
    _instance: Option<crate::instance::InstanceLock>,
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
            offline_error: None,
            listed: std::collections::HashMap::new(),
            _instance: None,
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
    /// Turning Gaming Mode off puts back its changes at once, game or not.
    pub fn set_settings(&mut self, settings: crate::settings::Settings) -> Result<crate::settings::Settings> {
        self.settings_store.save(&settings)?;
        let ending = self.settings.gaming_mode && !settings.gaming_mode;
        self.settings = settings;
        if ending {
            let failed: Vec<String> = self
                .end_play_session()
                .into_iter()
                .filter_map(|r| r.error.map(|e| format!("{}: {e}", r.tweak_id)))
                .collect();
            if !failed.is_empty() {
                return Err(EngineError::Internal {
                    detail: format!(
                        "Gaming Mode is off, but some of its changes could not be put back: {}",
                        failed.join("; ")
                    ),
                });
            }
        }
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
            tester_build: self.license.is_tester(),
        }
    }

    /// Re-run the probes. The environment is built here, in Rust, and nowhere
    /// else; there is no way to hand the engine one.
    pub fn rescan(&mut self) {
        let mut env = self.probe.probe(self.resolver.elevated());
        env.target_game = self.target_game.clone();
        // Per-game tweaks read where the game is through the resolver.
        self.resolver.set_game_installs(env.game_installs.clone());
        self.env = env;
    }

    /// Forget every cached probe result, then probe again. This is what the
    /// user's "rescan" does; `rescan` alone may reuse recent results.
    pub fn rescan_fresh(&mut self) {
        self.probe.invalidate_all();
        self.rescan();
    }

    /// Everything we know about this machine. Security and restore state are
    /// read again (a change or a new restore point alters them); the slow
    /// hardware probes and the background sample come from their caches. The
    /// user's "Check again" (`rescan` command, `rescan_fresh`) is what forgets
    /// those, and the UI asks for this audit right after it.
    pub fn audit(&mut self) -> crate::sysprobe::SystemAudit {
        self.probe.invalidate();
        self.rescan();
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

    /// Where a tweak id lives: the catalogue or the engine's own list.
    fn slot_of(&self, id: &str) -> Result<Slot> {
        if let Some(i) = self.tweaks.iter().position(|t| t.id() == id) {
            return Ok(Slot::Catalogue(i));
        }
        if let Some(i) = self.internal.iter().position(|t| t.id() == id) {
            return Ok(Slot::Internal(i));
        }
        if let Some(t) = crate::tweaks::startup::StartupToggle::from_id(id) {
            return Ok(Slot::Listed(Box::new(t)));
        }
        if let Some(instance) = id.strip_prefix(crate::tweaks::msi::ID_PREFIX) {
            let name = self.listed.get(id).map_or("", String::as_str);
            if let Some(t) = crate::tweaks::msi::MsiMode::new(instance, name) {
                return Ok(Slot::Listed(Box::new(t)));
            }
        }
        Err(EngineError::UnknownTweak {
            tweak_id: id.to_string(),
        })
    }

    fn tweak_in<'a>(&'a self, slot: &'a Slot) -> &'a dyn Tweak {
        match slot {
            Slot::Catalogue(i) => self.tweaks[*i].as_ref(),
            Slot::Internal(i) => self.internal[*i].as_ref(),
            Slot::Listed(t) => t.as_ref(),
        }
    }

    /// Why the licence does not cover this tweak, if it does not. The one tier
    /// check: `list` shows it, `apply` enforces it.
    fn tier_block(&self, tweak: &dyn Tweak) -> Option<BlockedReason> {
        let required = tweak.metadata().tier;
        (required > self.license.tier()).then(|| {
            BlockedReason::new(
                BlockedCode::TierRequired,
                format!("This change needs the {required:?} plan."),
            )
            .with_trigger(format!("{required:?}").to_lowercase())
        })
    }

    pub fn list(&self) -> Result<Vec<TweakView>> {
        Ok(self.tweaks.iter().map(|t| self.view_of(t.as_ref())).collect())
    }

    /// What the UI shows for one tweak: its state and why it is not offered.
    fn view_of(&self, tweak: &dyn Tweak) -> TweakView {
        let applied = self.journal.is_applied(tweak.id());
        // Predicate first: a blocked tweak's state is not probed (the probe
        // may not be meaningful), except that an apply still open in the
        // journal is reported as Applied, so it can be undone.
        let predicate = match tweak.evaluate_predicate(&self.env) {
            PredicateOutcome::Block(reason) => Some(reason),
            PredicateOutcome::Allow => None,
        };
        let state = match &predicate {
            Some(_) if applied => TweakState::Applied,
            Some(reason) => TweakState::Blocked { reason: reason.clone() },
            None => {
                // A read failure is its own state. Reporting it as Default
                // would invite an Apply on top of something we cannot see.
                match tweak.read_state(&self.resolver, applied) {
                    // Our apply is outstanding but Windows no longer has
                    // our value: changed outside PeakTweaks. Kept undoable.
                    Ok(TweakState::Default | TweakState::Foreign) if applied => TweakState::Drifted,
                    Ok(s) => s,
                    Err(e) => TweakState::Unknown { detail: e.to_string() },
                }
            }
        };

        // A plan that does not include the change blocks Apply but not
        // the reading of its state (the user may want to see it is set).
        let blocked = self.tier_block(tweak).or(predicate);
        TweakView {
            metadata: tweak.metadata(),
            context: tweak.execution_context(),
            state,
            blocked,
        }
    }

    /// Startup apps (CATALOGUE H12): every entry Windows starts at sign in,
    /// with its switch. `folders` are the two Startup folders on this PC.
    pub fn startup_apps(&mut self, folders: &crate::startup::StartupFolders) -> crate::startup::StartupList {
        let (toggles, problems) = crate::startup::entries(&self.resolver, folders);
        let turn_on: Vec<_> = toggles
            .iter()
            .map(|t| crate::tweaks::startup::StartupToggle::turning_on(t.source, &t.name))
            .collect();
        self.relist(
            crate::tweaks::startup::ID_PREFIX,
            toggles
                .iter()
                .chain(&turn_on)
                .map(|t| (t.id().to_owned(), String::new())),
        );
        let mut apps: Vec<crate::startup::StartupApp> = toggles
            .into_iter()
            .zip(turn_on)
            .map(|(t, on)| crate::startup::StartupApp {
                tweak: self.view_of(&t),
                turn_on: self.view_of(&on),
                name: crate::tweaks::startup::display_name(t.source, &t.name),
                source: t.source,
                command: t.command(&self.resolver).ok().flatten(),
            })
            .collect();
        apps.sort_by_key(|a| a.name.to_lowercase());
        crate::startup::StartupList { apps, problems }
    }

    /// MSI mode (CATALOGUE H6) for each graphics card and network adapter on
    /// the PCI bus.
    pub fn msi_devices(&mut self) -> crate::tweaks::msi::MsiDeviceList {
        use crate::tweaks::msi::{MsiDevice, MsiDeviceList, MsiMode, ID_PREFIX};
        let devices = match self.resolver.pci_devices() {
            Ok(d) => d,
            Err(e) => {
                self.relist(ID_PREFIX, std::iter::empty());
                return MsiDeviceList {
                    devices: Vec::new(),
                    problem: Some(e.to_string()),
                };
            }
        };
        let modes: Vec<(MsiMode, crate::system::DeviceClass)> = devices
            .iter()
            .filter_map(|d| Some((MsiMode::new(&d.instance_id, &d.name)?, d.class)))
            .collect();
        self.relist(
            ID_PREFIX,
            modes.iter().map(|(m, _)| (m.id().to_owned(), m.name.clone())),
        );
        let mut devices: Vec<MsiDevice> = modes
            .iter()
            .map(|(m, class)| MsiDevice {
                tweak: self.view_of(m),
                class: *class,
            })
            .collect();
        devices.sort_by_key(|d| {
            (
                d.class != crate::system::DeviceClass::Display,
                d.tweak.metadata.name.to_string(),
            )
        });
        MsiDeviceList { devices, problem: None }
    }

    /// Replace the listed ids starting with `prefix`.
    fn relist(&mut self, prefix: &str, ids: impl Iterator<Item = (String, String)>) {
        self.listed.retain(|id, _| !id.starts_with(prefix));
        self.listed.extend(ids);
    }

    pub fn apply(&mut self, id: &str) -> Result<Vec<JournalEntry>> {
        // The catalogue, or a change for something listed on this PC. The
        // engine's own changes are made by the engine, never asked for by id.
        let slot = match self.slot_of(id)? {
            Slot::Internal(_) => {
                return Err(EngineError::UnknownTweak {
                    tweak_id: id.to_string(),
                })
            }
            // Only what this PC has, as last listed. Undo needs no list.
            Slot::Listed(t) if !self.listed.contains_key(t.id()) => {
                return Err(EngineError::UnknownTweak {
                    tweak_id: id.to_string(),
                })
            }
            slot => slot,
        };

        // Tier is enforced here, in Rust, against a license the UI cannot set.
        if let Some(reason) = self.tier_block(self.tweak_in(&slot)) {
            return Err(EngineError::Blocked { reason });
        }

        // Fresh state for this decision: the state probes (security, restore) are
        // re-read; hardware, which does not change under a running app, may come
        // from the 10-minute cache.
        self.probe.invalidate();
        self.rescan();
        if let PredicateOutcome::Block(reason) = self.tweak_in(&slot).evaluate_predicate(&self.env) {
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
        self.tweak_in(&slot).read_state(&self.resolver, applied)?;

        let Engine {
            resolver,
            journal,
            tweaks,
            ..
        } = self;
        let tweak = match &slot {
            Slot::Catalogue(i) => tweaks[*i].as_ref(),
            Slot::Listed(t) => t.as_ref(),
            Slot::Internal(_) => unreachable!("refused above"),
        };

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
        if result.is_ok() {
            self.offline_error = None;
        }
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
        let tweak = match &slot {
            Slot::Catalogue(i) => tweaks[*i].as_ref(),
            Slot::Internal(i) => internal[*i].as_ref(),
            Slot::Listed(t) => t.as_ref(),
        };

        let mut tx = Transaction::begin(tweak, resolver, journal, JournalAction::Revert)?;
        // A failed revert leaves the transaction uncommitted on purpose: the
        // applies stay outstanding and a retry finishes the job.
        let reverted = tweak.revert(&mut tx);
        // Even a failed revert may have changed things (some values restored):
        // cached probe results must not outlive it.
        let result = reverted.and_then(|()| tx.commit());
        self.probe.invalidate();
        if result.is_ok() {
            self.offline_error = None;
        }
        result
    }

    /// After a panic on a worker thread: drop what is held in memory about the
    /// journal and the probes and read them again from disk and Windows, so a
    /// revert works from what is really there, not from a half-updated index.
    pub fn recover_after_panic(&mut self) -> Result<()> {
        self.journal.reload()?;
        self.rescan_fresh();
        Ok(())
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

    /// Test only: simulate an in-memory journal that lost track of its file.
    #[cfg(test)]
    pub(crate) fn forget_journal_for_test(&mut self) {
        let empty = tempfile::tempdir().unwrap();
        let mut blank = Journal::open(&crate::secure_dir::TrustedDir::insecure_for_tests(empty.path())).unwrap();
        // Point the blank journal back at the real directory so reload() reads it.
        blank.set_root_for_test(self.journal.root().to_path_buf());
        self.journal = blank;
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

    /// Gaming Mode's changes for a game that just started (`play.rs`), when the
    /// user has Gaming Mode on. Each is its own journalled change under the
    /// rules Apply follows: a plan that includes it and a verified restore
    /// point. One whose result is already in place is left as it is, and so is
    /// one still in effect from a session that never ended.
    pub fn start_play_session(&mut self) -> Vec<SessionStep> {
        if !self.settings.gaming_mode {
            return Vec::new();
        }
        crate::tweaks::session::SESSION_IDS
            .iter()
            .map(|id| match self.make_session_change(id) {
                Ok(made) => SessionStep {
                    tweak_id: (*id).to_owned(),
                    made,
                    error: None,
                },
                Err(e) => SessionStep {
                    tweak_id: (*id).to_owned(),
                    made: false,
                    // A block's own sentence is what the user reads.
                    error: Some(match e {
                        EngineError::Blocked { reason } => reason.message,
                        other => other.to_string(),
                    }),
                },
            })
            .collect()
    }

    fn make_session_change(&mut self, id: &str) -> Result<bool> {
        let Slot::Internal(idx) = self.slot_of(id)? else {
            return Err(EngineError::Internal {
                detail: format!("{id} is not a Gaming Mode change"),
            });
        };
        if self.journal.is_applied(id) {
            return Ok(false);
        }
        if let Some(reason) = self.tier_block(self.internal[idx].as_ref()) {
            return Err(EngineError::Blocked { reason });
        }
        self.probe.invalidate();
        if !self.probe.restore_gate_open() {
            return Err(EngineError::Blocked {
                reason: BlockedReason::new(
                    BlockedCode::NoRestorePoint,
                    "There is no verified restore point, so there is nothing to roll back to.",
                ),
            });
        }
        let Engine {
            resolver,
            journal,
            internal,
            ..
        } = self;
        let tweak = internal[idx].as_ref();
        if tweak.read_state(resolver, false)? != TweakState::Default {
            return Ok(false);
        }
        let mut tx = Transaction::begin(tweak, resolver, journal, JournalAction::Apply)?;
        let result = match tweak.apply(&mut tx) {
            Ok(()) => tx.commit().map(|_| true),
            Err(e) => {
                let _ = tx.rollback();
                Err(e)
            }
        };
        self.probe.invalidate();
        result
    }

    /// Put back every Gaming Mode change still in effect: the game closed,
    /// Gaming Mode was turned off, or a session a crash left open.
    pub fn end_play_session(&mut self) -> Vec<RevertResult> {
        let open: Vec<&str> = crate::tweaks::session::SESSION_IDS
            .iter()
            .rev()
            .copied()
            .filter(|id| self.journal.is_applied(id))
            .collect();
        open.into_iter()
            .map(|id| match self.revert(id) {
                Ok(_) => RevertResult {
                    tweak_id: id.to_owned(),
                    ok: true,
                    error: None,
                },
                Err(e) => RevertResult {
                    tweak_id: id.to_owned(),
                    ok: false,
                    error: Some(e.to_string()),
                },
            })
            .collect()
    }

    /// This PC is connected over Wi-Fi only (`play::wifi_only`). False when
    /// the adapters cannot be listed: nothing is said then.
    pub fn on_wifi_only(&self) -> bool {
        self.resolver
            .network_adapters()
            .is_ok_and(|a| crate::play::wifi_only(&a))
    }

    /// Some Gaming Mode change is in effect.
    pub fn play_session_open(&self) -> bool {
        crate::tweaks::session::SESSION_IDS
            .iter()
            .any(|id| self.journal.is_applied(id))
    }

    /// Journal a restore point that was created and verified, and make the next
    /// gate check look at Windows again.
    pub fn record_restore_point(
        &mut self,
        sequence_number: u32,
        description: &str,
        method: crate::journal::RestoreMethod,
        protection_enabled_by_us: Option<bool>,
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

    /// Journal a one-time action that ran (Tools > One-time actions), for the
    /// history in Backups: what it did, or why it did not. It changes no
    /// setting, so there is nothing to undo.
    pub fn record_action(
        &mut self,
        action: OneTimeAction,
        outcome: std::result::Result<ActionDone, String>,
    ) -> Result<()> {
        let (done, error) = match outcome {
            Ok(done) => (Some(done), None),
            Err(error) => (None, Some(error)),
        };
        let seq = self.journal.take_seq();
        self.journal.append_action(ActionRecord {
            seq,
            unix_ms: now_ms(),
            action,
            done,
            error,
        })
    }

    pub fn journal_view(&self) -> JournalView {
        let applied = self
            .journal
            .applied_tweaks_newest_first()
            .into_iter()
            .map(|id| {
                let (name, kind) = match self.slot_of(&id) {
                    Ok(Slot::Catalogue(i)) => (self.tweaks[i].metadata().name.to_string(), ChangeKind::Catalogue),
                    Ok(Slot::Internal(i)) => (self.internal[i].metadata().name.to_string(), ChangeKind::Internal),
                    Ok(Slot::Listed(t)) => (t.metadata().name.to_string(), ChangeKind::Catalogue),
                    Err(_) => (id.clone(), ChangeKind::Retired),
                };
                AppliedChange {
                    tweak_id: id,
                    name,
                    kind,
                }
            })
            .collect();
        JournalView {
            applied,
            records: self.journal.records().to_vec(),
            warnings: self.journal.warnings().to_vec(),
            offline_error: self.offline_error.clone(),
        }
    }

    /// Bring the offline undo files in line with the journal. Run at start-up:
    /// an apply that a crash cut short is outstanding in the journal but never
    /// reached the commit that would have added it. A failure does not stop
    /// the app; it is kept and shown in Backups.
    pub fn refresh_offline_undo(&mut self) {
        self.offline_error = crate::transaction::refresh_offline_set(&self.resolver, &self.journal, None)
            .err()
            .map(|e| e.to_string());
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
        // Before anything reads or writes the journal: one engine at a time.
        let instance = crate::instance::InstanceLock::acquire(&dir)?;
        let resolver = ContextResolver::detect(elevated)?;
        let journal = Journal::open(&dir)?;
        let proof = Arc::new(build_proof_service(&dir));
        let mut engine = Self::new(resolver, journal, tweaks, probe, license)
            .with_proof_service(proof)
            .with_settings_in(&dir);
        engine._instance = Some(instance);
        engine.refresh_offline_undo();
        Ok(engine)
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
