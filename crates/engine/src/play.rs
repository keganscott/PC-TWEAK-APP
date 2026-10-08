//! While you play (CATALOGUE step 5): which known game is running, and the
//! timer request held while it does (H2).
//!
//! **Watching.** The app looks at the list of running programs every few
//! seconds (`running_images`, names only: no game process is opened, read or
//! touched) and feeds the game it finds to `Watch`, which says when a game
//! started and when it closed. A game counts as closed only after it has been
//! missing from two looks in a row, so a launcher restarting it does not end
//! the session.
//!
//! **Gaming Mode** (H5, H15) is the session changes in `tweaks::session`, made
//! by `Engine::start_play_session` when a game starts and put back by
//! `Engine::end_play_session` when it closes.
//!
//! **Timer (H2).** `TimerRequest` asks Windows for its finest timer interval
//! while a game runs and gives it back when dropped. It is a request held by
//! this process, not a setting: Windows forgets it when PeakTweaks closes, so
//! there is nothing to journal or undo. On Windows 11 a request reaches other
//! programs only with `GlobalTimerResolutionRequests` set (the "Global timer
//! requests" tool, `scheduling.timerrequests`).
//!
//! VERIFY: the program names are each game's own as recalled (`games.rs`
//! `GAME_FACTS`; Fortnite's is NOTES N7), and `NtQueryTimerResolution` / `NtSetTimerResolution` are
//! undocumented; their parameters follow System Informer's `phnt` as recalled
//! (NOTES N80).

use serde::Serialize;
use ts_rs::TS;

/// A known game and the name of its program while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameProcess {
    pub game_id: &'static str,
    /// The program's file name, as the process list shows it.
    pub image: &'static str,
}

/// Minecraft Bedrock Edition's program. It is watched for but has no entry in
/// `GAME_FACTS.programs`, which also keys the per-game tools; Java Edition runs
/// as `javaw.exe`, which many other programs share, so it is not watched.
const MINECRAFT_BEDROCK: GameProcess = GameProcess {
    game_id: "minecraft",
    image: "Minecraft.Windows.exe",
};

/// Programs that mean a game PeakTweaks offers (`env::KNOWN_GAMES`) is
/// running: each one's programs from `games::GAME_FACTS`, and Minecraft
/// Bedrock Edition.
pub fn game_processes() -> Vec<GameProcess> {
    let mut list: Vec<GameProcess> = crate::env::KNOWN_GAMES
        .iter()
        .filter_map(|g| crate::games::facts(g.id))
        .flat_map(|f| f.programs.iter().map(|image| GameProcess { game_id: f.id, image }))
        .collect();
    if crate::env::KNOWN_GAMES
        .iter()
        .any(|g| g.id == MINECRAFT_BEDROCK.game_id)
    {
        list.push(MINECRAFT_BEDROCK);
    }
    list
}

/// The first known game among the running program names (any case).
pub fn game_running(images: &[String], games: &[GameProcess]) -> Option<&'static str> {
    games
        .iter()
        .find(|g| images.iter().any(|i| i.eq_ignore_ascii_case(g.image)))
        .map(|g| g.game_id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    Started(&'static str),
    Stopped(&'static str),
}

/// Looks a game must be missing from before it counts as closed.
const MISSING_LOOKS: u8 = 2;

/// Turns "which game is running now" into started and closed events.
#[derive(Debug, Default)]
pub struct Watch {
    current: Option<&'static str>,
    missing: u8,
}

impl Watch {
    pub fn new() -> Self {
        Self::default()
    }

    /// The game this watch says is running.
    pub fn current(&self) -> Option<&'static str> {
        self.current
    }

    /// One look at the process list. Another game starting while one runs
    /// keeps the session (it belongs to whichever game opened it).
    pub fn look(&mut self, running: Option<&'static str>) -> Option<WatchEvent> {
        match (self.current, running) {
            (None, Some(game)) => {
                self.current = Some(game);
                self.missing = 0;
                Some(WatchEvent::Started(game))
            }
            (Some(_), Some(_)) => {
                self.missing = 0;
                None
            }
            (Some(game), None) => {
                self.missing += 1;
                if self.missing >= MISSING_LOOKS {
                    self.current = None;
                    self.missing = 0;
                    Some(WatchEvent::Stopped(game))
                } else {
                    None
                }
            }
            (None, None) => None,
        }
    }
}

/// What the app shows about the session: the game it sees, and what is in
/// effect for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PlayStatus {
    /// Id from `env::KNOWN_GAMES` of the game running now.
    pub game: Option<String>,
    /// Gaming Mode's changes are in effect.
    pub gaming_mode_active: bool,
    /// The timer request is held, with the interval Windows granted, in 100 ns
    /// units.
    pub timer_held: Option<u32>,
    /// Why something the user turned on is not in effect, in plain words.
    pub problem: Option<String>,
    /// The running game's PC was on Wi-Fi only when it started (catalogue E5:
    /// the app says so; it changes nothing by itself).
    pub on_wifi: bool,
    /// Ids of the games watched for (`game_processes`), each once.
    pub watched: Vec<String>,
}

impl PlayStatus {
    /// Nothing running yet, and what is watched for.
    pub fn watching() -> Self {
        Self {
            watched: watched_ids(&game_processes()),
            ..Self::default()
        }
    }
}

/// Connected over Wi-Fi and not over a cable (catalogue E5).
pub fn wifi_only(adapters: &[crate::system::NetAdapter]) -> bool {
    adapters.iter().any(|a| a.up && a.wireless) && !adapters.iter().any(|a| a.up && a.wired)
}

/// The game ids in `games`, each once, in order.
pub fn watched_ids(games: &[GameProcess]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for g in games {
        if !ids.iter().any(|i| i == g.game_id) {
            ids.push(g.game_id.to_owned());
        }
    }
    ids
}

/// Every program running on this PC, by file name.
#[cfg(windows)]
pub fn running_images() -> crate::error::Result<Vec<String>> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    use crate::error::EngineError;

    struct Snapshot(HANDLE);
    impl Drop for Snapshot {
        fn drop(&mut self) {
            // SAFETY: a handle we opened and have not closed.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    let mut out = Vec::new();
    // SAFETY: a snapshot handle we own, and an entry sized as the API asks.
    unsafe {
        let snap = Snapshot(
            CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
                .map_err(|e| EngineError::win32("CreateToolhelp32Snapshot", e))?,
        );
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap.0, &mut entry).is_err() {
            return Ok(out);
        }
        loop {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            out.push(String::from_utf16_lossy(&entry.szExeFile[..len]));
            if Process32NextW(snap.0, &mut entry).is_err() {
                break;
            }
        }
    }
    Ok(out)
}

/// Windows' timer intervals, in 100 ns units: the coarsest, the finest, and
/// the one in effect now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerIntervals {
    pub coarsest: u32,
    pub finest: u32,
    pub current: u32,
}

#[cfg(windows)]
mod timer_win {
    use windows::core::{s, w};
    use windows::Win32::Foundation::NTSTATUS;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    use super::TimerIntervals;
    use crate::error::{EngineError, Result};

    type Query = unsafe extern "system" fn(*mut u32, *mut u32, *mut u32) -> NTSTATUS;
    type Set = unsafe extern "system" fn(u32, u8, *mut u32) -> NTSTATUS;

    fn ntdll(name: windows::core::PCSTR, what: &str) -> Result<unsafe extern "system" fn() -> isize> {
        // SAFETY: ntdll is loaded in every process; the name is a NUL-terminated
        // literal.
        unsafe {
            let module = GetModuleHandleW(w!("ntdll.dll")).map_err(|e| EngineError::win32("GetModuleHandleW", e))?;
            GetProcAddress(module, name).ok_or_else(|| EngineError::Internal {
                detail: format!("ntdll.dll does not export {what}"),
            })
        }
    }

    fn check(status: NTSTATUS, call: &str) -> Result<()> {
        if status.is_ok() {
            Ok(())
        } else {
            Err(EngineError::Internal {
                detail: format!("{call} failed with status 0x{:08X}", status.0),
            })
        }
    }

    pub fn query() -> Result<TimerIntervals> {
        let f = ntdll(s!("NtQueryTimerResolution"), "NtQueryTimerResolution")?;
        let (mut a, mut b, mut current) = (0u32, 0u32, 0u32);
        // SAFETY: the export's signature (phnt): three out-pointers to ULONG.
        unsafe {
            let query = std::mem::transmute::<unsafe extern "system" fn() -> isize, Query>(f);
            check(query(&mut a, &mut b, &mut current), "NtQueryTimerResolution")?;
        }
        // The first two are the coarsest and the finest interval; which name
        // goes with which differs between sources, so take them by size.
        Ok(TimerIntervals {
            coarsest: a.max(b),
            finest: a.min(b),
            current,
        })
    }

    /// Request (`on`) or give back the `interval`; returns the one in effect.
    pub fn set(interval: u32, on: bool) -> Result<u32> {
        let f = ntdll(s!("NtSetTimerResolution"), "NtSetTimerResolution")?;
        let mut actual = 0u32;
        // SAFETY: the export's signature (phnt): ULONG, BOOLEAN, out-pointer.
        unsafe {
            let set = std::mem::transmute::<unsafe extern "system" fn() -> isize, Set>(f);
            check(set(interval, u8::from(on), &mut actual), "NtSetTimerResolution")?;
        }
        Ok(actual)
    }
}

/// Windows' timer intervals now.
#[cfg(windows)]
pub fn timer_intervals() -> crate::error::Result<TimerIntervals> {
    timer_win::query()
}

/// The finest timer interval, asked for by this process until dropped.
#[cfg(windows)]
pub struct TimerRequest {
    interval: u32,
    granted: u32,
}

#[cfg(windows)]
impl TimerRequest {
    pub fn hold() -> crate::error::Result<Self> {
        let interval = timer_win::query()?.finest;
        let granted = timer_win::set(interval, true)?;
        Ok(Self { interval, granted })
    }

    /// The interval in effect when the request was made, in 100 ns units.
    pub fn granted(&self) -> u32 {
        self.granted
    }
}

#[cfg(windows)]
impl Drop for TimerRequest {
    fn drop(&mut self) {
        let _ = timer_win::set(self.interval, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_known_game_is_found_by_its_program_name_in_any_case() {
        let running = names(&["explorer.exe", "fortniteclient-win64-shipping.EXE", "Discord.exe"]);
        let games = game_processes();
        assert_eq!(game_running(&running, &games), Some("fortnite"));
        assert_eq!(game_running(&names(&["javaw.exe", "chrome.exe"]), &games), None);
        assert_eq!(
            game_running(&names(&["Minecraft.Windows.exe"]), &games),
            Some("minecraft")
        );
    }

    #[test]
    fn wifi_only_means_wifi_connected_and_no_cable_connected() {
        use crate::system::NetAdapter;
        let a = |wired: bool, up: bool| NetAdapter {
            guid: String::new(),
            name: String::new(),
            up,
            wireless: !wired,
            wired,
        };
        assert!(wifi_only(&[a(false, true)]));
        assert!(wifi_only(&[a(false, true), a(true, false)]), "an unplugged cable");
        assert!(!wifi_only(&[a(false, true), a(true, true)]));
        assert!(!wifi_only(&[a(true, true)]));
        assert!(!wifi_only(&[a(false, false)]), "not connected at all");
        assert!(!wifi_only(&[]));
    }

    #[test]
    fn every_offered_game_with_a_program_of_its_own_is_watched_once() {
        let games = game_processes();
        let ids = watched_ids(&games);
        for known in crate::env::KNOWN_GAMES {
            let has_program = crate::games::facts(known.id).is_some_and(|f| !f.programs.is_empty());
            assert_eq!(
                ids.iter().any(|i| i == known.id),
                has_program || known.id == "minecraft",
                "{}",
                known.id
            );
        }
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "each game once: {ids:?}");
    }

    #[test]
    fn every_watched_game_is_a_known_game() {
        for g in game_processes() {
            assert!(
                crate::env::KNOWN_GAMES.iter().any(|k| k.id == g.game_id),
                "{} is not in KNOWN_GAMES",
                g.game_id
            );
            assert!(g.image.ends_with(".exe") && !g.image.contains('\\'), "{}", g.image);
        }
    }

    #[test]
    fn a_game_starts_at_once_and_closes_after_two_looks_without_it() {
        let mut w = Watch::new();
        assert_eq!(w.look(None), None);
        assert_eq!(w.look(Some("fortnite")), Some(WatchEvent::Started("fortnite")));
        assert_eq!(w.look(Some("fortnite")), None);
        assert_eq!(w.look(None), None, "one look without it is not enough");
        assert_eq!(w.current(), Some("fortnite"));
        assert_eq!(w.look(None), Some(WatchEvent::Stopped("fortnite")));
        assert_eq!(w.current(), None);
        assert_eq!(w.look(None), None);
    }

    #[test]
    fn a_game_seen_again_after_one_missing_look_keeps_its_session() {
        let mut w = Watch::new();
        w.look(Some("roblox"));
        assert_eq!(w.look(None), None);
        assert_eq!(w.look(Some("roblox")), None);
        assert_eq!(w.look(None), None, "the count starts again");
        assert_eq!(w.look(None), Some(WatchEvent::Stopped("roblox")));
    }

    #[test]
    fn a_second_game_does_not_open_a_second_session() {
        let mut w = Watch::new();
        w.look(Some("roblox"));
        assert_eq!(w.look(Some("fortnite")), None);
        assert_eq!(w.current(), Some("roblox"));
    }

    #[cfg(windows)]
    #[test]
    fn lists_running_programs_and_reads_the_timer_on_this_pc() {
        let images = running_images().unwrap();
        assert!(images.len() > 5, "{images:?}");
        assert!(
            images.iter().any(|i| i.eq_ignore_ascii_case("cargo.exe")),
            "the test runner's own parent is listed"
        );
        let before = timer_intervals().unwrap();
        println!("timer intervals (100 ns): {before:?}");
        assert!(before.finest > 0 && before.finest <= before.coarsest, "{before:?}");
        let req = TimerRequest::hold().unwrap();
        let during = timer_intervals().unwrap();
        println!("while held: granted {}, now {during:?}", req.granted());
        assert!(
            req.granted() <= before.current,
            "granted {} vs {before:?}",
            req.granted()
        );
        assert!(during.current <= before.current, "{during:?} vs {before:?}");
        drop(req);
        println!("after: {:?}", timer_intervals().unwrap());
    }
}
