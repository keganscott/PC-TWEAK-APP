//! What the per-game tools need to know about each game: which anti-cheat it
//! runs and which program files Windows starts for it. Also the switch that
//! waits on Kegan for those tools (plan section 11; NOTES N75).
//!
//! The per-game tools (`tweaks/ifeo_priority.rs`, `tweaks/fullscreen.rs`) only
//! use Windows' own per-program settings, read by Windows when it starts the
//! program. Nothing here touches a game process.
//!
//! Anti-cheat names and program files are from memory (VERIFY, NOTES N75 and
//! N79).

use super::env::ALL_GAMES;
use super::types::{BlockedCode, BlockedReason};

/// Kegan's answer to "Unlock game priority and fullscreen tools for anti-cheat
/// games now?" (NOTES N75). `false`: they stay blocked for every game with an
/// anti-cheat until that game is in `CLEARED_GAMES`. Flip to `true` to unlock
/// them for every offered game.
pub const UNLOCK_ANTI_CHEAT_GAMES: bool = false;

/// Games whose anti-cheat has been tried with the per-game tools on and nothing
/// went wrong. Empty: none yet.
pub const CLEARED_GAMES: &[&str] = &[];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntiCheat {
    EasyAntiCheat,
    BattlEye,
    Hyperion,
    Vanguard,
    Vac,
    Ricochet,
    Javelin,
    AntiCheatExpert,
    GameGuard,
    /// The game runs an anti-cheat PeakTweaks has not identified yet.
    NotIdentified,
}

impl AntiCheat {
    pub fn name(self) -> &'static str {
        match self {
            AntiCheat::EasyAntiCheat => "Easy Anti-Cheat",
            AntiCheat::BattlEye => "BattlEye",
            AntiCheat::Hyperion => "Hyperion",
            AntiCheat::Vanguard => "Riot Vanguard",
            AntiCheat::Vac => "Valve Anti-Cheat",
            AntiCheat::Ricochet => "Ricochet",
            AntiCheat::Javelin => "EA Javelin",
            AntiCheat::AntiCheatExpert => "Anti-Cheat Expert",
            AntiCheat::GameGuard => "nProtect GameGuard",
            AntiCheat::NotIdentified => "an anti-cheat not identified yet",
        }
    }
}

pub struct GameFacts {
    /// An `ALL_GAMES` id.
    pub id: &'static str,
    pub anti_cheat: &'static [AntiCheat],
    /// The program file names Windows starts for the game itself, the names
    /// its per-program settings are keyed by. Empty when the game runs inside
    /// a shared program (Minecraft Java Edition runs in Java), or when they
    /// have not been checked yet (every game after Apex): no per-game tool is
    /// offered for a game without one.
    pub programs: &'static [&'static str],
    /// True when the program's folder stays put between updates, so a setting
    /// Windows keeps by full path (fullscreen optimizations) lasts. Roblox
    /// moves to a new `Versions` folder on each update (NOTES N56).
    pub stable_path: bool,
}

/// One entry per `ALL_GAMES` game, in the same order.
pub const GAME_FACTS: &[GameFacts] = &[
    GameFacts {
        id: "fortnite",
        anti_cheat: &[AntiCheat::EasyAntiCheat, AntiCheat::BattlEye],
        programs: &["FortniteClient-Win64-Shipping.exe"],
        stable_path: true,
    },
    GameFacts {
        id: "minecraft",
        anti_cheat: &[],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "roblox",
        anti_cheat: &[AntiCheat::Hyperion],
        programs: &["RobloxPlayerBeta.exe"],
        stable_path: false,
    },
    GameFacts {
        id: "valorant",
        anti_cheat: &[AntiCheat::Vanguard],
        programs: &["VALORANT-Win64-Shipping.exe"],
        stable_path: true,
    },
    GameFacts {
        id: "cs2",
        anti_cheat: &[AntiCheat::Vac],
        programs: &["cs2.exe"],
        stable_path: true,
    },
    GameFacts {
        id: "apex",
        anti_cheat: &[AntiCheat::EasyAntiCheat],
        // The DirectX 11 and DirectX 12 builds.
        programs: &["r5apex.exe", "r5apex_dx12.exe"],
        stable_path: true,
    },
    // The rest of the list (`env::ALL_GAMES`): anti-cheat from memory, and
    // `NotIdentified` where I am not sure which one it runs (VERIFY, NOTES
    // N99). No program files yet, so no per-game tools.
    GameFacts {
        id: "cod",
        anti_cheat: &[AntiCheat::Ricochet],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "league",
        anti_cheat: &[AntiCheat::Vanguard],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "dota2",
        anti_cheat: &[AntiCheat::Vac],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "pubg",
        anti_cheat: &[AntiCheat::BattlEye],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "overwatch",
        anti_cheat: &[AntiCheat::NotIdentified],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "r6siege",
        anti_cheat: &[AntiCheat::BattlEye],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "rocketleague",
        anti_cheat: &[AntiCheat::NotIdentified],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "gta5",
        anti_cheat: &[AntiCheat::BattlEye],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "marvelrivals",
        anti_cheat: &[AntiCheat::NotIdentified],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "destiny2",
        anti_cheat: &[AntiCheat::BattlEye],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "rust",
        anti_cheat: &[AntiCheat::EasyAntiCheat],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "tarkov",
        anti_cheat: &[AntiCheat::BattlEye],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "thefinals",
        anti_cheat: &[AntiCheat::EasyAntiCheat],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "tf2",
        anti_cheat: &[AntiCheat::Vac],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "dbd",
        anti_cheat: &[AntiCheat::EasyAntiCheat],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "warframe",
        anti_cheat: &[AntiCheat::NotIdentified],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "wow",
        anti_cheat: &[AntiCheat::NotIdentified],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "genshin",
        anti_cheat: &[AntiCheat::NotIdentified],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "eafc",
        anti_cheat: &[AntiCheat::Javelin],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "helldivers2",
        anti_cheat: &[AntiCheat::GameGuard],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "poe2",
        anti_cheat: &[AntiCheat::NotIdentified],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "deltaforce",
        anti_cheat: &[AntiCheat::AntiCheatExpert],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "battlefield6",
        anti_cheat: &[AntiCheat::Javelin],
        programs: &[],
        stable_path: false,
    },
    GameFacts {
        id: "naraka",
        anti_cheat: &[AntiCheat::NotIdentified],
        programs: &[],
        stable_path: false,
    },
];

pub fn facts(game_id: &str) -> Option<&'static GameFacts> {
    GAME_FACTS.iter().find(|f| f.id == game_id)
}

/// The game's name for the screen.
pub fn name(game_id: &str) -> &'static str {
    ALL_GAMES
        .iter()
        .find(|g| g.id == game_id)
        .map_or("this game", |g| g.name)
}

/// Why the per-game tools are not offered for this game yet, if they are not:
/// it runs an anti-cheat and neither `UNLOCK_ANTI_CHEAT_GAMES` nor
/// `CLEARED_GAMES` lets it through.
pub fn anti_cheat_block(game_id: &str) -> Option<BlockedReason> {
    let facts = facts(game_id)?;
    if facts.anti_cheat.is_empty() || UNLOCK_ANTI_CHEAT_GAMES || CLEARED_GAMES.contains(&game_id) {
        return None;
    }
    let names: Vec<&str> = facts.anti_cheat.iter().map(|a| a.name()).collect();
    Some(
        BlockedReason::new(
            BlockedCode::AntiCheatEligibility,
            format!(
                "Not available for {} yet: it has not been tried with its anti-cheat ({}).",
                name(game_id),
                names.join(" and ")
            ),
        )
        .with_trigger(game_id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_game_has_its_facts_in_the_same_order() {
        let ids: Vec<&str> = GAME_FACTS.iter().map(|f| f.id).collect();
        let all: Vec<&str> = ALL_GAMES.iter().map(|g| g.id).collect();
        assert_eq!(ids, all);
        for f in GAME_FACTS {
            for p in f.programs {
                assert!(
                    p.to_ascii_lowercase().ends_with(".exe") && !p.contains(['\\', '/']),
                    "{}: {p:?} must be a bare program file name",
                    f.id
                );
            }
        }
    }

    #[test]
    fn five_featured_shooters_and_twenty_five_more_each_once() {
        // Kegan, 2026-10-09: five buttons, then about 25 most-played games.
        let featured = ALL_GAMES.iter().filter(|g| g.featured).count();
        assert_eq!(featured, 5);
        assert_eq!(ALL_GAMES.len() - featured, 25);
        let mut ids: Vec<&str> = ALL_GAMES.iter().map(|g| g.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ALL_GAMES.len(), "an id is listed twice");
        // Install detection exists exactly for the games with program files
        // or a known data folder (`game_installs.rs`).
        for g in ALL_GAMES {
            assert_eq!(
                g.looked_for,
                matches!(g.id, "fortnite" | "minecraft" | "roblox" | "valorant" | "cs2" | "apex"),
                "{}",
                g.id
            );
        }
    }

    #[test]
    fn games_with_an_anti_cheat_stay_blocked_until_kegan_unlocks_them() {
        // Holds whichever way the switches are set (NOTES N75).
        for g in ["fortnite", "roblox", "valorant", "cs2", "apex"] {
            let open = UNLOCK_ANTI_CHEAT_GAMES || CLEARED_GAMES.contains(&g);
            match anti_cheat_block(g) {
                Some(r) => {
                    assert!(!open, "{g}");
                    assert_eq!(
                        (r.code, r.trigger.as_deref()),
                        (BlockedCode::AntiCheatEligibility, Some(g))
                    );
                }
                None => assert!(open, "{g}"),
            }
        }
        if let Some(fortnite) = anti_cheat_block("fortnite") {
            assert!(
                fortnite.message.contains("Easy Anti-Cheat and BattlEye"),
                "{}",
                fortnite.message
            );
        }
        // No anti-cheat, nothing to wait for.
        assert_eq!(anti_cheat_block("minecraft"), None);
        assert_eq!(anti_cheat_block("unknown"), None);
    }
}
