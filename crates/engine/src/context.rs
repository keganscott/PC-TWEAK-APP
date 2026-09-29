//! Interactive-user resolution and registry root routing.
//!
//! We ship one elevated binary, not a SYSTEM service. That changes the problem:
//!
//! `WTSQueryUserToken` needs `SE_TCB_NAME`, which is granted to SYSTEM and to
//! essentially nothing else. An elevated *user* process does not hold it, so
//! the call fails with ERROR_PRIVILEGE_NOT_HELD (1314). We therefore never call
//! it.
//!
//! Instead, in order of preference:
//!
//!   1. **Our own token.** UAC elevation produces a linked token for the *same*
//!      user — same SID, different groups and integrity level. So when the app
//!      was elevated normally, `GetTokenInformation(TokenUser)` on our own
//!      process token already *is* the interactive user's SID. This is the path
//!      almost every real run takes, it needs no privileges at all, and it is
//!      exact rather than heuristic.
//!
//!   2. **The interactive shell's token.** If the app was launched with "Run as
//!      different user" or from an admin account that is not the console user,
//!      our SID is the wrong one. We then find `explorer.exe` in the active
//!      console session and read its token. A High-IL process can open a
//!      Medium-IL process in the same session, so `PROCESS_QUERY_LIMITED_INFORMATION`
//!      is enough — we do not need to debug-privilege our way in.
//!
//! Once we have a SID we address `HKEY_USERS\<sid>` directly. When the SID
//! matches our own we use `HKEY_CURRENT_USER` instead, which is the same hive
//! by a shorter path and avoids a class of redirection surprise.
//!
//! We deliberately do not load an unloaded hive from NTUSER.DAT. That needs
//! `SE_RESTORE_NAME`/`SE_BACKUP_NAME` and it means writing to a profile of
//! someone who is not signed in. If the hive is not loaded, we fail loudly.

use std::ffi::c_void;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, LocalFree, HLOCAL};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, KEY_WRITE};
use winreg::RegKey;

use super::error::{EngineError, Result};
use super::types::RegRoot;

/// How we found the interactive user. Surfaced to the UI and written into the
/// journal, because "which hive did you write to" is the first question during
/// a support escalation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserResolution {
    /// Our own token. Elevation preserved the user identity.
    OwnToken,
    /// Read from explorer.exe in the active console session.
    InteractiveShell,
}

#[derive(Debug, Clone)]
pub struct UserContext {
    pub sid: String,
    pub resolution: UserResolution,
    /// True when the interactive SID equals our own, so HKCU is equivalent.
    pub is_self: bool,
}

/// Owns the resolved user context and hands out registry roots.
pub struct ContextResolver {
    user: UserContext,
    elevated: bool,
}

impl ContextResolver {
    pub fn detect(elevated: bool) -> Result<Self> {
        let own = current_process_sid()?;

        // Path 1: assume elevation preserved identity, then verify against the
        // shell. If the shell agrees, or we cannot read the shell at all, our
        // own token is the answer.
        let user = match interactive_shell_sid() {
            Ok(Some(shell_sid)) if shell_sid != own => UserContext {
                sid: shell_sid,
                resolution: UserResolution::InteractiveShell,
                is_self: false,
            },
            _ => UserContext {
                sid: own,
                resolution: UserResolution::OwnToken,
                is_self: true,
            },
        };

        // A SID we cannot address is worse than no SID: writes would silently
        // land in the wrong profile. Fail now, at startup, not mid-transaction.
        if !user.is_self {
            let users = RegKey::predef(HKEY_USERS);
            if users.open_subkey_with_flags(&user.sid, KEY_READ).is_err() {
                return Err(EngineError::UserHiveNotLoaded { sid: user.sid });
            }
        }

        Ok(Self { user, elevated })
    }

    pub fn user(&self) -> &UserContext {
        &self.user
    }

    pub fn elevated(&self) -> bool {
        self.elevated
    }

    /// Open a key under the given root, creating it if `create` is set.
    ///
    /// `RegRoot::InteractiveUser` resolves here and nowhere else — no caller
    /// outside this module ever names a hive directly.
    pub fn open(&self, root: RegRoot, path: &str, create: bool) -> Result<RegKey> {
        let (hive, full) = self.resolve(root, path);
        let base = RegKey::predef(hive);
        let flags = if self.elevated { KEY_READ | KEY_WRITE } else { KEY_READ };

        if create {
            base.create_subkey_with_flags(&full, flags)
                .map(|(k, _)| k)
                .map_err(|e| EngineError::registry(self.display_path(root, path), None, e))
        } else {
            base.open_subkey_with_flags(&full, flags)
                .map_err(|e| EngineError::registry(self.display_path(root, path), None, e))
        }
    }

    /// Read-only open. Works unelevated, used by `read_state`.
    pub fn open_read(&self, root: RegRoot, path: &str) -> Result<RegKey> {
        let (hive, full) = self.resolve(root, path);
        RegKey::predef(hive)
            .open_subkey_with_flags(&full, KEY_READ)
            .map_err(|e| EngineError::registry(self.display_path(root, path), None, e))
    }

    fn resolve(&self, root: RegRoot, path: &str) -> (winreg::HKEY, String) {
        match root {
            RegRoot::LocalMachine => (HKEY_LOCAL_MACHINE, path.to_string()),
            RegRoot::ClassesRoot => (HKEY_CLASSES_ROOT, path.to_string()),
            RegRoot::InteractiveUser => {
                if self.user.is_self {
                    (HKEY_CURRENT_USER, path.to_string())
                } else {
                    (HKEY_USERS, format!("{}\\{}", self.user.sid, path))
                }
            }
        }
    }

    /// Fully-qualified path for `.reg` files, journal entries and error text.
    /// Always writes the explicit `HKEY_USERS\<sid>` form even when we used
    /// HKCU, so a backup taken under one account restores correctly under
    /// another and stays meaningful during offline recovery.
    pub fn display_path(&self, root: RegRoot, path: &str) -> String {
        match root {
            RegRoot::LocalMachine => format!("HKEY_LOCAL_MACHINE\\{path}"),
            RegRoot::ClassesRoot => format!("HKEY_CLASSES_ROOT\\{path}"),
            RegRoot::InteractiveUser => format!("HKEY_USERS\\{}\\{}", self.user.sid, path),
        }
    }
}

// ---------------------------------------------------------------------------
// Win32 plumbing
// ---------------------------------------------------------------------------

/// SID of the user this process is running as.
fn current_process_sid() -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| EngineError::win32("OpenProcessToken", e))?;
        let guard = HandleGuard(token);
        sid_from_token(guard.0)
    }
}

/// SID of the user owning explorer.exe in the active console session.
/// `Ok(None)` means no interactive shell was found, which is legitimate — a
/// locked or headless machine — and is not an error.
fn interactive_shell_sid() -> Result<Option<String>> {
    unsafe {
        let console_session = WTSGetActiveConsoleSessionId();
        if console_session == 0xFFFF_FFFF {
            return Ok(None); // no console session attached
        }

        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|e| EngineError::win32("CreateToolhelp32Snapshot", e))?;
        let _snap_guard = HandleGuard(snapshot);

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_err() {
            return Ok(None);
        }

        loop {
            if process_name_is(&entry.szExeFile, "explorer.exe") {
                let mut session = 0u32;
                let in_console = ProcessIdToSessionId(entry.th32ProcessID, &mut session).is_ok()
                    && session == console_session;

                if in_console {
                    if let Ok(proc) =
                        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID)
                    {
                        let proc_guard = HandleGuard(proc);
                        let mut token = HANDLE::default();
                        if OpenProcessToken(proc_guard.0, TOKEN_QUERY, &mut token).is_ok() {
                            let token_guard = HandleGuard(token);
                            return sid_from_token(token_guard.0).map(Some);
                        }
                    }
                }
            }

            if Process32NextW(snapshot, &mut entry).is_err() {
                break;
            }
        }

        Ok(None)
    }
}

/// Extract and stringify the user SID from a token handle.
unsafe fn sid_from_token(token: HANDLE) -> Result<String> {
    let mut needed = 0u32;

    // First call is expected to fail with ERROR_INSUFFICIENT_BUFFER; we only
    // want the size, so the error is discarded deliberately.
    let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
    if needed == 0 {
        return Err(EngineError::UserContextUnresolved {
            detail: "GetTokenInformation reported a zero-length TOKEN_USER".into(),
        });
    }

    let mut buf = vec![0u8; needed as usize];
    GetTokenInformation(
        token,
        TokenUser,
        Some(buf.as_mut_ptr() as *mut c_void),
        needed,
        &mut needed,
    )
    .map_err(|e| EngineError::win32("GetTokenInformation", e))?;

    let token_user = &*(buf.as_ptr() as *const TOKEN_USER);

    let mut raw = PWSTR::null();
    ConvertSidToStringSidW(token_user.User.Sid, &mut raw)
        .map_err(|e| EngineError::win32("ConvertSidToStringSidW", e))?;

    let sid = raw.to_string().map_err(|e| EngineError::UserContextUnresolved {
        detail: format!("SID string was not valid UTF-16: {e}"),
    })?;
    let _ = LocalFree(HLOCAL(raw.0 as *mut c_void));

    Ok(sid)
}

/// Case-insensitive comparison against a fixed-size wide buffer.
fn process_name_is(buf: &[u16; 260], want: &str) -> bool {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..len]);
    name.eq_ignore_ascii_case(want)
}

/// Closes a handle on drop. Every early return above relies on this — the
/// original version of this module leaked a token handle on two error paths.
struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}
