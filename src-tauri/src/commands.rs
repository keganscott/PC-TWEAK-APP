//! Tauri command surface.
//!
//! Every command is async and does its engine work in `spawn_blocking`, so the
//! main thread (which also runs WebView2's message loop) is never blocked by
//! registry or disk I/O. The engine lives in `Arc<Mutex<_>>`; the lock is held
//! only for the duration of one engine call.
//!
//! No command accepts environment, license, tier or gate state: the engine
//! builds those itself. `tests::no_command_takes_system_env` enforces that.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use tauri::{AppHandle, Emitter, State};

use peaktweaks_engine::cleanup::{self, AreaSize, CleanupArea, CleanupReport};
use peaktweaks_engine::drive_optimize::{self, DriveOptimization};
use peaktweaks_engine::env::{GameInfo, KNOWN_GAMES};
use peaktweaks_engine::error::{EngineError, Result};
use peaktweaks_engine::journal::{now_ms, ActionDone, JournalEntry, OneTimeAction};
use peaktweaks_engine::launch;
use peaktweaks_engine::memory::{self, StandbyPurge};
use peaktweaks_engine::netcheck::{self, NetworkCheck};
use peaktweaks_engine::play::PlayStatus;
use peaktweaks_engine::proof::service::{BeginSession, ProofService};
use peaktweaks_engine::proof::store::{ProofRun, ProofSession, ProofSessionSummary, Side};
use peaktweaks_engine::proof::verdict::Comparison;
use peaktweaks_engine::restore::{create_restore_point as run_create_restore_point, RestoreOutcome};
use peaktweaks_engine::settings::Settings;
use peaktweaks_engine::startup::{StartupFolders, StartupList};
use peaktweaks_engine::tweaks::msi::MsiDeviceList;
use peaktweaks_engine::{ContextInfo, Engine, JournalView, Progress, RevertResult, SystemAudit, TweakView};

use crate::play::SharedPlay;

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

    /// The engine, when it started (for the game watcher).
    pub fn shared(&self) -> Option<SharedEngine> {
        self.0.as_ref().ok().cloned()
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

/// Live readings for Home: processor, memory and NVIDIA GPUs (`live.rs`).
/// Reads only and takes no engine lock, so it never waits behind a change.
#[tauri::command]
pub async fn live_readings() -> Result<peaktweaks_engine::live::LiveReadings> {
    tauri::async_runtime::spawn_blocking(peaktweaks_engine::live::read)
        .await
        .map_err(|e| EngineError::Internal {
            detail: format!("the live readings stopped: {e}"),
        })
}

#[tauri::command]
pub async fn list_games() -> Result<Vec<GameInfo>> {
    Ok(KNOWN_GAMES.to_vec())
}

/// Which known game is running and what is in effect for it (the game watcher,
/// `play.rs`). Before the watcher's first look, or when the engine did not
/// start, no game and nothing in effect.
#[tauri::command]
pub async fn play_status(play: State<'_, SharedPlay>) -> Result<PlayStatus> {
    Ok(play.lock().unwrap_or_else(PoisonError::into_inner).clone())
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
    let activity = Activity::start("a Proof recording")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _activity = activity;
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

/// Short one-time actions (emptying the standby list, looking for junk files)
/// would still disturb a Proof recording, so they wait for it. Long ones take
/// the activity slot instead.
fn refuse_while_recording(shared: &SharedEngine, what: &str) -> Result<()> {
    if proof_service(shared).is_ok_and(|svc| svc.is_capturing()) {
        return Err(EngineError::Internal {
            detail: format!("a Proof recording is running; {what} after it finishes"),
        });
    }
    Ok(())
}

/// The long-running work that keeps the disk busy (a Proof recording, a junk
/// cleanup, a drive optimisation) runs one at a time: a recording made while
/// the disk is busy would measure that too.
static ACTIVITY: Mutex<Option<&'static str>> = Mutex::new(None);

/// Holds the activity slot until dropped; moved into the worker, so the slot
/// stays taken as long as the work runs.
#[must_use]
struct Activity;

impl Activity {
    /// `name` says what runs, for the refusal another start gets.
    fn start(name: &'static str) -> Result<Self> {
        let mut slot = ACTIVITY.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(running) = *slot {
            return Err(EngineError::Internal {
                detail: format!("{running} is running; try again when it finishes"),
            });
        }
        *slot = Some(name);
        Ok(Self)
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        *ACTIVITY.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
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

/// Keep a line in the journal for a one-time action that ran, for the history
/// in Backups. The action has already happened, so neither a poisoned engine
/// nor a failed append makes it a reported failure, which would invite running
/// it again (as with a change's side effects).
fn record_action(shared: &SharedEngine, action: OneTimeAction, outcome: std::result::Result<ActionDone, String>) {
    if let Ok(mut engine) = shared.lock() {
        let _ = engine.record_action(action, outcome);
    }
}

/// Empty Windows' standby list (catalogue E6). It changes no setting, so it
/// needs no restore point and leaves nothing to undo. Refused while a Proof
/// capture records, because it would disturb the measurement.
#[tauri::command]
pub async fn purge_standby_memory(app: AppHandle, engine: State<'_, EngineHandle>) -> Result<StandbyPurge> {
    let shared = engine.get()?;
    refuse_while_recording(&shared, "empty the standby list")?;
    progress(&app, "standby", None, "Emptying the standby list");
    let out = tauri::async_runtime::spawn_blocking(move || {
        let out = memory::purge_standby(memory::system().as_ref(), now_ms());
        let outcome = out.as_ref().map(StandbyPurge::done).map_err(ToString::to_string);
        record_action(&shared, OneTimeAction::PurgeStandby, outcome);
        out
    })
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

/// Ask this PC's router and two public DNS servers for echoes and report the
/// round trips, jitter and lost echoes (catalogue E4). Sends only ICMP echo
/// requests, only when the user starts it, and changes nothing. Refused while
/// a Proof capture records, so the measurement has the network to itself.
#[tauri::command]
pub async fn check_connection(app: AppHandle, engine: State<'_, EngineHandle>) -> Result<NetworkCheck> {
    let shared = engine.get()?;
    refuse_while_recording(&shared, "check the connection")?;
    progress(&app, "netcheck", None, "Checking the connection");
    let out = tauri::async_runtime::spawn_blocking(|| {
        netcheck::run(
            netcheck::system().as_ref(),
            netcheck::ECHOES,
            std::time::Duration::from_millis(netcheck::GAP_MS),
            now_ms(),
        )
    })
    .await
    .map_err(|e| EngineError::Internal {
        detail: format!("connection check worker failed: {e}"),
    });
    progress(
        &app,
        if out.is_ok() {
            "netcheck_done"
        } else {
            "netcheck_failed"
        },
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

/// The programs Windows starts when the user signs in, each with its switch
/// (catalogue H12). Reads only; turning one off is `apply_tweak` with its id.
#[tauri::command]
pub async fn list_startup_apps(engine: State<'_, EngineHandle>) -> Result<StartupList> {
    let sid = user_sid(&engine.get()?)?;
    blocking(&engine, move |e| {
        let folders = StartupFolders::from_places(&cleanup::places(&sid));
        Ok(e.startup_apps(&folders))
    })
    .await
}

/// MSI mode for each graphics card and network adapter on the PCI bus
/// (catalogue H6). Reads only; a device's change is `apply_tweak` with its id.
#[tauri::command]
pub async fn list_msi_devices(engine: State<'_, EngineHandle>) -> Result<MsiDeviceList> {
    blocking(&engine, |e| Ok(e.msi_devices())).await
}

/// Ask Steam to start a game the last scan found in a Steam library (new
/// ideas #4). The engine builds Steam's link from its own findings; it is
/// opened through the Windows desktop as the signed-in user, so the game never
/// gets PeakTweaks' administrator rights (`launch.rs`). Changes nothing.
#[tauri::command]
pub async fn launch_game(engine: State<'_, EngineHandle>, game_id: String) -> Result<()> {
    let link = blocking(&engine, move |e| e.launch_link(&game_id)).await?;
    tauri::async_runtime::spawn_blocking(move || launch::open_unelevated(&link))
        .await
        .map_err(|e| EngineError::Internal {
            detail: format!("launch worker failed: {e}"),
        })?
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
    let sid = user_sid(&shared)?;
    let activity = Activity::start("a junk cleanup")?;
    let worker_app = app.clone();
    let out = tauri::async_runtime::spawn_blocking(move || {
        let _activity = activity;
        let report = cleanup::clean(&cleanup::places(&sid), &areas, SystemTime::now(), now_ms(), &|area| {
            progress(&worker_app, "cleanup", None, format!("Clearing {}", area.label()))
        });
        record_action(&shared, OneTimeAction::Cleanup, Ok(report.done()));
        report
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

/// Run Windows' own drive optimisation on the Windows drive now (catalogue
/// H29). Changes no setting, so no restore point and nothing to undo. Takes an
/// hour or more on a hard drive; the engine lock is not held meanwhile.
#[tauri::command]
pub async fn optimize_drive(app: AppHandle, engine: State<'_, EngineHandle>) -> Result<DriveOptimization> {
    let shared = engine.get()?;
    let activity = Activity::start("a drive optimization")?;
    progress(&app, "drive", None, "Optimizing the Windows drive");
    let out = tauri::async_runtime::spawn_blocking(move || {
        let _activity = activity;
        let out = drive_optimize::windows_drive()
            .and_then(|drive| drive_optimize::optimize(drive_optimize::system().as_ref(), &drive, now_ms()));
        let outcome = out.as_ref().map(DriveOptimization::done).map_err(ToString::to_string);
        record_action(&shared, OneTimeAction::OptimizeDrive, outcome);
        out
    })
    .await
    .map_err(|e| EngineError::Internal {
        detail: format!("drive worker failed: {e}"),
    })?;
    progress(&app, if out.is_ok() { "drive_done" } else { "drive_failed" }, None, "");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_work_runs_one_at_a_time() {
        let first = Activity::start("a drive optimization").unwrap();
        let Err(EngineError::Internal { detail }) = Activity::start("a Proof recording") else {
            panic!("a second start was allowed");
        };
        assert_eq!(detail, "a drive optimization is running; try again when it finishes");
        drop(first);
        drop(Activity::start("a Proof recording").expect("free again once the first ends"));
    }
}
