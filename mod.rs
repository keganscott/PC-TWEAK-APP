//! Engine orchestration and Tauri command surface.

pub mod context;
pub mod error;
pub mod journal;
pub mod types;

use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;

use context::{ContextResolver, UserResolution};
use error::{EngineError, Result};
use journal::{Journal, JournalAction, JournalEntry, Transaction};
use types::{
    BlockedCode, BlockedReason, ExecutionContext, PredicateOutcome, SystemEnv, Tweak, TweakMetadata, TweakState,
};

/// A tweak plus its live state, as sent to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct TweakView {
    #[serde(flatten)]
    pub metadata: TweakMetadata,
    pub context: ExecutionContext,
    pub state: TweakState,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextInfo {
    pub sid: String,
    pub resolution: UserResolution,
    pub is_self: bool,
    pub elevated: bool,
}

pub struct Engine {
    resolver: ContextResolver,
    journal: Journal,
    tweaks: Vec<Box<dyn Tweak>>,
    env: SystemEnv,
}

impl Engine {
    pub fn new(app_data: &Path, elevated: bool, tweaks: Vec<Box<dyn Tweak>>) -> Result<Self> {
        let resolver = ContextResolver::detect(elevated)?;
        let journal = Journal::open(app_data)?;
        let env = SystemEnv { elevated, ..Default::default() };
        Ok(Self { resolver, journal, tweaks, env })
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

    /// Replace the environment snapshot; predicates re-evaluate against it on
    /// the next `list`. Called when the user picks a different target game or
    /// after a hardware rescan.
    pub fn set_env(&mut self, env: SystemEnv) {
        self.env = env;
    }

    pub fn env(&self) -> &SystemEnv {
        &self.env
    }

    fn find(&self, id: &str) -> Result<&dyn Tweak> {
        self.tweaks
            .iter()
            .find(|t| t.id() == id)
            .map(|b| b.as_ref())
            .ok_or_else(|| EngineError::UnknownTweak { tweak_id: id.to_string() })
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
                    let applied = self.journal.is_applied(tweak.id())?;
                    // A read failure is a state, not a crash — a missing key on
                    // an unusual SKU should degrade to Default, not kill the list.
                    tweak
                        .read_state(&self.resolver, applied)
                        .unwrap_or(TweakState::Default)
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
        let tweak = self.find(id)?;

        if let PredicateOutcome::Block(reason) = tweak.evaluate_predicate(&self.env) {
            return Err(EngineError::Blocked { reason });
        }

        // Refuse to mutate without a rollback point. The frontend gates this
        // too, but the gate that matters is the one closest to the write.
        if !self.env.system_protection_enabled {
            return Err(EngineError::Blocked {
                reason: BlockedReason::new(
                    BlockedCode::NoRestorePoint,
                    "System protection is off, so there is nothing to roll back to.",
                ),
            });
        }

        let (id_static, ctx) = (tweak.id(), tweak.execution_context());
        // Re-borrow: `tweak` borrows self immutably, the transaction needs the
        // journal mutably. Resolve the pointer before splitting the borrow.
        let idx = self
            .tweaks
            .iter()
            .position(|t| t.id() == id_static)
            .ok_or_else(|| EngineError::UnknownTweak { tweak_id: id.to_string() })?;

        let Engine { resolver, journal, tweaks, .. } = self;
        let tweak = &tweaks[idx];

        let mut tx = Transaction::begin(id_static, ctx, resolver, journal, JournalAction::Apply)?;
        tweak.apply(&mut tx)?;
        Ok(tx.written().to_vec())
    }

    pub fn revert(&mut self, id: &str) -> Result<Vec<JournalEntry>> {
        let idx = self
            .tweaks
            .iter()
            .position(|t| t.id() == id)
            .ok_or_else(|| EngineError::UnknownTweak { tweak_id: id.to_string() })?;

        let (id_static, ctx) = (self.tweaks[idx].id(), self.tweaks[idx].execution_context());
        let Engine { resolver, journal, tweaks, .. } = self;
        let tweak = &tweaks[idx];

        let mut tx = Transaction::begin(id_static, ctx, resolver, journal, JournalAction::Revert)?;
        tweak.revert(&mut tx)?;
        Ok(tx.written().to_vec())
    }

    /// Revert everything with an outstanding apply, newest first. A failure on
    /// one tweak does not abort the rest — a partial revert is strictly better
    /// than stopping halfway and leaving the user to work out what happened.
    pub fn revert_all(&mut self) -> Result<Vec<(String, Option<String>)>> {
        let ids: Vec<String> = self
            .tweaks
            .iter()
            .map(|t| t.id().to_string())
            .filter(|id| self.journal.is_applied(id).unwrap_or(false))
            .collect();

        let mut results = Vec::new();
        for id in ids.into_iter().rev() {
            match self.revert(&id) {
                Ok(_) => results.push((id, None)),
                Err(e) => results.push((id, Some(e.to_string()))),
            }
        }
        Ok(results)
    }

    pub fn journal_entries(&self) -> Result<Vec<JournalEntry>> {
        self.journal.entries()
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

pub type EngineState = Mutex<Engine>;

/// The mutex is poisoned only if a command panicked while holding it, which
/// means engine state is untrustworthy. Surface that rather than papering over
/// it with `into_inner`.
fn lock(state: &EngineState) -> Result<std::sync::MutexGuard<'_, Engine>> {
    state.lock().map_err(|_| EngineError::UserContextUnresolved {
        detail: "engine state was poisoned by an earlier panic; restart PeakTweaks".into(),
    })
}

#[tauri::command]
pub fn engine_context(state: tauri::State<'_, EngineState>) -> Result<ContextInfo> {
    Ok(lock(&state)?.context_info())
}

#[tauri::command]
pub fn list_tweaks(state: tauri::State<'_, EngineState>) -> Result<Vec<TweakView>> {
    lock(&state)?.list()
}

#[tauri::command]
pub fn set_environment(state: tauri::State<'_, EngineState>, env: SystemEnv) -> Result<Vec<TweakView>> {
    let mut engine = lock(&state)?;
    engine.set_env(env);
    engine.list()
}

#[tauri::command]
pub fn apply_tweak(state: tauri::State<'_, EngineState>, id: String) -> Result<Vec<JournalEntry>> {
    lock(&state)?.apply(&id)
}

#[tauri::command]
pub fn revert_tweak(state: tauri::State<'_, EngineState>, id: String) -> Result<Vec<JournalEntry>> {
    lock(&state)?.revert(&id)
}

#[tauri::command]
pub fn revert_all(state: tauri::State<'_, EngineState>) -> Result<Vec<(String, Option<String>)>> {
    lock(&state)?.revert_all()
}

#[tauri::command]
pub fn list_journal(state: tauri::State<'_, EngineState>) -> Result<Vec<JournalEntry>> {
    lock(&state)?.journal_entries()
}
