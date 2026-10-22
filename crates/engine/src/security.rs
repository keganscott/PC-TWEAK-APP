//! Security-state probes: Secure Boot, Memory Integrity (HVCI), TPM, IOMMU.
//!
//! Display only. Nothing here toggles anything (plan section 12). Every probe
//! returns `Probe<T>`: Yes, No, or Unknown, and never guesses.

use serde::Serialize;
use ts_rs::TS;

use super::error::{EngineError, Result};
use super::probe::Probe;
use super::registry::{Hive, RegistryBackend};
use super::wmi::{WmiRow, WmiSource, NS_DEVICEGUARD, NS_TPM};

const SECURE_BOOT_KEY: &str = r"SYSTEM\CurrentControlSet\Control\SecureBoot\State";
const SECURE_BOOT_VALUE: &str = "UEFISecureBootEnabled";
const HVCI_KEY: &str = r"SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity";
const HVCI_VALUE: &str = "Enabled";

pub(crate) const WQL_DEVICE_GUARD: &str = "SELECT SecurityServicesRunning, SecurityServicesConfigured, \
     AvailableSecurityProperties, VirtualizationBasedSecurityStatus FROM Win32_DeviceGuard";
pub(crate) const WQL_TPM: &str = "SELECT IsEnabled_InitialValue, IsActivated_InitialValue, SpecVersion FROM Win32_Tpm";

/// `SecurityServicesRunning` value meaning Memory Integrity / HVCI. Microsoft's
/// "Enable memory integrity" page (learn.microsoft.com/windows/security/
/// hardware-security/enable-virtualization-based-protection-of-code-integrity,
/// checked 2026-10-08): "1 If present, Credential Guard is running", "2 If
/// present, memory integrity is running", 3 System Guard Secure Launch, 4 SMM
/// Firmware Measurement.
const SERVICE_HVCI: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TpmInfo {
    /// e.g. "2.0". First field of `Win32_Tpm.SpecVersion` ("2.0, 0, 1.38").
    pub spec_version: String,
}

impl TpmInfo {
    pub fn is_2_0(&self) -> bool {
        self.spec_version.starts_with("2.")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SecurityReport {
    pub secure_boot: Probe<()>,
    pub memory_integrity: Probe<()>,
    pub tpm: Probe<TpmInfo>,
    pub iommu: Probe<()>,
}

// ---------------------------------------------------------------------------
// Secure Boot
// ---------------------------------------------------------------------------

pub fn probe_secure_boot(reg: &dyn RegistryBackend) -> Probe<()> {
    let exists = match reg.key_exists(Hive::LocalMachine, SECURE_BOOT_KEY) {
        Ok(e) => e,
        Err(e) => return Probe::unknown(format!("cannot read the Secure Boot state key: {e}")),
    };
    if !exists {
        return Probe::no("no Secure Boot state key: legacy BIOS boot, or Secure Boot is unsupported");
    }
    match reg.read_value(Hive::LocalMachine, SECURE_BOOT_KEY, SECURE_BOOT_VALUE) {
        Err(e) => Probe::unknown(format!("cannot read {SECURE_BOOT_VALUE}: {e}")),
        Ok(None) => Probe::unknown(format!("{SECURE_BOOT_VALUE} is missing from the Secure Boot state key")),
        Ok(Some(v)) => match v.as_dword() {
            Some(1) => Probe::yes(()),
            Some(0) => Probe::no("Secure Boot is turned off in firmware"),
            Some(other) => Probe::unknown(format!("{SECURE_BOOT_VALUE} has the unexpected value {other}")),
            None => Probe::unknown(format!("{SECURE_BOOT_VALUE} is not a DWORD")),
        },
    }
}

// ---------------------------------------------------------------------------
// Device Guard (HVCI) and the IOMMU question
// ---------------------------------------------------------------------------

/// What `Win32_DeviceGuard` told us, kept raw so the IOMMU reason can quote it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceGuardFacts {
    pub services_running: Vec<u64>,
    pub services_configured: Vec<u64>,
    pub available_properties: Vec<u64>,
}

fn device_guard(wmi: &dyn WmiSource) -> Result<DeviceGuardFacts> {
    let rows = wmi.query(NS_DEVICEGUARD, WQL_DEVICE_GUARD)?;
    let row = rows.first().ok_or_else(|| EngineError::Wmi {
        namespace: NS_DEVICEGUARD.into(),
        detail: "Win32_DeviceGuard returned no instance".into(),
        timed_out: false,
    })?;
    let arr = |name: &str| row.get(name).and_then(|v| v.as_u64_array()).unwrap_or_default();
    // "Memory Integrity is off" is only said when Windows gave a list of running
    // services to look in. A missing, null or non-numeric list is "could not
    // tell", never an empty list (which would read as "nothing is running").
    let running = row
        .get("SecurityServicesRunning")
        .and_then(|v| v.as_u64_array())
        .ok_or_else(|| EngineError::Wmi {
            namespace: NS_DEVICEGUARD.into(),
            detail: "Win32_DeviceGuard did not return a readable SecurityServicesRunning list".into(),
            timed_out: false,
        })?;
    Ok(DeviceGuardFacts {
        services_running: running,
        services_configured: arr("SecurityServicesConfigured"),
        available_properties: arr("AvailableSecurityProperties"),
    })
}

fn hvci_registry(reg: &dyn RegistryBackend) -> Result<Option<u32>> {
    Ok(reg
        .read_value(Hive::LocalMachine, HVCI_KEY, HVCI_VALUE)?
        .and_then(|v| v.as_dword()))
}

/// Memory Integrity is only "on" when Windows says it is *running*. The registry
/// says what is configured, which can differ (pending reboot, unsupported
/// hardware), so it is used to explain a No and never to promote to Yes.
pub fn hvci_from(dg: &Result<DeviceGuardFacts>, registry: &Result<Option<u32>>) -> Probe<()> {
    match dg {
        Ok(f) if f.services_running.contains(&SERVICE_HVCI) => Probe::yes(()),
        Ok(_) => match registry {
            Ok(Some(1)) => Probe::no("Memory Integrity is configured on but not running (restart pending, or unsupported hardware)"),
            _ => Probe::no("Memory Integrity is not running"),
        },
        Err(e) => match registry {
            Ok(Some(1)) => Probe::unknown(format!(
                "cannot query Device Guard ({e}); the registry says it is configured on, which does not prove it is running"
            )),
            _ => Probe::unknown(format!("cannot query Device Guard: {e}")),
        },
    }
}

pub fn probe_iommu(dg: &Result<DeviceGuardFacts>) -> Probe<()> {
    // Windows offers no reliable user-mode signal that VT-d / AMD-Vi is enabled.
    // Win32_DeviceGuard.AvailableSecurityProperties can list "DMA protection",
    // but that says the platform *supports* it, not that a game's IOMMU
    // requirement is met. We quote what we saw and stay Unknown until a
    // documented signal is verified (NOTES.md N21).
    let seen = match dg {
        Ok(f) if !f.available_properties.is_empty() => {
            format!(
                " (Win32_DeviceGuard.AvailableSecurityProperties: {:?})",
                f.available_properties
            )
        }
        _ => String::new(),
    };
    Probe::unknown(format!(
        "Windows exposes no reliable way to tell from a normal program whether the IOMMU (VT-d / AMD-Vi) is enabled; \
         check the firmware setup screen{seen}"
    ))
}

// ---------------------------------------------------------------------------
// TPM
// ---------------------------------------------------------------------------

/// HRESULTs WMI returns for a namespace or class that does not exist.
fn wmi_target_absent(e: &EngineError) -> bool {
    match e {
        EngineError::Wmi {
            detail,
            timed_out: false,
            ..
        } => {
            let d = detail.to_ascii_lowercase();
            d.contains("0x8004100e") // WBEM_E_INVALID_NAMESPACE
                || d.contains("0x80041010") // WBEM_E_INVALID_CLASS
                || d.contains("invalid namespace")
                || d.contains("invalid class")
        }
        _ => false,
    }
}

pub fn tpm_from(rows: &Result<Vec<WmiRow>>) -> Probe<TpmInfo> {
    let rows = match rows {
        Ok(r) => r,
        Err(e) if wmi_target_absent(e) => {
            return Probe::no("this PC has no TPM interface (the WMI TPM class is absent)")
        }
        Err(e) => return Probe::unknown(format!("cannot query the TPM: {e}")),
    };
    let Some(row) = rows.first() else {
        return Probe::no("Windows reports no TPM");
    };
    let spec = row
        .str("SpecVersion")
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let (enabled, activated) = (row.bool("IsEnabled_InitialValue"), row.bool("IsActivated_InitialValue"));
    match (spec, enabled, activated) {
        (Some(spec), Some(true), Some(true)) => Probe::yes(TpmInfo { spec_version: spec }),
        (Some(spec), Some(e), Some(a)) => Probe::no(format!(
            "TPM {spec} is present but {}{}",
            if e { "" } else { "not enabled" },
            match (e, a) {
                (false, false) => " and not activated",
                (true, false) => "not activated",
                _ => "",
            }
        )),
        _ => Probe::unknown("the TPM was found but its state fields were missing"),
    }
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

pub fn probe_security(wmi: &dyn WmiSource, reg: &dyn RegistryBackend) -> SecurityReport {
    let dg = device_guard(wmi);
    let hvci_reg = hvci_registry(reg);
    let tpm_rows = wmi.query(NS_TPM, WQL_TPM);
    SecurityReport {
        secure_boot: probe_secure_boot(reg),
        memory_integrity: hvci_from(&dg, &hvci_reg),
        tpm: tpm_from(&tpm_rows),
        iommu: probe_iommu(&dg),
    }
}

// ---------------------------------------------------------------------------
// Anti-cheat readiness (display only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SecurityFeature {
    SecureBoot,
    Tpm,
    Iommu,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(tag = "status", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ReadinessStatus {
    /// Everything the game asks for is confirmed on.
    Ready,
    /// At least one required feature is confirmed off. `unresolved` lists any
    /// others we could not determine.
    NotReady {
        missing: Vec<SecurityFeature>,
        unresolved: Vec<SecurityFeature>,
    },
    /// Nothing confirmed off, but some requirements could not be determined.
    Unknown { unresolved: Vec<SecurityFeature> },
    /// We know of no such requirement for this game.
    NoKnownRequirements,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GameReadiness {
    pub game_id: String,
    pub requires: Vec<SecurityFeature>,
    /// Where the requirement applies, e.g. "tournaments".
    pub scope: String,
    /// Where the requirement comes from and how far to trust it.
    pub source: String,
    pub status: ReadinessStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AntiCheatReadiness {
    pub secure_boot: Probe<()>,
    pub tpm: Probe<TpmInfo>,
    pub iommu: Probe<()>,
    pub per_game: Vec<GameReadiness>,
}

struct Requirement {
    game_id: &'static str,
    requires: &'static [SecurityFeature],
    scope: &'static str,
    source: &'static str,
}

/// Requirements we know about, one entry per `ALL_GAMES` game in the same
/// order. Only the offered games are reported (`env::KNOWN_GAMES`); offering
/// a title is a product decision (plan section 11, item 5; NOTES N75).
const REQUIREMENTS: &[Requirement] = &[
    Requirement {
        game_id: "fortnite",
        requires: &[SecurityFeature::SecureBoot, SecurityFeature::Tpm, SecurityFeature::Iommu],
        scope: "tournaments",
        source: "Epic's announcement of tournament requirements from 2026-02-19, as recorded in the dev plan (section 4.3). \
                 Not independently verified.",
    },
    Requirement {
        game_id: "minecraft",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "roblox",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "valorant",
        requires: &[SecurityFeature::Tpm, SecurityFeature::SecureBoot],
        scope: "playing on Windows 11",
        // VERIFY against Riot's articles before offering Valorant (NOTES N75).
        source: "Riot's support articles for the VAN 9001 and VAN 9003 errors, from memory. \
                 Not independently verified.",
    },
    Requirement {
        game_id: "cs2",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "apex",
        requires: &[],
        scope: "",
        source: "",
    },    Requirement {
        game_id: "cod",
        requires: &[SecurityFeature::Tpm, SecurityFeature::SecureBoot],
        scope: "Black Ops 7 and Warzone",
        // VERIFY against the publisher's own page (NOTES N99).
        source: "Activision's announcement for Black Ops 7, from memory. \
                 Not independently verified.",
    },
    Requirement {
        game_id: "league",
        requires: &[SecurityFeature::Tpm, SecurityFeature::SecureBoot],
        scope: "playing on Windows 11",
        // VERIFY against the publisher's own page (NOTES N99).
        source: "Riot Vanguard's requirements, the same as Valorant's, from memory. \
                 Not independently verified.",
    },
    Requirement {
        game_id: "dota2",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "pubg",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "overwatch",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "r6siege",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "rocketleague",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "gta5",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "marvelrivals",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "destiny2",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "rust",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "tarkov",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "thefinals",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "tf2",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "dbd",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "warframe",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "wow",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "genshin",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "eafc",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "helldivers2",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "poe2",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "deltaforce",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "battlefield6",
        requires: &[SecurityFeature::SecureBoot],
        scope: "playing",
        // VERIFY against the publisher's own page (NOTES N99).
        source: "EA's requirements for Battlefield 6 (EA Javelin), from memory. \
                 Not independently verified.",
    },
    Requirement {
        game_id: "naraka",
        requires: &[],
        scope: "",
        source: "",
    },
    Requirement {
        game_id: "arcraiders",
        requires: &[],
        scope: "",
        source: "",
    },
];

pub fn evaluate_requirements(requires: &[SecurityFeature], report: &SecurityReport) -> ReadinessStatus {
    if requires.is_empty() {
        return ReadinessStatus::NoKnownRequirements;
    }
    // TPM counts as met only when it is 2.0; a TPM 1.2 is a definite "not met".
    let tpm_state = match &report.tpm {
        Probe::Yes { value } if value.is_2_0() => Some(true),
        Probe::Yes { .. } | Probe::No { .. } => Some(false),
        Probe::Unknown { .. } => None,
    };
    let state = |f: SecurityFeature| match f {
        SecurityFeature::SecureBoot => tri(&report.secure_boot),
        SecurityFeature::Iommu => tri(&report.iommu),
        SecurityFeature::Tpm => tpm_state,
    };
    let (mut missing, mut unresolved) = (Vec::new(), Vec::new());
    for &f in requires {
        match state(f) {
            Some(true) => {}
            Some(false) => missing.push(f),
            None => unresolved.push(f),
        }
    }
    match (missing.is_empty(), unresolved.is_empty()) {
        (true, true) => ReadinessStatus::Ready,
        (true, false) => ReadinessStatus::Unknown { unresolved },
        (false, _) => ReadinessStatus::NotReady { missing, unresolved },
    }
}

fn tri<T>(p: &Probe<T>) -> Option<bool> {
    match p {
        Probe::Yes { .. } => Some(true),
        Probe::No { .. } => Some(false),
        Probe::Unknown { .. } => None,
    }
}

pub fn anti_cheat_readiness(report: &SecurityReport) -> AntiCheatReadiness {
    AntiCheatReadiness {
        secure_boot: report.secure_boot.clone(),
        tpm: report.tpm.clone(),
        iommu: report.iommu.clone(),
        per_game: REQUIREMENTS
            .iter()
            .filter(|r| crate::env::is_offered(r.game_id))
            .map(|r| GameReadiness {
                game_id: r.game_id.to_owned(),
                requires: r.requires.to_vec(),
                scope: r.scope.to_owned(),
                source: r.source.to_owned(),
                status: evaluate_requirements(r.requires, report),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::fake::FakeRegistry;
    use crate::types::RawValue;
    use crate::wmi::{FakeWmi, WmiValue};

    fn reg_with_secure_boot(v: Option<u32>) -> FakeRegistry {
        let r = FakeRegistry::new();
        match v {
            Some(v) => r.set_external(
                Hive::LocalMachine,
                SECURE_BOOT_KEY,
                SECURE_BOOT_VALUE,
                RawValue::dword(v),
            ),
            None => {
                // key present, value absent
                r.set_external(Hive::LocalMachine, SECURE_BOOT_KEY, "Other", RawValue::dword(0));
            }
        }
        r
    }

    #[test]
    fn secure_boot_states() {
        assert!(probe_secure_boot(&reg_with_secure_boot(Some(1))).is_yes());
        assert!(probe_secure_boot(&reg_with_secure_boot(Some(0))).is_no());
        assert!(probe_secure_boot(&reg_with_secure_boot(Some(7))).is_unknown());
        assert!(probe_secure_boot(&reg_with_secure_boot(None)).is_unknown());
        // Key absent entirely: legacy BIOS. A definite No, with a reason.
        let p = probe_secure_boot(&FakeRegistry::new());
        assert!(
            matches!(&p, Probe::No { reason } if reason.contains("legacy BIOS")),
            "{p:?}"
        );
    }

    fn dg_rows(running: &[u64], available: &[u64]) -> FakeWmi {
        let arr = |v: &[u64]| WmiValue::Array(v.iter().map(|n| WmiValue::UInt(*n)).collect());
        FakeWmi::new().with_rows(
            NS_DEVICEGUARD,
            WQL_DEVICE_GUARD,
            vec![vec![
                ("SecurityServicesRunning", arr(running)),
                ("SecurityServicesConfigured", arr(running)),
                ("AvailableSecurityProperties", arr(available)),
            ]],
        )
    }

    #[test]
    fn hvci_needs_the_service_running() {
        let reg = FakeRegistry::new();
        let running = probe_security(&dg_rows(&[1, 2], &[]), &reg);
        assert!(running.memory_integrity.is_yes());

        let off = probe_security(&dg_rows(&[], &[]), &reg);
        assert!(off.memory_integrity.is_no());
    }

    #[test]
    fn an_unreadable_running_list_is_unknown_not_off() {
        let reg = FakeRegistry::new();
        for bad in [
            WmiValue::Null,
            WmiValue::Str("none".into()),
            WmiValue::Array(vec![WmiValue::Null]),
        ] {
            let wmi = FakeWmi::new().with_rows(
                NS_DEVICEGUARD,
                WQL_DEVICE_GUARD,
                vec![vec![
                    ("SecurityServicesRunning", bad.clone()),
                    ("SecurityServicesConfigured", WmiValue::Array(vec![])),
                    ("AvailableSecurityProperties", WmiValue::Array(vec![])),
                ]],
            );
            let p = probe_security(&wmi, &reg).memory_integrity;
            assert!(p.is_unknown(), "{bad:?} gave {p:?}");
        }
        // A property that is missing from the row altogether is the same.
        let wmi = FakeWmi::new().with_rows(NS_DEVICEGUARD, WQL_DEVICE_GUARD, vec![vec![]]);
        assert!(probe_security(&wmi, &reg).memory_integrity.is_unknown());
    }

    #[test]
    fn registry_can_explain_a_no_but_never_promote_to_yes() {
        let reg = FakeRegistry::new();
        reg.set_external(Hive::LocalMachine, HVCI_KEY, HVCI_VALUE, RawValue::dword(1));
        let p = probe_security(&dg_rows(&[], &[]), &reg).memory_integrity;
        assert!(
            matches!(&p, Probe::No { reason } if reason.contains("configured on but not running")),
            "{p:?}"
        );

        // WMI unavailable + registry says on: Unknown, not Yes.
        let broken = FakeWmi::new();
        let p = probe_security(&broken, &reg).memory_integrity;
        assert!(p.is_unknown(), "{p:?}");
    }

    #[test]
    fn iommu_is_always_unknown_and_quotes_what_windows_said() {
        let p = probe_security(&dg_rows(&[], &[1, 2, 3]), &FakeRegistry::new()).iommu;
        assert!(
            matches!(&p, Probe::Unknown { reason } if reason.contains("[1, 2, 3]")),
            "{p:?}"
        );
        assert!(probe_security(&FakeWmi::new(), &FakeRegistry::new()).iommu.is_unknown());
    }

    fn tpm_rows(enabled: bool, activated: bool, spec: &str) -> FakeWmi {
        FakeWmi::new().with_rows(
            NS_TPM,
            WQL_TPM,
            vec![vec![
                ("IsEnabled_InitialValue", WmiValue::Bool(enabled)),
                ("IsActivated_InitialValue", WmiValue::Bool(activated)),
                ("SpecVersion", WmiValue::Str(spec.into())),
            ]],
        )
    }

    #[test]
    fn tpm_states() {
        let reg = FakeRegistry::new();
        let ok = probe_security(&tpm_rows(true, true, "2.0, 0, 1.38"), &reg).tpm;
        assert_eq!(
            ok,
            Probe::yes(TpmInfo {
                spec_version: "2.0".into()
            })
        );

        assert!(probe_security(&tpm_rows(false, false, "2.0, 0, 1.38"), &reg)
            .tpm
            .is_no());
        assert!(probe_security(&tpm_rows(true, false, "2.0, 0, 1.38"), &reg).tpm.is_no());

        // Namespace missing (no TPM interface): definite No.
        let absent = FakeWmi::new().with_error(NS_TPM, WQL_TPM, "cannot connect: HRESULT Call failed with: 0x8004100E");
        assert!(probe_security(&absent, &reg).tpm.is_no());

        // Access denied (not elevated): Unknown, never No.
        let denied = FakeWmi::new().with_error(NS_TPM, WQL_TPM, "query failed: HRESULT Call failed with: 0x80041003");
        assert!(probe_security(&denied, &reg).tpm.is_unknown());

        // Class exists but no instance.
        let empty = FakeWmi::new().with_rows(NS_TPM, WQL_TPM, vec![]);
        assert!(probe_security(&empty, &reg).tpm.is_no());
    }

    fn report(sb: Probe<()>, tpm: Probe<TpmInfo>, iommu: Probe<()>) -> SecurityReport {
        SecurityReport {
            secure_boot: sb,
            memory_integrity: Probe::no("x"),
            tpm,
            iommu,
        }
    }

    fn tpm2() -> Probe<TpmInfo> {
        Probe::yes(TpmInfo {
            spec_version: "2.0".into(),
        })
    }

    #[test]
    fn readiness_never_reads_unknown_as_yes_or_no() {
        let req = [
            SecurityFeature::SecureBoot,
            SecurityFeature::Tpm,
            SecurityFeature::Iommu,
        ];

        let all = report(Probe::yes(()), tpm2(), Probe::yes(()));
        assert_eq!(evaluate_requirements(&req, &all), ReadinessStatus::Ready);

        // Realistic today: IOMMU is always Unknown, so Fortnite is never "Ready".
        let iommu_unknown = report(Probe::yes(()), tpm2(), Probe::unknown("?"));
        assert_eq!(
            evaluate_requirements(&req, &iommu_unknown),
            ReadinessStatus::Unknown {
                unresolved: vec![SecurityFeature::Iommu]
            }
        );

        let sb_off = report(Probe::no("off"), tpm2(), Probe::unknown("?"));
        assert_eq!(
            evaluate_requirements(&req, &sb_off),
            ReadinessStatus::NotReady {
                missing: vec![SecurityFeature::SecureBoot],
                unresolved: vec![SecurityFeature::Iommu]
            }
        );

        let tpm12 = report(
            Probe::yes(()),
            Probe::yes(TpmInfo {
                spec_version: "1.2".into(),
            }),
            Probe::yes(()),
        );
        assert_eq!(
            evaluate_requirements(&req, &tpm12),
            ReadinessStatus::NotReady {
                missing: vec![SecurityFeature::Tpm],
                unresolved: vec![]
            }
        );

        assert_eq!(evaluate_requirements(&[], &all), ReadinessStatus::NoKnownRequirements);
    }

    #[test]
    fn per_game_table_covers_the_known_games_and_no_others() {
        let table: Vec<&str> = REQUIREMENTS.iter().map(|r| r.game_id).collect();
        let all: Vec<&str> = crate::env::ALL_GAMES.iter().map(|g| g.id).collect();
        assert_eq!(
            table, all,
            "requirements table and ALL_GAMES must list the same games in the same order"
        );
        let r = anti_cheat_readiness(&report(Probe::yes(()), tpm2(), Probe::unknown("?")));
        let ids: Vec<&str> = r.per_game.iter().map(|g| g.game_id.as_str()).collect();
        let known: Vec<&str> = crate::env::KNOWN_GAMES.iter().map(|g| g.id).collect();
        assert_eq!(ids, known, "only the offered games are reported");
        assert!(r.per_game[0].source.contains("Not independently verified"));
        for req in REQUIREMENTS.iter().filter(|r| !r.requires.is_empty()) {
            assert!(req.source.contains("Not independently verified"), "{}", req.game_id);
        }
    }
}
