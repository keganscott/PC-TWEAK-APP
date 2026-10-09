//! In-memory registry for tests, with fault injection.
//!
//! Keys and value names are case-insensitive, as on Windows. The fake models
//! only what the engine uses: keys, values with a type and bytes, and
//! empty-key deletion.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::{components, Hive, RegistryBackend};
use crate::error::{EngineError, Result};
use crate::types::RawValue;

#[derive(Default)]
struct Inner {
    /// (hive, lower-cased normalised path) -> values keyed by lower-cased name.
    keys: BTreeMap<(Hive, String), BTreeMap<String, (String, RawValue)>>,
    /// Counts `write_value` and `delete_value` calls since construction.
    mutations: usize,
    /// Fail the mutation whose 1-based ordinal equals this.
    fail_at: Option<usize>,
}

#[derive(Default)]
pub struct FakeRegistry {
    inner: Mutex<Inner>,
}

fn key_id(hive: Hive, path: &str) -> (Hive, String) {
    (hive, components(path).join("\\").to_ascii_lowercase())
}

fn ancestors(path: &str) -> Vec<String> {
    let parts: Vec<String> = components(path).iter().map(|c| c.to_ascii_lowercase()).collect();
    (1..=parts.len()).map(|n| parts[..n].join("\\")).collect()
}

impl FakeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make the `n`th (1-based) mutating call from now fail with a registry
    /// error, exactly once. `n` counts from the current call total.
    pub fn fail_mutation_number(&self, n: usize) {
        let mut g = self.inner.lock().unwrap();
        g.fail_at = Some(g.mutations + n);
    }

    /// Disarm any pending injected failure.
    pub fn clear_faults(&self) {
        self.inner.lock().unwrap().fail_at = None;
    }

    pub fn mutation_count(&self) -> usize {
        self.inner.lock().unwrap().mutations
    }

    /// Set a value as if another program had done it: bypasses the failure
    /// counter and the mutation count.
    pub fn set_external(&self, hive: Hive, path: &str, name: &str, value: RawValue) {
        let mut g = self.inner.lock().unwrap();
        for a in ancestors(path) {
            g.keys.entry((hive, a)).or_default();
        }
        g.keys
            .get_mut(&key_id(hive, path))
            .unwrap()
            .insert(name.to_ascii_lowercase(), (name.to_owned(), value));
    }

    /// Delete a key and everything under it, as if a device were removed.
    pub fn remove_key_external(&self, hive: Hive, path: &str) {
        let (h, p) = key_id(hive, path);
        let prefix = format!("{p}\\");
        self.inner
            .lock()
            .unwrap()
            .keys
            .retain(|(kh, kp), _| !(*kh == h && (*kp == p || kp.starts_with(&prefix))));
    }

    pub fn remove_external(&self, hive: Hive, path: &str, name: &str) {
        let mut g = self.inner.lock().unwrap();
        if let Some(k) = g.keys.get_mut(&key_id(hive, path)) {
            k.remove(&name.to_ascii_lowercase());
        }
    }

    /// Every value in the registry, for whole-state assertions.
    pub fn snapshot(&self) -> BTreeMap<(Hive, String, String), RawValue> {
        let g = self.inner.lock().unwrap();
        let mut out = BTreeMap::new();
        for ((hive, path), vals) in &g.keys {
            for (lname, (_, v)) in vals {
                out.insert((*hive, path.clone(), lname.clone()), v.clone());
            }
        }
        out
    }

    /// Read without going through the trait, for assertions.
    pub fn read_value_for_test(&self, hive: Hive, path: &str, name: &str) -> Option<RawValue> {
        self.read_value(hive, path, name).unwrap()
    }

    pub fn key_exists_for_test(&self, hive: Hive, path: &str) -> bool {
        self.key_exists(hive, path).unwrap()
    }

    /// All existing key paths (lower-cased), for asserting key cleanup.
    pub fn key_paths(&self) -> Vec<(Hive, String)> {
        self.inner.lock().unwrap().keys.keys().cloned().collect()
    }
}

impl Inner {
    fn tick(&mut self, hive: Hive, path: &str, name: Option<&str>) -> Result<()> {
        self.mutations += 1;
        if self.fail_at == Some(self.mutations) {
            self.fail_at = None;
            return Err(EngineError::registry_msg(
                format!("{}\\{}", hive.name(), path),
                name,
                "injected failure",
            ));
        }
        Ok(())
    }
}

impl RegistryBackend for FakeRegistry {
    fn key_exists(&self, hive: Hive, path: &str) -> Result<bool> {
        Ok(self.inner.lock().unwrap().keys.contains_key(&key_id(hive, path)))
    }

    fn read_value(&self, hive: Hive, path: &str, name: &str) -> Result<Option<RawValue>> {
        let g = self.inner.lock().unwrap();
        Ok(g.keys
            .get(&key_id(hive, path))
            .and_then(|k| k.get(&name.to_ascii_lowercase()))
            .map(|(_, v)| v.clone()))
    }

    fn value_names(&self, hive: Hive, path: &str) -> Result<Vec<String>> {
        let g = self.inner.lock().unwrap();
        Ok(g.keys
            .get(&key_id(hive, path))
            .map(|k| k.values().map(|(name, _)| name.clone()).collect())
            .unwrap_or_default())
    }

    fn subkey_names(&self, hive: Hive, path: &str) -> Result<Vec<String>> {
        // Keys are kept in lower case, so names come back in lower case.
        let (_, parent) = key_id(hive, path);
        let g = self.inner.lock().unwrap();
        Ok(g.keys
            .keys()
            .filter(|(h, _)| *h == hive)
            .filter_map(|(_, k)| match parent.as_str() {
                "" => Some(k.as_str()),
                p => k.strip_prefix(p)?.strip_prefix('\\'),
            })
            .filter(|rest| !rest.is_empty() && !rest.contains('\\'))
            .map(str::to_owned)
            .collect())
    }

    fn write_value(&self, hive: Hive, path: &str, name: &str, value: &RawValue) -> Result<()> {
        // Same refusal as WinRegistry, before anything changes (contract_tests).
        if !value.is_supported_type() {
            return Err(EngineError::UnsupportedValueType {
                path: format!("{}\\{}", hive.name(), path),
                value: name.to_owned(),
                vtype: value.vtype,
            });
        }
        let mut g = self.inner.lock().unwrap();
        g.tick(hive, path, Some(name))?;
        for a in ancestors(path) {
            g.keys.entry((hive, a)).or_default();
        }
        g.keys
            .get_mut(&key_id(hive, path))
            .unwrap()
            .insert(name.to_ascii_lowercase(), (name.to_owned(), value.clone()));
        Ok(())
    }

    fn delete_value(&self, hive: Hive, path: &str, name: &str) -> Result<bool> {
        let mut g = self.inner.lock().unwrap();
        g.tick(hive, path, Some(name))?;
        Ok(g.keys
            .get_mut(&key_id(hive, path))
            .is_some_and(|k| k.remove(&name.to_ascii_lowercase()).is_some()))
    }

    fn create_key(&self, hive: Hive, path: &str) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        for a in ancestors(path) {
            g.keys.entry((hive, a)).or_default();
        }
        Ok(())
    }

    fn delete_key_if_empty(&self, hive: Hive, path: &str) -> Result<bool> {
        let mut g = self.inner.lock().unwrap();
        let id = key_id(hive, path);
        let prefix = format!("{}\\", id.1);
        let has_children = g.keys.keys().any(|(h, p)| *h == hive && p.starts_with(&prefix));
        match g.keys.get(&id) {
            Some(vals) if vals.is_empty() && !has_children => {
                g.keys.remove(&id);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_keys_are_case_insensitive() {
        let r = FakeRegistry::new();
        r.write_value(Hive::LocalMachine, r"SOFTWARE\Foo", "Bar", &RawValue::dword(1))
            .unwrap();
        assert_eq!(
            r.read_value(Hive::LocalMachine, r"software\FOO", "BAR").unwrap(),
            Some(RawValue::dword(1))
        );
        assert!(r.key_exists(Hive::LocalMachine, "software").unwrap());
    }

    #[test]
    fn delete_key_if_empty_respects_values_and_children() {
        let r = FakeRegistry::new();
        r.write_value(Hive::LocalMachine, r"A\B", "v", &RawValue::dword(1))
            .unwrap();
        assert!(!r.delete_key_if_empty(Hive::LocalMachine, "A").unwrap(), "has a child");
        assert!(
            !r.delete_key_if_empty(Hive::LocalMachine, r"A\B").unwrap(),
            "has a value"
        );
        r.delete_value(Hive::LocalMachine, r"A\B", "v").unwrap();
        assert!(r.delete_key_if_empty(Hive::LocalMachine, r"A\B").unwrap());
        assert!(r.delete_key_if_empty(Hive::LocalMachine, "A").unwrap());
        assert!(r.key_paths().is_empty());
    }

    #[test]
    fn fault_injection_fails_exactly_the_requested_mutation() {
        let r = FakeRegistry::new();
        r.fail_mutation_number(2);
        r.write_value(Hive::LocalMachine, "K", "a", &RawValue::dword(1))
            .unwrap();
        assert!(r
            .write_value(Hive::LocalMachine, "K", "b", &RawValue::dword(2))
            .is_err());
        r.write_value(Hive::LocalMachine, "K", "c", &RawValue::dword(3))
            .unwrap();
        assert!(r.read_value(Hive::LocalMachine, "K", "b").unwrap().is_none());
    }
}
