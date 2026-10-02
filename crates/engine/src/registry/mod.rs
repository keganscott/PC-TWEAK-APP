//! Registry access behind a trait, so journal and transaction logic runs in
//! tests on any OS against an in-memory fake, and only this module's Windows
//! implementation touches `winreg`.
//!
//! The trait speaks in concrete hives. Routing `RegRoot::InteractiveUser` to
//! `HKEY_CURRENT_USER` or `HKEY_USERS\<sid>` is `ContextResolver`'s job, so the
//! backends stay dumb and the routing rule is tested once.

use super::error::Result;
use super::types::RawValue;

#[cfg(test)]
mod contract_tests;
#[cfg(any(test, feature = "test-support"))]
pub mod fake;
#[cfg(windows)]
pub mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hive {
    LocalMachine,
    ClassesRoot,
    CurrentUser,
    Users,
}

impl Hive {
    pub fn name(self) -> &'static str {
        match self {
            Self::LocalMachine => "HKEY_LOCAL_MACHINE",
            Self::ClassesRoot => "HKEY_CLASSES_ROOT",
            Self::CurrentUser => "HKEY_CURRENT_USER",
            Self::Users => "HKEY_USERS",
        }
    }
}

/// Split a registry path into non-empty components. Tolerates stray or doubled
/// separators so `a\\b\` and `a\b` name the same key.
pub fn components(path: &str) -> Vec<&str> {
    path.split('\\').filter(|c| !c.is_empty()).collect()
}

/// Normalised form used for comparisons: components joined by one backslash.
pub fn normalise_path(path: &str) -> String {
    components(path).join("\\")
}

/// Registry names compare case-insensitively.
pub fn path_eq(a: &str, b: &str) -> bool {
    let (a, b) = (components(a), components(b));
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// True when `ancestor` is `path` or one of its parents.
pub fn is_ancestor_or_equal(ancestor: &str, path: &str) -> bool {
    let (a, p) = (components(ancestor), components(path));
    a.len() <= p.len() && a.iter().zip(&p).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// The operations the engine needs, and no more. All methods take `&self`;
/// implementations synchronise internally, which lets tests keep a handle to the
/// fake and change it "externally" between engine calls.
pub trait RegistryBackend: Send + Sync {
    /// Does the key exist?
    fn key_exists(&self, hive: Hive, path: &str) -> Result<bool>;

    /// Read a value. `Ok(None)` when the key or the value is absent. The value
    /// is returned with whatever type it has; callers decide whether they can
    /// handle it.
    fn read_value(&self, hive: Hive, path: &str, name: &str) -> Result<Option<RawValue>>;

    /// Write a value, creating the key (and missing parents) if needed.
    fn write_value(&self, hive: Hive, path: &str, name: &str, value: &RawValue) -> Result<()>;

    /// Delete a value. `Ok(false)` when it was already absent.
    fn delete_value(&self, hive: Hive, path: &str, name: &str) -> Result<bool>;

    /// Create a key and any missing parents. Idempotent.
    fn create_key(&self, hive: Hive, path: &str) -> Result<()>;

    /// Delete the key only if it has no values and no subkeys. `Ok(false)` when
    /// it is absent or not empty.
    fn delete_key_if_empty(&self, hive: Hive, path: &str) -> Result<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_helpers_ignore_case_and_stray_separators() {
        assert!(path_eq(r"Control Panel\Mouse", r"control panel\\MOUSE\"));
        assert!(!path_eq(r"Control Panel\Mouse", r"Control Panel\Mouse2"));
        assert_eq!(normalise_path(r"\a\\b\"), r"a\b");
        assert!(is_ancestor_or_equal(r"A\B", r"a\b\c"));
        assert!(is_ancestor_or_equal(r"A\B", r"a\b"));
        assert!(!is_ancestor_or_equal(r"A\B\C", r"a\b"));
        assert!(!is_ancestor_or_equal(r"A\BX", r"a\b\c"));
    }
}
