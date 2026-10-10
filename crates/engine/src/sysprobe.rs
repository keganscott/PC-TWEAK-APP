//! The live environment probe: hardware, security and restore state, cached.
//!
//! Hardware barely changes, so it is kept for ten minutes. Security and restore
//! state are kept for thirty seconds. Anything that could change the answers
//! (a restore point was created, a tweak applied or reverted, an explicit
//! rescan) calls `invalidate`. The restore *gate* has its own shorter cache and
//! is asked again immediately before every apply.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use ts_rs::TS;

use super::background::BackgroundLoad;
use super::env::EnvProbe;
use super::hardware::RigClass;
use super::hardware::{probe_hardware, HardwareReport, OsFacts};
use super::journal::now_ms;
use super::probe::Probe;
use super::registry::{Hive, RegistryBackend};
use super::restore::RestoreService;
use super::scanner::{scan, ScanReport};
use super::security::{anti_cheat_readiness, probe_security, AntiCheatReadiness, SecurityReport};
use super::settings::Settings;
use super::types::SystemEnv;
use super::wmi::{WmiSource, NS_CIMV2};

const HARDWARE_TTL_MS: u64 = 10 * 60 * 1000;
const STATE_TTL_MS: u64 = 30 * 1000;
/// The background sample takes `background_wait` to measure, so it is kept for
/// a minute and is not re-measured before every apply (`invalidate`).
const BACKGROUND_TTL_MS: u64 = 60 * 1000;
const BACKGROUND_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// What `audit_system` returns.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SystemAudit {
    pub env: SystemEnv,
    pub anti_cheat: Option<AntiCheatReadiness>,
    /// Findings from the scanner, computed from `env` alone.
    pub scan: ScanReport,
    pub settings: Settings,
    /// The user's override if set, else the detected rig class.
    pub effective_rig_class: Option<RigClass>,
}

impl SystemAudit {
    pub fn from_env(env: SystemEnv, settings: Settings, effective_rig_class: Option<RigClass>) -> Self {
        let anti_cheat = env.security.as_ref().map(anti_cheat_readiness);
        let scan = scan(&env);
        Self {
            env,
            anti_cheat,
            scan,
            settings,
            effective_rig_class,
        }
    }
}

#[derive(Default)]
struct Cache {
    hardware: Option<(u64, HardwareReport, Vec<crate::game_installs::GameInstall>)>,
    state: Option<(u64, SecurityReport, super::restore::RestoreStatus)>,
    background: Option<(u64, Probe<BackgroundLoad>)>,
}

pub struct SystemProbe {
    wmi: Arc<dyn WmiSource>,
    reg: Arc<dyn RegistryBackend>,
    facts: Arc<dyn OsFacts>,
    restore: Arc<RestoreService>,
    cache: Mutex<Cache>,
    background_wait: std::time::Duration,
    /// `%ProgramData%` and the interactive user's profile folder, where the
    /// known games record their installs (`game_installs.rs`).
    program_data: std::path::PathBuf,
    profile: Option<std::path::PathBuf>,
    /// The interactive user's hive and the path prefix inside it: `HKCU` and
    /// nothing when that user is us, else `HKU` and their SID. `None` when the
    /// user could not be worked out.
    user_root: Option<(Hive, String)>,
}

impl SystemProbe {
    pub fn new(
        wmi: Arc<dyn WmiSource>,
        reg: Arc<dyn RegistryBackend>,
        facts: Arc<dyn OsFacts>,
        restore: Arc<RestoreService>,
    ) -> Self {
        Self {
            wmi,
            reg,
            facts,
            restore,
            cache: Mutex::default(),
            background_wait: BACKGROUND_WAIT,
            program_data: std::env::var_os("ProgramData")
                .map(Into::into)
                .unwrap_or_else(|| r"C:\ProgramData".into()),
            profile: interactive_profile(),
            user_root: interactive_user_root(),
        }
    }

    /// Tests and fixtures: read per-user settings from this hive and prefix.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_user_root(mut self, root: Option<(Hive, String)>) -> Self {
        self.user_root = root;
        self
    }

    /// Tests and fixtures: look for game installs under these folders.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_game_folders(mut self, program_data: std::path::PathBuf, profile: Option<std::path::PathBuf>) -> Self {
        self.program_data = program_data;
        self.profile = profile;
        self
    }

    /// Tests and fixtures: sample the background load without waiting.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_background_wait(mut self, wait: std::time::Duration) -> Self {
        self.background_wait = wait;
        self
    }
}

/// The interactive user's profile folder: per-user games live there, and under
/// alternate admin credentials it is not this process's own profile.
#[cfg(windows)]
fn interactive_profile() -> Option<std::path::PathBuf> {
    let user = crate::identity::detect_user().ok()?;
    crate::identity::profile_dir_for_sid(&user.sid).ok().flatten()
}

#[cfg(not(windows))]
fn interactive_profile() -> Option<std::path::PathBuf> {
    None
}

/// Where the interactive user's own settings are, routed as `ContextResolver`
/// routes `RegRoot::InteractiveUser`.
#[cfg(windows)]
fn interactive_user_root() -> Option<(Hive, String)> {
    let user = crate::identity::detect_user().ok()?;
    Some(if user.is_self {
        (Hive::CurrentUser, String::new())
    } else {
        (Hive::Users, user.sid)
    })
}

#[cfg(not(windows))]
fn interactive_user_root() -> Option<(Hive, String)> {
    None
}

impl SystemProbe {
    /// The launcher folders the game finder needs from the registry (VERIFY,
    /// NOTES N75). A value that is missing or not text leaves its launcher out.
    fn launchers(&self) -> crate::game_installs::Launchers {
        let folder = |keys: &[&str], name: &str| {
            keys.iter().find_map(|key| {
                let v = self.reg.read_value(Hive::LocalMachine, key, name).ok()??;
                let s = v.as_sz()?;
                (!s.trim().is_empty()).then(|| std::path::PathBuf::from(s.trim()))
            })
        };
        crate::game_installs::Launchers {
            steam: folder(
                &[r"SOFTWARE\WOW6432Node\Valve\Steam", r"SOFTWARE\Valve\Steam"],
                "InstallPath",
            ),
            apex_ea: folder(&[r"SOFTWARE\Respawn\Apex"], "Install Dir"),
        }
    }

    fn gpu_choices(&self, installs: &[crate::game_installs::GameInstall]) -> Vec<crate::gpu_choice::GameGpuChoice> {
        let Some((hive, prefix)) = &self.user_root else {
            return crate::gpu_choice::probe_gpu_choices(installs, None);
        };
        let key = if prefix.is_empty() {
            crate::gpu_choice::KEY.to_owned()
        } else {
            format!("{prefix}\\{}", crate::gpu_choice::KEY)
        };
        let read = |exe: &str| self.reg.read_value(*hive, &key, exe);
        crate::gpu_choice::probe_gpu_choices(installs, Some(&read))
    }
}

impl EnvProbe for SystemProbe {
    fn probe(&self, elevated: bool) -> SystemEnv {
        let now = now_ms();
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());

        let (hardware, game_installs) = match &cache.hardware {
            Some((at, h, g)) if now.saturating_sub(*at) < HARDWARE_TTL_MS => (h.clone(), g.clone()),
            _ => {
                let h = probe_hardware(self.wmi.as_ref(), self.facts.as_ref());
                // Installs rarely move, so they share the hardware cache.
                let g = crate::game_installs::probe_game_installs(
                    &self.program_data,
                    self.profile.as_deref(),
                    &self.launchers(),
                    &|drive| crate::hardware::probe_drive(self.wmi.as_ref(), drive),
                );
                cache.hardware = Some((now, h.clone(), g.clone()));
                (h, g)
            }
        };
        let (security, restore) = match &cache.state {
            Some((at, s, r)) if now.saturating_sub(*at) < STATE_TTL_MS => (s.clone(), r.clone()),
            _ => {
                let s = probe_security(self.wmi.as_ref(), self.reg.as_ref());
                let r = self.restore.status();
                cache.state = Some((now, s.clone(), r.clone()));
                (s, r)
            }
        };

        let background = match &cache.background {
            Some((at, b)) if now.saturating_sub(*at) < BACKGROUND_TTL_MS => b.clone(),
            _ => {
                let logical = hardware.cpu.value().map(|c| c.logical_processors);
                let wmi = self.wmi.clone();
                let b = crate::background::sample(
                    &move || crate::background::snapshot_from(&wmi.query(NS_CIMV2, crate::background::WQL_PROCESSES)),
                    self.background_wait,
                    logical,
                );
                cache.background = Some((now, b.clone()));
                b
            }
        };

        SystemEnv {
            elevated,
            target_game: None, // the engine fills this in
            restore_gate_open: restore.gate_open,
            hardware: Some(hardware),
            security: Some(security),
            restore: Some(restore),
            // Cheap (one registry read) and it changes when the user changes it,
            // so it is not cached.
            power_plan: Some(crate::power::probe_power_plan(self.reg.as_ref())),
            background: Some(background),
            // A few registry reads, and the user may change the setting at any
            // time, so it is not cached.
            gpu_choices: Some(self.gpu_choices(&game_installs)),
            game_installs: Some(game_installs),
        }
    }

    fn restore_gate_open(&self) -> bool {
        self.restore.gate_open()
    }

    fn invalidate(&self) {
        if let Ok(mut c) = self.cache.lock() {
            c.state = None;
        }
        self.restore.invalidate();
    }

    fn invalidate_all(&self) {
        if let Ok(mut c) = self.cache.lock() {
            *c = Cache::default();
        }
        self.restore.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{DisplayInfo, GpuAdapter};
    use crate::registry::fake::FakeRegistry;
    use crate::restore::{FakeRestoreOps, RestoreService};
    use crate::wmi::FakeWmi;

    struct Facts;
    impl OsFacts for Facts {
        fn system_drive(&self) -> String {
            "C:".into()
        }
        fn display(&self) -> crate::error::Result<DisplayInfo> {
            Err(crate::error::EngineError::Internal {
                detail: "no display".into(),
            })
        }
        fn gpu_adapters(&self) -> crate::error::Result<Vec<GpuAdapter>> {
            Ok(vec![])
        }
    }

    fn probe() -> (SystemProbe, Arc<FakeWmi>) {
        let wmi = Arc::new(FakeWmi::new());
        let reg = Arc::new(FakeRegistry::new());
        let restore =
            Arc::new(RestoreService::new(Arc::new(FakeRestoreOps::new()), reg.clone(), wmi.clone()).without_waiting());
        (
            SystemProbe::new(wmi.clone(), reg, Arc::new(Facts), restore)
                .with_background_wait(std::time::Duration::ZERO),
            wmi,
        )
    }

    #[test]
    fn a_dead_wmi_still_produces_a_complete_env_of_unknowns_and_a_closed_gate() {
        let (p, _) = probe();
        let env = p.probe(true);
        assert!(env.elevated);
        assert!(!env.restore_gate_open);
        let hw = env.hardware.as_ref().unwrap();
        assert!(hw.cpu.is_unknown() && hw.memory.is_unknown());
        assert!(env.security.as_ref().unwrap().iommu.is_unknown());
        assert_eq!(env.logical_processors(), None);
        assert_eq!(env.os_build(), None);
        assert!(!p.restore_gate_open());
    }

    #[test]
    fn results_are_cached_until_invalidated() {
        let (p, wmi) = probe();
        p.probe(true);
        let first = wmi.calls.lock().unwrap().len();
        assert!(first > 0);
        p.probe(true);
        assert_eq!(
            wmi.calls.lock().unwrap().len(),
            first,
            "second probe was served from cache"
        );

        p.invalidate(); // state only: hardware stays cached
        let hw_queries_before = wmi
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, q)| q.contains("Win32_Processor"))
            .count();
        p.probe(true);
        let hw_queries_after = wmi
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, q)| q.contains("Win32_Processor"))
            .count();
        assert_eq!(hw_queries_before, hw_queries_after);
        assert!(
            wmi.calls.lock().unwrap().len() > first,
            "security and restore were re-read"
        );

        p.invalidate_all();
        p.probe(true);
        let hw_queries_final = wmi
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, q)| q.contains("Win32_Processor"))
            .count();
        assert!(
            hw_queries_final > hw_queries_after,
            "hardware re-read after invalidate_all"
        );
    }

    #[test]
    fn graphics_choices_are_read_from_the_interactive_users_hive() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        // Joined the way `game_installs.rs` joins it, so it also works on Linux.
        let version = profile.join(r"AppData\Local\Roblox\Versions").join("version-1");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(version.join("RobloxPlayerBeta.exe"), b"").unwrap();
        let exe = version
            .join("RobloxPlayerBeta.exe")
            .to_string_lossy()
            .replace('/', "\\");

        let wmi = Arc::new(FakeWmi::new());
        let reg = Arc::new(FakeRegistry::new());
        let sid = "S-1-5-21-1-2-3-1001";
        reg.set_external(
            Hive::Users,
            &format!(r"{sid}\{}", crate::gpu_choice::KEY),
            &exe,
            crate::types::RawValue::sz("GpuPreference=1;"),
        );
        // The same value in our own HKCU must not be what is read.
        reg.set_external(
            Hive::CurrentUser,
            crate::gpu_choice::KEY,
            &exe,
            crate::types::RawValue::sz("GpuPreference=2;"),
        );
        let restore =
            Arc::new(RestoreService::new(Arc::new(FakeRestoreOps::new()), reg.clone(), wmi.clone()).without_waiting());
        let p = SystemProbe::new(wmi, reg, Arc::new(Facts), restore)
            .with_background_wait(std::time::Duration::ZERO)
            .with_game_folders(dir.path().join("pd"), Some(profile));

        let read = |p: &SystemProbe| p.probe(true).gpu_choices.unwrap();
        let p = p.with_user_root(Some((Hive::Users, sid.into())));
        let choices = read(&p);
        assert_eq!(choices.len(), 1, "{choices:?}");
        assert_eq!(choices[0].exe.as_deref(), Some(exe.as_str()));
        assert_eq!(
            choices[0].preference,
            Probe::yes(crate::gpu_choice::GpuPreference::PowerSaving)
        );

        let p = p.with_user_root(Some((Hive::CurrentUser, String::new())));
        assert_eq!(
            read(&p)[0].preference,
            Probe::yes(crate::gpu_choice::GpuPreference::HighPerformance)
        );

        let p = p.with_user_root(None);
        assert!(read(&p)[0].preference.is_unknown(), "no user, no guess");
    }

    #[test]
    fn the_audit_carries_anti_cheat_readiness_only_when_security_was_probed() {
        let (p, _) = probe();
        let audit = SystemAudit::from_env(p.probe(true), Settings::default(), None);
        let ac = audit.anti_cheat.expect("security was probed");
        assert_eq!(ac.per_game.len(), crate::env::KNOWN_GAMES.len());
        assert!(SystemAudit::from_env(SystemEnv::default(), Settings::default(), None)
            .anti_cheat
            .is_none());
    }
}

#[cfg(all(test, windows))]
mod live_tests {
    use super::*;
    use crate::hardware::OsFacts;
    use crate::osfacts::WindowsFacts;
    use crate::registry::windows::WinRegistry;
    use crate::restore_win::WindowsRestoreOps;
    use crate::wmi::WmiWorker;

    /// Probes the machine the tests run on and prints the whole audit. CI shows
    /// this output as evidence; the assertions are only the ones that must hold
    /// on any Windows machine.
    #[test]
    fn live_probe_report() {
        let wmi: Arc<dyn WmiSource> = Arc::new(WmiWorker::start());
        let reg: Arc<dyn RegistryBackend> = Arc::new(WinRegistry::new());
        let facts: Arc<dyn OsFacts> = Arc::new(WindowsFacts);
        let restore = Arc::new(RestoreService::new(
            Arc::new(WindowsRestoreOps::new(wmi.clone())),
            reg.clone(),
            wmi.clone(),
        ));
        let probe = SystemProbe::new(wmi, reg, facts, restore);

        let started = std::time::Instant::now();
        let env = probe.probe(crate::identity::is_elevated());
        let took = started.elapsed();
        let audit = SystemAudit::from_env(env.clone(), Settings::default(), None);
        println!("probed in {took:?}");
        println!("{}", serde_json::to_string_pretty(&audit).unwrap());

        let hw = env.hardware.as_ref().expect("hardware report");
        assert!(hw.os.is_yes(), "Win32_OperatingSystem must be readable: {:?}", hw.os);
        assert!(hw.cpu.is_yes(), "Win32_Processor must be readable: {:?}", hw.cpu);
        assert!(
            env.logical_processors().is_some_and(|n| n >= 1),
            "logical processors: {:?}",
            env.logical_processors()
        );
        // IOMMU is never claimed, on any machine.
        assert!(env.security.as_ref().unwrap().iommu.is_unknown());
        // The gate must agree with the restore status it was computed from.
        assert_eq!(env.restore_gate_open, env.restore.as_ref().unwrap().gate_open);
        assert!(took < std::time::Duration::from_secs(120), "probing took {took:?}");
    }
}
