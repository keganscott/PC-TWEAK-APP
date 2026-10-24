//! The real System Restore operations on Windows.
//!
//! Creating a restore point tries `SRSetRestorePointW` first, then PowerShell's
//! `Checkpoint-Computer`. The API lives in `srclient.dll` on client Windows (the
//! `windows` crate's metadata names `sfc.dll`, which forwards to it). It is
//! loaded at run time, from System32 only, because it does not exist on Windows
//! Server and a normal import would stop the whole app from starting there.

use std::sync::Arc;
use std::time::Duration;

use windows::core::{s, w, PCWSTR};
use windows::Win32::Foundation::{BOOL, HMODULE, WIN32_ERROR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows::Win32::System::Restore::{
    BEGIN_SYSTEM_CHANGE, END_SYSTEM_CHANGE, MODIFY_SETTINGS, RESTOREPOINTINFOW, STATEMGRSTATUS,
};

use super::error::{EngineError, Result};
use super::journal::RestoreMethod;
use super::restore::{list_points_wmi, CreatedVia, RestoreOps, RestorePoint};
use super::shell::run_powershell_checked;
use super::wmi::WmiSource;

/// Fixed scripts. The description here must equal `RESTORE_DESCRIPTION` (tested).
const ENABLE_SCRIPT: &str = r#"Enable-ComputerRestore -Drive "$env:SystemDrive\" -ErrorAction Stop"#;
const CREATE_SCRIPT: &str = "Checkpoint-Computer -Description 'PeakTweaks: before changes' \
     -RestorePointType 'MODIFY_SETTINGS' -ErrorAction Stop";

const ENABLE_TIMEOUT: Duration = Duration::from_secs(60);
const CREATE_TIMEOUT: Duration = Duration::from_secs(180);

type SrSetRestorePointW = unsafe extern "system" fn(*const RESTOREPOINTINFOW, *mut STATEMGRSTATUS) -> BOOL;

pub struct WindowsRestoreOps {
    wmi: Arc<dyn WmiSource>,
}

impl WindowsRestoreOps {
    pub fn new(wmi: Arc<dyn WmiSource>) -> Self {
        Self { wmi }
    }
}

impl RestoreOps for WindowsRestoreOps {
    fn list_points(&self) -> Result<Vec<RestorePoint>> {
        list_points_wmi(self.wmi.as_ref())
    }

    fn enable_protection(&self) -> Result<()> {
        run_powershell_checked("Enable-ComputerRestore", ENABLE_SCRIPT, ENABLE_TIMEOUT).map(|_| ())
    }

    fn create_point(&self, description: &str) -> Result<CreatedVia> {
        let api_error = match create_via_api(description) {
            Ok(seq) => {
                return Ok(CreatedVia {
                    method: RestoreMethod::Api,
                    sequence_number: Some(seq),
                })
            }
            Err(e) => e,
        };
        match run_powershell_checked("Checkpoint-Computer", CREATE_SCRIPT, CREATE_TIMEOUT) {
            Ok(_) => Ok(CreatedVia {
                method: RestoreMethod::PowerShell,
                sequence_number: None,
            }),
            Err(EngineError::Command {
                what,
                exit_code,
                detail,
            }) => Err(EngineError::Command {
                what,
                exit_code,
                detail: format!("{detail} (the SRSetRestorePointW call also failed: {api_error})"),
            }),
            Err(other) => Err(other),
        }
    }
}

fn utf16_description(description: &str) -> [u16; 256] {
    let mut buf = [0u16; 256];
    for (slot, unit) in buf.iter_mut().take(255).zip(description.encode_utf16()) {
        *slot = unit;
    }
    buf
}

/// Begin and end a system change around a `MODIFY_SETTINGS` restore point.
fn create_via_api(description: &str) -> std::result::Result<u32, String> {
    unsafe {
        let mut lib = None;
        for (label, name) in [("srclient.dll", w!("srclient.dll")), ("sfc.dll", w!("sfc.dll"))] {
            if let Some(h) = load_system32(name) {
                if let Some(f) = GetProcAddress(h, s!("SRSetRestorePointW")) {
                    let f = std::mem::transmute::<unsafe extern "system" fn() -> isize, SrSetRestorePointW>(f);
                    lib = Some((label, f));
                    break;
                }
            }
        }
        let (dll, call) = lib.ok_or("SRSetRestorePointW is not exported by srclient.dll or sfc.dll")?;

        let mut status = STATEMGRSTATUS {
            nStatus: WIN32_ERROR(0),
            llSequenceNumber: 0,
        };
        let begin = RESTOREPOINTINFOW {
            dwEventType: BEGIN_SYSTEM_CHANGE,
            dwRestorePtType: MODIFY_SETTINGS,
            llSequenceNumber: 0,
            szDescription: utf16_description(description),
        };
        if !call(&begin, &mut status).as_bool() {
            let code = status.nStatus.0;
            return Err(format!(
                "{dll}!SRSetRestorePointW(BEGIN) failed with Win32 error {code}"
            ));
        }
        let seq = status.llSequenceNumber;

        let end = RESTOREPOINTINFOW {
            dwEventType: END_SYSTEM_CHANGE,
            dwRestorePtType: MODIFY_SETTINGS,
            llSequenceNumber: seq,
            szDescription: utf16_description(description),
        };
        let mut end_status = STATEMGRSTATUS {
            nStatus: WIN32_ERROR(0),
            llSequenceNumber: 0,
        };
        if !call(&end, &mut end_status).as_bool() {
            let code = end_status.nStatus.0;
            return Err(format!("{dll}!SRSetRestorePointW(END) failed with Win32 error {code}"));
        }
        u32::try_from(seq).map_err(|_| format!("Windows returned the out-of-range sequence number {seq}"))
    }
}

/// `LoadLibraryExW` restricted to System32, so a DLL of the same name dropped
/// next to the exe or on `PATH` is never loaded into an elevated process.
unsafe fn load_system32(name: PCWSTR) -> Option<HMODULE> {
    LoadLibraryExW(name, None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::restore::RESTORE_DESCRIPTION;
    use crate::wmi::WmiWorker;

    #[test]
    fn the_powershell_description_matches_the_constant_the_verifier_looks_for() {
        assert!(CREATE_SCRIPT.contains(&format!("'{RESTORE_DESCRIPTION}'")));
    }

    #[test]
    fn the_description_buffer_is_nul_terminated_and_truncated() {
        let d = utf16_description("abc");
        assert_eq!(&d[..4], &[97, 98, 99, 0]);
        let long = utf16_description(&"x".repeat(1000));
        assert_eq!(long[255], 0);
        assert_eq!(long[254], 'x' as u16);
    }

    /// On GitHub's Windows Server runners System Restore does not exist, so this
    /// records what the real code does there. It must return promptly and never
    /// panic or hang; on a client machine it may even succeed, which is fine.
    #[test]
    fn the_real_operations_fail_cleanly_or_succeed_but_never_hang() {
        let ops = WindowsRestoreOps::new(Arc::new(WmiWorker::start()));
        let list = ops.list_points();
        println!("list_points: {list:?}");
        // Do not create or enable anything on a developer's real machine when
        // this test is run locally: only exercise those on CI.
        if std::env::var_os("GITHUB_ACTIONS").is_some() {
            println!("enable_protection: {:?}", ops.enable_protection());
            println!("create_point: {:?}", ops.create_point(RESTORE_DESCRIPTION));
        }
    }
}
