//! "A test greps for sockets" (plan 6.4): the app collects nothing and makes no
//! network connection of its own, so no source file may reach for a network
//! API. Covers every crate's Rust and Cargo manifests (a `windows` networking
//! feature would show there), the PowerShell the engine runs (it lives in Rust
//! string literals), and the UI code that ships in the bundle.
//!
//! One exception, Kegan's (DECISIONS 15.22): the connection check
//! (`netcheck.rs`, catalogue E4) sends ICMP echo requests, what `ping` sends,
//! when the user starts it. `ALLOWED` lets exactly what it needs through, in
//! exactly those places; sockets, HTTP and Winsock stay denied there too.
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
    "WinSock::",
    "WSAStartup",
    "URLDownloadToFile",
    "Invoke-WebRequest",
    "Invoke-RestMethod",
    "Net.WebClient",
    "Net.Http",
    "Start-BitsTransfer",
    "DownloadFile",
];

/// The connection check's needs, per file (path suffix, pattern, the line it
/// may appear on, trimmed). Each must still be in use, so the list cannot
/// outlive the code.
const ALLOWED: &[(&str, &str, &str)] = &[
    // The address type only; `TcpStream` and `UdpSocket` stay denied.
    ("crates/engine/src/netcheck.rs", "std::net", "use std::net::Ipv4Addr;"),
    // IcmpSendEcho and GetBestRoute; GetBestRoute's binding needs WinSock's
    // types enabled, not its functions (`WinSock::` stays denied).
    (
        "crates/engine/Cargo.toml",
        "Win32_NetworkManagement",
        "\"Win32_NetworkManagement_IpHelper\",",
    ),
    (
        "crates/engine/Cargo.toml",
        "Win32_Networking",
        "\"Win32_Networking_WinSock\",",
    ),
    // NDIS's types only: `MIB_IF_ROW2`, which GetIfEntry2 (IP Helper) fills
    // with the name of the connection the router is reached through.
    (
        "crates/engine/Cargo.toml",
        "Win32_NetworkManagement",
        "\"Win32_NetworkManagement_Ndis\",",
    ),
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
    hits_allowing(files, deny, &[], &mut Vec::new())
}

/// Like `hits`, letting through each `allowed` (file suffix, pattern, line)
/// and noting in `used` which ones were met.
fn hits_allowing(
    files: &[PathBuf],
    deny: &[&str],
    allowed: &[(&str, &str, &str)],
    used: &mut Vec<usize>,
) -> Vec<String> {
    let mut found = Vec::new();
    for file in files {
        let text = fs::read_to_string(file).unwrap();
        let path = file.to_string_lossy().replace('\\', "/");
        for (n, line) in text.lines().enumerate() {
            for pattern in deny {
                if !line.contains(pattern) {
                    continue;
                }
                let allowance = allowed
                    .iter()
                    .position(|(f, p, l)| path.ends_with(f) && p == pattern && line.trim() == *l);
                match allowance {
                    Some(i) => used.push(i),
                    None => found.push(format!("{}:{}: {pattern}", file.display(), n + 1)),
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

    let mut used = Vec::new();
    let found: Vec<String> = hits_allowing(&rust, RUST_DENY, ALLOWED, &mut used)
        .into_iter()
        .chain(hits(&ui, UI_DENY))
        .collect();
    assert!(found.is_empty(), "network APIs in the source:\n{}", found.join("\n"));
    for (i, allowance) in ALLOWED.iter().enumerate() {
        assert!(used.contains(&i), "allowed but no longer used: {allowance:?}");
    }

    // A scan that read nothing would also find nothing.
    assert!(rust.len() > 40, "only {} Rust/Cargo files scanned", rust.len());
    assert!(ui.len() > 20, "only {} UI files scanned", ui.len());
    println!(
        "no network APIs in {} Rust/Cargo files and {} UI files",
        rust.len(),
        ui.len()
    );
}

/// The exception lets through only its own lines: the same pattern anywhere
/// else, or a socket in the allowed file, is still caught.
#[test]
fn the_connection_checks_allowance_is_that_narrow() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("crates/engine/src/netcheck.rs");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        "use std::net::Ipv4Addr;\nuse std::net::UdpSocket;\nuse std::net::{Ipv4Addr, TcpStream};\n",
    )
    .unwrap();
    let other = dir.path().join("crates/engine/src/other.rs");
    fs::write(&other, "use std::net::Ipv4Addr;\n").unwrap();
    let mut used = Vec::new();
    let found = hits_allowing(&[file, other], RUST_DENY, ALLOWED, &mut used);
    assert_eq!(found.len(), 5, "{found:#?}");
    assert_eq!(used, [0]);
}
