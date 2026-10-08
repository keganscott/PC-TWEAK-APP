//! The per-game tools (H31, H19, H26) and the csrss one (H3) through the engine,
//! against the in-memory registry: the anti-cheat and install gates, the
//! values written, and Undo after the game moved.

use std::sync::{Arc, Mutex};

use crate::context::ContextResolver;
use crate::engine::Engine;
use crate::env::{EnvProbe, License};
use crate::error::EngineError;
use crate::game_installs::GameInstall;
use crate::journal::Journal;
use crate::probe::Probe;
use crate::registry::fake::FakeRegistry;
use crate::registry::{Hive, RegistryBackend};
use crate::secure_dir::TrustedDir;
use crate::system::{FakeSystem, SideEffect};
use crate::testutil::{hklm_dword, user};
use crate::tweaks::fullscreen::{FullscreenOptimizations, LAYERS};
use crate::tweaks::ifeo_priority::{CsrssPriority, IfeoPriority};
use crate::types::{BlockedCode, PredicateOutcome, RawValue, SystemEnv, Tier, Tweak, TweakState};

const IFEO: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options";

/// Reports the installs it holds, which a test can change between calls.
struct GamesProbe(Arc<Mutex<Option<Vec<GameInstall>>>>);

impl EnvProbe for GamesProbe {
    fn probe(&self, elevated: bool) -> SystemEnv {
        SystemEnv {
            elevated,
            restore_gate_open: true,
            game_installs: self.0.lock().unwrap().clone(),
            ..SystemEnv::default()
        }
    }

    fn restore_gate_open(&self) -> bool {
        true
    }
}

fn install(game_id: &str, exe: Option<&str>) -> GameInstall {
    GameInstall {
        game_id: game_id.into(),
        name: crate::games::name(game_id).into(),
        path: r"D:\Games".into(),
        drive: "D:".into(),
        disk: Probe::unknown("not looked up in tests"),
        exe: exe.map(str::to_owned),
    }
}

struct Rig {
    fake: Arc<FakeRegistry>,
    sys: Arc<FakeSystem>,
    installs: Arc<Mutex<Option<Vec<GameInstall>>>>,
    engine: Engine,
    _dir: tempfile::TempDir,
}

fn rig(tweaks: Vec<Box<dyn Tweak>>, installs: Option<Vec<GameInstall>>) -> Rig {
    let fake = Arc::new(FakeRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let installs = Arc::new(Mutex::new(installs));
    // `user(true)`: the interactive user is us, so HKCU is their hive.
    let sys = Arc::new(FakeSystem::new());
    let resolver = ContextResolver::new(user(true), true, fake.clone()).with_system(sys.clone());
    let journal = Journal::open(&TrustedDir::insecure_for_tests(dir.path())).unwrap();
    let mut engine = Engine::new(
        resolver,
        journal,
        tweaks,
        Box::new(GamesProbe(installs.clone())),
        License::dev(Tier::Ultimate),
    );
    // As the app's first audit does.
    engine.rescan();
    Rig {
        fake,
        sys,
        installs,
        engine,
        _dir: dir,
    }
}

fn state(r: &Rig, id: &str) -> TweakState {
    r.engine
        .list()
        .unwrap()
        .into_iter()
        .find(|v| v.metadata.id == id)
        .unwrap()
        .state
}

fn layers(r: &Rig, exe: &str) -> Option<String> {
    r.fake
        .read_value_for_test(Hive::CurrentUser, LAYERS, exe)
        .and_then(|v| v.as_sz())
}

fn blocked_code(t: &dyn Tweak, env: &SystemEnv) -> Option<BlockedCode> {
    match t.evaluate_predicate(env) {
        PredicateOutcome::Block(r) => Some(r.code),
        PredicateOutcome::Allow => None,
    }
}

#[test]
fn the_shipped_per_game_tools_are_blocked_by_anti_cheat_and_offered_only_for_offered_games() {
    let cat = crate::tweaks::catalogue();
    let per_game: Vec<&str> = cat
        .iter()
        .map(|t| t.id())
        .filter(|id| id.starts_with("priority.ifeo.") || id.starts_with("gaming.fullscreen."))
        .collect();
    let mut expected = Vec::new();
    for g in crate::env::KNOWN_GAMES {
        if IfeoPriority::for_game(g.id).is_some() {
            expected.push(format!("priority.ifeo.{}", g.id));
        }
    }
    for g in crate::env::KNOWN_GAMES {
        if FullscreenOptimizations::for_game(g.id).is_some() {
            expected.push(format!("gaming.fullscreen.{}", g.id));
        }
    }
    assert_eq!(per_game, expected);
    // The shipped default set (NOTES N75): Fortnite and Roblox priority,
    // Fortnite fullscreen. Minecraft runs in Java, Roblox moves every update.
    if !crate::env::OFFER_VALORANT_CS2_APEX {
        assert_eq!(
            per_game,
            [
                "priority.ifeo.fortnite",
                "priority.ifeo.roblox",
                "gaming.fullscreen.fortnite"
            ]
        );
    }

    // A game whose anti-cheat is not cleared: blocked, even when it is
    // installed. Otherwise only a game that was not found is.
    let env = SystemEnv {
        game_installs: Some(vec![install(
            "fortnite",
            Some(r"D:\Games\FortniteClient-Win64-Shipping.exe"),
        )]),
        ..SystemEnv::default()
    };
    for t in cat.iter().filter(|t| per_game.contains(&t.id())) {
        let game = t.id().rsplit('.').next().unwrap();
        let expected = if crate::games::anti_cheat_block(game).is_some() {
            Some(BlockedCode::AntiCheatEligibility)
        } else if game == "fortnite" {
            None
        } else {
            Some(BlockedCode::GameNotInstalled)
        };
        assert_eq!(blocked_code(t.as_ref(), &env), expected, "{}", t.id());
    }
}

#[test]
fn game_priority_is_high_for_every_program_of_the_game_and_needs_the_game_found() {
    let apex = IfeoPriority::for_game("apex").unwrap().cleared_for_tests();
    let not_found = SystemEnv {
        game_installs: Some(vec![install("cs2", None)]),
        ..SystemEnv::default()
    };
    assert_eq!(blocked_code(&apex, &not_found), Some(BlockedCode::GameNotInstalled));
    // Not looked for: the setting is keyed by file name, so that alone blocks nothing.
    assert_eq!(blocked_code(&apex, &SystemEnv::default()), None);
    assert!(IfeoPriority::for_game("minecraft").is_none());

    let mut r = rig(vec![Box::new(apex)], Some(vec![install("apex", None)]));
    assert_eq!(state(&r, "priority.ifeo.apex"), TweakState::Default);
    r.engine.apply("priority.ifeo.apex").unwrap();
    for exe in ["r5apex.exe", "r5apex_dx12.exe"] {
        let key = format!(r"{IFEO}\{exe}\PerfOptions");
        assert_eq!(
            hklm_dword(&r.fake, &key, "CpuPriorityClass"),
            Some(3),
            "High, never Realtime: {exe}"
        );
    }
    assert_eq!(state(&r, "priority.ifeo.apex"), TweakState::Applied);

    // Someone sets Realtime on one of them: no longer what this tool sets.
    let key = format!(r"{IFEO}\r5apex.exe\PerfOptions");
    r.fake
        .write_value(Hive::LocalMachine, &key, "CpuPriorityClass", &RawValue::dword(4))
        .unwrap();
    assert_eq!(state(&r, "priority.ifeo.apex"), TweakState::Drifted);

    r.engine.revert("priority.ifeo.apex").unwrap();
    assert!(r.fake.snapshot().is_empty(), "{:?}", r.fake.snapshot());
}

#[test]
fn csrss_priority_writes_what_hone_does_and_undoes_cleanly() {
    let mut r = rig(vec![Box::new(CsrssPriority)], None);
    let meta = CsrssPriority.metadata();
    assert!(meta.requires_reboot && meta.safety == crate::types::SafetyTier::Extreme);
    assert_eq!(state(&r, "scheduling.csrss"), TweakState::Default);

    r.engine.apply("scheduling.csrss").unwrap();
    let key = format!(r"{IFEO}\csrss.exe\PerfOptions");
    assert_eq!(hklm_dword(&r.fake, &key, "CpuPriorityClass"), Some(4));
    assert_eq!(hklm_dword(&r.fake, &key, "IoPriority"), Some(3));
    assert_eq!(state(&r, "scheduling.csrss"), TweakState::Applied);

    r.engine.revert("scheduling.csrss").unwrap();
    assert!(r.fake.snapshot().is_empty());
    assert!(
        !r.fake
            .key_paths()
            .iter()
            .any(|(_, p)| p.to_ascii_lowercase().contains("csrss")),
        "the keys we created are gone"
    );
}

#[test]
fn fullscreen_is_set_for_the_found_program_keeps_other_layers_and_undo_follows_the_journal() {
    let old = r"D:\Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe";
    let new = r"E:\Epic\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe";
    let id = "gaming.fullscreen.fortnite";
    let tool = || -> Vec<Box<dyn Tweak>> {
        vec![Box::new(
            FullscreenOptimizations::for_game("fortnite")
                .unwrap()
                .cleared_for_tests(),
        )]
    };

    // Not looked for yet, then looked for and not found, then found without
    // its program file.
    let r = rig(tool(), None);
    assert!(matches!(state(&r, id), TweakState::Unknown { .. }));
    let r = rig(tool(), Some(vec![]));
    assert!(matches!(
        state(&r, id),
        TweakState::Blocked { reason } if reason.code == BlockedCode::GameNotInstalled
    ));
    let r = rig(tool(), Some(vec![install("fortnite", None)]));
    assert!(matches!(
        state(&r, id),
        TweakState::Blocked { reason } if reason.code == BlockedCode::GameNotInstalled
    ));

    let mut r = rig(tool(), Some(vec![install("fortnite", Some(old))]));
    r.fake
        .write_value(Hive::CurrentUser, LAYERS, old, &RawValue::sz("~ RUNASADMIN"))
        .unwrap();
    assert_eq!(state(&r, id), TweakState::Default);
    r.engine.apply(id).unwrap();
    assert_eq!(
        layers(&r, old).as_deref(),
        Some("~ RUNASADMIN DISABLEDXMAXIMIZEDWINDOWEDMODE")
    );
    assert_eq!(state(&r, id), TweakState::Applied);

    // The game moved: the new path has no setting, the old one is still ours
    // to undo.
    *r.installs.lock().unwrap() = Some(vec![install("fortnite", Some(new))]);
    r.engine.rescan();
    assert_eq!(state(&r, id), TweakState::Drifted);
    r.engine.revert(id).unwrap();
    assert_eq!(layers(&r, old).as_deref(), Some("~ RUNASADMIN"), "restored exactly");
    assert_eq!(layers(&r, new), None);

    // A fresh value is just ours.
    r.engine.apply(id).unwrap();
    assert_eq!(layers(&r, new).as_deref(), Some("~ DISABLEDXMAXIMIZEDWINDOWEDMODE"));
    r.engine.revert(id).unwrap();
    assert_eq!(layers(&r, new), None);
}

#[test]
fn fullscreen_leaves_settings_it_does_not_recognise_and_writes_only_a_declared_program() {
    let exe = r"D:\Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe";
    let id = "gaming.fullscreen.fortnite";
    let tool = || -> Vec<Box<dyn Tweak>> {
        vec![Box::new(
            FullscreenOptimizations::for_game("fortnite")
                .unwrap()
                .cleared_for_tests(),
        )]
    };
    let mut r = rig(tool(), Some(vec![install("fortnite", Some(exe))]));
    r.fake
        .write_value(Hive::CurrentUser, LAYERS, exe, &RawValue::sz("$ WIN7RTM"))
        .unwrap();
    assert!(matches!(state(&r, id), TweakState::Unknown { .. }));
    assert!(r.engine.apply(id).is_err());
    assert_eq!(layers(&r, exe).as_deref(), Some("$ WIN7RTM"), "left alone");

    // A program file the tool did not declare is refused by the allowlist.
    let mut r = rig(tool(), Some(vec![install("fortnite", Some(r"D:\Games\other.exe"))]));
    let err = r.engine.apply(id).unwrap_err();
    assert!(matches!(err, EngineError::ContextViolation { .. }), "{err:?}");
    assert!(r.fake.snapshot().is_empty());
}

/// H26: game traffic priority (`tweaks/qos.rs`).
mod game_qos {
    use super::*;
    use crate::tweaks::qos::{policy_key, GameQos, ID, NO_NLA, TCPIP_QOS};

    const FORTNITE: &str = r"D:\Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe";
    const ROBLOX: &str = r"C:\Users\KFS\AppData\Local\Roblox\Versions\version-1\RobloxPlayerBeta.exe";

    fn program(game_id: &str) -> &'static str {
        crate::games::facts(game_id).unwrap().programs[0]
    }

    fn policy(r: &Rig, game_id: &str, name: &str) -> Option<String> {
        r.fake
            .read_value_for_test(Hive::LocalMachine, &policy_key(program(game_id)), name)
            .and_then(|v| v.as_sz())
    }

    fn nla(r: &Rig) -> Option<String> {
        r.fake
            .read_value_for_test(Hive::LocalMachine, TCPIP_QOS, NO_NLA)
            .and_then(|v| v.as_sz())
    }

    fn blocked(r: &Rig) -> (BlockedCode, String) {
        match state(r, ID) {
            TweakState::Blocked { reason } => (reason.code, reason.message),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn it_waits_while_every_game_found_waits_on_its_anti_cheat() {
        let shipped = || -> Vec<Box<dyn Tweak>> { vec![Box::new(GameQos::new())] };
        let mut r = rig(
            shipped(),
            Some(vec![
                install("fortnite", Some(FORTNITE)),
                install("roblox", Some(ROBLOX)),
            ]),
        );
        assert_eq!(
            blocked(&r),
            (
                BlockedCode::AntiCheatEligibility,
                "Not available yet: Fortnite and Roblox have not been tried with their anti-cheat.".into()
            )
        );
        assert!(matches!(r.engine.apply(ID), Err(EngineError::Blocked { .. })));
        assert!(r.fake.snapshot().is_empty());

        let r = rig(shipped(), Some(vec![install("roblox", Some(ROBLOX))]));
        let (code, message) = blocked(&r);
        assert_eq!(code, BlockedCode::AntiCheatEligibility);
        assert!(message.starts_with("Not available for Roblox yet"), "{message}");

        // Minecraft runs inside Java: no program of its own to tag.
        for installs in [Some(vec![install("minecraft", None)]), Some(Vec::new()), None] {
            let r = rig(shipped(), installs);
            assert_eq!(blocked(&r).0, BlockedCode::GameNotInstalled);
        }
    }

    #[test]
    fn it_adds_a_policy_for_each_game_found_and_undo_removes_them() {
        let mut r = rig(
            vec![Box::new(GameQos::cleared_for_tests())],
            Some(vec![install("fortnite", Some(FORTNITE))]),
        );
        assert_eq!(state(&r, ID), TweakState::Default);

        r.engine.apply(ID).unwrap();
        assert_eq!(
            policy(&r, "fortnite", "Application Name").as_deref(),
            Some("FortniteClient-Win64-Shipping.exe"),
            "matched by name, wherever the game is"
        );
        assert_eq!(policy(&r, "fortnite", "DSCP Value").as_deref(), Some("46"));
        assert_eq!(policy(&r, "fortnite", "Throttle Rate").as_deref(), Some("-1"));
        assert_eq!(policy(&r, "roblox", "DSCP Value"), None, "not found here");
        assert_eq!(nla(&r).as_deref(), Some("1"));
        assert_eq!(r.sys.effects(), [SideEffect::RefreshPolicy]);
        assert_eq!(state(&r, ID), TweakState::Applied);

        r.engine.revert(ID).unwrap();
        assert!(r.fake.snapshot().is_empty(), "{:?}", r.fake.snapshot());
        assert!(
            !r.fake
                .key_exists_for_test(Hive::LocalMachine, r"SOFTWARE\Policies\Microsoft\Windows\QoS"),
            "the keys it made are gone too"
        );
        assert_eq!(r.sys.effects(), [SideEffect::RefreshPolicy, SideEffect::RefreshPolicy]);
    }

    #[test]
    fn a_game_found_later_is_covered_by_applying_again() {
        let mut r = rig(
            vec![Box::new(GameQos::cleared_for_tests())],
            Some(vec![install("fortnite", Some(FORTNITE))]),
        );
        r.engine.apply(ID).unwrap();
        *r.installs.lock().unwrap() = Some(vec![
            install("fortnite", Some(FORTNITE)),
            install("roblox", Some(ROBLOX)),
        ]);
        r.engine.rescan();
        assert_eq!(state(&r, ID), TweakState::Drifted);
        r.engine.apply(ID).unwrap();
        assert_eq!(policy(&r, "roblox", "DSCP Value").as_deref(), Some("46"));
        assert_eq!(state(&r, ID), TweakState::Applied);

        r.engine.revert(ID).unwrap();
        assert!(r.fake.snapshot().is_empty());
    }

    #[test]
    fn values_set_before_come_back_on_undo() {
        let mut r = rig(
            vec![Box::new(GameQos::cleared_for_tests())],
            Some(vec![install("fortnite", Some(FORTNITE))]),
        );
        let key = policy_key(program("fortnite"));
        r.fake
            .write_value(Hive::LocalMachine, TCPIP_QOS, NO_NLA, &RawValue::sz("1"))
            .unwrap();
        r.fake
            .write_value(Hive::LocalMachine, &key, "DSCP Value", &RawValue::sz("40"))
            .unwrap();
        let before = r.fake.snapshot();
        assert_eq!(state(&r, ID), TweakState::Default, "a different tag is not ours");

        r.engine.apply(ID).unwrap();
        assert_eq!(policy(&r, "fortnite", "DSCP Value").as_deref(), Some("46"));
        r.engine.revert(ID).unwrap();
        assert_eq!(r.fake.snapshot(), before);
        assert_eq!(nla(&r).as_deref(), Some("1"), "set before, so kept");
    }

    #[test]
    fn it_may_write_only_its_own_policies_and_the_nla_switch() {
        let t = GameQos::new();
        let keys: Vec<String> = t.touches().into_iter().map(|t| t.key).collect();
        let mut expected: Vec<String> = crate::env::KNOWN_GAMES
            .iter()
            .filter_map(|g| crate::games::facts(g.id))
            .flat_map(|f| f.programs.iter().map(|p| policy_key(p)))
            .collect();
        expected.push(TCPIP_QOS.into());
        assert_eq!(keys, expected);
        assert!(keys.iter().all(|k| !k.contains('*')));
        assert_eq!(t.effect_targets(), [SideEffect::RefreshPolicy]);
    }
}
