//! Tauri command surface.
//!
//! Every command is async and does its engine work in `spawn_blocking`, so the
//! main thread (which also runs WebView2's message loop) is never blocked by
//! registry or disk I/O. The engine lives in `Arc<Mutex<_>>`; the lock is held
//! only for the duration of one engine call.
//!
//! No command accepts environment, license, tier or gate state: the engine
//! builds those itself. `tests::no_command_takes_system_env` enforces that.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, State};

use peaktweaks_engine::env::{GameInfo, KNOWN_GAMES};
use peaktweaks_engine::error::{EngineError, Result};
use peaktweaks_engine::journal::JournalEntry;
use peaktweaks_engine::{ContextInfo, Engine, JournalView, Progress, RevertResult, TweakView};

pub type SharedEngine = Arc<Mutex<Engine>>;

fn progress(app: &AppHandle, stage: &'static str, tweak_id: Option<&str>, message: impl Into<String>) {
    // Progress is advisory; a failed emit must never fail the operation.
    let _ = app.emit(
        "engine://progress",
        Progress {
            stage: stage.to_owned(),
            tweak_id: tweak_id.map(str::to_owned),
            message: message.into(),
        },
    );
}

/// Run engine work off the main thread. A poisoned lock means an earlier panic
/// left engine state untrustworthy, so it is surfaced, never papered over.
async fn blocking<T, F>(engine: &State<'_, SharedEngine>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&mut Engine) -> Result<T> + Send + 'static,
{
    let engine = Arc::clone(engine.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = engine.lock().map_err(|_| EngineError::Internal {
            detail: "engine state was poisoned by an earlier panic; restart PeakTweaks".into(),
        })?;
        f(&mut guard)
    })
    .await
    .map_err(|e| EngineError::Internal {
        detail: format!("engine worker failed: {e}"),
    })?
}

#[tauri::command]
pub async fn engine_context(engine: State<'_, SharedEngine>) -> Result<ContextInfo> {
    blocking(&engine, |e| Ok(e.context_info())).await
}

#[tauri::command]
pub async fn list_tweaks(engine: State<'_, SharedEngine>) -> Result<Vec<TweakView>> {
    blocking(&engine, |e| e.list()).await
}

#[tauri::command]
pub async fn list_games() -> Result<Vec<GameInfo>> {
    Ok(KNOWN_GAMES.to_vec())
}

/// Pick (or clear) the target game. The id is validated against the engine's
/// own list; the environment is then rebuilt in Rust.
#[tauri::command]
pub async fn select_target_game(engine: State<'_, SharedEngine>, game_id: Option<String>) -> Result<Vec<TweakView>> {
    blocking(&engine, move |e| {
        e.select_target_game(game_id)?;
        e.list()
    })
    .await
}

/// Re-run the probes inside the engine and return the refreshed list.
#[tauri::command]
pub async fn rescan(app: AppHandle, engine: State<'_, SharedEngine>) -> Result<Vec<TweakView>> {
    progress(&app, "rescan", None, "Checking this PC");
    let out = blocking(&engine, |e| {
        e.rescan();
        e.list()
    })
    .await;
    progress(&app, "rescan_done", None, "Done");
    out
}

#[tauri::command]
pub async fn apply_tweak(app: AppHandle, engine: State<'_, SharedEngine>, id: String) -> Result<Vec<JournalEntry>> {
    progress(&app, "apply", Some(&id), "Applying");
    let tid = id.clone();
    let out = blocking(&engine, move |e| e.apply(&tid)).await;
    progress(
        &app,
        if out.is_ok() { "apply_done" } else { "apply_failed" },
        Some(&id),
        "",
    );
    out
}

#[tauri::command]
pub async fn revert_tweak(app: AppHandle, engine: State<'_, SharedEngine>, id: String) -> Result<Vec<JournalEntry>> {
    progress(&app, "revert", Some(&id), "Undoing");
    let tid = id.clone();
    let out = blocking(&engine, move |e| e.revert(&tid)).await;
    progress(
        &app,
        if out.is_ok() { "revert_done" } else { "revert_failed" },
        Some(&id),
        "",
    );
    out
}

#[tauri::command]
pub async fn revert_all(app: AppHandle, engine: State<'_, SharedEngine>) -> Result<Vec<RevertResult>> {
    progress(&app, "revert_all", None, "Undoing everything");
    let out = blocking(&engine, |e| Ok(e.revert_all())).await;
    progress(&app, "revert_all_done", None, "Done");
    out
}

#[tauri::command]
pub async fn list_journal(engine: State<'_, SharedEngine>) -> Result<JournalView> {
    blocking(&engine, |e| Ok(e.journal_view())).await
}

#[cfg(test)]
mod tests {

    /// The names registered with `generate_handler!` and listed in the app manifest
    /// (`build.rs`). A test keeps the two lists, and `main.rs`, in step.
    const COMMAND_NAMES: &[&str] = &[
        "engine_context",
        "list_tweaks",
        "list_games",
        "select_target_game",
        "rescan",
        "apply_tweak",
        "revert_tweak",
        "revert_all",
        "list_journal",
    ];

    const SOURCE: &str = include_str!("commands.rs");

    /// Every `#[tauri::command]` function's parameter list, as text.
    fn command_signatures() -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = SOURCE;
        while let Some(i) = rest.find("#[tauri::command]") {
            rest = &rest[i + "#[tauri::command]".len()..];
            let fn_at = rest.find("fn ").expect("fn after attribute");
            let open = rest[fn_at..].find('(').unwrap() + fn_at;
            // Find the matching close paren.
            let mut depth = 0;
            let mut end = open;
            for (k, ch) in rest[open..].char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + k;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            out.push(rest[fn_at..=end].to_string());
        }
        out
    }

    #[test]
    fn the_signature_scanner_finds_every_command() {
        assert_eq!(command_signatures().len(), COMMAND_NAMES.len());
    }

    #[test]
    fn no_command_takes_system_env() {
        // The environment is built by the engine from probes; nothing the
        // webview sends may become one. Same for the license and tier.
        for sig in command_signatures() {
            for banned in ["SystemEnv", "License", "Tier", "StubProbe", "EnvProbe"] {
                assert!(!sig.contains(banned), "command takes {banned}: {sig}");
            }
        }
    }

    #[test]
    fn no_command_named_set_environment() {
        assert!(!COMMAND_NAMES.iter().any(|n| n.contains("environment")));
        assert!(!SOURCE.contains(concat!("fn set_", "environment")));
    }

    #[test]
    fn command_list_matches_main_and_the_app_manifest() {
        let main = include_str!("main.rs");
        let build = include_str!("../build.rs");
        for name in COMMAND_NAMES {
            assert!(
                main.contains(&format!("commands::{name},")),
                "{name} missing from generate_handler!"
            );
            assert!(
                build.contains(&format!("\"{name}\"")),
                "{name} missing from build.rs manifest"
            );
            assert!(SOURCE.contains(&format!("fn {name}(")), "{name} has no fn");
        }
    }
}
