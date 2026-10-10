//! The graphics driver tool (Kegan, 2026-10-10: "a tool that allows you to
//! uninstall your graphics driver and install one that you select"), built
//! inside plan section 1's rule "Never host or redistribute GPU drivers. Link
//! to vendor pages only" (the recommended option on the 2026-10-10 decision
//! card). PeakTweaks downloads nothing and opens no connection:
//!
//! - The card maker's own driver page opens in the user's browser through the
//!   desktop shell, as the signed-in user (`launch::open_unelevated`). The
//!   address is one of the fixed ones below, never text from the UI.
//! - A driver file the user downloaded from NVIDIA is installed as a clean
//!   install (the old driver and its settings removed first). The file is
//!   picked in Windows' own Open dialog, so its path never comes from the UI;
//!   it is copied into the protected data folder, and only that copy, which no
//!   other account can change, is checked and run. It runs only when its
//!   Authenticode signature checks out offline (no revocation lookups, so no
//!   connection) and names NVIDIA as the signer. The restore gate and the
//!   journal line before it starts are the engine's
//!   (`Engine::begin_gpu_driver_install`); System Restore puts the old driver
//!   back.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::{EngineError, Result};
use super::journal::ActionDone;

/// What the errors name, so the UI words them (`src/lib/errors.ts`).
pub const WHAT: &str = "NVIDIA driver install";
/// What a failure to open a driver page names.
pub const PAGE_WHAT: &str = "Driver page";

/// The signer an NVIDIA driver package must carry: the simple display name of
/// its signing certificate. VERIFY on a real GeForce package (NOTES N119).
pub const NVIDIA_SIGNER: &str = "NVIDIA Corporation";

/// NVIDIA's installer switches for a silent clean install without a restart
/// and without the licence page. VERIFY: not in NVIDIA's own GeForce
/// documentation; reported working by users on NVIDIA's developer forum
/// ("Nvidia Geforce GameReady/Studio Driver Silent deployment Parameters that
/// exclude Nvidia App": "-s -clean -noreboot -noeula"). NVIDIA's data-centre
/// guide documents `-s` and `-n` for `setup.exe` (NOTES N119).
pub const CLEAN_INSTALL_ARGS: [&str; 4] = ["-s", "-clean", "-noreboot", "-noeula"];

/// How long the installer may take before it is stopped.
pub const INSTALL_LIMIT: Duration = Duration::from_secs(60 * 60);

/// A driver package larger than this is refused rather than copied.
pub const MAX_BYTES: u64 = 4 << 30;

/// The name the copy gets in the protected folder, whatever the file was called.
const STAGED_NAME: &str = "nvidia-driver-setup.exe";

/// A graphics card maker whose driver page PeakTweaks can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DriverVendor {
    Nvidia,
    Amd,
    Intel,
}

impl DriverVendor {
    /// The maker's own driver download page.
    ///
    /// - NVIDIA: the page `nvidia.com/Download/Find.aspx` now leads to
    ///   ("Download The Official NVIDIA Drivers", Manual Driver Search, seen
    ///   2026-10-10). NVIDIA's driver help (`nvidia.com/en-gb/drivers/drivers-faq`)
    ///   sends older drivers to "Beta and Archived Drivers" from there.
    /// - AMD: "Drivers and Support for Processors and Graphics" (seen 2026-10-10).
    /// - Intel: "Intel Driver & Support Assistant", which also lists downloads
    ///   (seen 2026-10-10).
    pub fn page(self) -> &'static str {
        match self {
            Self::Nvidia => "https://www.nvidia.com/en-us/drivers/",
            Self::Amd => "https://www.amd.com/en/support/download/drivers.html",
            Self::Intel => "https://www.intel.com/content/www/us/en/support/detect.html",
        }
    }
}

/// Open the maker's driver page in the user's browser, without PeakTweaks'
/// administrator rights.
pub fn open_page(vendor: DriverVendor) -> Result<()> {
    super::launch::open_unelevated(vendor.page()).map_err(|e| match e {
        EngineError::Command { exit_code, detail, .. } => EngineError::Command {
            what: PAGE_WHAT.into(),
            exit_code,
            detail,
        },
        other => other,
    })
}

/// One clean install that ran to the end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GpuDriverInstall {
    /// The file's name as downloaded.
    pub file: String,
    /// NVIDIA's number from the file's name (`566.36-desktop-...exe`), when it has one.
    pub version: Option<String>,
    /// The installer asked for a restart to finish.
    pub restart: bool,
    pub unix_ms: u64,
    pub seconds: u64,
}

impl GpuDriverInstall {
    /// The line the journal keeps for the history.
    pub fn done(&self) -> ActionDone {
        ActionDone::GpuDriverInstalled {
            file: self.file.clone(),
            version: self.version.clone(),
            restart: self.restart,
        }
    }
}

/// A checked copy of the driver package in the protected folder, ready to run.
#[derive(Debug)]
pub struct Staged {
    pub path: PathBuf,
    pub file: String,
    pub version: Option<String>,
}

impl Staged {
    /// Delete the copy (it is the size of a driver package). A copy left
    /// behind is replaced by the next install.
    pub fn discard(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// What the install needs from Windows: the real one on Windows.
pub trait DriverSystem: Send + Sync {
    /// The signer's name when the file's Authenticode signature checks out;
    /// otherwise why not.
    fn signer(&self, path: &Path) -> Result<String>;
    /// Run the installer and wait for it: its exit code, `None` if it had none.
    fn run(&self, path: &Path, args: &[&str], limit: Duration) -> Result<Option<i32>>;
}

pub(crate) fn refused(detail: impl Into<String>) -> EngineError {
    EngineError::Command {
        what: WHAT.into(),
        exit_code: None,
        detail: detail.into(),
    }
}

/// NVIDIA's driver number from the start of a package's file name, the way
/// NVIDIA names its downloads: `581.80-desktop-win10-win11-64bit-international-dch-whql.exe`.
pub fn version_from_file_name(name: &str) -> Option<String> {
    let lead: String = name.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let (major, minor) = lead.split_once('.')?;
    let ok = (2..=4).contains(&major.len()) && minor.len() == 2 && minor.chars().all(|c| c.is_ascii_digit());
    ok.then(|| format!("{major}.{minor}"))
}

/// Only NVIDIA's own signature is accepted.
pub fn check_signer(signer: &str) -> Result<()> {
    if signer.trim() == NVIDIA_SIGNER {
        Ok(())
    } else {
        Err(refused(format!(
            "the file is signed by {}, not {NVIDIA_SIGNER}; PeakTweaks only installs NVIDIA's own driver packages",
            if signer.trim().is_empty() {
                "no one it can name"
            } else {
                signer.trim()
            }
        )))
    }
}

/// The installer's exit code: done (and whether a restart is needed), or why
/// not. 0 and 1 as NVIDIA's data-centre guide gives them for `setup.exe`
/// (installed; installed, restart needed). VERIFY for GeForce packages.
pub fn exit_meaning(code: Option<i32>) -> Result<bool> {
    match code {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        Some(c) => Err(EngineError::Command {
            what: WHAT.into(),
            exit_code: Some(c),
            detail: format!("NVIDIA's installer stopped with code {c}; nothing was reported installed"),
        }),
        None => Err(refused("NVIDIA's installer ended without an exit code")),
    }
}

/// Copy `src` into `dir\drivers` under a fixed name and check the copy is an
/// NVIDIA-signed package. The original is only read.
pub fn prepare(sys: &dyn DriverSystem, src: &Path, dir: &Path) -> Result<Staged> {
    let storage = |path: &Path, e: std::io::Error| EngineError::Storage {
        path: path.display().to_string(),
        detail: e.to_string(),
    };
    let meta = std::fs::metadata(src).map_err(|e| storage(src, e))?;
    if !meta.is_file() {
        return Err(refused(format!("{} is not a file", src.display())));
    }
    if meta.len() > MAX_BYTES {
        return Err(refused(format!(
            "{} is {} bytes, more than a driver package",
            src.display(),
            meta.len()
        )));
    }
    let file = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !file.to_ascii_lowercase().ends_with(".exe") {
        return Err(refused(format!("{file} is not a program (.exe)")));
    }

    let folder = dir.join("drivers");
    super::fsutil::create_dir_durable(&folder)?;
    let path = folder.join(STAGED_NAME);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(storage(&path, e)),
    }
    let staged = Staged {
        path,
        version: version_from_file_name(&file),
        file,
    };
    if let Err(e) = std::fs::copy(src, &staged.path) {
        staged.discard();
        return Err(storage(&staged.path, e));
    }
    if let Err(e) = sys.signer(&staged.path).and_then(|s| check_signer(&s)) {
        staged.discard();
        return Err(e);
    }
    Ok(staged)
}

/// Run NVIDIA's installer from the checked copy as a clean install, then
/// delete the copy.
pub fn install(sys: &dyn DriverSystem, staged: Staged, unix_ms: u64) -> Result<GpuDriverInstall> {
    let started = Instant::now();
    let code = sys.run(&staged.path, &CLEAN_INSTALL_ARGS, INSTALL_LIMIT);
    staged.discard();
    let restart = exit_meaning(code?)?;
    Ok(GpuDriverInstall {
        file: staged.file,
        version: staged.version,
        restart,
        unix_ms,
        seconds: started.elapsed().as_secs(),
    })
}

/// The real Windows calls.
#[cfg(windows)]
pub fn system() -> Box<dyn DriverSystem> {
    Box::new(win::Windows)
}

/// Windows' own Open dialog, owned by the app's window, for the file to
/// install. `None` when the user cancels.
#[cfg(windows)]
pub fn choose_file(owner: isize) -> Result<Option<PathBuf>> {
    win::choose_file(owner)
}

#[cfg(not(windows))]
pub fn choose_file(_owner: isize) -> Result<Option<PathBuf>> {
    Err(refused("installing a driver needs Windows"))
}

#[cfg(not(windows))]
pub fn system() -> Box<dyn DriverSystem> {
    struct NotWindows;
    impl DriverSystem for NotWindows {
        fn signer(&self, _path: &Path) -> Result<String> {
            Err(refused("checking a signature needs Windows"))
        }
        fn run(&self, _path: &Path, _args: &[&str], _limit: Duration) -> Result<Option<i32>> {
            Err(refused("installing a driver needs Windows"))
        }
    }
    Box::new(NotWindows)
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{ERROR_CANCELLED, HANDLE, HWND};
    use windows::Win32::Security::Cryptography::{CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE};
    use windows::Win32::Security::WinTrust::{
        WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE,
        WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, FOS_DONTADDTORECENT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR,
        FOS_PATHMUSTEXIST, SIGDN_FILESYSPATH,
    };

    use super::{refused, DriverSystem, Result};

    pub struct Windows;

    // winerror.h
    const TRUST_E_NOSIGNATURE: u32 = 0x800B_0100;
    const TRUST_E_BAD_DIGEST: u32 = 0x8009_6010;
    const CERT_E_UNTRUSTEDROOT: u32 = 0x800B_0109;
    const TRUST_E_EXPLICIT_DISTRUST: u32 = 0x800B_0111;

    fn why_untrusted(status: i32) -> String {
        match status as u32 {
            TRUST_E_NOSIGNATURE => "the file is not signed".into(),
            TRUST_E_BAD_DIGEST => "the file was changed after it was signed".into(),
            CERT_E_UNTRUSTEDROOT => "the file's signature does not lead to a certificate Windows trusts".into(),
            TRUST_E_EXPLICIT_DISTRUST => "Windows distrusts the file's signer".into(),
            other => format!("Windows did not accept the file's signature (WinVerifyTrust 0x{other:08X})"),
        }
    }

    impl DriverSystem for Windows {
        fn signer(&self, path: &Path) -> Result<String> {
            let wide = HSTRING::from(path.as_os_str());
            let mut file = WINTRUST_FILE_INFO {
                cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
                pcwszFilePath: PCWSTR(wide.as_ptr()),
                hFile: HANDLE::default(),
                pgKnownSubject: std::ptr::null_mut(),
            };
            let mut data = WINTRUST_DATA {
                cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
                dwUIChoice: WTD_UI_NONE,
                fdwRevocationChecks: WTD_REVOKE_NONE,
                dwUnionChoice: WTD_CHOICE_FILE,
                Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
                dwStateAction: WTD_STATEACTION_VERIFY,
                // Offline: no revocation lookups and no downloads while the
                // chain is built, so the check opens no connection.
                dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE,
                ..Default::default()
            };
            let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
            // INVALID_HANDLE_VALUE as the window: never any user interface.
            let no_ui = HWND(-1isize as *mut c_void);
            // SAFETY: `data` and `file` point at locals that outlive both
            // calls; the state the first call opens is closed by the second.
            unsafe {
                let status = WinVerifyTrust(no_ui, &mut action, &mut data as *mut _ as *mut c_void);
                let out = if status == 0 {
                    signer_name(&data)
                } else {
                    Err(refused(why_untrusted(status)))
                };
                data.dwStateAction = WTD_STATEACTION_CLOSE;
                WinVerifyTrust(no_ui, &mut action, &mut data as *mut _ as *mut c_void);
                out
            }
        }

        fn run(&self, path: &Path, args: &[&str], limit: Duration) -> Result<Option<i32>> {
            let mut cmd = Command::new(path);
            cmd.args(args);
            let out = crate::proc::run_limited(cmd, super::WHAT, limit, Duration::from_secs(1))?;
            Ok(out.exit_code)
        }
    }

    /// The signer's simple display name, from the state a successful
    /// `WinVerifyTrust` left open.
    ///
    /// # Safety
    /// `data` must hold the open state of a successful verify.
    unsafe fn signer_name(data: &WINTRUST_DATA) -> Result<String> {
        let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
        if provider.is_null() {
            return Err(refused("Windows kept no details of the file's signature"));
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
        if signer.is_null() {
            return Err(refused("Windows found no signer on the file"));
        }
        let cert = WTHelperGetProvCertFromChain(signer, 0);
        if cert.is_null() || (*cert).pCert.is_null() {
            return Err(refused("Windows found no certificate for the file's signer"));
        }
        let context = (*cert).pCert;
        let len = CertGetNameStringW(context, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, None);
        let mut name = vec![0u16; len.max(1) as usize];
        let written = CertGetNameStringW(context, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut name));
        let end = (written as usize).saturating_sub(1).min(name.len());
        Ok(String::from_utf16_lossy(&name[..end]))
    }

    pub fn choose_file(owner: isize) -> Result<Option<PathBuf>> {
        // Its own thread in a single-threaded apartment, as the dialog
        // expects, so the caller's COM state never matters.
        std::thread::spawn(move || {
            // SAFETY: COM is started on this new thread and stopped after the
            // dialog and every interface it handed out are dropped.
            unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                    .ok()
                    .map_err(|e| refused(format!("could not start COM: {e}")))?;
                let out = show_dialog(HWND(owner as *mut c_void));
                CoUninitialize();
                out
            }
        })
        .join()
        .unwrap_or_else(|_| Err(refused("the file dialog stopped")))
    }

    /// # Safety
    /// COM must be started on this thread.
    unsafe fn show_dialog(owner: HWND) -> Result<Option<PathBuf>> {
        let failed = |step: &str, e: windows::core::Error| refused(format!("{step}: {e}"));
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| failed("could not open Windows' file dialog", e))?;
        let types = [COMDLG_FILTERSPEC {
            pszName: w!("Driver package (*.exe)"),
            pszSpec: w!("*.exe"),
        }];
        dialog.SetFileTypes(&types).map_err(|e| failed("file dialog", e))?;
        dialog
            .SetTitle(w!("Choose the NVIDIA driver you downloaded"))
            .map_err(|e| failed("file dialog", e))?;
        dialog
            .SetOkButtonLabel(w!("Install"))
            .map_err(|e| failed("file dialog", e))?;
        let options = dialog.GetOptions().map_err(|e| failed("file dialog", e))?;
        dialog
            .SetOptions(
                options
                    | FOS_FILEMUSTEXIST
                    | FOS_PATHMUSTEXIST
                    | FOS_FORCEFILESYSTEM
                    | FOS_NOCHANGEDIR
                    | FOS_DONTADDTORECENT,
            )
            .map_err(|e| failed("file dialog", e))?;
        match dialog.Show(owner) {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => return Ok(None),
            Err(e) => return Err(failed("Windows' file dialog did not open", e)),
        }
        let item = dialog.GetResult().map_err(|e| failed("file dialog", e))?;
        let name = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|e| failed("the chosen file has no path", e))?;
        let path = name.to_string();
        CoTaskMemFree(Some(name.0 as *const c_void));
        let path = path.map_err(|e| refused(format!("the chosen file's path is not readable: {e}")))?;
        Ok(Some(PathBuf::from(path)))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A file with no signature at all is refused with a reason.
        #[test]
        fn an_unsigned_file_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("unsigned.exe");
            std::fs::write(&path, b"MZ not a real program").unwrap();
            let e = Windows.signer(&path).unwrap_err().to_string();
            println!("unsigned file: {e}");
            assert!(e.contains("not signed") || e.contains("WinVerifyTrust"), "{e}");
        }

        /// The pinned PresentMon (fetched before the tests on Windows CI) is
        /// signed by Intel: the check reads that name, and the install would
        /// refuse it as not NVIDIA's.
        #[test]
        fn a_signed_program_names_its_signer_and_is_not_nvidias() {
            let pin = crate::proof::presentmon::pinned();
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/presentmon/PresentMon-x64.exe");
            if !crate::proof::presentmon::is_pinned_file(&path, &pin) {
                println!("signer check: no verified PresentMon at {}, skipped", path.display());
                return;
            }
            let signer = Windows.signer(&path).unwrap();
            println!("signer check: PresentMon {} is signed by {signer}", pin.version);
            assert!(signer.contains(&pin.signer), "{signer} is not {}", pin.signer);
            let refused = super::super::check_signer(&signer).unwrap_err().to_string();
            println!("signer check: {refused}");
        }

        /// A copy changed after signing no longer passes.
        #[test]
        fn a_changed_signed_program_is_refused() {
            let pin = crate::proof::presentmon::pinned();
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/presentmon/PresentMon-x64.exe");
            if !crate::proof::presentmon::is_pinned_file(&path, &pin) {
                println!("tamper check: no verified PresentMon at {}, skipped", path.display());
                return;
            }
            let dir = tempfile::tempdir().unwrap();
            let copy = dir.path().join("changed.exe");
            let mut bytes = std::fs::read(&path).unwrap();
            // A byte in the middle of the program, inside what the signature covers.
            let middle = bytes.len() / 2;
            bytes[middle] ^= 0xFF;
            std::fs::write(&copy, bytes).unwrap();
            let e = Windows.signer(&copy).unwrap_err().to_string();
            println!("tamper check: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct Fake {
        signer: std::result::Result<String, String>,
        exit: Option<i32>,
        ran: Mutex<Vec<(PathBuf, Vec<String>, bool)>>,
    }

    impl Fake {
        fn signed_by(signer: &str, exit: Option<i32>) -> Self {
            Self {
                signer: Ok(signer.into()),
                exit,
                ran: Mutex::new(Vec::new()),
            }
        }
    }

    impl DriverSystem for Fake {
        fn signer(&self, path: &Path) -> Result<String> {
            assert!(path.exists(), "the copy is checked, so it must exist");
            self.signer.clone().map_err(refused)
        }
        fn run(&self, path: &Path, args: &[&str], _limit: Duration) -> Result<Option<i32>> {
            self.ran.lock().unwrap().push((
                path.to_owned(),
                args.iter().map(|a| a.to_string()).collect(),
                path.exists(),
            ));
            Ok(self.exit)
        }
    }

    fn download(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"MZ driver package").unwrap();
        path
    }

    const NAME: &str = "581.80-desktop-win10-win11-64bit-international-dch-whql.exe";

    #[test]
    fn version_comes_from_nvidia_file_names_only() {
        assert_eq!(version_from_file_name(NAME).as_deref(), Some("581.80"));
        assert_eq!(
            version_from_file_name("566.36-notebook-win10-win11-64bit-international-dch-whql.exe").as_deref(),
            Some("566.36")
        );
        assert_eq!(version_from_file_name("setup.exe"), None);
        assert_eq!(version_from_file_name("5.1-thing.exe"), None);
        assert_eq!(version_from_file_name("581.8.exe"), None);
    }

    #[test]
    fn only_nvidia_signatures_pass() {
        assert!(check_signer("NVIDIA Corporation").is_ok());
        let other = check_signer("Intel Corporation").unwrap_err().to_string();
        assert!(
            other.contains("signed by Intel Corporation, not NVIDIA Corporation"),
            "{other}"
        );
        assert!(check_signer("NVIDIA Corporation Fake").is_err());
        assert!(check_signer("").unwrap_err().to_string().contains("no one it can name"));
    }

    #[test]
    fn exit_codes() {
        assert!(!exit_meaning(Some(0)).unwrap());
        assert!(exit_meaning(Some(1)).unwrap(), "1 is installed, restart needed");
        let EngineError::Command { exit_code, detail, .. } = exit_meaning(Some(-522190823)).unwrap_err() else {
            panic!("not a command error");
        };
        assert_eq!(exit_code, Some(-522190823));
        assert!(detail.contains("stopped with code -522190823"), "{detail}");
        assert!(exit_meaning(None).is_err());
    }

    #[test]
    fn a_signed_package_is_copied_checked_run_clean_and_deleted() {
        let downloads = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let src = download(downloads.path(), NAME);
        let sys = Fake::signed_by(NVIDIA_SIGNER, Some(1));

        let staged = prepare(&sys, &src, data.path()).unwrap();
        assert_eq!(staged.path, data.path().join("drivers").join(STAGED_NAME));
        assert_eq!(staged.file, NAME);
        let done = install(&sys, staged, 1_791_331_200_000).unwrap();

        assert_eq!(done.version.as_deref(), Some("581.80"));
        assert!(done.restart);
        assert_eq!(done.unix_ms, 1_791_331_200_000);
        let ran = sys.ran.lock().unwrap();
        assert_eq!(ran.len(), 1);
        let (path, args, existed) = &ran[0];
        assert_eq!(
            path,
            &data.path().join("drivers").join(STAGED_NAME),
            "the copy runs, not the download"
        );
        assert!(existed);
        assert_eq!(args, &["-s", "-clean", "-noreboot", "-noeula"]);
        assert!(!path.exists(), "the copy is deleted afterwards");
        assert!(src.exists(), "the download is left alone");
        assert_eq!(
            done.done(),
            ActionDone::GpuDriverInstalled {
                file: NAME.into(),
                version: Some("581.80".into()),
                restart: true
            }
        );
    }

    #[test]
    fn a_package_signed_by_anyone_else_never_runs() {
        let downloads = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let src = download(downloads.path(), NAME);
        let sys = Fake::signed_by("Someone Else Ltd", Some(0));
        let e = prepare(&sys, &src, data.path()).unwrap_err().to_string();
        assert!(e.contains("signed by Someone Else Ltd"), "{e}");
        assert!(
            !data.path().join("drivers").join(STAGED_NAME).exists(),
            "the copy is deleted"
        );

        let unsigned = Fake {
            signer: Err("the file is not signed".into()),
            exit: Some(0),
            ran: Mutex::new(Vec::new()),
        };
        assert!(prepare(&unsigned, &src, data.path())
            .unwrap_err()
            .to_string()
            .contains("not signed"));
        assert!(sys.ran.lock().unwrap().is_empty() && unsigned.ran.lock().unwrap().is_empty());
    }

    #[test]
    fn not_a_program_or_not_a_file_is_refused_before_copying() {
        let downloads = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let sys = Fake::signed_by(NVIDIA_SIGNER, Some(0));
        let zip = download(downloads.path(), "driver.zip");
        assert!(prepare(&sys, &zip, data.path())
            .unwrap_err()
            .to_string()
            .contains("not a program"));
        assert!(prepare(&sys, downloads.path(), data.path())
            .unwrap_err()
            .to_string()
            .contains("is not a file"));
        assert!(prepare(&sys, &downloads.path().join("missing.exe"), data.path()).is_err());
        assert!(!data.path().join("drivers").join(STAGED_NAME).exists());
    }

    #[test]
    fn a_failed_install_still_deletes_the_copy() {
        let downloads = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let src = download(downloads.path(), NAME);
        let sys = Fake::signed_by(NVIDIA_SIGNER, Some(7));
        let staged = prepare(&sys, &src, data.path()).unwrap();
        let path = staged.path.clone();
        let e = install(&sys, staged, 0).unwrap_err();
        assert!(matches!(e, EngineError::Command { exit_code: Some(7), .. }), "{e:?}");
        assert!(!path.exists());
    }

    #[test]
    fn driver_pages_are_the_makers_own() {
        assert!(DriverVendor::Nvidia.page().starts_with("https://www.nvidia.com/"));
        assert!(DriverVendor::Amd.page().starts_with("https://www.amd.com/"));
        assert!(DriverVendor::Intel.page().starts_with("https://www.intel.com/"));
    }
}
