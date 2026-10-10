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

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::probe::Probe;
use crate::proof::nvml::{GpuLive, ThrottleReason, ThrottleSummary, ThrottleTally};

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
    /// What the graphics card did during the last game that closed while
    /// PeakTweaks was open. Kept until PeakTweaks closes or the history is
    /// forgotten; the history keeps it longer.
    pub last_session: Option<PlayReport>,
    /// The reports of the last games watched, newest last, kept on this PC
    /// across restarts (`play_history.rs`, at most `play_history::KEEP`).
    pub history: Vec<PlayReport>,
    /// Why the last game could not be kept in the history (its file could
    /// not be saved). Not a Gaming Mode problem: nothing the user turned on
    /// depends on it.
    pub history_problem: Option<String>,
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

/// What the graphics card did while a game ran (plan 6.2 item 6: GPU
/// throttling, advice only). Read through NVML on NVIDIA cards, every few
/// seconds with the watcher's look; nothing is changed by it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PlayReport {
    /// Id from `env::KNOWN_GAMES`.
    pub game: String,
    /// The first look that saw the game: when PeakTweaks started watching
    /// it, which is later than the game's start when PeakTweaks opened after.
    #[ts(type = "number")]
    pub started_unix_ms: u64,
    /// The last look that saw the game running.
    #[ts(type = "number")]
    pub ended_unix_ms: u64,
    /// The reasons NVIDIA's driver gave for holding clocks down, and in how
    /// many of the readings each was active.
    pub gpu_throttle: Probe<ThrottleSummary>,
    /// Of `gpu_throttle`'s readings, how many had either heat reason
    /// (software or hardware thermal slowdown), each reading counted once.
    pub heat_readings: u32,
    /// The same for the card's own slowdowns that are not only heat:
    /// hardware slowdown or the power brake.
    pub hardware_readings: u32,
    /// The highest GPU temperature read, in degrees Celsius.
    pub gpu_hottest_c: Probe<u32>,
    /// Why a temperature was not read in some looks or on some card, when
    /// `gpu_hottest_c` is yes from the others.
    pub temperature_missed: Option<String>,
}

/// Builds a `PlayReport` from one reading per look while a game runs.
#[derive(Default)]
pub struct PlayTally {
    game: String,
    started_unix_ms: u64,
    last_seen_unix_ms: u64,
    throttle: ThrottleTally,
    heat_readings: u32,
    hardware_readings: u32,
    hottest: Option<u32>,
    /// NVML answered and listed no NVIDIA GPU.
    no_gpu: Option<String>,
    /// Why a temperature could not be read.
    temperature_problem: Option<String>,
}

impl PlayTally {
    pub fn new(game: &str, now_unix_ms: u64) -> Self {
        Self {
            game: game.to_owned(),
            started_unix_ms: now_unix_ms,
            last_seen_unix_ms: now_unix_ms,
            ..Self::default()
        }
    }

    pub fn game(&self) -> &str {
        &self.game
    }

    /// One look's readings, taken while the game was running.
    pub fn add(&mut self, now_unix_ms: u64, reasons: Probe<Vec<ThrottleReason>>, gpus: Probe<Vec<GpuLive>>) {
        use ThrottleReason::*;
        self.last_seen_unix_ms = now_unix_ms.max(self.last_seen_unix_ms);
        if let Probe::Yes { value } = &reasons {
            if value
                .iter()
                .any(|r| matches!(r, SoftwareThermalSlowdown | HardwareThermalSlowdown))
            {
                self.heat_readings += 1;
            }
            if value.iter().any(|r| matches!(r, HardwareSlowdown | HardwarePowerBrake)) {
                self.hardware_readings += 1;
            }
        }
        self.throttle.add(reasons);
        match gpus {
            Probe::Yes { value } => {
                for gpu in value {
                    match gpu.temperature_c {
                        Probe::Yes { value } => self.hottest = Some(self.hottest.map_or(value, |h| h.max(value))),
                        Probe::No { reason } | Probe::Unknown { reason } => {
                            self.temperature_problem.get_or_insert(reason);
                        }
                    }
                }
            }
            Probe::No { reason } => {
                self.no_gpu.get_or_insert(reason);
            }
            Probe::Unknown { reason } => {
                self.temperature_problem.get_or_insert(reason);
            }
        }
    }

    pub fn finish(self) -> PlayReport {
        let throttle = self.throttle.finish();
        let gpu_throttle = match (&throttle, &self.no_gpu) {
            (Probe::Unknown { .. }, Some(reason)) => Probe::no(reason.clone()),
            _ => throttle,
        };
        let (gpu_hottest_c, temperature_missed) = match (self.hottest, self.no_gpu, self.temperature_problem) {
            (Some(t), _, missed) => (Probe::yes(t), missed),
            (None, Some(reason), _) => (Probe::no(reason), None),
            (None, None, Some(reason)) => (Probe::unknown(reason), None),
            (None, None, None) => (Probe::unknown("no GPU readings were taken while the game ran"), None),
        };
        PlayReport {
            game: self.game,
            started_unix_ms: self.started_unix_ms,
            ended_unix_ms: self.last_seen_unix_ms,
            gpu_throttle,
            heat_readings: self.heat_readings,
            hardware_readings: self.hardware_readings,
            gpu_hottest_c,
            temperature_missed,
        }
    }
}

/// The watcher's game reports: a tally for the game being watched and the
/// report of the last one that closed.
#[derive(Default)]
pub struct PlayReports {
    tally: Option<PlayTally>,
    last: Option<PlayReport>,
}

impl PlayReports {
    /// After each look: `event` and `running` as `Watch::look` and
    /// `game_running` gave them. `read` is called only while the watched game
    /// itself is running, so a second game that keeps the session open (see
    /// `Watch`) adds nothing to the first one's report.
    ///
    /// Returns the report of a game that ended with this look, for the
    /// history to keep.
    pub fn look<F>(
        &mut self,
        event: Option<WatchEvent>,
        running: Option<&str>,
        now_unix_ms: u64,
        read: F,
    ) -> Option<PlayReport>
    where
        F: FnOnce() -> (Probe<Vec<ThrottleReason>>, Probe<Vec<GpuLive>>),
    {
        let mut ended = None;
        match event {
            Some(WatchEvent::Started(game)) => self.tally = Some(PlayTally::new(game, now_unix_ms)),
            Some(WatchEvent::Stopped(_)) => {
                if let Some(t) = self.tally.take() {
                    let report = t.finish();
                    self.last = Some(report.clone());
                    ended = Some(report);
                }
            }
            None => {}
        }
        if let (Some(t), Some(game)) = (self.tally.as_mut(), running) {
            if t.game() == game {
                let (reasons, gpus) = read();
                t.add(now_unix_ms, reasons, gpus);
            }
        }
        ended
    }

    pub fn last(&self) -> Option<&PlayReport> {
        self.last.as_ref()
    }
}

/// One look at the graphics card for `PlayTally::add`: the clock-limit
/// reasons and the live readings, through the NVML library the live readings
/// on Home use. Unknown off Windows.
pub fn gpu_look() -> (Probe<Vec<ThrottleReason>>, Probe<Vec<GpuLive>>) {
    #[cfg(windows)]
    {
        use crate::proof::nvml::ThrottleSampler;
        let nvml = crate::proof::nvml::shared();
        (nvml.sample(), nvml.live())
    }
    #[cfg(not(windows))]
    {
        let why = "graphics card readings are only taken on Windows";
        (Probe::unknown(why), Probe::unknown(why))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn gpu(temperature: Probe<u32>) -> GpuLive {
        GpuLive {
            name: Probe::yes("GPU".to_owned()),
            busy_percent: Probe::yes(90),
            temperature_c: temperature,
            memory_used_bytes: Probe::yes(1),
            memory_total_bytes: Probe::yes(2),
        }
    }

    #[test]
    fn a_session_report_counts_each_reason_and_keeps_the_hottest_reading() {
        use ThrottleReason::*;
        let mut tally = PlayTally::new("fortnite", 1_000);
        tally.add(
            4_000,
            Probe::yes(vec![SoftwarePowerCap]),
            Probe::yes(vec![gpu(Probe::yes(70))]),
        );
        tally.add(
            7_000,
            Probe::yes(vec![SoftwarePowerCap, SoftwareThermalSlowdown]),
            Probe::yes(vec![gpu(Probe::yes(83)), gpu(Probe::unknown("no sensor"))]),
        );
        // A look whose reading failed counts for neither.
        tally.add(10_000, Probe::unknown("NVML busy"), Probe::unknown("NVML busy"));
        let report = tally.finish();
        assert_eq!(report.game, "fortnite");
        assert_eq!((report.started_unix_ms, report.ended_unix_ms), (1_000, 10_000));
        assert_eq!(report.gpu_hottest_c, Probe::yes(83));
        let Probe::Yes { value } = report.gpu_throttle else {
            panic!("throttle not summed: {:?}", report.gpu_throttle)
        };
        assert_eq!(value.samples, 2);
        let count = |r| value.seen.iter().find(|s| s.reason == r).map(|s| s.samples);
        assert_eq!(count(SoftwarePowerCap), Some(2));
        assert_eq!(count(SoftwareThermalSlowdown), Some(1));
        assert_eq!(count(HardwareThermalSlowdown), None);
        assert_eq!((report.heat_readings, report.hardware_readings), (1, 0));
        assert_eq!(report.temperature_missed.as_deref(), Some("no sensor"));
    }

    #[test]
    fn heat_counts_each_reading_once_whichever_reason_it_carried() {
        use ThrottleReason::*;
        let mut tally = PlayTally::new("fortnite", 0);
        let none = || Probe::yes(vec![gpu(Probe::yes(60))]);
        tally.add(1, Probe::yes(vec![SoftwareThermalSlowdown]), none());
        tally.add(2, Probe::yes(vec![HardwareThermalSlowdown]), none());
        tally.add(
            3,
            Probe::yes(vec![SoftwareThermalSlowdown, HardwareThermalSlowdown, HardwareSlowdown]),
            none(),
        );
        tally.add(4, Probe::yes(vec![]), none());
        let report = tally.finish();
        assert_eq!((report.heat_readings, report.hardware_readings), (3, 1));
        assert_eq!(report.temperature_missed, None);
    }

    #[test]
    fn a_report_is_made_when_the_game_closes_and_only_from_its_own_looks() {
        use ThrottleReason::*;
        let mut reports = PlayReports::default();
        let mut reads = 0;
        let mut look = |reports: &mut PlayReports, event, running, now| {
            reports.look(event, running, now, || {
                reads += 1;
                (
                    Probe::yes(vec![SoftwarePowerCap]),
                    Probe::yes(vec![gpu(Probe::yes(70))]),
                )
            })
        };
        look(&mut reports, None, None, 0);
        look(
            &mut reports,
            Some(WatchEvent::Started("fortnite")),
            Some("fortnite"),
            3_000,
        );
        look(&mut reports, None, Some("fortnite"), 6_000);
        // Fortnite closed while Roblox runs: the watcher keeps the session,
        // but Roblox's readings are not Fortnite's.
        look(&mut reports, None, Some("roblox"), 9_000);
        look(&mut reports, None, None, 12_000);
        assert!(reports.last().is_none(), "no report before the game counts as closed");
        let ended = look(&mut reports, Some(WatchEvent::Stopped("fortnite")), None, 15_000);
        let report = reports.last().expect("a report").clone();
        assert_eq!(ended.as_ref(), Some(&report), "the finished report is handed on once");
        assert_eq!(report.game, "fortnite");
        assert_eq!((report.started_unix_ms, report.ended_unix_ms), (3_000, 6_000));
        let Probe::Yes { value } = &report.gpu_throttle else {
            panic!("{:?}", report.gpu_throttle)
        };
        assert_eq!(value.samples, 2);
        // The next game's report replaces it only when that game closes.
        look(
            &mut reports,
            Some(WatchEvent::Started("roblox")),
            Some("roblox"),
            18_000,
        );
        assert_eq!(reports.last().map(|r| r.game.as_str()), Some("fortnite"));
        look(&mut reports, Some(WatchEvent::Stopped("roblox")), None, 24_000);
        assert_eq!(reports.last().map(|r| r.game.as_str()), Some("roblox"));
        assert_eq!(reads, 3);
    }

    #[test]
    fn a_session_with_some_failed_looks_keeps_the_ones_that_answered() {
        use ThrottleReason::*;
        let mut tally = PlayTally::new("fortnite", 0);
        tally.add(1, Probe::unknown("NVML busy"), Probe::unknown("NVML busy"));
        tally.add(
            2,
            Probe::yes(vec![SoftwarePowerCap]),
            Probe::yes(vec![gpu(Probe::yes(66))]),
        );
        let report = tally.finish();
        assert!(matches!(&report.gpu_throttle, Probe::Yes { value } if value.samples == 1));
        assert_eq!(report.gpu_hottest_c, Probe::yes(66));
        assert_eq!(report.temperature_missed.as_deref(), Some("NVML busy"));
    }

    /// On Windows CI: a look at the graphics card answers (the runners have
    /// no NVIDIA card, so not with readings) and makes a report.
    #[cfg(windows)]
    #[test]
    fn a_look_at_the_graphics_card_answers_on_this_pc() {
        let (reasons, gpus) = gpu_look();
        println!("graphics card look on this runner: {reasons:?} / {gpus:?}");
        let mut tally = PlayTally::new("fortnite", 0);
        tally.add(3_000, reasons, gpus);
        let report = tally.finish();
        println!("session report on this runner: {report:?}");
        if let Probe::Yes { value } = &report.gpu_throttle {
            assert_eq!(value.samples, 1);
        }
    }

    #[test]
    fn a_pc_without_an_nvidia_card_reports_no_not_unknown() {
        let mut tally = PlayTally::new("roblox", 0);
        let none = "NVML reports no NVIDIA GPU";
        tally.add(3_000, Probe::no(none), Probe::no(none));
        let report = tally.finish();
        assert_eq!(report.gpu_throttle, Probe::no(none));
        assert_eq!(report.gpu_hottest_c, Probe::no(none));
    }

    #[test]
    fn a_missing_library_stays_unknown_with_its_reason() {
        let mut tally = PlayTally::new("roblox", 0);
        let why = "nvml.dll was not found";
        tally.add(3_000, Probe::unknown(why), Probe::unknown(why));
        let report = tally.finish();
        assert_eq!(report.gpu_throttle, Probe::unknown(why));
        assert_eq!(report.gpu_hottest_c, Probe::unknown(why));
        // A game that closed before any look read anything.
        let report = PlayTally::new("roblox", 0).finish();
        assert!(matches!(report.gpu_throttle, Probe::Unknown { .. }));
        assert!(matches!(report.gpu_hottest_c, Probe::Unknown { .. }));
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
            pnp_id: String::new(),
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
