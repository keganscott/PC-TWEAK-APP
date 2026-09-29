//! Audits of the IPC surface, run as tests. They read the source files, so they
//! catch a command added in one place and forgotten in another.
//!
//! - no command accepts environment, license, tier or gate state;
//! - `set_environment` stays deleted;
//! - `commands.rs`, `main.rs` (`generate_handler!`), the app manifest in
//!   `build.rs` and the capability file all list the same commands.

const COMMANDS_RS: &str = include_str!("commands.rs");
const MAIN_RS: &str = include_str!("main.rs");
const BUILD_RS: &str = include_str!("../build.rs");
const CAPABILITY: &str = include_str!("../capabilities/default.json");
const IPC_TS: &str = include_str!("../../src/ipc.ts");

/// The `(name, parameter text)` of every `#[tauri::command]` function.
fn commands() -> Vec<(String, String)> {
    let lines: Vec<&str> = COMMANDS_RS.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "#[tauri::command]" {
            // Signature runs from the `fn` line to the line that opens the body.
            let mut sig = String::new();
            let mut j = i + 1;
            loop {
                sig.push_str(lines[j]);
                sig.push('\n');
                if lines[j].trim_end().ends_with('{') {
                    break;
                }
                j += 1;
            }
            let after_fn = sig.split("fn ").nth(1).expect("fn in signature");
            let name = after_fn.split('(').next().unwrap().trim().to_string();
            out.push((name, sig));
            i = j;
        }
        i += 1;
    }
    out
}

#[test]
fn the_scanner_finds_the_commands() {
    let names: Vec<String> = commands().into_iter().map(|(n, _)| n).collect();
    assert!(names.len() >= 9, "{names:?}");
    assert!(names.contains(&"apply_tweak".to_string()));
    assert!(names.contains(&"select_target_game".to_string()));
}

#[test]
fn no_command_takes_environment_license_tier_or_gate_state() {
    for (name, sig) in commands() {
        for banned in [
            "SystemEnv",
            "License",
            "Tier",
            "StubProbe",
            "EnvProbe",
            "restore_gate",
            "gate",
        ] {
            assert!(!sig.contains(banned), "{name} takes {banned}:\n{sig}");
        }
    }
}

#[test]
fn set_environment_stays_deleted() {
    let old_name = concat!("set_", "environment");
    for (label, src) in [
        ("commands.rs", COMMANDS_RS),
        ("main.rs", MAIN_RS),
        ("build.rs", BUILD_RS),
        ("capability", CAPABILITY),
    ] {
        assert!(!src.contains(old_name), "{old_name} found in {label}");
        assert!(!src.contains("set-environment"), "set-environment found in {label}");
    }
}

#[test]
fn every_command_is_registered_listed_and_permitted() {
    let cmds = commands();
    assert!(!cmds.is_empty());
    for (name, _) in &cmds {
        assert!(
            MAIN_RS.contains(&format!("commands::{name},")),
            "{name} is missing from generate_handler! in main.rs"
        );
        assert!(
            BUILD_RS.contains(&format!("\"{name}\"")),
            "{name} is missing from build.rs"
        );
        let permission = format!("allow-{}", name.replace('_', "-"));
        assert!(
            CAPABILITY.contains(&format!("\"{permission}\"")),
            "{permission} missing from capability"
        );
    }
    // And nothing extra is listed that has no function behind it.
    let capability_perms = CAPABILITY.matches("\"allow-").count();
    assert_eq!(
        capability_perms,
        cmds.len(),
        "capability lists a permission with no command"
    );
}

#[test]
fn the_capability_grants_no_plugin_permissions() {
    for banned in ["fs:", "shell:", "http:", "opener:", "dialog:", "process:"] {
        assert!(!CAPABILITY.contains(banned), "capability grants {banned}");
    }
    assert!(CAPABILITY.contains("\"core:default\""));
}

#[test]
fn every_command_has_a_typed_client_call() {
    for (name, _) in commands() {
        assert!(
            IPC_TS.contains(&format!("\"{name}\"")),
            "{name} has no wrapper in src/ipc.ts"
        );
    }
}

#[test]
fn the_client_never_sends_environment_license_or_gate_state() {
    // Code only: the header comment explains this rule in those very words.
    let code: String = IPC_TS
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("/*") || t.starts_with('*'))
        })
        .collect::<Vec<_>>()
        .join("\n");
    for banned in [
        "systemEnv",
        "SystemEnv",
        "license",
        "License",
        "tier:",
        "gateOpen",
        "restoreGate",
    ] {
        assert!(!code.contains(banned), "src/ipc.ts mentions {banned}");
    }
}
