//! Tauri command surface. Scaffold version: thin wrappers over the engine.

use std::sync::Mutex;

use peaktweaks_engine::error::{EngineError, Result};
use peaktweaks_engine::journal::JournalEntry;
use peaktweaks_engine::types::SystemEnv;
use peaktweaks_engine::{ContextInfo, Engine, TweakView};

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
