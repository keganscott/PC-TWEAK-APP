//! The directory holding the journal and `.reg` backups.
//!
//! The journal is replayed as administrator, so whoever can write it can make
//! us write the registry. `TrustedDir` is a witness type: the only way to get
//! one outside tests is to create or verify `%ProgramData%\PeakTweaks` with a
//! protected DACL (see `imp`), and `Journal::open` takes one.
//!
//! On every start we verify, and refuse if anything is off:
//!   * the directory is not a reparse point (junction or symlink),
//!   * its owner is SYSTEM or Administrators, which defeats a standard user
//!     pre-creating the directory to squat on it,
//!   * its DACL is not NULL and grants nothing that lets a principal other than
//!     SYSTEM or Administrators write, delete, or change permissions,
//!   * the same holds for an existing `journal.jsonl`.
//!
//! We never adopt or repair a directory that fails: files planted in it could
//! be journal lines, and fixing the DACL would not remove them.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct TrustedDir(PathBuf);

impl TrustedDir {
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Tests only: wrap any directory without checking permissions.
    #[cfg(any(test, feature = "test-support"))]
    pub fn insecure_for_tests(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }
}

#[cfg(windows)]
pub use imp::DIR_NAME;

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{LocalFree, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION,
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateDirectoryW, GetFileAttributesW, FILE_ATTRIBUTE_REPARSE_POINT, INVALID_FILE_ATTRIBUTES,
    };
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

    use super::TrustedDir;
    use crate::error::{EngineError, Result};
    use crate::identity::sid_to_string;
    use crate::journal::JOURNAL_FILE;

    pub const DIR_NAME: &str = "PeakTweaks";

    /// Owner Administrators; protected (no inherited ACEs) and auto-inherited to
    /// children; SYSTEM and Administrators full control, object + container
    /// inherit. Nothing for Users.
    const PROTECTED_SDDL: PCWSTR = w!("O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");

    const SID_SYSTEM: &str = "S-1-5-18";
    const SID_ADMINISTRATORS: &str = "S-1-5-32-544";

    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const ACCESS_DENIED_ACE_TYPE: u8 = 1;

    // Access-mask bits that let a principal alter the directory or its files.
    const FILE_WRITE_DATA: u32 = 0x0000_0002; // also "add file"
    const FILE_APPEND_DATA: u32 = 0x0000_0004; // also "add subdirectory"
    const FILE_WRITE_EA: u32 = 0x0000_0010;
    const FILE_DELETE_CHILD: u32 = 0x0000_0040;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0000_0100;
    const DELETE: u32 = 0x0001_0000;
    const WRITE_DAC: u32 = 0x0004_0000;
    const WRITE_OWNER: u32 = 0x0008_0000;
    const GENERIC_ALL: u32 = 0x1000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const WRITE_LIKE: u32 = FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | FILE_DELETE_CHILD
        | FILE_WRITE_ATTRIBUTES
        | DELETE
        | WRITE_DAC
        | WRITE_OWNER
        | GENERIC_ALL
        | GENERIC_WRITE;

    impl TrustedDir {
        /// `%ProgramData%\PeakTweaks`, created with a protected DACL if absent,
        /// verified either way.
        pub fn ensure_program_data() -> Result<Self> {
            Self::ensure_at(&program_data()?.join(DIR_NAME))
        }

        pub(crate) fn ensure_at(path: &Path) -> Result<Self> {
            match create_protected(path)? {
                Created::Yes | Created::AlreadyThere => {}
            }
            verify_secure(path)?;
            let journal = path.join(JOURNAL_FILE);
            if journal.exists() {
                verify_secure(&journal)?;
            }
            Ok(Self(path.to_path_buf()))
        }
    }

    enum Created {
        Yes,
        AlreadyThere,
    }

    fn insecure(path: &Path, detail: impl Into<String>) -> EngineError {
        EngineError::InsecureStorage {
            path: path.display().to_string(),
            detail: detail.into(),
        }
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    fn program_data() -> Result<PathBuf> {
        unsafe {
            let p = SHGetKnownFolderPath(&FOLDERID_ProgramData, KF_FLAG_DEFAULT, None)
                .map_err(|e| EngineError::win32("SHGetKnownFolderPath", e))?;
            let path = PathBuf::from(std::ffi::OsString::from_wide(p.as_wide()));
            CoTaskMemFree(Some(p.0 as *const c_void));
            Ok(path)
        }
    }

    fn create_protected(path: &Path) -> Result<Created> {
        unsafe {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PROTECTED_SDDL, SDDL_REVISION_1, &mut sd, None)
                .map_err(|e| EngineError::win32("ConvertStringSecurityDescriptorToSecurityDescriptorW", e))?;
            let _free = LocalFreeGuard(sd.0);

            let attrs = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            let w = wide(path);
            match CreateDirectoryW(PCWSTR(w.as_ptr()), Some(&attrs)) {
                Ok(()) => Ok(Created::Yes),
                Err(e) if e.code() == ERROR_ALREADY_EXISTS.to_hresult() => Ok(Created::AlreadyThere),
                Err(e) => Err(EngineError::win32("CreateDirectoryW", e)),
            }
        }
    }

    struct LocalFreeGuard(*mut c_void);
    impl Drop for LocalFreeGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = LocalFree(HLOCAL(self.0));
                }
            }
        }
    }

    /// Fail unless `path` (a directory or a file) is owned by, and writable
    /// only by, SYSTEM and Administrators.
    pub(crate) fn verify_secure(path: &Path) -> Result<()> {
        unsafe {
            let w = wide(path);
            let name = PCWSTR(w.as_ptr());

            let attrs = GetFileAttributesW(name);
            if attrs == INVALID_FILE_ATTRIBUTES {
                return Err(insecure(path, "cannot read file attributes"));
            }
            if attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
                return Err(insecure(path, "is a reparse point (junction or symlink)"));
            }

            let mut owner = PSID::default();
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut sd = PSECURITY_DESCRIPTOR::default();
            let rc = GetNamedSecurityInfoW(
                name,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                &mut sd,
            );
            if rc != ERROR_SUCCESS {
                return Err(EngineError::Win32 {
                    call: "GetNamedSecurityInfoW".into(),
                    code: rc.0,
                    detail: "could not read owner and DACL".into(),
                });
            }
            let _free = LocalFreeGuard(sd.0);

            let owner_sid = sid_to_string(owner)?;
            if owner_sid != SID_SYSTEM && owner_sid != SID_ADMINISTRATORS {
                return Err(insecure(
                    path,
                    format!("owned by {owner_sid}, not SYSTEM or Administrators; delete it or take ownership as an administrator"),
                ));
            }
            if dacl.is_null() {
                return Err(insecure(path, "has a NULL DACL, which grants everyone full control"));
            }

            let mut info = ACL_SIZE_INFORMATION::default();
            GetAclInformation(
                dacl,
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
            .map_err(|e| EngineError::win32("GetAclInformation", e))?;

            for i in 0..info.AceCount {
                let mut ace: *mut c_void = std::ptr::null_mut();
                GetAce(dacl, i, &mut ace).map_err(|e| EngineError::win32("GetAce", e))?;
                let header = &*(ace as *const ACE_HEADER);
                match header.AceType {
                    ACCESS_DENIED_ACE_TYPE => continue,
                    ACCESS_ALLOWED_ACE_TYPE => {
                        let a = &*(ace as *const ACCESS_ALLOWED_ACE);
                        let sid = PSID(&a.SidStart as *const u32 as *mut c_void);
                        let who = sid_to_string(sid)?;
                        if who != SID_SYSTEM && who != SID_ADMINISTRATORS && a.Mask & WRITE_LIKE != 0 {
                            return Err(insecure(
                                path,
                                format!("{who} is granted write access (mask 0x{:08x})", a.Mask),
                            ));
                        }
                    }
                    other => {
                        return Err(insecure(path, format!("has an ACE of unexpected type {other}")));
                    }
                }
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::journal::Journal;

        fn scratch() -> PathBuf {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            program_data()
                .unwrap()
                .join(format!("PeakTweaksTest-{}-{nanos}", std::process::id()))
        }

        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn creates_a_protected_directory_and_reverifies_it() {
            let path = scratch();
            let _c = Cleanup(path.clone());
            let dir = TrustedDir::ensure_at(&path).expect("create and verify");
            TrustedDir::ensure_at(&path).expect("second start verifies the existing dir");

            // And the journal works inside it.
            let mut j = Journal::open(&dir).unwrap();
            assert_eq!(j.take_seq(), 1);
        }

        #[test]
        fn an_ordinary_directory_under_programdata_is_refused() {
            // Inherits ProgramData's ACL, which lets Users add files/folders.
            let path = scratch();
            let _c = Cleanup(path.clone());
            std::fs::create_dir(&path).unwrap();
            let err = TrustedDir::ensure_at(&path).unwrap_err();
            assert!(matches!(err, EngineError::InsecureStorage { .. }), "{err:?}");
        }

        #[test]
        fn granting_users_write_afterwards_is_detected() {
            let path = scratch();
            let _c = Cleanup(path.clone());
            TrustedDir::ensure_at(&path).unwrap();
            let status = std::process::Command::new("icacls")
                .arg(&path)
                .args(["/grant", "*S-1-5-32-545:(OI)(CI)M"]) // BUILTIN\Users: modify
                .output()
                .expect("icacls");
            assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stdout));
            let err = TrustedDir::ensure_at(&path).unwrap_err();
            assert!(matches!(err, EngineError::InsecureStorage { .. }), "{err:?}");
        }

        /// The squatting attack the owner check exists for: a standard user
        /// owns the directory (they pre-created it) so files in it are theirs.
        /// CI creates a real standard local user and hands the directory to
        /// them, then expects the engine to refuse it.
        #[test]
        fn a_directory_owned_by_a_standard_user_is_refused() {
            let user = format!("ptsq{}", std::process::id() % 100_000);
            let password = "Pt!Sq-7Tmp-9x2Qz";
            let created = std::process::Command::new("net")
                .args(["user", &user, password, "/add"])
                .output()
                .expect("net user");
            if !created.status.success() {
                println!(
                    "NOT VERIFIED: could not create a local user here: {}",
                    String::from_utf8_lossy(&created.stdout)
                );
                return;
            }
            struct DeleteUser(String);
            impl Drop for DeleteUser {
                fn drop(&mut self) {
                    let _ = std::process::Command::new("net")
                        .args(["user", &self.0, "/delete"])
                        .output();
                }
            }
            let _cleanup_user = DeleteUser(user.clone());

            let path = scratch();
            let _c = Cleanup(path.clone());
            TrustedDir::ensure_at(&path).expect("created by us, so trusted");

            let out = std::process::Command::new("icacls")
                .arg(&path)
                .args(["/setowner", &user])
                .output()
                .expect("icacls");
            if !out.status.success() {
                println!(
                    "NOT VERIFIED: could not hand the directory to a standard user: {}",
                    String::from_utf8_lossy(&out.stdout)
                );
                return;
            }
            let err = TrustedDir::ensure_at(&path).unwrap_err();
            println!("refused: {err}");
            assert!(
                matches!(&err, EngineError::InsecureStorage { detail, .. } if detail.contains("owned by")),
                "{err:?}"
            );
        }

        #[test]
        fn a_reparse_point_is_refused() {
            let base = scratch();
            let _c = Cleanup(base.clone());
            std::fs::create_dir(&base).unwrap();
            let target = base.join("target");
            std::fs::create_dir(&target).unwrap();
            let link = base.join("link");
            // Junctions need no privilege; mklink /J.
            let out = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&target)
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
            let err = verify_secure(&link).unwrap_err();
            assert!(
                matches!(&err, EngineError::InsecureStorage { detail, .. } if detail.contains("reparse")),
                "{err:?}"
            );
        }
    }
}
