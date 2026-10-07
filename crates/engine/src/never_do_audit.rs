//! The plan's never-do list (section 12), as tests rather than intentions.
//!
//! Registry: no tweak, shipped or internal, may declare a target under the keys
//! behind Memory Integrity and VBS, Secure Boot, code-integrity policy (which
//! holds the vulnerable-driver blocklist) or the TPM service. Every write is
//! limited to `touches()` (R1), so checking the declarations covers the writes.
//!
//! Source: no code for game memory access or injection, CPU affinity, Roblox
//! fast flags, loading drivers, boot configuration or clearing the TPM.

use std::path::PathBuf;

use crate::network_audit::{hits, walk, workspace_root};
use crate::registry::pattern_is_ancestor_or_equal;
use crate::types::{RegRoot, Tweak};

/// `HKLM` keys no tweak may touch, nor anything below them.
const FORBIDDEN_HKLM: &[(&str, &str)] = &[
    (
        r"SYSTEM\CurrentControlSet\Control\DeviceGuard",
        "Memory Integrity / VBS",
    ),
    (r"SYSTEM\CurrentControlSet\Control\SecureBoot", "Secure Boot"),
    (
        r"SYSTEM\CurrentControlSet\Control\CI",
        "code integrity and the vulnerable-driver blocklist",
    ),
    (r"SYSTEM\CurrentControlSet\Services\TPM", "the TPM"),
];

/// Every declared target that falls under a forbidden key.
fn forbidden_targets(tweaks: &[Box<dyn Tweak>]) -> Vec<String> {
    let mut found = Vec::new();
    for t in tweaks {
        for target in t.touches() {
            if target.root != RegRoot::LocalMachine {
                continue;
            }
            for (key, what) in FORBIDDEN_HKLM {
                if pattern_is_ancestor_or_equal(key, &target.key) {
                    found.push(format!("{}: {} ({what})", t.id(), target.key));
                }
            }
        }
    }
    found
}

#[test]
fn no_tweak_targets_a_security_feature() {
    let all: Vec<Box<dyn Tweak>> = crate::tweaks::catalogue()
        .into_iter()
        .chain(crate::tweaks::internal())
        .collect();
    assert!(all.len() >= 3, "expected the catalogue and internal tweaks");
    let found = forbidden_targets(&all);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn the_registry_check_catches_a_planted_violation() {
    let planted: Vec<Box<dyn Tweak>> = vec![
        Box::new(crate::testutil::TestTweak::new(
            "planted.hvci",
            r"system\currentcontrolset\control\deviceguard\Scenarios\HypervisorEnforcedCodeIntegrity",
            &[("Enabled", 0)],
        )),
        Box::new(crate::testutil::TestTweak::new(
            "planted.blocklist",
            r"SYSTEM\CurrentControlSet\Control\CI\Config",
            &[("VulnerableDriverBlocklistEnable", 0)],
        )),
        Box::new(crate::testutil::TestTweak::new(
            "fine",
            r"SYSTEM\CurrentControlSet\Control\PriorityControl",
            &[("Win32PrioritySeparation", 2)],
        )),
    ];
    let found = forbidden_targets(&planted);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].contains("Memory Integrity") && found[1].contains("vulnerable-driver"));
}

/// A `*` segment could stand for a forbidden key, so it counts as one.
#[test]
fn a_wildcard_target_that_could_reach_a_security_key_is_caught() {
    let mut t = crate::testutil::TestTweak::new("planted.wild", r"SYSTEM\CurrentControlSet\Control\X", &[("V", 0)]);
    t.allow_key = r"SYSTEM\CurrentControlSet\Control\*".into();
    let planted: Vec<Box<dyn Tweak>> = vec![Box::new(t)];
    assert_eq!(
        forbidden_targets(&planted).len(),
        3,
        "DeviceGuard, SecureBoot and CI sit under Control"
    );
}

/// Win32 calls and strings that only the forbidden features need.
const SOURCE_DENY: &[&str] = &[
    // Game memory access and injection.
    "WriteProcessMemory",
    "ReadProcessMemory",
    "VirtualAllocEx",
    "CreateRemoteThread",
    "NtCreateThreadEx",
    "SetWindowsHookEx",
    // CPU affinity (games or anything else).
    "SetProcessAffinityMask",
    "SetThreadAffinityMask",
    "SetProcessDefaultCpuSets",
    // Roblox fast flags.
    "ClientAppSettings",
    "FFlag",
    // Drivers and firmware-adjacent state.
    "NtLoadDriver",
    "SERVICE_KERNEL_DRIVER",
    "bcdedit",
    "Clear-Tpm",
    "Disable-Tpm",
    "Initialize-Tpm",
];

#[test]
fn no_source_implements_a_never_do() {
    let root = workspace_root();
    let this_file = root.join("crates/engine/src/never_do_audit.rs");
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);
    walk(&root.join("src-tauri"), &mut files);
    walk(&root.join("src"), &mut files);
    let files: Vec<PathBuf> = files
        .into_iter()
        .filter(|p| *p != this_file)
        .filter(|p| {
            let n = p.to_string_lossy();
            n.ends_with(".rs") || n.ends_with(".ts") || n.ends_with(".tsx") || n.ends_with("Cargo.toml")
        })
        .collect();
    assert!(files.len() > 100, "only {} files scanned", files.len());
    let found = hits(&files, SOURCE_DENY);
    assert!(found.is_empty(), "never-do code in the source:\n{}", found.join("\n"));
}
