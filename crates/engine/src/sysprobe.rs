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

use super::env::EnvProbe;
use super::hardware::{probe_hardware, HardwareReport, OsFacts};
use super::journal::now_ms;
use super::registry::RegistryBackend;
use super::restore::RestoreService;
use super::scanner::{scan, ScanReport};
use super::security::{anti_cheat_readiness, probe_security, AntiCheatReadiness, SecurityReport};
use super::types::SystemEnv;
use super::wmi::WmiSource;

const HARDWARE_TTL_MS: u64 = 10 * 60 * 1000;
const STATE_TTL_MS: u64 = 30 * 1000;

/// What `audit_system` returns.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SystemAudit {
    pub env: SystemEnv,
    pub anti_cheat: Option<AntiCheatReadiness>,
    /// Findings from the scanner, computed from `env` alone.
    pub scan: ScanReport,
}

impl SystemAudit {
    pub fn from_env(env: SystemEnv) -> Self {
        let anti_cheat = env.security.as_ref().map(anti_cheat_readiness);
        let scan = scan(&env);
        Self { env, anti_cheat, scan }
    }
}

#[derive(Default)]
struct Cache {
    hardware: Option<(u64, HardwareReport)>,
    state: Option<(u64, SecurityReport, super::restore::RestoreStatus)>,
}

pub struct SystemProbe {
    wmi: Arc<dyn WmiSource>,
    reg: Arc<dyn RegistryBackend>,
    facts: Arc<dyn OsFacts>,
    restore: Arc<RestoreService>,
    cache: Mutex<Cache>,
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
        }
    }
}

impl EnvProbe for SystemProbe {
    fn probe(&self, elevated: bool) -> SystemEnv {
        let now = now_ms();
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());

        let hardware = match &cache.hardware {
            Some((at, h)) if now.saturating_sub(*at) < HARDWARE_TTL_MS => h.clone(),
            _ => {
                let h = probe_hardware(self.wmi.as_ref(), self.facts.as_ref());
                cache.hardware = Some((now, h.clone()));
                h
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
        (SystemProbe::new(wmi.clone(), reg, Arc::new(Facts), restore), wmi)
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
    fn the_audit_carries_anti_cheat_readiness_only_when_security_was_probed() {
        let (p, _) = probe();
        let audit = SystemAudit::from_env(p.probe(true));
        let ac = audit.anti_cheat.expect("security was probed");
        assert_eq!(ac.per_game.len(), crate::env::KNOWN_GAMES.len());
        assert!(SystemAudit::from_env(SystemEnv::default()).anti_cheat.is_none());
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
        let audit = SystemAudit::from_env(env.clone());
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
