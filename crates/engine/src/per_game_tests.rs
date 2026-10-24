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
use crate::system::FakeSystem;
use crate::testutil::{hklm_dword, user};
use crate::tweaks::fullscreen::{FullscreenOptimizations, LAYERS};
use crate::tweaks::gpu_pref::HighPerformanceGpu;
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
        steam_app: None,
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
    use crate::system::{SysItem, SysState};
    use crate::tweaks::qos::{policy, wanted, GameQos, ID};

    const FORTNITE: &str = r"D:\Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe";
    const ROBLOX: &str = r"C:\Users\KFS\AppData\Local\Roblox\Versions\version-1\RobloxPlayerBeta.exe";

    fn program(game_id: &str) -> &'static str {
        crate::games::facts(game_id).unwrap().programs[0]
    }

    fn on_pc(r: &Rig, game_id: &str) -> SysState {
        r.sys.get(&policy(program(game_id)))
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
        assert_eq!(on_pc(&r, "fortnite"), SysState::Absent);

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
            on_pc(&r, "fortnite"),
            SysState::QosPolicy {
                program: "FortniteClient-Win64-Shipping.exe".into(),
                dscp: 46
            },
            "matched by name, wherever the game is"
        );
        assert_eq!(on_pc(&r, "roblox"), SysState::Absent, "not found here");
        assert!(r.fake.snapshot().is_empty(), "no registry value is written");
        assert_eq!(state(&r, ID), TweakState::Applied);

        r.engine.revert(ID).unwrap();
        assert_eq!(on_pc(&r, "fortnite"), SysState::Absent);
        assert_eq!(state(&r, ID), TweakState::Default);
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
        assert_eq!(on_pc(&r, "roblox"), wanted(program("roblox")));
        assert_eq!(state(&r, ID), TweakState::Applied);

        r.engine.revert(ID).unwrap();
        assert_eq!(on_pc(&r, "fortnite"), SysState::Absent);
        assert_eq!(on_pc(&r, "roblox"), SysState::Absent);
    }

    #[test]
    fn a_policy_of_the_same_name_set_before_comes_back_on_undo() {
        let mut r = rig(
            vec![Box::new(GameQos::cleared_for_tests())],
            Some(vec![install("fortnite", Some(FORTNITE))]),
        );
        let theirs = SysState::QosPolicy {
            program: program("fortnite").into(),
            dscp: 40,
        };
        r.sys.set(&policy(program("fortnite")), theirs.clone());
        assert_eq!(state(&r, ID), TweakState::Default, "a different tag is not ours");

        r.engine.apply(ID).unwrap();
        assert_eq!(on_pc(&r, "fortnite"), wanted(program("fortnite")));
        r.engine.revert(ID).unwrap();
        assert_eq!(on_pc(&r, "fortnite"), theirs);
    }

    #[test]
    fn the_values_windows_keeps_for_a_policy_are_backed_up_before_the_change() {
        let mut r = rig(
            vec![Box::new(GameQos::cleared_for_tests())],
            Some(vec![install("fortnite", Some(FORTNITE))]),
        );
        let key = r"SOFTWARE\Policies\Microsoft\Windows\QoS\PeakTweaks FortniteClient-Win64-Shipping";
        r.fake
            .set_external(Hive::LocalMachine, key, "DSCP", RawValue::dword(40));
        r.sys.set(
            &policy(program("fortnite")),
            SysState::QosPolicy {
                program: program("fortnite").into(),
                dscp: 40,
            },
        );
        r.engine.apply(ID).unwrap();
        let change = r
            .engine
            .journal_view()
            .records
            .into_iter()
            .find_map(|rec| match rec {
                crate::journal::Record::Change(c) => Some(c),
                _ => None,
            })
            .unwrap();
        let names: Vec<&str> = change.reg_backups.iter().map(|b| b.value_name.as_str()).collect();
        assert_eq!(
            names,
            ["Version", "NetProfile", "Precedence", "AppName", "Protocol", "DSCP"]
        );
        let dscp = change.reg_backups.iter().find(|b| b.value_name == "DSCP").unwrap();
        assert_eq!(dscp.previous, Some(RawValue::dword(40)));
        assert!(dscp.display_path.ends_with(key), "{}", dscp.display_path);
    }

    #[test]
    fn a_policy_already_there_with_the_tag_reads_as_already_set() {
        let r = rig(
            vec![Box::new(GameQos::cleared_for_tests())],
            Some(vec![install("fortnite", Some(FORTNITE))]),
        );
        r.sys.set(&policy(program("fortnite")), wanted(program("fortnite")));
        assert_eq!(state(&r, ID), TweakState::Foreign);
    }

    #[test]
    fn it_may_change_only_its_own_policies() {
        let t = GameQos::new();
        assert!(t.touches().is_empty());
        assert!(t.effect_targets().is_empty());
        let names: Vec<String> = t
            .system_targets()
            .into_iter()
            .map(|i| match i {
                SysItem::QosPolicy { name } => name,
                other => panic!("{other:?}"),
            })
            .collect();
        let expected: Vec<String> = crate::env::KNOWN_GAMES
            .iter()
            .filter_map(|g| crate::games::facts(g.id))
            .flat_map(|f| {
                f.programs
                    .iter()
                    .map(|p| format!("PeakTweaks {}", p.trim_end_matches(".exe")))
            })
            .collect();
        assert_eq!(names, expected);
        assert!(names.iter().all(|n| !n.contains('*')));
    }
}

fn gpu_value(r: &Rig, exe: &str) -> Option<String> {
    r.fake
        .read_value_for_test(Hive::CurrentUser, crate::gpu_choice::KEY, exe)
        .and_then(|v| v.as_sz())
}

fn gpu_rig(before: Option<&str>) -> (Rig, &'static str) {
    const EXE: &str = r"D:\Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe";
    let r = rig(
        vec![Box::new(
            HighPerformanceGpu::for_game("fortnite").unwrap().cleared_for_tests(),
        )],
        Some(vec![install("fortnite", Some(EXE))]),
    );
    if let Some(data) = before {
        r.fake
            .set_external(Hive::CurrentUser, crate::gpu_choice::KEY, EXE, RawValue::sz(data));
    }
    (r, EXE)
}

#[test]
fn high_performance_chip_is_set_for_the_found_program_and_undo_removes_it() {
    let (mut r, exe) = gpu_rig(None);
    assert_eq!(state(&r, "gpu.choice.fortnite"), TweakState::Default);
    r.engine.apply("gpu.choice.fortnite").unwrap();
    assert_eq!(gpu_value(&r, exe).as_deref(), Some("GpuPreference=2;"));
    assert_eq!(state(&r, "gpu.choice.fortnite"), TweakState::Applied);
    r.engine.revert("gpu.choice.fortnite").unwrap();
    assert_eq!(gpu_value(&r, exe), None);
}

#[test]
fn high_performance_chip_keeps_other_entries_windows_keeps_for_the_game() {
    let (mut r, exe) = gpu_rig(Some("AutoHDREnable=1;GpuPreference=1;"));
    r.engine.apply("gpu.choice.fortnite").unwrap();
    assert_eq!(gpu_value(&r, exe).as_deref(), Some("AutoHDREnable=1;GpuPreference=2;"));
    r.engine.revert("gpu.choice.fortnite").unwrap();
    assert_eq!(gpu_value(&r, exe).as_deref(), Some("AutoHDREnable=1;GpuPreference=1;"));
}

#[test]
fn high_performance_chip_already_chosen_reads_as_already_set_and_an_odd_choice_is_left_alone() {
    let (r, _) = gpu_rig(Some("GpuPreference=2;"));
    assert_eq!(state(&r, "gpu.choice.fortnite"), TweakState::Foreign);

    let (mut r, exe) = gpu_rig(Some("GpuPreference=7;"));
    assert!(matches!(state(&r, "gpu.choice.fortnite"), TweakState::Unknown { .. }));
    assert!(r.engine.apply("gpu.choice.fortnite").is_err());
    assert_eq!(gpu_value(&r, exe).as_deref(), Some("GpuPreference=7;"));
}

#[test]
fn high_performance_chip_is_not_offered_with_one_chip_or_before_the_anti_cheat_is_cleared() {
    use crate::hardware::{GpuAdapter, HardwareReport};
    let with_chips = |n: usize| HardwareReport {
        os: Probe::unknown("test"),
        cpu: Probe::unknown("test"),
        memory: Probe::unknown("test"),
        gpus: Probe::yes(
            (0..n)
                .map(|i| GpuAdapter {
                    name: format!("chip {i}"),
                    vendor_id: 0x10DE,
                    dedicated_vram_bytes: 0,
                    shared_memory_bytes: 0,
                    is_software: false,
                })
                .collect(),
        ),
        gpu_drivers: Probe::unknown("test"),
        boot_disk: Probe::unknown("test"),
        display: Probe::unknown("test"),
        is_laptop: Probe::unknown("test"),
        rig_class: Probe::unknown("test"),
    };
    let mut env = SystemEnv {
        game_installs: Some(vec![install("fortnite", Some(r"D:\x.exe"))]),
        ..SystemEnv::default()
    };
    let cleared = HighPerformanceGpu::for_game("fortnite").unwrap().cleared_for_tests();
    env.hardware = Some(with_chips(1));
    assert_eq!(blocked_code(&cleared, &env), Some(BlockedCode::HardwareUnsupported));
    env.hardware = Some(with_chips(2));
    assert_eq!(blocked_code(&cleared, &env), None);
    let shipped = HighPerformanceGpu::for_game("fortnite").unwrap();
    if crate::games::anti_cheat_block("fortnite").is_some() {
        assert_eq!(blocked_code(&shipped, &env), Some(BlockedCode::AntiCheatEligibility));
    }
}

/// Evidence on a real Windows (CI step "Per-game tools on this runner"): the
/// csrss priority, a game's priority, fullscreen optimizations and graphics chip applied and
/// undone through the engine on the real registry, every value printed before,
/// during and after, and the fullscreen `.reg` backup imported with Windows'
/// own `reg.exe`. The game is made up for the test (an install path that does
/// not exist), so no real game's setting is touched. Changes nothing unless
/// `PEAKTWEAKS_REAL_SYSTEM_CHANGES=1`, which only that CI step sets.
#[cfg(windows)]
#[test]
fn per_game_tools_apply_and_undo_on_this_pc() {
    use std::process::Command;

    use crate::context::{UserContext, UserResolution};
    use crate::identity;
    use crate::registry::windows::WinRegistry;

    if std::env::var("PEAKTWEAKS_REAL_SYSTEM_CHANGES").as_deref() != Ok("1") {
        println!(
            "SKIPPED: set PEAKTWEAKS_REAL_SYSTEM_CHANGES=1 to set and undo csrss and game priority and a \
             fullscreen setting on this PC for real"
        );
        return;
    }
    let reg = Arc::new(WinRegistry::new());
    let exe = format!(
        r"C:\PeakTweaksTest-{}\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe",
        std::process::id()
    );

    /// Removes the made-up game's setting however the test ends.
    struct Cleanup(Arc<WinRegistry>, String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.delete_value(Hive::CurrentUser, LAYERS, &self.1);
            let _ = self.0.delete_value(Hive::CurrentUser, crate::gpu_choice::KEY, &self.1);
        }
    }
    let _cleanup = Cleanup(reg.clone(), exe.clone());
    // A layer the user set before: it must survive apply, reg.exe and Undo.
    reg.write_value(Hive::CurrentUser, LAYERS, &exe, &RawValue::sz("~ RUNASADMIN"))
        .unwrap();

    let csrss = format!(r"{IFEO}\csrss.exe\PerfOptions");
    let apex = format!(r"{IFEO}\r5apex.exe\PerfOptions");
    let apex12 = format!(r"{IFEO}\r5apex_dx12.exe\PerfOptions");
    let values: Vec<(Hive, String, &str)> = vec![
        (Hive::LocalMachine, csrss.clone(), "CpuPriorityClass"),
        (Hive::LocalMachine, csrss.clone(), "IoPriority"),
        (Hive::LocalMachine, apex.clone(), "CpuPriorityClass"),
        (Hive::LocalMachine, apex12.clone(), "CpuPriorityClass"),
        (Hive::CurrentUser, LAYERS.to_owned(), exe.as_str()),
        (Hive::CurrentUser, crate::gpu_choice::KEY.to_owned(), exe.as_str()),
    ];
    let read = |label: &str| -> Vec<Option<RawValue>> {
        let now: Vec<Option<RawValue>> = values
            .iter()
            .map(|(hive, key, name)| reg.read_value(*hive, key, name).unwrap())
            .collect();
        for ((hive, key, name), v) in values.iter().zip(&now) {
            let shown = match v {
                None => "absent".to_owned(),
                Some(v) => v
                    .as_dword()
                    .map(|d| d.to_string())
                    .or_else(|| v.as_sz().map(|s| format!("{s:?}")))
                    .unwrap_or_else(|| format!("type {}", v.vtype)),
            };
            println!("{label}: {hive:?}\\{key}\\{name} = {shown}");
        }
        now
    };
    let keys = [csrss.as_str(), apex.as_str(), apex12.as_str()];
    let keys_before: Vec<bool> = keys
        .iter()
        .map(|k| reg.key_exists(Hive::LocalMachine, k).unwrap())
        .collect();
    let before = read("before");

    let user = UserContext {
        sid: identity::current_process_sid().unwrap(),
        resolution: UserResolution::OwnToken,
        is_self: true,
    };
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::new(
        ContextResolver::new(user, identity::is_elevated(), reg.clone()),
        Journal::open(&TrustedDir::insecure_for_tests(dir.path())).unwrap(),
        vec![
            Box::new(CsrssPriority),
            Box::new(IfeoPriority::for_game("apex").unwrap().cleared_for_tests()),
            Box::new(
                FullscreenOptimizations::for_game("fortnite")
                    .unwrap()
                    .cleared_for_tests(),
            ),
            Box::new(HighPerformanceGpu::for_game("fortnite").unwrap().cleared_for_tests()),
        ],
        Box::new(GamesProbe(Arc::new(Mutex::new(Some(vec![
            install("fortnite", Some(&exe)),
            install("apex", None),
        ]))))),
        License::dev(Tier::Ultimate),
    );
    engine.rescan();
    let ids = [
        "scheduling.csrss",
        "priority.ifeo.apex",
        "gaming.fullscreen.fortnite",
        "gpu.choice.fortnite",
    ];
    let states = |engine: &Engine| -> Vec<TweakState> {
        let list = engine.list().unwrap();
        ids.iter()
            .map(|id| list.iter().find(|v| v.metadata.id == *id).unwrap().state.clone())
            .collect()
    };
    println!("states before: {:?}", states(&engine));

    let mut fullscreen_backups = Vec::new();
    for id in ids {
        let entries = engine.apply(id).unwrap();
        println!(
            "applied {id}: {} writes, backups {:?}",
            entries.len(),
            entries.iter().map(|e| &e.backup_file).collect::<Vec<_>>()
        );
        if id.starts_with("gaming.") {
            fullscreen_backups = entries.iter().map(|e| e.backup_file.clone()).collect();
        }
    }
    let during = read("applied");
    assert_eq!(
        during[0].as_ref().and_then(RawValue::as_dword),
        Some(4),
        "csrss Realtime"
    );
    assert_eq!(
        during[1].as_ref().and_then(RawValue::as_dword),
        Some(3),
        "csrss I/O High"
    );
    assert_eq!(during[2].as_ref().and_then(RawValue::as_dword), Some(3), "Apex High");
    assert_eq!(
        during[3].as_ref().and_then(RawValue::as_dword),
        Some(3),
        "Apex DirectX 12 High"
    );
    assert_eq!(
        during[4].as_ref().and_then(RawValue::as_sz).as_deref(),
        Some("~ RUNASADMIN DISABLEDXMAXIMIZEDWINDOWEDMODE")
    );
    assert_eq!(
        during[5].as_ref().and_then(RawValue::as_sz).as_deref(),
        Some("GpuPreference=2;"),
        "Fortnite on the high-performance chip"
    );
    assert_eq!(states(&engine), vec![TweakState::Applied; 4]);

    // The offline path: Windows' own reg.exe puts the earlier layers back
    // from the backup, a value named by a full program path included.
    assert_eq!(fullscreen_backups.len(), 1);
    let file = dir.path().join(&fullscreen_backups[0]);
    let out = Command::new("reg.exe").arg("import").arg(&file).output().unwrap();
    assert!(
        out.status.success(),
        "reg import {} failed: {}{}",
        file.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let imported = read("after reg.exe import of the fullscreen backup")[4].clone();
    assert_eq!(
        imported.as_ref().and_then(RawValue::as_sz).as_deref(),
        Some("~ RUNASADMIN")
    );
    assert_eq!(states(&engine)[2], TweakState::Drifted);

    for id in ids {
        engine.revert(id).unwrap();
    }
    let after = read("undone");
    assert_eq!(after, before, "Undo puts every value back exactly");
    let keys_after: Vec<bool> = keys
        .iter()
        .map(|k| reg.key_exists(Hive::LocalMachine, k).unwrap())
        .collect();
    assert_eq!(keys_after, keys_before, "keys Undo created are gone again");
    println!(
        "per-game tools on this PC: 4 applied, backup imported with reg.exe, 4 undone; keys there before: {keys_before:?}"
    );
}

#[test]
fn play_links_come_from_the_engine_s_own_scan_and_only_for_steam_installs() {
    let steam = GameInstall {
        steam_app: Some(730),
        ..install("cs2", None)
    };
    let mut r = rig(Vec::new(), Some(vec![steam, install("fortnite", None)]));
    assert_eq!(r.engine.launch_link("cs2").unwrap(), "steam://rungameid/730");
    assert!(r.engine.launch_link("fortnite").is_err(), "not found through Steam");
    assert!(matches!(
        r.engine.launch_link("steam://rungameid/1"),
        Err(EngineError::UnknownGame { .. })
    ));
    // A game that is no longer found loses its link at the next scan.
    *r.installs.lock().unwrap() = Some(Vec::new());
    r.engine.rescan_fresh();
    assert!(r.engine.launch_link("cs2").is_err());
}
