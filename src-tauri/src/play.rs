//! The game watcher (CATALOGUE step 5). A thread of its own looks at the names
//! of the running programs every few seconds (`peaktweaks_engine::play`); while
//! a known game runs it keeps Gaming Mode's changes and the timer request in
//! effect, as the user's settings say, and puts them back when the game closes.
//! What it sees goes to the UI as the `engine://play` event and the
//! `play_status` command.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use peaktweaks_engine::journal::now_ms;
use peaktweaks_engine::play::{
    game_processes, game_running, play_look, running_images, watched_ids, PlayReports, PlayStatus, TimerRequest, Watch,
    WatchEvent,
};

use crate::commands::SharedEngine;

pub type SharedPlay = Arc<Mutex<PlayStatus>>;

const LOOK_EVERY: Duration = Duration::from_secs(3);
/// Gaming Mode that could not be made (no restore point yet, say) is tried
/// again this many looks later: about once a minute.
const RETRY_AFTER_LOOKS: u32 = 20;

pub fn new_status() -> SharedPlay {
    Arc::new(Mutex::new(PlayStatus::watching()))
}

/// What the watcher holds between looks.
#[derive(Default)]
struct Session {
    timer: Option<TimerRequest>,
    /// Gaming Mode was tried for this game (and these settings): a failure,
    /// such as no restore point yet, is not retried every few seconds.
    tried: bool,
    looks_since_try: u32,
    gaming_mode_was_on: bool,
    /// Why Gaming Mode's changes were not all made.
    not_made: Option<String>,
    /// Why some could not be put back (they stay listed in Backups).
    not_put_back: Option<String>,
    timer_problem: Option<String>,
    /// On Wi-Fi only when the game started.
    on_wifi: bool,
}

impl Session {
    fn problem(&self) -> Option<String> {
        let parts: Vec<&str> = [&self.not_put_back, &self.not_made, &self.timer_problem]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

pub fn start(app: AppHandle, engine: SharedEngine, status: SharedPlay) {
    std::thread::Builder::new()
        .name("game-watcher".into())
        .spawn(move || watch(&app, &engine, &status))
        .expect("the game watcher thread starts");
}

fn watch(app: &AppHandle, engine: &SharedEngine, status: &SharedPlay) {
    let games = game_processes();
    let mut watch = Watch::new();
    let mut session = Session::default();
    let mut reports = PlayReports::default();
    // A finished report waiting for the engine lock to be kept.
    let mut ended = None;
    let mut first = true;
    loop {
        let running = running_images().ok().and_then(|images| game_running(&images, &games));
        let event = watch.look(running);
        // The graphics card's readings, outside the engine lock: they read
        // NVML only and change nothing.
        if let Some(report) = reports.look(event.clone(), running, now_ms(), play_look) {
            ended = Some(report);
        }
        // Engine work only while the lock is free of an earlier panic; the
        // next look tries again.
        if let Ok(mut e) = engine.lock() {
            // A session a crash left open, with no game running now.
            if first && watch.current().is_none() && e.play_session_open() {
                session.not_put_back = sentence(
                    "Some Gaming Mode changes could not be put back; Undo them in Backups:",
                    e.end_play_session().into_iter().filter_map(|r| r.error),
                );
            }
            if let Some(report) = ended.take() {
                // A failure is shown as `history_problem`.
                let _ = e.record_play(report);
            }
            let settings = e.settings();
            match event {
                Some(WatchEvent::Started(_)) => {
                    session.tried = false;
                    session.not_made = None;
                    session.not_put_back = None;
                    // Once per game: listing adapters starts PowerShell.
                    session.on_wifi = e.on_wifi_only();
                }
                Some(WatchEvent::Stopped(_)) => {
                    session.timer = None;
                    session.timer_problem = None;
                    session.not_made = None;
                    session.not_put_back = sentence(
                        "Some Gaming Mode changes could not be put back; Undo them in Backups:",
                        e.end_play_session().into_iter().filter_map(|r| r.error),
                    );
                }
                None => {}
            }
            if !settings.gaming_mode {
                // Turning it off put its changes back (Engine::set_settings).
                session.not_made = None;
            }
            if watch.current().is_some() {
                session.looks_since_try = session.looks_since_try.saturating_add(1);
                if (settings.gaming_mode && !session.gaming_mode_was_on)
                    || (session.not_made.is_some() && session.looks_since_try >= RETRY_AFTER_LOOKS)
                {
                    session.tried = false;
                }
                if settings.gaming_mode && !session.tried {
                    session.tried = true;
                    session.looks_since_try = 0;
                    session.not_made = sentence(
                        "Gaming Mode is not fully on:",
                        e.start_play_session().into_iter().filter_map(|s| s.error),
                    );
                }
                if settings.game_timer && session.timer.is_none() && session.timer_problem.is_none() {
                    match TimerRequest::hold() {
                        Ok(t) => session.timer = Some(t),
                        Err(err) => session.timer_problem = Some(format!("The game timer could not be set: {err}")),
                    }
                }
            }
            if !settings.game_timer {
                session.timer = None;
                session.timer_problem = None;
            }
            if !e.play_session_open() {
                session.not_put_back = None;
            }
            session.gaming_mode_was_on = settings.gaming_mode;

            let now = PlayStatus {
                game: watch.current().map(str::to_owned),
                gaming_mode_active: e.play_session_open(),
                timer_held: session.timer.as_ref().map(TimerRequest::granted),
                problem: session.problem(),
                on_wifi: watch.current().is_some() && session.on_wifi,
                watched: watched_ids(&games),
                last_session: e.last_play(),
                history: e.play_history(),
                history_problem: e.play_history_problem(),
            };
            drop(e);
            let changed = {
                let mut shown = status.lock().unwrap_or_else(PoisonError::into_inner);
                let changed = *shown != now;
                if changed {
                    *shown = now.clone();
                }
                changed
            };
            if changed {
                // Outside both locks: the icon's text is set on the window's
                // thread, and this waits for it.
                crate::tray::show_play(app, &now);
                // Advisory; the UI also asks with `play_status`.
                let _ = app.emit("engine://play", now);
            }
        }
        first = false;
        std::thread::sleep(LOOK_EVERY);
    }
}

/// `lead` and the errors, as one line, if there were any.
fn sentence(lead: &str, errors: impl Iterator<Item = String>) -> Option<String> {
    let mut list: Vec<String> = errors.collect();
    list.dedup();
    (!list.is_empty()).then(|| format!("{lead} {}", list.join(" ")))
}
