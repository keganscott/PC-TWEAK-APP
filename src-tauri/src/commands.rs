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
use std::time::SystemTime;

use tauri::{AppHandle, Emitter, State};

use peaktweaks_engine::cleanup::{self, AreaSize, CleanupArea, CleanupReport};
use peaktweaks_engine::env::{GameInfo, KNOWN_GAMES};
use peaktweaks_engine::error::{EngineError, Result};
use peaktweaks_engine::journal::{now_ms, JournalEntry};
use peaktweaks_engine::memory::{self, StandbyPurge};
use peaktweaks_engine::proof::service::{BeginSession, ProofService};
use peaktweaks_engine::proof::store::{ProofRun, ProofSession, ProofSessionSummary, Side};
use peaktweaks_engine::proof::verdict::Comparison;
use peaktweaks_engine::restore::{create_restore_point as run_create_restore_point, RestoreOutcome};
use peaktweaks_engine::settings::Settings;
use peaktweaks_engine::{ContextInfo, Engine, JournalView, Progress, RevertResult, SystemAudit, TweakView};

pub type SharedEngine = Arc<Mutex<Engine>>;

/// What the commands reach the engine through. If the engine could not start,
/// the window still opens and every command answers with the reason, so the UI
/// can show it (plan section 7: boot failure screen) instead of the app
/// vanishing with nothing on screen.
pub struct EngineHandle(std::result::Result<SharedEngine, EngineError>);

impl EngineHandle {
    pub fn ready(engine: Engine) -> Self {
        Self(Ok(Arc::new(Mutex::new(engine))))
    }

    pub fn failed(error: EngineError) -> Self {
        Self(Err(error))
    }

    fn get(&self) -> Result<SharedEngine> {
        self.0.clone()
    }
}

fn progress(app: &AppHandle, stage: &str, tweak_id: Option<&str>, message: impl Into<String>) {
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
async fn blocking<T, F>(engine: &State<'_, EngineHandle>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&mut Engine) -> Result<T> + Send + 'static,
{
    run(engine, false, f).await
}

/// Like `blocking`, for getting back to how things were. Undoing must always be
/// possible, so after a panic this re-reads the journal and probes from disk and
/// Windows, clears the poison, and carries on, instead of asking for a restart.
async fn blocking_recovering<T, F>(engine: &State<'_, EngineHandle>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&mut Engine) -> Result<T> + Send + 'static,
{
    run(engine, true, f).await
}

async fn run<T, F>(engine: &State<'_, EngineHandle>, recover: bool, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&mut Engine) -> Result<T> + Send + 'static,
{
    let engine = engine.get()?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = match engine.lock() {
            Ok(guard) => guard,
            Err(poisoned) if recover => {
                let mut guard = poisoned.into_inner();
                guard.recover_after_panic()?;
                engine.clear_poison();
                guard
            }
            Err(_) => {
                return Err(EngineError::Internal {
                    detail: "engine state was poisoned by an earlier panic; restart PeakTweaks".into(),
                })
            }
        };
        f(&mut guard)
    })
    .await
    .map_err(|e| EngineError::Internal {
        detail: format!("engine worker failed: {e}"),
    })?
}

#[tauri::command]
pub async fn engine_context(engine: State<'_, EngineHandle>) -> Result<ContextInfo> {
    blocking(&engine, |e| Ok(e.context_info())).await
}

#[tauri::command]
pub async fn list_tweaks(engine: State<'_, EngineHandle>) -> Result<Vec<TweakView>> {
    // `rescan` reuses recent probe results, so this is cheap after the first call.
    blocking(&engine, |e| {
        e.rescan();
        e.list()
    })
    .await
}

/// The user's preferences (rig-class override, wording). Preferences only: they
/// change defaults and copy, never a gate, licence or safety check.
#[tauri::command]
pub async fn get_settings(engine: State<'_, EngineHandle>) -> Result<Settings> {
    blocking(&engine, |e| Ok(e.settings())).await
}

/// Replace the preferences. The engine saves them before it uses them and
/// returns what is now stored.
#[tauri::command]
pub async fn set_settings(engine: State<'_, EngineHandle>, settings: Settings) -> Result<Settings> {
    blocking(&engine, move |e| e.set_settings(settings)).await
}

#[tauri::command]
pub async fn list_games() -> Result<Vec<GameInfo>> {
    Ok(KNOWN_GAMES.to_vec())
}

/// Pick (or clear) the target game. The id is validated against the engine's
/// own list; the environment is then rebuilt in Rust.
#[tauri::command]
pub async fn select_target_game(engine: State<'_, EngineHandle>, game_id: Option<String>) -> Result<Vec<TweakView>> {
    blocking(&engine, move |e| {
        e.select_target_game(game_id)?;
        e.list()
    })
    .await
}

/// Re-run every probe inside the engine (nothing cached) and return the
/// refreshed list.
#[tauri::command]
pub async fn rescan(app: AppHandle, engine: State<'_, EngineHandle>) -> Result<Vec<TweakView>> {
    progress(&app, "rescan", None, "Checking this PC");
    let out = blocking(&engine, |e| {
        e.rescan_fresh();
        e.list()
    })
    .await;
    progress(&app, "rescan_done", None, "Done");
    out
}

/// Probe this PC and report what was found: hardware, security state, restore
/// state and anti-cheat readiness. Every field is Yes / No / Unknown, and the
/// whole thing is built inside the engine.
#[tauri::command]
pub async fn audit_system(app: AppHandle, engine: State<'_, EngineHandle>) -> Result<SystemAudit> {
    progress(&app, "audit", None, "Checking this PC");
    let out = blocking(&engine, |e| Ok(e.audit())).await;
    progress(&app, "audit_done", None, "Done");
    out
}

/// Turn on System Protection if needed, create a restore point and prove
/// Windows recorded it. The slow Windows calls run without the engine lock held.
#[tauri::command]
pub async fn create_restore_point(app: AppHandle, engine: State<'_, EngineHandle>) -> Result<RestoreOutcome> {
    let shared = engine.get()?;
    tauri::async_runtime::spawn_blocking(move || {
        let svc = shared
            .lock()
            .map_err(|_| EngineError::Internal {
                detail: "engine state was poisoned by an earlier panic; restart PeakTweaks".into(),
            })?
            .restore_service()
            .ok_or_else(|| EngineError::Internal {
                detail: "this build has no restore-point service".into(),
            })?;
        let result = run_create_restore_point(&shared, &svc, &|stage, message| progress(&app, stage, None, message));
        progress(
            &app,
            if result.is_ok() {
                "restore_done"
            } else {
                "restore_failed"
            },
            None,
            "",
        );
        result
    })
    .await
    .map_err(|e| EngineError::Internal {
        detail: format!("restore worker failed: {e}"),
    })?
}

/// The proof service, fetched under a brief engine lock.
fn proof_service(shared: &SharedEngine) -> Result<Arc<ProofService>> {
    shared
        .lock()
        .map_err(|_| EngineError::Internal {
            detail: "engine state was poisoned by an earlier panic; restart PeakTweaks".into(),
        })?
        .proof_service()
        .ok_or_else(|| EngineError::Internal {
            detail: "this build has no proof service".into(),
        })
}

/// Start a before/after comparison for a game. The free plan allows one.
#[tauri::command]
pub async fn proof_begin_session(
    engine: State<'_, EngineHandle>,
    exe: String,
    game_id: Option<String>,
    game_build: Option<String>,
) -> Result<ProofSession> {
    let shared = engine.get()?;
    tauri::async_runtime::spawn_blocking(move || {
        let (rig, tier) = shared
            .lock()
            .map_err(|_| EngineError::Internal {
                detail: "engine state was poisoned by an earlier panic; restart PeakTweaks".into(),
            })?
            .proof_context();
        proof_service(&shared)?.begin_session(
            BeginSession {
                exe,
                game_id,
                game_build,
            },
            rig,
            tier,
        )
    })
    .await
    .map_err(|e| EngineError::Internal {
        detail: format!("proof worker failed: {e}"),
    })?
}

/// Capture one run on one side of a session. Takes as long as the delay plus the
/// capture length; the engine lock is not held meanwhile.
#[tauri::command]
pub async fn proof_capture(
    app: AppHandle,
    engine: State<'_, EngineHandle>,
    session_id: String,
    side: Side,
    seconds: u32,
    delay_seconds: u32,
) -> Result<ProofRun> {
    let shared = engine.get()?;
    tauri::async_runtime::spawn_blocking(move || {
        let (svc, applied) = {
            let guard = shared.lock().map_err(|_| EngineError::Internal {
                detail: "engine state was poisoned by an earlier panic; restart PeakTweaks".into(),
            })?;
            (
                guard.proof_service().ok_or_else(|| EngineError::Internal {
                    detail: "this build has no proof service".into(),
                })?,
                guard.applied_tweak_ids(),
            )
        };
        let result = svc.capture(&session_id, side, seconds, delay_seconds, applied, &|stage, message| {
            progress(&app, stage, None, message)
        });
        progress(
            &app,
            if result.is_ok() { "proof_done" } else { "proof_failed" },
            None,
            "",
        );
        result
    })
    .await
    .map_err(|e| EngineError::Internal {
        detail: format!("proof worker failed: {e}"),
    })?
}

/// Better / no measurable change / worse, from the stored runs only.
#[tauri::command]
pub async fn proof_compare(engine: State<'_, EngineHandle>, session_id: String) -> Result<Comparison> {
    let shared = engine.get()?;
    tauri::async_runtime::spawn_blocking(move || proof_service(&shared)?.compare(&session_id))
        .await
        .map_err(|e| EngineError::Internal {
            detail: format!("proof worker failed: {e}"),
        })?
}

#[tauri::command]
pub async fn proof_list_sessions(engine: State<'_, EngineHandle>) -> Result<Vec<ProofSessionSummary>> {
    let shared = engine.get()?;
    tauri::async_runtime::spawn_blocking(move || proof_service(&shared)?.sessions())
        .await
        .map_err(|e| EngineError::Internal {
            detail: format!("proof worker failed: {e}"),
        })?
}

#[tauri::command]
pub async fn proof_runs(engine: State<'_, EngineHandle>, session_id: String) -> Result<Vec<ProofRun>> {
    let shared = engine.get()?;
    tauri::async_runtime::spawn_blocking(move || proof_service(&shared)?.runs(&session_id))
        .await
        .map_err(|e| EngineError::Internal {
            detail: format!("proof worker failed: {e}"),
        })?
}

#[tauri::command]
pub async fn apply_tweak(app: AppHandle, engine: State<'_, EngineHandle>, id: String) -> Result<Vec<JournalEntry>> {
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
pub async fn revert_tweak(app: AppHandle, engine: State<'_, EngineHandle>, id: String) -> Result<Vec<JournalEntry>> {
    progress(&app, "revert", Some(&id), "Undoing");
    let tid = id.clone();
    let out = blocking_recovering(&engine, move |e| e.revert(&tid)).await;
    progress(
        &app,
        if out.is_ok() { "revert_done" } else { "revert_failed" },
        Some(&id),
        "",
    );
    out
}

#[tauri::command]
pub async fn revert_all(app: AppHandle, engine: State<'_, EngineHandle>) -> Result<Vec<RevertResult>> {
    progress(&app, "revert_all", None, "Undoing everything");
    let out = blocking_recovering(&engine, |e| Ok(e.revert_all())).await;
    progress(&app, "revert_all_done", None, "Done");
    out
}

#[tauri::command]
pub async fn list_journal(engine: State<'_, EngineHandle>) -> Result<JournalView> {
    blocking_recovering(&engine, |e| Ok(e.journal_view())).await
}

/// One-time actions read or delete a lot at once, which would disturb a Proof
/// recording, so they wait for it.
fn refuse_while_recording(shared: &SharedEngine, what: &str) -> Result<()> {
    if proof_service(shared).is_ok_and(|svc| svc.is_capturing()) {
        return Err(EngineError::Internal {
            detail: format!("a Proof recording is running; {what} after it finishes"),
        });
    }
    Ok(())
}

/// The interactive user's SID, as the engine resolved it at start: whose
/// folders the junk cleaner looks in.
fn user_sid(shared: &SharedEngine) -> Result<String> {
    Ok(shared
        .lock()
        .map_err(|_| EngineError::Internal {
            detail: "engine state was poisoned by an earlier panic; restart PeakTweaks".into(),
        })?
        .context_info()
        .sid)
}

/// Empty Windows' standby list (catalogue E6). It changes no setting, so it
/// needs no restore point and leaves nothing to undo. Refused while a Proof
/// capture records, because it would disturb the measurement.
#[tauri::command]
pub async fn purge_standby_memory(app: AppHandle, engine: State<'_, EngineHandle>) -> Result<StandbyPurge> {
    refuse_while_recording(&engine.get()?, "empty the standby list")?;
    progress(&app, "standby", None, "Emptying the standby list");
    let out = tauri::async_runtime::spawn_blocking(|| memory::purge_standby(memory::system().as_ref(), now_ms()))
        .await
        .map_err(|e| EngineError::Internal {
            detail: format!("memory worker failed: {e}"),
        })?;
    progress(
        &app,
        if out.is_ok() { "standby_done" } else { "standby_failed" },
        None,
        "",
    );
    out
}

/// What each junk-file area holds that a cleanup would delete now (catalogue
/// H28). Reads only.
#[tauri::command]
pub async fn cleanup_measure(engine: State<'_, EngineHandle>) -> Result<Vec<AreaSize>> {
    let shared = engine.get()?;
    refuse_while_recording(&shared, "look for junk files")?;
    let sid = user_sid(&shared)?;
    tauri::async_runtime::spawn_blocking(move || {
        cleanup::measure(&cleanup::places(&sid), &CleanupArea::ALL, SystemTime::now())
    })
    .await
    .map_err(|e| EngineError::Internal {
        detail: format!("cleanup worker failed: {e}"),
    })
}

/// Delete the junk files in the chosen areas. Cannot be undone; the screen
/// shows the sizes and asks first. Changes no setting, so no restore point.
#[tauri::command]
pub async fn cleanup_run(
    app: AppHandle,
    engine: State<'_, EngineHandle>,
    areas: Vec<CleanupArea>,
) -> Result<CleanupReport> {
    let shared = engine.get()?;
    refuse_while_recording(&shared, "clear junk files")?;
    let sid = user_sid(&shared)?;
    let worker_app = app.clone();
    let out = tauri::async_runtime::spawn_blocking(move || {
        cleanup::clean(&cleanup::places(&sid), &areas, SystemTime::now(), now_ms(), &|area| {
            progress(&worker_app, "cleanup", None, format!("Clearing {}", area.label()))
        })
    })
    .await
    .map_err(|e| EngineError::Internal {
        detail: format!("cleanup worker failed: {e}"),
    });
    progress(
        &app,
        if out.is_ok() { "cleanup_done" } else { "cleanup_failed" },
        None,
        "",
    );
    out
}
