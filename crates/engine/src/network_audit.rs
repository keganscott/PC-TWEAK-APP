//! "A test greps for sockets" (plan 6.4): the app collects nothing and makes no
//! network connection of its own, so no source file may reach for a network
//! API. Covers every crate's Rust and Cargo manifests (a `windows` networking
//! feature would show there), the PowerShell the engine runs (it lives in Rust
//! string literals), and the UI code that ships in the bundle.
//!
//! The dependency side is `scripts/check-no-network-deps.sh`; what the running
//! app actually does is `scripts/check-no-outbound.ps1` on Windows CI.

use std::fs;
use std::path::{Path, PathBuf};

/// Rust and Cargo: sockets, WinHTTP/WinINet, Winsock, and the networking
/// feature families of the `windows` crate. PowerShell inside Rust strings:
/// its download commands.
const RUST_DENY: &[&str] = &[
    "std::net",
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "Win32_Networking",
    "Win32_NetworkManagement",
    "WinHttp",
    "WinInet",
    "ws2_32",
    "WSAStartup",
    "URLDownloadToFile",
    "Invoke-WebRequest",
    "Invoke-RestMethod",
    "Net.WebClient",
    "Net.Http",
    "Start-BitsTransfer",
    "DownloadFile",
];

/// UI code: every way a page can talk to the network. The CSP blocks these at
/// run time too; this keeps them out of the source in the first place.
const UI_DENY: &[&str] = &[
    "fetch(",
    "XMLHttpRequest",
    "WebSocket",
    "EventSource",
    "sendBeacon",
    "RTCPeerConnection",
    "importScripts",
];

pub(crate) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

pub(crate) fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if !matches!(name.as_str(), "target" | "node_modules" | "gen" | "dist") {
                walk(&path, out);
            }
        } else {
            out.push(path);
        }
    }
}

pub(crate) fn hits(files: &[PathBuf], deny: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for file in files {
        let text = fs::read_to_string(file).unwrap();
        for (n, line) in text.lines().enumerate() {
            for pattern in deny {
                if line.contains(pattern) {
                    found.push(format!("{}:{}: {pattern}", file.display(), n + 1));
                }
            }
        }
    }
    found
}

#[test]
fn no_source_file_reaches_for_the_network() {
    let root = workspace_root();
    let this_file = root.join("crates/engine/src/network_audit.rs");

    let mut all = Vec::new();
    walk(&root.join("crates"), &mut all);
    walk(&root.join("src-tauri"), &mut all);
    let rust: Vec<PathBuf> = all
        .into_iter()
        .filter(|p| *p != this_file)
        .filter(|p| p.extension().is_some_and(|e| e == "rs") || p.ends_with("Cargo.toml"))
        .chain([root.join("Cargo.toml")])
        .collect();

    let mut ui = Vec::new();
    walk(&root.join("src"), &mut ui);
    // Tests run in Node, not in the app; everything else under src/ ships.
    ui.retain(|p| {
        let name = p.to_string_lossy();
        (name.ends_with(".ts") || name.ends_with(".tsx")) && !name.contains(".test.")
    });

    let found: Vec<String> = hits(&rust, RUST_DENY).into_iter().chain(hits(&ui, UI_DENY)).collect();
    assert!(found.is_empty(), "network APIs in the source:\n{}", found.join("\n"));

    // A scan that read nothing would also find nothing.
    assert!(rust.len() > 40, "only {} Rust/Cargo files scanned", rust.len());
    assert!(ui.len() > 20, "only {} UI files scanned", ui.len());
    println!(
        "no network APIs in {} Rust/Cargo files and {} UI files",
        rust.len(),
        ui.len()
    );
}
