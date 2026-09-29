//! Things the engine owns and the webview can never set: the environment
//! snapshot, the restore-point gate, the license, and the list of known games.

use ts_rs::TS;

use super::types::{SystemEnv, Tier};

/// Produces the `SystemEnv` and answers whether a verified restore point
/// exists. Phase 3 supplies the real implementation (WMI probes, restore-point
/// verification). Until then `StubProbe` reports an empty environment and a
/// closed gate, which is the correct failure direction: nothing can be applied.
pub trait EnvProbe: Send + Sync {
    /// A fresh snapshot. The engine fills in `target_game` itself.
    fn probe(&self, elevated: bool) -> SystemEnv;

    /// True only while a restore point has been verified. Re-checked by
    /// `Engine::apply` immediately before it writes.
    fn restore_gate_open(&self) -> bool;
}

pub struct StubProbe {
    gate_open: bool,
}

impl StubProbe {
    /// Production default until the Phase 3 restore engine exists.
    pub fn closed() -> Self {
        Self { gate_open: false }
    }

    /// Development and tests only: pretend a restore point is verified.
    #[cfg(any(test, feature = "test-support", feature = "dev-stubs"))]
    pub fn open_for_dev() -> Self {
        Self { gate_open: true }
    }
}

impl EnvProbe for StubProbe {
    fn probe(&self, elevated: bool) -> SystemEnv {
        SystemEnv {
            elevated,
            system_protection_enabled: self.gate_open,
            ..SystemEnv::default()
        }
    }

    fn restore_gate_open(&self) -> bool {
        self.gate_open
    }
}

/// The license the engine enforces. Held in Rust; the UI can display it but
/// never sets it. Real, offline-verifiable tokens arrive in Phase 7; until then
/// production builds are Free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct License {
    tier: Tier,
}

impl License {
    pub fn free() -> Self {
        Self { tier: Tier::Free }
    }

    /// Development and tests only.
    #[cfg(any(test, feature = "test-support", feature = "dev-stubs"))]
    pub fn dev(tier: Tier) -> Self {
        Self { tier }
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }
}

/// A game the engine knows about. Ids are validated against this list; the
/// webview can only pick from it.
#[derive(Debug, Clone, Copy, serde::Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GameInfo {
    pub id: &'static str,
    pub name: &'static str,
}

/// Titles named in the plan's game cards. More are added only when Kegan
/// decides which anti-cheat titles to support (plan section 11, item 5).
pub const KNOWN_GAMES: &[GameInfo] = &[
    GameInfo {
        id: "fortnite",
        name: "Fortnite",
    },
    GameInfo {
        id: "minecraft",
        name: "Minecraft",
    },
    GameInfo {
        id: "roblox",
        name: "Roblox",
    },
];
