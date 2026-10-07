//! Empty Windows' standby list (catalogue E6, ExitLag's "RAM Cleaner").
//!
//! Windows keeps file data it read recently in memory that nothing else is
//! using (the standby list), and hands those pages to a program the moment it
//! asks for memory. Emptying the list moves them to the free list now; Windows
//! fills it again as files are read. No setting changes and nothing is written,
//! so there is nothing to undo and no restore point is needed; the journal
//! keeps a line for the history (`StandbyPurge::done`).
//!
//! Only the standby list. ExitLag also trims every program's working set,
//! which reaches into game processes (plan section 12), so that is left out
//! (NOTES N69).
//!
//! `NtSetSystemInformation` with `SystemMemoryListInformation` (80) and the
//! command `MemoryPurgeStandbyList` (4) is undocumented (Sysinternals RAMMap's
//! "Empty Standby List" makes this call). Checked 2026-10-07 against System
//! Informer's `phnt/ntexapi.h` (class 80, command 4, the signature, and that it
//! needs `SeProfileSingleProcessPrivilege`) and on Windows Server 2025 by the
//! Windows test below in CI run 37571551831: files kept in memory went from
//! 4723 to 197 MiB.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::{EngineError, Result};
use super::journal::ActionDone;

/// Physical memory as Windows reports it (`GetPerformanceInfo`), in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MemoryUse {
    pub total_bytes: u64,
    /// What programs can be given at once: free memory plus the standby list.
    pub available_bytes: u64,
    /// The standby list plus the system's own working set (`SystemCache`).
    pub cached_bytes: u64,
}

/// One emptying of the standby list, with memory use just before and after.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StandbyPurge {
    pub before: MemoryUse,
    pub after: MemoryUse,
    pub unix_ms: u64,
}

impl StandbyPurge {
    /// The line the journal keeps for the history.
    pub fn done(&self) -> ActionDone {
        ActionDone::PurgeStandby {
            cached_before: self.before.cached_bytes,
            cached_after: self.after.cached_bytes,
        }
    }
}

/// What emptying the standby list needs from Windows.
pub trait MemoryLists: Send + Sync {
    fn usage(&self) -> Result<MemoryUse>;
    fn purge_standby(&self) -> Result<()>;
}

/// Empty the standby list, reading memory use just before and after.
pub fn purge_standby(lists: &dyn MemoryLists, unix_ms: u64) -> Result<StandbyPurge> {
    let before = lists.usage()?;
    lists.purge_standby()?;
    let after = lists.usage()?;
    Ok(StandbyPurge { before, after, unix_ms })
}

/// The real thing on Windows; elsewhere, a refusal.
pub fn system() -> Box<dyn MemoryLists> {
    #[cfg(windows)]
    {
        Box::new(imp::WinMemory)
    }
    #[cfg(not(windows))]
    {
        Box::new(Unavailable)
    }
}

/// Builds that are not Windows have no standby list to empty.
pub struct Unavailable;

impl MemoryLists for Unavailable {
    fn usage(&self) -> Result<MemoryUse> {
        Err(EngineError::Internal {
            detail: "memory lists exist only on Windows".into(),
        })
    }
    fn purge_standby(&self) -> Result<()> {
        self.usage().map(drop)
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use windows::core::{s, w, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED, HANDLE, LUID, NTSTATUS};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use super::{MemoryLists, MemoryUse};
    use crate::error::{EngineError, Result};

    /// `SYSTEM_INFORMATION_CLASS::SystemMemoryListInformation` (phnt).
    const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 80;
    /// `SYSTEM_MEMORY_LIST_COMMAND::MemoryPurgeStandbyList` (phnt).
    const MEMORY_PURGE_STANDBY_LIST: i32 = 4;

    type NtSetSystemInformation = unsafe extern "system" fn(i32, *mut c_void, u32) -> NTSTATUS;

    pub struct WinMemory;

    struct Token(HANDLE);

    impl Drop for Token {
        fn drop(&mut self) {
            // SAFETY: a handle we opened and have not closed.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// Windows only empties the list for a caller holding this privilege,
    /// which administrators have but is off until asked for.
    fn enable_profile_privilege() -> Result<()> {
        // SAFETY: plain Win32 calls with valid out-pointers; the token is
        // closed by `Token`.
        unsafe {
            let mut handle = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut handle)
                .map_err(|e| EngineError::win32("OpenProcessToken", e))?;
            let token = Token(handle);
            let mut luid = LUID::default();
            LookupPrivilegeValueW(PCWSTR::null(), w!("SeProfileSingleProcessPrivilege"), &mut luid)
                .map_err(|e| EngineError::win32("LookupPrivilegeValueW", e))?;
            let wanted = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            AdjustTokenPrivileges(token.0, false, Some(std::ptr::from_ref(&wanted)), 0, None, None)
                .map_err(|e| EngineError::win32("AdjustTokenPrivileges", e))?;
            // It reports success even when the account lacks the privilege.
            if GetLastError() == ERROR_NOT_ALL_ASSIGNED {
                return Err(EngineError::Win32 {
                    call: "AdjustTokenPrivileges".into(),
                    code: ERROR_NOT_ALL_ASSIGNED.0,
                    detail: "this account does not hold the privilege to empty the standby list".into(),
                });
            }
        }
        Ok(())
    }

    impl MemoryLists for WinMemory {
        fn usage(&self) -> Result<MemoryUse> {
            let mut info = PERFORMANCE_INFORMATION {
                cb: std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32,
                ..Default::default()
            };
            let size = info.cb;
            // SAFETY: valid out-pointer and its size.
            unsafe { GetPerformanceInfo(&mut info, size) }.map_err(|e| EngineError::win32("GetPerformanceInfo", e))?;
            let page = info.PageSize as u64;
            Ok(MemoryUse {
                total_bytes: info.PhysicalTotal as u64 * page,
                available_bytes: info.PhysicalAvailable as u64 * page,
                cached_bytes: info.SystemCache as u64 * page,
            })
        }

        fn purge_standby(&self) -> Result<()> {
            enable_profile_privilege()?;
            // SAFETY: ntdll is always loaded; the pointer is transmuted to the
            // function's signature in phnt (see the note at the top), and the
            // command is a 4-byte value that outlives the call.
            unsafe {
                let ntdll = GetModuleHandleW(w!("ntdll.dll")).map_err(|e| EngineError::win32("GetModuleHandleW", e))?;
                let f = GetProcAddress(ntdll, s!("NtSetSystemInformation")).ok_or_else(|| EngineError::Internal {
                    detail: "ntdll.dll does not export NtSetSystemInformation".into(),
                })?;
                let set = std::mem::transmute::<unsafe extern "system" fn() -> isize, NtSetSystemInformation>(f);
                let mut command = MEMORY_PURGE_STANDBY_LIST;
                let status = set(
                    SYSTEM_MEMORY_LIST_INFORMATION,
                    std::ptr::from_mut(&mut command).cast(),
                    std::mem::size_of::<i32>() as u32,
                );
                if status.is_err() {
                    return Err(EngineError::Win32 {
                        call: "NtSetSystemInformation".into(),
                        code: status.0 as u32,
                        detail: format!(
                            "Windows refused to empty the standby list (NTSTATUS 0x{:08X})",
                            status.0
                        ),
                    });
                }
            }
            Ok(())
        }
    }

    /// Runs the real call. Needs an elevated process (CI runners are); prints
    /// the numbers for the evidence log.
    #[test]
    fn the_standby_list_empties_on_this_pc() {
        if !crate::identity::is_elevated() {
            eprintln!("SKIPPED: not elevated, so the standby list cannot be emptied here");
            return;
        }
        let out = super::purge_standby(&WinMemory, 0).expect("emptying the standby list");
        eprintln!(
            "standby purge: cached {} -> {} MiB, available {} -> {} MiB, total {} MiB",
            out.before.cached_bytes >> 20,
            out.after.cached_bytes >> 20,
            out.before.available_bytes >> 20,
            out.after.available_bytes >> 20,
            out.after.total_bytes >> 20
        );
        assert!(out.before.total_bytes > 0 && out.before.total_bytes == out.after.total_bytes);
        assert!(out.after.available_bytes <= out.after.total_bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Records the order of calls; the standby list shrinks on purge.
    struct Fake {
        calls: Mutex<Vec<&'static str>>,
        cached: Mutex<u64>,
        refuse: bool,
    }

    impl MemoryLists for Fake {
        fn usage(&self) -> Result<MemoryUse> {
            self.calls.lock().unwrap().push("usage");
            Ok(MemoryUse {
                total_bytes: 16 << 30,
                available_bytes: 10 << 30,
                cached_bytes: *self.cached.lock().unwrap(),
            })
        }
        fn purge_standby(&self) -> Result<()> {
            self.calls.lock().unwrap().push("purge");
            if self.refuse {
                return Err(EngineError::Internal { detail: "no".into() });
            }
            *self.cached.lock().unwrap() = 1 << 30;
            Ok(())
        }
    }

    fn fake(refuse: bool) -> Fake {
        Fake {
            calls: Mutex::new(Vec::new()),
            cached: Mutex::new(6 << 30),
            refuse,
        }
    }

    #[test]
    fn memory_is_read_just_before_and_just_after_the_purge() {
        let f = fake(false);
        let out = purge_standby(&f, 7).unwrap();
        assert_eq!(*f.calls.lock().unwrap(), ["usage", "purge", "usage"]);
        assert_eq!((out.before.cached_bytes, out.after.cached_bytes), (6 << 30, 1 << 30));
        assert_eq!(out.unix_ms, 7);
    }

    #[test]
    fn a_refused_purge_is_an_error_not_a_result() {
        let f = fake(true);
        assert!(purge_standby(&f, 0).is_err());
        assert_eq!(*f.calls.lock().unwrap(), ["usage", "purge"]);
    }

    #[cfg(not(windows))]
    #[test]
    fn other_systems_refuse_plainly() {
        assert!(purge_standby(system().as_ref(), 0).is_err());
    }
}
