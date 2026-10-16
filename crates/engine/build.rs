//! Fails the build with instructions, not a bare "couldn't read file", when the
//! PresentMon binary is meant to be embedded but has not been fetched.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=../../vendor/presentmon/PresentMon-x64.exe");
    println!("cargo:rerun-if-changed=../../vendor/presentmon/PINNED.json");
    if std::env::var_os("CARGO_FEATURE_BUNDLE_PRESENTMON").is_some() {
        let exe = Path::new("../../vendor/presentmon/PresentMon-x64.exe");
        if !exe.is_file() {
            panic!(
                "the `bundle-presentmon` feature needs vendor/presentmon/PresentMon-x64.exe.\n\
                 Fetch and verify it with:  powershell -File scripts/fetch-presentmon.ps1\n\
                 (or build without the feature; proof runs will then be unavailable)."
            );
        }
    }
}
