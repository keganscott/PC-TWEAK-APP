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

    /// Forget anything cached. Called after a change that could alter the
    /// answers (a restore point was created, a tweak was applied or reverted).
    fn invalidate(&self) {}

    /// Forget everything, including slow-changing hardware facts. Used by an
    /// explicit rescan.
    fn invalidate_all(&self) {
        self.invalidate();
    }
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
            restore_gate_open: self.gate_open,
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
    tester: bool,
}

impl License {
    pub fn free() -> Self {
        Self {
            tier: Tier::Free,
            tester: false,
        }
    }

    /// Development and tests only.
    #[cfg(any(test, feature = "test-support", feature = "dev-stubs"))]
    pub fn dev(tier: Tier) -> Self {
        Self { tier, tester: false }
    }

    /// The tester build (Cargo feature `tester`, docs/TEST-ON-YOUR-PC.md):
    /// every plan unlocked so the paid changes can be tried on a real PC
    /// before licensing exists (Phase 7). Only the plan check changes; the
    /// restore gate, the journal and every allowlist are the same as in a
    /// release, and the UI labels the build from `ContextInfo::tester_build`.
    #[cfg(feature = "tester")]
    pub fn tester() -> Self {
        Self {
            tier: Tier::Ultimate,
            tester: true,
        }
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }

    pub fn is_tester(&self) -> bool {
        self.tester
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
    /// One of the competitive shooters shown as buttons on Games; the rest
    /// are in its list of the most-played games (Kegan, 2026-10-09).
    pub featured: bool,
    /// PeakTweaks looks for this game's install (`game_installs.rs`), so "not
    /// found" means something. For the other games it is not looked for.
    pub looked_for: bool,
}

/// Kegan's answer to "Add Valorant, CS2 and Apex Legends to PeakTweaks' game
/// list?" (plan section 11, item 5; NOTES N75): yes, 2026-10-09 ("5 options
/// for top 5 most competitive FPS games like apex"). Offering a game lists it
/// and lets it be picked; its per-game tools still wait on its anti-cheat
/// (`games::anti_cheat_block`, N75).
pub const OFFER_VALORANT_CS2_APEX: bool = true;

const fn game(id: &'static str, name: &'static str, featured: bool, looked_for: bool) -> GameInfo {
    GameInfo {
        id,
        name,
        featured,
        looked_for,
    }
}

const FORTNITE: GameInfo = game("fortnite", "Fortnite", true, true);
const MINECRAFT: GameInfo = game("minecraft", "Minecraft", false, true);
const ROBLOX: GameInfo = game("roblox", "Roblox", false, true);

/// Every game PeakTweaks can recognise, offered or not (`games.rs` has their
/// anti-cheat and program files). The five featured shooters, then the
/// most-played games on PC (Kegan asked for about 25; the list is mine, from
/// public player-count charts as I recall them, NOTES N99). The first six
/// have program files; the rest can be picked as the main game but have no
/// per-game tools. Those sold on Steam are looked for in its libraries
/// (`game_installs::STEAM_GAMES`).
pub const ALL_GAMES: &[GameInfo] = &[
    FORTNITE,
    MINECRAFT,
    ROBLOX,
    game("valorant", "Valorant", true, true),
    game("cs2", "Counter-Strike 2", true, true),
    game("apex", "Apex Legends", true, true),
    game("cod", "Call of Duty", true, true),
    game("league", "League of Legends", false, false),
    game("dota2", "Dota 2", false, true),
    game("pubg", "PUBG: Battlegrounds", false, true),
    game("overwatch", "Overwatch 2", false, true),
    game("r6siege", "Rainbow Six Siege", false, true),
    game("rocketleague", "Rocket League", false, true),
    game("gta5", "Grand Theft Auto V", false, true),
    game("marvelrivals", "Marvel Rivals", false, true),
    game("destiny2", "Destiny 2", false, true),
    game("rust", "Rust", false, true),
    game("tarkov", "Escape from Tarkov", false, false),
    game("thefinals", "The Finals", false, true),
    game("tf2", "Team Fortress 2", false, true),
    game("dbd", "Dead by Daylight", false, true),
    game("warframe", "Warframe", false, true),
    game("wow", "World of Warcraft", false, false),
    game("genshin", "Genshin Impact", false, false),
    game("eafc", "EA Sports FC", false, false),
    game("helldivers2", "Helldivers 2", false, true),
    game("poe2", "Path of Exile 2", false, true),
    game("deltaforce", "Delta Force", false, true),
    game("battlefield6", "Battlefield 6", false, true),
    game("naraka", "Naraka: Bladepoint", false, true),
];

/// The games offered. Ids are validated against this list.
pub const KNOWN_GAMES: &[GameInfo] = if OFFER_VALORANT_CS2_APEX {
    ALL_GAMES
} else {
    &[FORTNITE, MINECRAFT, ROBLOX]
};

/// A known game's name; its id when it is not one.
pub fn game_name(id: &str) -> &str {
    ALL_GAMES.iter().find(|g| g.id == id).map_or(id, |g| g.name)
}

/// True for a game in `KNOWN_GAMES`.
pub fn is_offered(game_id: &str) -> bool {
    KNOWN_GAMES.iter().any(|g| g.id == game_id)
}
