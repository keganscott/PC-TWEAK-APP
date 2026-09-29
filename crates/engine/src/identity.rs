//! Who is the interactive user? (Windows only.)
//!
//! We ship one elevated binary, not a SYSTEM service. `WTSQueryUserToken` needs
//! `SE_TCB_NAME`, which is granted to SYSTEM and essentially nothing else, so
//! an elevated *user* process gets ERROR_PRIVILEGE_NOT_HELD (1314) from it. We
//! never call it. In order of preference:
//!
//!   1. **Our own token.** Under classic UAC an elevated process is the same
//!      user with a linked token: same SID, different groups and integrity
//!      level. `TokenUser` on our own token is then exactly the interactive
//!      user, needs no privileges, and is not a heuristic.
//!
//!   2. **The interactive shell's token.** That assumption breaks in two cases:
//!      the app was launched with alternate admin credentials, and Windows 11
//!      Administrator Protection, where the elevated token belongs to a hidden,
//!      system-managed account with a *different* SID and its own HKCU
//!      (Microsoft Learn, "Administrator Protection"). In both, we find
//!      `explorer.exe` in **our own session** and read its token. A High-IL
//!      process can open a Medium-IL process in the same session, so
//!      `PROCESS_QUERY_LIMITED_INFORMATION` is enough.
//!
//! Anchoring on our own session, not the console session, is what keeps this
//! right over RDP, where the console session is a different user.

use std::ffi::c_void;
use std::path::PathBuf;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TokenUser, PSID, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::context::{UserContext, UserResolution};
use super::error::{EngineError, Result};
use super::profile::{expand_percent_vars, webview2_dir_in_profile};
use super::registry::windows::WinRegistry;
use super::registry::{Hive, RegistryBackend};

/// True when the process token is elevated. The manifest requests
/// `requireAdministrator`, so this should always be true; we check anyway,
/// because a manifest can be stripped and a mutating engine that assumes its
/// own privilege is a bad engine.
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let _guard = HandleGuard(token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut c_void),
            size,
            &mut size,
        )
        .is_ok()
            && elevation.TokenIsElevated != 0
    }
}

/// Resolve the interactive user for this session.
pub fn detect_user() -> Result<UserContext> {
    let own = current_process_sid()?;
    Ok(match interactive_shell_sid() {
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
    })
}

/// `ProfileImagePath` for a SID, from the machine's ProfileList.
pub fn profile_dir_for_sid(sid: &str) -> Result<Option<PathBuf>> {
    let key = format!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{sid}");
    let Some(v) = WinRegistry::new().read_value(Hive::LocalMachine, &key, "ProfileImagePath")? else {
        return Ok(None);
    };
    Ok(v.as_sz()
        .map(|raw| PathBuf::from(expand_percent_vars(&raw, |name| std::env::var(name).ok()))))
}

/// A WebView2 data folder the interactive user owns, for when this process is
/// *not* running as that user. Under Administrator Protection the elevated
/// token is a hidden account whose profile WebView2 cannot start in
/// (tauri-apps/tauri#13926), so the default location fails. `None` means the
/// default is fine (same identity) or the profile could not be found; either
/// way the caller leaves WebView2's default alone.
pub fn webview2_user_data_dir() -> Option<PathBuf> {
    let user = detect_user().ok()?;
    if user.is_self {
        return None;
    }
    let profile = profile_dir_for_sid(&user.sid).ok()??;
    Some(webview2_dir_in_profile(&profile.to_string_lossy()))
}

/// SID of the user this process is running as.
pub fn current_process_sid() -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| EngineError::win32("OpenProcessToken", e))?;
        let guard = HandleGuard(token);
        sid_from_token(guard.0)
    }
}

/// SID of the user owning explorer.exe in *our* session. `Ok(None)` means no
/// shell was found there, which is legitimate and not an error.
fn interactive_shell_sid() -> Result<Option<String>> {
    unsafe {
        let mut own_session = 0u32;
        ProcessIdToSessionId(GetCurrentProcessId(), &mut own_session)
            .map_err(|e| EngineError::win32("ProcessIdToSessionId", e))?;

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
                let in_our_session =
                    ProcessIdToSessionId(entry.th32ProcessID, &mut session).is_ok() && session == own_session;

                if in_our_session {
                    if let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID) {
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

    // u64 backing keeps the buffer 8-byte aligned, as TOKEN_USER requires.
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    GetTokenInformation(
        token,
        TokenUser,
        Some(buf.as_mut_ptr() as *mut c_void),
        needed,
        &mut needed,
    )
    .map_err(|e| EngineError::win32("GetTokenInformation", e))?;

    let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
    sid_to_string(token_user.User.Sid)
}

/// Stringify a SID (`S-1-5-...`).
pub(crate) unsafe fn sid_to_string(sid: PSID) -> Result<String> {
    let mut raw = PWSTR::null();
    ConvertSidToStringSidW(sid, &mut raw).map_err(|e| EngineError::win32("ConvertSidToStringSidW", e))?;

    let s = raw.to_string().map_err(|e| EngineError::UserContextUnresolved {
        detail: format!("SID string was not valid UTF-16: {e}"),
    });
    let _ = LocalFree(HLOCAL(raw.0 as *mut c_void));
    s
}

/// Case-insensitive comparison against a fixed-size wide buffer.
fn process_name_is(buf: &[u16; 260], want: &str) -> bool {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..len]);
    name.eq_ignore_ascii_case(want)
}

/// Closes a handle on drop, so every early return above is leak-free.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_sid_is_a_well_formed_sid_and_has_a_profile_folder() {
        let sid = current_process_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        let dir = profile_dir_for_sid(&sid)
            .unwrap()
            .expect("runner user has a ProfileList entry");
        assert!(dir.is_dir(), "{dir:?}");
    }

    #[test]
    fn user_resolution_finds_a_user_in_our_own_session() {
        let u = detect_user().unwrap();
        assert!(u.sid.starts_with("S-1-5-"));
        // On a headless CI runner there may be no explorer.exe; either path is valid.
        println!("resolved via {:?}, is_self={}", u.resolution, u.is_self);
    }

    #[test]
    fn the_test_process_is_elevated_on_ci() {
        // GitHub-hosted Windows runners run as an elevated administrator.
        if std::env::var_os("GITHUB_ACTIONS").is_some() {
            assert!(is_elevated());
        }
    }
}
