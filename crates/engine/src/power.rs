//! Which Windows power plan is active. Read-only.
//!
//! The active plan is a GUID under `HKLM\SYSTEM\CurrentControlSet\Control\Power\
//! User\PowerSchemes\ActivePowerScheme`. Windows 11 also has a separate "power
//! mode" overlay (Best performance / Balanced / Best power efficiency) that is
//! not stored here, so this reading is the plan only and the scanner's copy says
//! so (NOTES.md N43).

use serde::Serialize;
use ts_rs::TS;

use super::probe::Probe;
use super::registry::{Hive, RegistryBackend};

const KEY: &str = r"SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes";
const VALUE: &str = "ActivePowerScheme";

/// The four schemes Windows itself ships. GUIDs are from Microsoft's
/// documentation of `powercfg` (from memory; VERIFY on a real machine, N43).
const BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
const HIGH_PERFORMANCE: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";
const POWER_SAVER: &str = "a1841308-3541-4fab-bc81-f71556f20b4a";
const ULTIMATE_PERFORMANCE: &str = "e9a42b02-d5df-448d-aa00-03f14749eb61";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum PowerPlanKind {
    Balanced,
    HighPerformance,
    PowerSaver,
    UltimatePerformance,
    /// A plan we do not recognise: one the user, the PC maker or another tool made.
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PowerPlan {
    pub kind: PowerPlanKind,
    /// Lower-case GUID as Windows stores it.
    pub guid: String,
}

fn kind_of(guid: &str) -> PowerPlanKind {
    match guid {
        BALANCED => PowerPlanKind::Balanced,
        HIGH_PERFORMANCE => PowerPlanKind::HighPerformance,
        POWER_SAVER => PowerPlanKind::PowerSaver,
        ULTIMATE_PERFORMANCE => PowerPlanKind::UltimatePerformance,
        _ => PowerPlanKind::Custom,
    }
}

fn looks_like_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(n, p)| p.len() == *n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub fn probe_power_plan(reg: &dyn RegistryBackend) -> Probe<PowerPlan> {
    let raw = match reg.read_value(Hive::LocalMachine, KEY, VALUE) {
        Ok(Some(v)) => v,
        // Windows uses a default plan when nothing is recorded, but which one
        // depends on the edition and the PC maker. Not guessed.
        Ok(None) => return Probe::unknown("Windows has not recorded an active power plan"),
        Err(e) => return Probe::unknown(format!("could not read the active power plan: {e}")),
    };
    let Some(text) = raw.as_sz() else {
        return Probe::unknown("the active power plan is stored in a format we do not read");
    };
    let guid = text.trim().trim_matches(|c| c == '{' || c == '}').to_ascii_lowercase();
    if !looks_like_guid(&guid) {
        return Probe::unknown("the active power plan value is not a GUID");
    }
    Probe::yes(PowerPlan {
        kind: kind_of(&guid),
        guid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::fake::FakeRegistry;
    use crate::types::RawValue;

    fn with(value: Option<RawValue>) -> FakeRegistry {
        let r = FakeRegistry::new();
        if let Some(v) = value {
            r.write_value(Hive::LocalMachine, KEY, VALUE, &v).unwrap();
        }
        r
    }

    #[test]
    fn the_four_shipped_plans_are_recognised_whatever_the_case_or_braces() {
        for (guid, kind) in [
            (BALANCED, PowerPlanKind::Balanced),
            (HIGH_PERFORMANCE, PowerPlanKind::HighPerformance),
            (POWER_SAVER, PowerPlanKind::PowerSaver),
            (ULTIMATE_PERFORMANCE, PowerPlanKind::UltimatePerformance),
        ] {
            for text in [guid.to_owned(), guid.to_ascii_uppercase(), format!("{{{guid}}}")] {
                let p = probe_power_plan(&with(Some(RawValue::sz(&text))));
                assert_eq!(p.value().map(|p| p.kind), Some(kind), "{guid}");
            }
        }
    }

    #[test]
    fn another_guid_is_a_custom_plan_and_keeps_its_guid() {
        let p = probe_power_plan(&with(Some(RawValue::sz("12345678-1234-1234-1234-123456789abc"))));
        let plan = p.value().unwrap();
        assert_eq!(plan.kind, PowerPlanKind::Custom);
        assert_eq!(plan.guid, "12345678-1234-1234-1234-123456789abc");
    }

    #[test]
    fn missing_wrong_typed_or_malformed_values_are_unknown_never_guessed() {
        assert!(probe_power_plan(&with(None)).is_unknown());
        assert!(probe_power_plan(&with(Some(RawValue::dword(1)))).is_unknown());
        assert!(probe_power_plan(&with(Some(RawValue::sz("balanced")))).is_unknown());
    }
}
