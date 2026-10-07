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

/// The one wildcard a declared target key may hold: a whole `*` component,
/// standing for exactly one key whose name differs per PC (a network adapter's
/// `{guid}`, a device instance, a `00xx` adapter index). Never more than one per
/// target, and never part of a component.
pub const WILDCARD: &str = "*";

/// Number of `*` components in `pattern`.
fn wildcards(pattern: &[&str]) -> usize {
    pattern.iter().filter(|c| **c == WILDCARD).count()
}

/// One component against one pattern component: `*` matches any one name.
fn component_matches(pattern: &str, name: &str) -> bool {
    pattern == WILDCARD || pattern.eq_ignore_ascii_case(name)
}

/// Does the concrete `path` name the key that `pattern` (a declared target,
/// possibly with one `*` component) allows? Same number of components, so `*`
/// matches exactly one. A pattern with two or more `*` matches nothing.
pub fn pattern_eq(pattern: &str, path: &str) -> bool {
    let (p, k) = (components(pattern), components(path));
    wildcards(&p) <= 1 && p.len() == k.len() && p.iter().zip(&k).all(|(x, y)| component_matches(x, y))
}

/// True when the concrete key `ancestor` is the key `pattern` allows, or one of
/// its parents. Used for removing keys a write created, and by the never-do
/// audit (where a `*` could stand for a forbidden key, so it counts as a match).
pub fn pattern_is_ancestor_or_equal(ancestor: &str, pattern: &str) -> bool {
    let (a, p) = (components(ancestor), components(pattern));
    wildcards(&p) <= 1 && a.len() <= p.len() && p.iter().zip(&a).all(|(x, y)| component_matches(x, y))
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

    #[test]
    fn a_wildcard_segment_matches_exactly_one_component() {
        let p = r"SYSTEM\Tcpip\Interfaces\*";
        assert!(pattern_eq(p, r"system\tcpip\interfaces\{0A1B-22}"));
        assert!(
            !pattern_eq(p, r"SYSTEM\Tcpip\Interfaces"),
            "must not match zero components"
        );
        assert!(
            !pattern_eq(p, r"SYSTEM\Tcpip\Interfaces\{0A1B-22}\Sub"),
            "must not match two components"
        );
        assert!(pattern_eq(
            r"Enum\PCI\*\Device Parameters",
            r"enum\pci\VEN_10DE\device parameters"
        ));
        assert!(!pattern_eq(
            r"Enum\PCI\*\Device Parameters",
            r"Enum\PCI\a\b\Device Parameters"
        ));
        // Only a whole `*` component is a wildcard.
        assert!(!pattern_eq(r"A\0*", r"A\0001"));
        assert!(pattern_eq(r"A\0*", r"A\0*"));
        // Two wildcards are refused outright, even where they would line up.
        assert!(!pattern_eq(r"A\*\*", r"A\b\c"));
        // Without a wildcard it is plain path equality.
        assert!(pattern_eq(r"A\B", r"a\b"));
    }

    #[test]
    fn a_concrete_key_is_on_the_path_to_a_wildcard_target_only_within_its_length() {
        assert!(pattern_is_ancestor_or_equal(r"A", r"A\*\C"));
        assert!(pattern_is_ancestor_or_equal(r"a\guid", r"A\*\C"));
        assert!(pattern_is_ancestor_or_equal(r"a\guid\c", r"A\*\C"));
        assert!(!pattern_is_ancestor_or_equal(r"a\guid\c\d", r"A\*\C"));
        assert!(!pattern_is_ancestor_or_equal(r"a\guid\x", r"A\*\C"));
        assert!(!pattern_is_ancestor_or_equal(r"a\b", r"A\*\*"));
    }
}
