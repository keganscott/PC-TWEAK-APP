//! Interactive-user context and registry root routing.
//!
//! `ContextResolver` is pure routing: it knows which user we are acting for and
//! maps a `RegRoot` plus a path to a concrete hive and path. Reads and writes
//! go through a `RegistryBackend`, so all of this runs in tests against the
//! fake. Working out *who* the interactive user is needs Win32 and lives in
//! `identity.rs`.
//!
//! When the resolved SID is our own we address `HKEY_CURRENT_USER`, which is the
//! same hive by a shorter path. Otherwise we address `HKEY_USERS\<sid>`
//! directly. We deliberately do not load an unloaded hive from NTUSER.DAT: that
//! needs `SE_RESTORE_NAME`/`SE_BACKUP_NAME` and means writing to a profile of
//! someone who is not signed in. If the hive is not loaded, we fail loudly.

use std::sync::Arc;

use serde::Serialize;
use ts_rs::TS;

use super::error::{EngineError, Result};
use super::registry::{Hive, RegistryBackend};
use super::system::{SysItem, SysState, SystemBackend, Unavailable};
use super::types::{RawValue, RegRoot};

/// How we found the interactive user. Surfaced to the UI, because "which hive
/// did you write to" is the first question during a support escalation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum UserResolution {
    /// Our own token. Elevation preserved the user identity (classic UAC).
    OwnToken,
    /// Read from explorer.exe in our own session. Needed whenever our token is
    /// not the signed-in user's: alternate admin credentials, or Administrator
    /// Protection, where the elevated token is a hidden system-managed account
    /// with its own SID and its own HKCU.
    InteractiveShell,
}

#[derive(Debug, Clone)]
pub struct UserContext {
    pub sid: String,
    pub resolution: UserResolution,
    /// True when the interactive SID equals our own, so HKCU is equivalent.
    pub is_self: bool,
}

/// Owns the resolved user context and hands out registry access.
pub struct ContextResolver {
    user: UserContext,
    elevated: bool,
    backend: Arc<dyn RegistryBackend>,
    /// Non-registry changes (`system.rs`). `Unavailable` unless one is given.
    system: Arc<dyn SystemBackend>,
}

impl ContextResolver {
    pub fn new(user: UserContext, elevated: bool, backend: Arc<dyn RegistryBackend>) -> Self {
        Self {
            user,
            elevated,
            backend,
            system: Arc::new(Unavailable),
        }
    }

    /// Use `system` for changes that are not registry values.
    pub fn with_system(mut self, system: Arc<dyn SystemBackend>) -> Self {
        self.system = system;
        self
    }

    /// The non-registry backend, for `Transaction`. Tweaks read through
    /// `read_system` / `read_file` below.
    pub(crate) fn system(&self) -> &dyn SystemBackend {
        self.system.as_ref()
    }

    /// The current state of a non-registry item (safe for tweaks).
    pub fn read_system(&self, item: &SysItem) -> Result<SysState> {
        self.system.read(item)
    }

    /// A file's bytes, `None` when absent (safe for tweaks).
    pub fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.system.read_file(path)
    }

    /// Resolve the interactive user with Win32 and bind the real registry.
    #[cfg(windows)]
    pub fn detect(elevated: bool) -> Result<Self> {
        let user = super::identity::detect_user()?;
        let backend: Arc<dyn RegistryBackend> = Arc::new(super::registry::windows::WinRegistry::new());

        // A SID we cannot address is worse than no SID: writes would silently
        // land in the wrong profile. Fail now, at startup, not mid-transaction.
        if !user.is_self && !backend.key_exists(Hive::Users, &user.sid)? {
            return Err(EngineError::UserHiveNotLoaded { sid: user.sid });
        }
        Ok(Self::new(user, elevated, backend).with_system(Arc::new(super::system_win::WinSystem)))
    }

    pub fn user(&self) -> &UserContext {
        &self.user
    }

    pub fn elevated(&self) -> bool {
        self.elevated
    }

    /// The backend, for `Transaction`. Tweaks get a `&ContextResolver` and must
    /// use the read helpers below; a test scans the tweak sources to keep them
    /// off this.
    pub(crate) fn backend(&self) -> &dyn RegistryBackend {
        self.backend.as_ref()
    }

    /// Map a root and a path to a concrete hive and path. `InteractiveUser`
    /// resolves here and nowhere else, so no caller names a hive directly.
    pub fn route(&self, root: RegRoot, path: &str) -> (Hive, String) {
        match root {
            RegRoot::LocalMachine => (Hive::LocalMachine, path.to_string()),
            RegRoot::ClassesRoot => (Hive::ClassesRoot, path.to_string()),
            RegRoot::InteractiveUser => {
                if self.user.is_self {
                    (Hive::CurrentUser, path.to_string())
                } else {
                    (Hive::Users, format!("{}\\{}", self.user.sid, path))
                }
            }
        }
    }

    /// Fully-qualified path for `.reg` files, journal entries and error text.
    /// Always the explicit `HKEY_USERS\<sid>` form for the user hive even when
    /// we used HKCU, so a backup taken under one account restores correctly
    /// under another and stays meaningful during offline recovery.
    pub fn display_path(&self, root: RegRoot, path: &str) -> String {
        match root {
            RegRoot::LocalMachine => format!("HKEY_LOCAL_MACHINE\\{path}"),
            RegRoot::ClassesRoot => format!("HKEY_CLASSES_ROOT\\{path}"),
            RegRoot::InteractiveUser => format!("HKEY_USERS\\{}\\{}", self.user.sid, path),
        }
    }

    // ---- reads (safe for tweaks) ------------------------------------------

    pub fn key_exists(&self, root: RegRoot, path: &str) -> Result<bool> {
        let (hive, full) = self.route(root, path);
        self.backend.key_exists(hive, &full)
    }

    /// `Ok(None)` when the key or value is absent.
    pub fn read_raw(&self, root: RegRoot, path: &str, name: &str) -> Result<Option<RawValue>> {
        let (hive, full) = self.route(root, path);
        self.backend.read_value(hive, &full, name)
    }

    /// `Ok(None)` when absent; an error when present with another type, so a
    /// tweak cannot mistake a mistyped value for the default.
    pub fn read_dword(&self, root: RegRoot, path: &str, name: &str) -> Result<Option<u32>> {
        match self.read_raw(root, path, name)? {
            None => Ok(None),
            Some(v) => v.as_dword().map(Some).ok_or_else(|| {
                EngineError::registry_msg(
                    self.display_path(root, path),
                    Some(name),
                    format!(
                        "expected a REG_DWORD, found type {} with {} bytes",
                        v.vtype,
                        v.bytes.len()
                    ),
                )
            }),
        }
    }

    pub fn read_string(&self, root: RegRoot, path: &str, name: &str) -> Result<Option<String>> {
        match self.read_raw(root, path, name)? {
            None => Ok(None),
            Some(v) => v.as_sz().map(Some).ok_or_else(|| {
                EngineError::registry_msg(
                    self.display_path(root, path),
                    Some(name),
                    format!("expected a string, found type {}", v.vtype),
                )
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::fake::FakeRegistry;

    fn resolver(is_self: bool) -> (ContextResolver, Arc<FakeRegistry>) {
        let fake = Arc::new(FakeRegistry::new());
        let user = UserContext {
            sid: "S-1-5-21-1-2-3-1001".into(),
            resolution: if is_self {
                UserResolution::OwnToken
            } else {
                UserResolution::InteractiveShell
            },
            is_self,
        };
        (ContextResolver::new(user, true, fake.clone()), fake)
    }

    #[test]
    fn own_sid_routes_to_hkcu_but_displays_the_sid_form() {
        let (r, _) = resolver(true);
        assert_eq!(
            r.route(RegRoot::InteractiveUser, r"Control Panel\Mouse"),
            (Hive::CurrentUser, r"Control Panel\Mouse".to_string())
        );
        assert_eq!(
            r.display_path(RegRoot::InteractiveUser, r"Control Panel\Mouse"),
            r"HKEY_USERS\S-1-5-21-1-2-3-1001\Control Panel\Mouse"
        );
    }

    #[test]
    fn other_sid_routes_to_hku() {
        let (r, _) = resolver(false);
        assert_eq!(
            r.route(RegRoot::InteractiveUser, "X"),
            (Hive::Users, r"S-1-5-21-1-2-3-1001\X".to_string())
        );
        assert_eq!(r.route(RegRoot::LocalMachine, "SOFTWARE").0, Hive::LocalMachine);
    }

    #[test]
    fn typed_reads_reject_wrong_types() {
        let (r, fake) = resolver(true);
        fake.set_external(Hive::LocalMachine, "K", "S", RawValue::sz("x"));
        fake.set_external(Hive::LocalMachine, "K", "D", RawValue::dword(7));
        assert_eq!(r.read_dword(RegRoot::LocalMachine, "K", "D").unwrap(), Some(7));
        assert_eq!(
            r.read_string(RegRoot::LocalMachine, "K", "S").unwrap().as_deref(),
            Some("x")
        );
        assert!(r.read_dword(RegRoot::LocalMachine, "K", "S").is_err());
        assert!(r.read_string(RegRoot::LocalMachine, "K", "D").is_err());
        assert_eq!(r.read_dword(RegRoot::LocalMachine, "K", "Missing").unwrap(), None);
    }
}
