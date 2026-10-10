//! Providing the pinned PresentMon binary.
//!
//! `vendor/presentmon/PINNED.json` is the single source of truth for which
//! release we trust: version, SHA-256, size, signer. Release builds embed that
//! exact file (feature `bundle-presentmon`); at run time it is written to the
//! protected `tools` directory and its hash is checked again before every use,
//! so a swapped file is never executed. A copy that does not hash to the pinned
//! value is never run, wherever it was found.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{EngineError, Result};
#[cfg(feature = "bundle-presentmon")]
use crate::fsutil;
use crate::secure_dir::TrustedDir;

const PINNED_JSON: &str = include_str!("../../../../vendor/presentmon/PINNED.json");

#[cfg(feature = "bundle-presentmon")]
static BUNDLED: &[u8] = include_bytes!("../../../../vendor/presentmon/PresentMon-x64.exe");

#[derive(Debug, Clone, Deserialize)]
pub struct Pinned {
    pub version: String,
    pub file: String,
    pub sha256: String,
    pub size: u64,
    pub signer: String,
}

pub fn pinned() -> Pinned {
    // The JSON is compiled in and covered by a test, so this cannot fail at run time.
    serde_json::from_str(PINNED_JSON).expect("vendor/presentmon/PINNED.json is valid")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Is the file at `path` exactly the pinned release? (Size first, so a huge
/// wrong file is not read into memory.)
pub fn is_pinned_file(path: &Path, pin: &Pinned) -> bool {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() == pin.size => {}
        _ => return false,
    }
    std::fs::read(path).is_ok_and(|b| sha256_hex(&b).eq_ignore_ascii_case(&pin.sha256))
}

fn tools_dir(dir: &TrustedDir) -> PathBuf {
    dir.path().join("tools")
}

fn extracted_path(dir: &TrustedDir, pin: &Pinned) -> PathBuf {
    tools_dir(dir).join(format!("PresentMon-{}-x64.exe", pin.version))
}

fn unavailable(detail: impl Into<String>) -> EngineError {
    EngineError::Command {
        what: "Provide PresentMon".into(),
        exit_code: None,
        detail: detail.into(),
    }
}

/// Make sure a verified PresentMon is on disk and return its path.
pub fn provision(dir: &TrustedDir) -> Result<PathBuf> {
    let pin = pinned();
    let target = extracted_path(dir, &pin);
    if is_pinned_file(&target, &pin) {
        return Ok(target);
    }

    #[cfg(feature = "bundle-presentmon")]
    {
        if !sha256_hex(BUNDLED).eq_ignore_ascii_case(&pin.sha256) || BUNDLED.len() as u64 != pin.size {
            return Err(unavailable(
                "the embedded PresentMon does not match vendor/presentmon/PINNED.json; rebuild with the verified file",
            ));
        }
        fsutil::create_dir_durable(&tools_dir(dir))?;
        fsutil::write_durable(&target, BUNDLED)?;
        if is_pinned_file(&target, &pin) {
            return Ok(target);
        }
        Err(unavailable(format!(
            "{} did not verify after it was written",
            target.display()
        )))
    }

    #[cfg(not(feature = "bundle-presentmon"))]
    {
        // Development builds: accept a copy next to the executable, but only if
        // it is byte-for-byte the pinned release.
        if let Some(beside) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.join(&pin.file)))
        {
            if is_pinned_file(&beside, &pin) {
                return Ok(beside);
            }
        }
        Err(unavailable(
            "this build does not include PresentMon (built without the `bundle-presentmon` feature)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pin_is_well_formed() {
        let p = pinned();
        assert_eq!(p.sha256.len(), 64);
        assert!(p.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(p.size > 100_000, "a real PresentMon is not tiny");
        assert_eq!(p.signer, "Intel Corporation");
        assert!(p.version.split('.').count() == 3);
    }

    #[test]
    fn the_license_text_is_shipped_with_the_pin() {
        let license = include_str!("../../../../vendor/presentmon/LICENSE.txt");
        assert!(license.contains("Permission is hereby granted, free of charge"));
        assert!(license.contains("Intel Corporation"));
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_file_with_the_wrong_content_or_size_is_never_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let pin = pinned();
        let bad = dir.path().join("x.exe");
        std::fs::write(&bad, b"not presentmon").unwrap();
        assert!(!is_pinned_file(&bad, &pin));
        // Right size, wrong bytes.
        std::fs::write(&bad, vec![0u8; pin.size as usize]).unwrap();
        assert!(!is_pinned_file(&bad, &pin));
        assert!(!is_pinned_file(&dir.path().join("missing.exe"), &pin));
    }

    /// Only when the real file has been fetched (CI does; a laptop may not).
    #[test]
    fn the_fetched_binary_matches_the_pin_if_present() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/presentmon/PresentMon-x64.exe");
        if path.is_file() {
            assert!(
                is_pinned_file(&path, &pinned()),
                "vendor copy does not match PINNED.json"
            );
        }
    }

    #[cfg(feature = "bundle-presentmon")]
    #[test]
    fn provisioning_extracts_and_verifies_the_embedded_copy() {
        let dir = tempfile::tempdir().unwrap();
        let trusted = TrustedDir::insecure_for_tests(dir.path());
        let path = provision(&trusted).unwrap();
        assert!(is_pinned_file(&path, &pinned()));
        // A tampered copy is replaced, not run.
        std::fs::write(&path, b"tampered").unwrap();
        let again = provision(&trusted).unwrap();
        assert!(is_pinned_file(&again, &pinned()));
    }

    #[cfg(not(feature = "bundle-presentmon"))]
    #[test]
    fn without_the_feature_provisioning_says_why_it_cannot() {
        let dir = tempfile::tempdir().unwrap();
        let e = provision(&TrustedDir::insecure_for_tests(dir.path())).unwrap_err();
        assert!(
            matches!(&e, EngineError::Command { detail, .. } if detail.contains("bundle-presentmon")),
            "{e:?}"
        );
    }
}
