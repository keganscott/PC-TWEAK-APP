//! NVIDIA driver settings through NvAPI's driver settings (DRS) interface:
//! the same store NVIDIA Control Panel's "Manage 3D settings" writes.
//!
//! `nvapi64.dll` ships with NVIDIA's driver in System32, so it is loaded at
//! run time (never bundled) and its absence is an ordinary answer: no NVIDIA
//! graphics card. Only DWORD settings of the base profile (Control Panel's
//! global settings) are read or written, one session per call, so a change
//! made in Control Panel meanwhile is seen.
//!
//! NvAPI checks every structure's version, which carries its size: a layout
//! that does not match the driver's is refused with an error, not written
//! past. The function ids, status codes and the structure layout are from
//! NVIDIA's public NvAPI headers as recalled; VERIFY (NOTES N86).

/// A setting's state in the base profile: a value set there (by PeakTweaks,
/// Control Panel or another tool), or `None` when it has none of its own
/// and the driver's default applies.
pub type Stored = Option<u32>;

/// `NvAPI_Status` codes this module tells apart. VERIFY (nvapi.h).
pub mod status {
    pub const OK: i32 = 0;
    pub const NVIDIA_DEVICE_NOT_FOUND: i32 = -6;
    pub const INCOMPATIBLE_STRUCT_VERSION: i32 = -9;
    pub const INVALID_USER_PRIVILEGE: i32 = -137;
    pub const SETTING_NOT_FOUND: i32 = -160;
}

/// Function ids for `nvapi_QueryInterface`. VERIFY (nvapi_interface.h).
pub mod func {
    pub const INITIALIZE: u32 = 0x0150_E828;
    pub const DRS_CREATE_SESSION: u32 = 0x0694_D52E;
    pub const DRS_DESTROY_SESSION: u32 = 0xDAD9_CFF8;
    pub const DRS_LOAD_SETTINGS: u32 = 0x375D_BD6B;
    pub const DRS_SAVE_SETTINGS: u32 = 0xFCBC_7E14;
    pub const DRS_GET_BASE_PROFILE: u32 = 0xDA84_66A0;
    pub const DRS_GET_SETTING: u32 = 0x73BF_8338;
    pub const DRS_SET_SETTING: u32 = 0x577D_D202;
    pub const DRS_RESTORE_PROFILE_DEFAULT_SETTING: u32 = 0x53F0_381E;
}

/// `NVDRS_SETTING_V1`. The two value unions are kept as bytes: a DWORD
/// value is their first four.
#[repr(C)]
pub struct DrsSetting {
    pub version: u32,
    pub name: [u16; 2048],
    pub id: u32,
    pub kind: u32,
    pub location: u32,
    pub is_current_predefined: u32,
    pub is_predefined_valid: u32,
    pub predefined: [u8; 4100],
    pub current: [u8; 4100],
}

/// `MAKE_NVAPI_VERSION(NVDRS_SETTING_V1, 1)`: the size and version 1.
pub const DRS_SETTING_VERSION: u32 = std::mem::size_of::<DrsSetting>() as u32 | (1 << 16);
/// `NVDRS_DWORD_TYPE`.
pub const DWORD_TYPE: u32 = 0;
/// `NVDRS_DEFAULT_PROFILE_LOCATION`: the value is the driver's default.
const DEFAULT_PROFILE_LOCATION: u32 = 3;

impl DrsSetting {
    pub fn boxed() -> Box<Self> {
        Box::new(Self {
            version: DRS_SETTING_VERSION,
            name: [0; 2048],
            id: 0,
            kind: 0,
            location: 0,
            is_current_predefined: 0,
            is_predefined_valid: 0,
            predefined: [0; 4100],
            current: [0; 4100],
        })
    }

    pub fn dword(id: u32, value: u32) -> Box<Self> {
        let mut s = Self::boxed();
        s.id = id;
        s.kind = DWORD_TYPE;
        s.current[..4].copy_from_slice(&value.to_le_bytes());
        s
    }

    /// What a successful `NvAPI_DRS_GetSetting` on the base profile means
    /// here. NVIDIA's own value for the profile, or the driver default,
    /// counts as none of its own.
    pub fn stored(&self) -> std::result::Result<Stored, NvError> {
        if self.kind != DWORD_TYPE {
            return Err(NvError::Failed(format!(
                "NVIDIA setting 0x{:08X} is not a number",
                self.id
            )));
        }
        if self.is_current_predefined != 0 || self.location == DEFAULT_PROFILE_LOCATION {
            return Ok(None);
        }
        let mut b = [0u8; 4];
        b.copy_from_slice(&self.current[..4]);
        Ok(Some(u32::from_le_bytes(b)))
    }
}

/// Why a call did not work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NvError {
    /// No NVIDIA graphics card, or no NVIDIA driver: nothing to change.
    NoNvidia,
    /// The driver is there and refused or failed, in plain words.
    Failed(String),
}

impl std::fmt::Display for NvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoNvidia => f.write_str("no NVIDIA graphics card with its driver was found"),
            Self::Failed(detail) => f.write_str(detail),
        }
    }
}

/// What a failed call's status code means.
pub fn error(call: &str, code: i32) -> NvError {
    match code {
        status::NVIDIA_DEVICE_NOT_FOUND => NvError::NoNvidia,
        status::INCOMPATIBLE_STRUCT_VERSION => {
            NvError::Failed(format!("the NVIDIA driver refused {call}: its settings format differs"))
        }
        status::INVALID_USER_PRIVILEGE => NvError::Failed(format!(
            "the NVIDIA driver refused {call}: not allowed for this account"
        )),
        _ => NvError::Failed(format!("{call} failed with NvAPI code {code}")),
    }
}

#[cfg(windows)]
pub use real::NvApi;

#[cfg(windows)]
mod real {
    use std::ffi::c_void;
    use std::sync::Mutex;

    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

    use super::*;

    type Handle = *mut c_void;
    type QueryInterface = unsafe extern "C" fn(u32) -> *mut c_void;
    type Initialize = unsafe extern "C" fn() -> i32;
    type CreateSession = unsafe extern "C" fn(*mut Handle) -> i32;
    type WithSession = unsafe extern "C" fn(Handle) -> i32;
    type GetBaseProfile = unsafe extern "C" fn(Handle, *mut Handle) -> i32;
    type GetSetting = unsafe extern "C" fn(Handle, Handle, u32, *mut DrsSetting) -> i32;
    type SetSetting = unsafe extern "C" fn(Handle, Handle, *mut DrsSetting) -> i32;
    type RestoreDefault = unsafe extern "C" fn(Handle, Handle, u32) -> i32;

    struct Api {
        create_session: CreateSession,
        destroy_session: WithSession,
        load_settings: WithSession,
        save_settings: WithSession,
        get_base_profile: GetBaseProfile,
        get_setting: GetSetting,
        set_setting: SetSetting,
        restore_default: RestoreDefault,
    }

    // Function pointers into a library that stays loaded for the process;
    // every call is made under the mutex below.
    unsafe impl Send for Api {}

    /// NvAPI, loaded and initialised on first use. A failed load is tried
    /// again next time (a driver installed meanwhile is found).
    #[derive(Default)]
    pub struct NvApi {
        api: Mutex<Option<Api>>,
    }

    unsafe fn load() -> std::result::Result<Api, NvError> {
        let module =
            LoadLibraryExW(w!("nvapi64.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).map_err(|_| NvError::NoNvidia)?;
        let query: QueryInterface = match GetProcAddress(module, s!("nvapi_QueryInterface")) {
            Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryInterface>(f),
            None => return Err(NvError::Failed("nvapi64.dll has no nvapi_QueryInterface".into())),
        };
        macro_rules! func {
            ($id:expr, $ty:ty) => {{
                let p = query($id);
                if p.is_null() {
                    return Err(NvError::Failed(format!(
                        "this NVIDIA driver has no function 0x{:08X}",
                        $id
                    )));
                }
                std::mem::transmute::<*mut c_void, $ty>(p)
            }};
        }
        let initialize: Initialize = func!(func::INITIALIZE, Initialize);
        let api = Api {
            create_session: func!(func::DRS_CREATE_SESSION, CreateSession),
            destroy_session: func!(func::DRS_DESTROY_SESSION, WithSession),
            load_settings: func!(func::DRS_LOAD_SETTINGS, WithSession),
            save_settings: func!(func::DRS_SAVE_SETTINGS, WithSession),
            get_base_profile: func!(func::DRS_GET_BASE_PROFILE, GetBaseProfile),
            get_setting: func!(func::DRS_GET_SETTING, GetSetting),
            set_setting: func!(func::DRS_SET_SETTING, SetSetting),
            restore_default: func!(func::DRS_RESTORE_PROFILE_DEFAULT_SETTING, RestoreDefault),
        };
        let rc = initialize();
        if rc != status::OK {
            return Err(error("NvAPI_Initialize", rc));
        }
        Ok(api)
    }

    fn check(call: &str, rc: i32) -> std::result::Result<(), NvError> {
        if rc == status::OK {
            Ok(())
        } else {
            Err(error(call, rc))
        }
    }

    impl NvApi {
        pub fn new() -> Self {
            Self::default()
        }

        /// Run `f` with a fresh session on the loaded settings and the base
        /// profile. The session is always destroyed.
        fn with_base<T>(
            &self,
            f: impl FnOnce(&Api, Handle, Handle) -> std::result::Result<T, NvError>,
        ) -> std::result::Result<T, NvError> {
            let mut guard = self.api.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.is_none() {
                *guard = Some(unsafe { load()? });
            }
            let api = guard.as_ref().expect("loaded above");
            unsafe {
                let mut session: Handle = std::ptr::null_mut();
                check("NvAPI_DRS_CreateSession", (api.create_session)(&mut session))?;
                let result = (|| {
                    check("NvAPI_DRS_LoadSettings", (api.load_settings)(session))?;
                    let mut profile: Handle = std::ptr::null_mut();
                    check(
                        "NvAPI_DRS_GetBaseProfile",
                        (api.get_base_profile)(session, &mut profile),
                    )?;
                    f(api, session, profile)
                })();
                (api.destroy_session)(session);
                result
            }
        }

        /// The base profile's own value for `id`, if it has one.
        pub fn get(&self, id: u32) -> std::result::Result<Stored, NvError> {
            self.with_base(|api, session, profile| unsafe {
                let mut setting = DrsSetting::boxed();
                match (api.get_setting)(session, profile, id, &mut *setting) {
                    status::SETTING_NOT_FOUND => Ok(None),
                    rc => {
                        check("NvAPI_DRS_GetSetting", rc)?;
                        setting.stored()
                    }
                }
            })
        }

        /// Give the base profile `value` for `id`, or with `None` put the
        /// driver's default back, and save.
        pub fn set(&self, id: u32, value: Stored) -> std::result::Result<(), NvError> {
            self.with_base(|api, session, profile| unsafe {
                match value {
                    Some(v) => {
                        let mut setting = DrsSetting::dword(id, v);
                        check(
                            "NvAPI_DRS_SetSetting",
                            (api.set_setting)(session, profile, &mut *setting),
                        )?;
                    }
                    None => match (api.restore_default)(session, profile, id) {
                        // Nothing of its own to remove.
                        status::SETTING_NOT_FOUND => return Ok(()),
                        rc => check("NvAPI_DRS_RestoreProfileDefaultSetting", rc)?,
                    },
                }
                check("NvAPI_DRS_SaveSettings", (api.save_settings)(session))
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Read-only, on this machine: a PC without an NVIDIA driver answers
        /// with a reason, never a crash. Power management mode (VERIFY id).
        #[test]
        fn reading_a_setting_answers_without_crashing() {
            let api = NvApi::new();
            let first = api.get(0x1057_EB71);
            println!("NVIDIA power management mode in the base profile: {first:?}");
            if let Err(NvError::Failed(e)) = &first {
                assert!(!e.is_empty());
            }
            assert_eq!(first.is_ok(), api.get(0x1057_EB71).is_ok());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_structure_has_nvapis_size_and_version() {
        // 4 + 2048 * 2 + 5 * 4 + 2 * (4 + 4096) bytes; version 1 above it.
        assert_eq!(std::mem::size_of::<DrsSetting>(), 0x3020);
        assert_eq!(DRS_SETTING_VERSION, 0x0001_3020);
        assert_eq!(std::mem::offset_of!(DrsSetting, id), 4100);
        assert_eq!(std::mem::offset_of!(DrsSetting, current), 8220);
    }

    #[test]
    fn a_value_of_its_own_is_read_and_a_default_is_none() {
        let mut s = DrsSetting::dword(0x1057_EB71, 1);
        assert_eq!(s.stored(), Ok(Some(1)));
        s.is_current_predefined = 1;
        assert_eq!(s.stored(), Ok(None), "NVIDIA's own value for the profile");
        s.is_current_predefined = 0;
        s.location = DEFAULT_PROFILE_LOCATION;
        assert_eq!(s.stored(), Ok(None), "the driver default");
        s.kind = 3;
        assert!(s.stored().is_err(), "a text setting is not read as a number");
    }

    #[test]
    fn failures_read_plainly() {
        assert_eq!(
            error("NvAPI_Initialize", status::NVIDIA_DEVICE_NOT_FOUND),
            NvError::NoNvidia
        );
        assert_eq!(
            error("NvAPI_DRS_SaveSettings", -1).to_string(),
            "NvAPI_DRS_SaveSettings failed with NvAPI code -1"
        );
    }
}
