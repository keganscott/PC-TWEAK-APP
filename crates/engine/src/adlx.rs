//! AMD graphics settings through ADLX, AMD's published settings library: the
//! same global settings AMD Software: Adrenalin Edition shows under Gaming >
//! Graphics.
//!
//! `amdadlx64.dll` ships with AMD's driver, so it is loaded from System32 at
//! run time (never bundled) and its absence is an ordinary answer: no AMD
//! graphics card. ADLX's C interface is a table of functions per object; the
//! slots called here were read from AMD's public headers by compiling them
//! (github.com/GPUOpen-LibrariesAndSDKs/ADLX, main, 2026-10-08:
//! `ISystem.h`, `I3DSettings.h`, `ADLX.h`, `ADLXDefines.h`, version 2.0.0.125),
//! as were the result codes, the vertical refresh modes and the Anti-Lag
//! levels. Not yet run against a real AMD driver (NOTES N87).

/// Why a call did not work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdlxError {
    /// No AMD graphics card, or no AMD driver with ADLX: nothing to change.
    NoAmd,
    /// The driver is there and refused or failed, in plain words.
    Failed(String),
}

impl std::fmt::Display for AdlxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAmd => f.write_str("no AMD graphics card with its driver was found"),
            Self::Failed(detail) => f.write_str(detail),
        }
    }
}

/// `ADLX_RESULT` values this module tells apart (`ADLXDefines.h`).
pub mod result {
    pub const OK: i32 = 0;
    pub const ALREADY_ENABLED: i32 = 1;
    pub const ALREADY_INITIALIZED: i32 = 2;
    pub const BAD_VER: i32 = 5;
    pub const UNKNOWN_INTERFACE: i32 = 6;
    pub const ADL_INIT_ERROR: i32 = 8;
    pub const NOT_SUPPORTED: i32 = 12;
}

/// What a failed call's result means.
pub fn error(call: &str, code: i32) -> AdlxError {
    match code {
        // ADLX loads but finds no AMD display driver to talk to.
        result::ADL_INIT_ERROR => AdlxError::NoAmd,
        result::BAD_VER => AdlxError::Failed(format!("the AMD driver refused {call}: its ADLX version differs")),
        _ => AdlxError::Failed(format!("{call} failed with ADLX code {code}")),
    }
}

/// `ADLX_SUCCEEDED`: OK, or a state that was already as asked.
pub fn succeeded(code: i32) -> bool {
    code == result::OK || code == result::ALREADY_ENABLED || code == result::ALREADY_INITIALIZED
}

/// `ADLX_WAIT_FOR_VERTICAL_REFRESH_MODE`.
pub mod vsync {
    pub const ALWAYS_OFF: u32 = 0;
    pub const OFF_UNLESS_APP_SPECIFIES: u32 = 1;
    pub const ON_UNLESS_APP_SPECIFIES: u32 = 2;
    pub const ALWAYS_ON: u32 = 3;
}

/// `ADLX_ANTILAG_STATE`, read through `IADLX3DAntiLag1`, which drivers from
/// late 2023 have. "Anti-Lag Next" is the level AMD describes as "an advanced
/// algorithm in supported DX11 and DX12 games"; PeakTweaks never selects it.
pub mod anti_lag_level {
    pub const ANTI_LAG: u32 = 0;
    pub const NEXT: u32 = 1;
}

/// Function-table slots, counted from 0 (`offsetof(<Vtbl>, <method>) / 8`
/// with AMD's headers). Every ADLX object but the system has Acquire,
/// Release and QueryInterface first.
pub mod slot {
    pub const RELEASE: usize = 1;
    pub const QUERY_INTERFACE: usize = 2;
    pub const SYSTEM_GET_GPUS: usize = 1;
    pub const SYSTEM_GET_3D_SETTINGS_SERVICES: usize = 7;
    pub const LIST_SIZE: usize = 3;
    pub const GPU_LIST_AT: usize = 11;
    pub const GPU_NAME: usize = 7;
    pub const GPU_PNP_STRING: usize = 9;
    pub const SERVICES_GET_ANTI_LAG: usize = 3;
    pub const SERVICES_GET_CHILL: usize = 4;
    pub const SERVICES_GET_WAIT_FOR_VERTICAL_REFRESH: usize = 8;
    pub const ANTI_LAG_IS_SUPPORTED: usize = 3;
    pub const ANTI_LAG_IS_ENABLED: usize = 4;
    pub const ANTI_LAG_SET_ENABLED: usize = 5;
    pub const ANTI_LAG_GET_LEVEL: usize = 6;
    pub const ANTI_LAG_SET_LEVEL: usize = 7;
    pub const CHILL_IS_SUPPORTED: usize = 3;
    pub const CHILL_IS_ENABLED: usize = 4;
    pub const CHILL_SET_ENABLED: usize = 8;
    pub const VSYNC_IS_SUPPORTED: usize = 3;
    pub const VSYNC_GET_MODE: usize = 5;
    pub const VSYNC_SET_MODE: usize = 6;
}

/// One AMD graphics card as ADLX lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmdGpu {
    /// Its Plug and Play id, which names it in the change record.
    pub id: String,
    pub name: String,
}

/// The settings PeakTweaks changes, with their state: `Some(value)` where
/// the card has the setting, `None` where it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// Radeon Anti-Lag: 1 on, 0 off.
    AntiLag,
    /// Which Anti-Lag: an `anti_lag_level`. Absent on drivers without it.
    AntiLagLevel,
    /// Radeon Chill: 1 on, 0 off. AMD turns it off when Anti-Lag is turned
    /// on, as the two cannot be on together.
    Chill,
    /// Wait for Vertical Refresh: a `vsync` mode.
    WaitForVerticalRefresh,
}

impl Setting {
    pub const ALL: [Self; 4] = [
        Self::AntiLag,
        Self::AntiLagLevel,
        Self::Chill,
        Self::WaitForVerticalRefresh,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::AntiLag => "anti_lag",
            Self::AntiLagLevel => "anti_lag_level",
            Self::Chill => "chill",
            Self::WaitForVerticalRefresh => "wait_for_vertical_refresh",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::AntiLag => "Radeon Anti-Lag",
            Self::AntiLagLevel => "Radeon Anti-Lag level",
            Self::Chill => "Radeon Chill",
            Self::WaitForVerticalRefresh => "Wait for Vertical Refresh",
        }
    }
}

#[cfg(windows)]
pub use real::Adlx;

#[cfg(windows)]
mod real {
    use std::ffi::{c_char, c_void, CStr};
    use std::sync::Mutex;

    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

    use super::*;

    type Obj = *mut c_void;
    type QueryFullVersion = unsafe extern "C" fn(*mut u64) -> i32;
    type Initialize = unsafe extern "C" fn(u64, *mut Obj) -> i32;

    /// Function `slot` of `obj`'s table, as `F`.
    ///
    /// # Safety
    /// `obj` is a live ADLX object whose table has `slot` with signature `F`.
    unsafe fn method<F: Copy>(obj: Obj, slot: usize) -> F {
        let table = *(obj as *const *const usize);
        let f = *table.add(slot);
        std::mem::transmute_copy::<usize, F>(&f)
    }

    /// An ADLX object that is released when dropped.
    struct Owned(Obj);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { method::<unsafe extern "C" fn(Obj) -> i32>(self.0, slot::RELEASE)(self.0) };
            }
        }
    }

    fn check(call: &str, code: i32) -> std::result::Result<(), AdlxError> {
        if succeeded(code) {
            Ok(())
        } else {
            Err(error(call, code))
        }
    }

    /// Call a `Get…(this, [arg,] out)` method and own what it returns.
    unsafe fn get(call: &str, this: Obj, slot: usize, arg: Option<Obj>) -> std::result::Result<Owned, AdlxError> {
        let mut out: Obj = std::ptr::null_mut();
        let rc = match arg {
            Some(a) => method::<unsafe extern "C" fn(Obj, Obj, *mut Obj) -> i32>(this, slot)(this, a, &mut out),
            None => method::<unsafe extern "C" fn(Obj, *mut Obj) -> i32>(this, slot)(this, &mut out),
        };
        check(call, rc)?;
        if out.is_null() {
            return Err(AdlxError::Failed(format!("{call} returned nothing")));
        }
        Ok(Owned(out))
    }

    unsafe fn text(call: &str, this: Obj, slot: usize) -> std::result::Result<String, AdlxError> {
        let mut out: *const c_char = std::ptr::null();
        check(
            call,
            method::<unsafe extern "C" fn(Obj, *mut *const c_char) -> i32>(this, slot)(this, &mut out),
        )?;
        Ok(if out.is_null() {
            String::new()
        } else {
            CStr::from_ptr(out).to_string_lossy().into_owned()
        })
    }

    unsafe fn flag(call: &str, this: Obj, slot: usize) -> std::result::Result<bool, AdlxError> {
        let mut out: u8 = 0;
        check(
            call,
            method::<unsafe extern "C" fn(Obj, *mut u8) -> i32>(this, slot)(this, &mut out),
        )?;
        Ok(out != 0)
    }

    /// The Anti-Lag interface with levels (its id is `IID_IADLX3DAntiLag1`,
    /// the interface's name), or `None` on a driver that predates it.
    unsafe fn anti_lag1(anti_lag: Obj) -> std::result::Result<Option<Owned>, AdlxError> {
        let mut out: Obj = std::ptr::null_mut();
        let rc = method::<unsafe extern "C" fn(Obj, *const u16, *mut Obj) -> i32>(anti_lag, slot::QUERY_INTERFACE)(
            anti_lag,
            w!("IADLX3DAntiLag1").as_ptr(),
            &mut out,
        );
        if rc == result::UNKNOWN_INTERFACE {
            return Ok(None);
        }
        check("Anti-Lag QueryInterface", rc)?;
        if out.is_null() {
            return Err(AdlxError::Failed("Anti-Lag QueryInterface returned nothing".into()));
        }
        Ok(Some(Owned(out)))
    }

    struct System(Obj);

    // The system object lives as long as the loaded library, which is never
    // unloaded; every call is made under the mutex below.
    unsafe impl Send for System {}

    /// ADLX, loaded and initialised on first use. A failed load is tried
    /// again next time.
    #[derive(Default)]
    pub struct Adlx {
        system: Mutex<Option<System>>,
    }

    unsafe fn load() -> std::result::Result<System, AdlxError> {
        let module =
            LoadLibraryExW(w!("amdadlx64.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).map_err(|_| AdlxError::NoAmd)?;
        let version: QueryFullVersion = match GetProcAddress(module, s!("ADLXQueryFullVersion")) {
            Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryFullVersion>(f),
            None => return Err(AdlxError::Failed("amdadlx64.dll has no ADLXQueryFullVersion".into())),
        };
        let initialize: Initialize = match GetProcAddress(module, s!("ADLXInitialize")) {
            Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, Initialize>(f),
            None => return Err(AdlxError::Failed("amdadlx64.dll has no ADLXInitialize".into())),
        };
        // The library's own version: every slot used here dates from ADLX's
        // first version, so any version has them.
        let mut full = 0u64;
        check("ADLXQueryFullVersion", version(&mut full))?;
        let mut system: Obj = std::ptr::null_mut();
        check("ADLXInitialize", initialize(full, &mut system))?;
        if system.is_null() {
            return Err(AdlxError::Failed("ADLXInitialize returned nothing".into()));
        }
        Ok(System(system))
    }

    impl Adlx {
        pub fn new() -> Self {
            Self::default()
        }

        fn with_system<T>(
            &self,
            f: impl FnOnce(Obj) -> std::result::Result<T, AdlxError>,
        ) -> std::result::Result<T, AdlxError> {
            let mut guard = self.system.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.is_none() {
                *guard = Some(unsafe { load()? });
            }
            f(guard.as_ref().expect("loaded above").0)
        }

        /// Each AMD graphics card, with its id and name, and the card object.
        unsafe fn each_gpu(
            system: Obj,
            mut f: impl FnMut(&AmdGpu, Obj) -> std::result::Result<(), AdlxError>,
        ) -> std::result::Result<(), AdlxError> {
            let list = get("GetGPUs", system, slot::SYSTEM_GET_GPUS, None)?;
            let size = method::<unsafe extern "C" fn(Obj) -> u32>(list.0, slot::LIST_SIZE)(list.0);
            for i in 0..size {
                let mut gpu: Obj = std::ptr::null_mut();
                check(
                    "GPU list At",
                    method::<unsafe extern "C" fn(Obj, u32, *mut Obj) -> i32>(list.0, slot::GPU_LIST_AT)(
                        list.0, i, &mut gpu,
                    ),
                )?;
                let gpu = Owned(gpu);
                if gpu.0.is_null() {
                    continue;
                }
                let card = AmdGpu {
                    id: text("PNPString", gpu.0, slot::GPU_PNP_STRING)?,
                    name: text("Name", gpu.0, slot::GPU_NAME)?,
                };
                f(&card, gpu.0)?;
            }
            Ok(())
        }

        pub fn gpus(&self) -> std::result::Result<Vec<AmdGpu>, AdlxError> {
            self.with_system(|system| unsafe {
                let mut all = Vec::new();
                Self::each_gpu(system, |card, _| {
                    all.push(card.clone());
                    Ok(())
                })?;
                Ok(all)
            })
        }

        /// The setting's interface for the card `gpu_id`, and what to do with
        /// it. `Ok(None)` when no AMD card has that id.
        fn with_setting<T>(
            &self,
            gpu_id: &str,
            setting: Setting,
            f: impl FnOnce(Obj) -> std::result::Result<T, AdlxError>,
        ) -> std::result::Result<Option<T>, AdlxError> {
            self.with_system(|system| unsafe {
                let services = get(
                    "Get3DSettingsServices",
                    system,
                    slot::SYSTEM_GET_3D_SETTINGS_SERVICES,
                    None,
                )?;
                let mut f = Some(f);
                let mut out = None;
                Self::each_gpu(system, |card, gpu| {
                    if out.is_some() || card.id != gpu_id {
                        return Ok(());
                    }
                    let (call, slot) = match setting {
                        Setting::AntiLag | Setting::AntiLagLevel => ("GetAntiLag", slot::SERVICES_GET_ANTI_LAG),
                        Setting::Chill => ("GetChill", slot::SERVICES_GET_CHILL),
                        Setting::WaitForVerticalRefresh => (
                            "GetWaitForVerticalRefresh",
                            slot::SERVICES_GET_WAIT_FOR_VERTICAL_REFRESH,
                        ),
                    };
                    let iface = get(call, services.0, slot, Some(gpu))?;
                    out = Some(f.take().expect("called once")(iface.0)?);
                    Ok(())
                })?;
                Ok(out)
            })
        }

        /// The card's value for `setting`: `Some(Some(v))`, `Some(None)` when
        /// the card does not have the setting, `None` when there is no such card.
        pub fn get(&self, gpu_id: &str, setting: Setting) -> std::result::Result<Option<Option<u32>>, AdlxError> {
            self.with_setting(gpu_id, setting, |iface| unsafe {
                match setting {
                    Setting::AntiLag => {
                        if !flag("Anti-Lag IsSupported", iface, slot::ANTI_LAG_IS_SUPPORTED)? {
                            return Ok(None);
                        }
                        Ok(Some(u32::from(flag(
                            "Anti-Lag IsEnabled",
                            iface,
                            slot::ANTI_LAG_IS_ENABLED,
                        )?)))
                    }
                    Setting::AntiLagLevel => {
                        if !flag("Anti-Lag IsSupported", iface, slot::ANTI_LAG_IS_SUPPORTED)? {
                            return Ok(None);
                        }
                        let Some(level) = anti_lag1(iface)? else {
                            return Ok(None);
                        };
                        let mut out: i32 = 0;
                        check(
                            "Anti-Lag GetLevel",
                            method::<unsafe extern "C" fn(Obj, *mut i32) -> i32>(level.0, slot::ANTI_LAG_GET_LEVEL)(
                                level.0, &mut out,
                            ),
                        )?;
                        Ok(Some(out as u32))
                    }
                    Setting::Chill => {
                        if !flag("Chill IsSupported", iface, slot::CHILL_IS_SUPPORTED)? {
                            return Ok(None);
                        }
                        Ok(Some(u32::from(flag("Chill IsEnabled", iface, slot::CHILL_IS_ENABLED)?)))
                    }
                    Setting::WaitForVerticalRefresh => {
                        if !flag("VSync IsSupported", iface, slot::VSYNC_IS_SUPPORTED)? {
                            return Ok(None);
                        }
                        let mut mode: i32 = 0;
                        check(
                            "VSync GetMode",
                            method::<unsafe extern "C" fn(Obj, *mut i32) -> i32>(iface, slot::VSYNC_GET_MODE)(
                                iface, &mut mode,
                            ),
                        )?;
                        Ok(Some(mode as u32))
                    }
                }
            })
        }

        /// Give the card `value` for `setting`. `Ok(false)` when there is no
        /// such card.
        pub fn set(&self, gpu_id: &str, setting: Setting, value: u32) -> std::result::Result<bool, AdlxError> {
            let done = self.with_setting(gpu_id, setting, |iface| unsafe {
                match setting {
                    Setting::AntiLag => check(
                        "Anti-Lag SetEnabled",
                        method::<unsafe extern "C" fn(Obj, u8) -> i32>(iface, slot::ANTI_LAG_SET_ENABLED)(
                            iface,
                            u8::from(value != 0),
                        ),
                    ),
                    Setting::AntiLagLevel => {
                        if value > anti_lag_level::NEXT {
                            return Err(AdlxError::Failed(format!("{value} is not an Anti-Lag level")));
                        }
                        let Some(level) = anti_lag1(iface)? else {
                            return Err(AdlxError::Failed("this AMD driver has no Anti-Lag level".into()));
                        };
                        check(
                            "Anti-Lag SetLevel",
                            method::<unsafe extern "C" fn(Obj, i32) -> i32>(level.0, slot::ANTI_LAG_SET_LEVEL)(
                                level.0,
                                value as i32,
                            ),
                        )
                    }
                    Setting::Chill => check(
                        "Chill SetEnabled",
                        method::<unsafe extern "C" fn(Obj, u8) -> i32>(iface, slot::CHILL_SET_ENABLED)(
                            iface,
                            u8::from(value != 0),
                        ),
                    ),
                    Setting::WaitForVerticalRefresh => {
                        if value > vsync::ALWAYS_ON {
                            return Err(AdlxError::Failed(format!("{value} is not a vertical refresh mode")));
                        }
                        check(
                            "VSync SetMode",
                            method::<unsafe extern "C" fn(Obj, i32) -> i32>(iface, slot::VSYNC_SET_MODE)(
                                iface,
                                value as i32,
                            ),
                        )
                    }
                }
            })?;
            Ok(done.is_some())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Read-only, on this machine: a PC without an AMD driver answers
        /// with a reason, never a crash.
        #[test]
        fn listing_amd_cards_answers_without_crashing() {
            let adlx = Adlx::new();
            let gpus = adlx.gpus();
            println!("AMD graphics cards: {gpus:?}");
            if let Ok(list) = &gpus {
                for g in list {
                    for s in Setting::ALL {
                        println!("{} ({}): {} {:?}", g.name, g.id, s.label(), adlx.get(&g.id, s));
                    }
                }
            }
            assert_eq!(gpus.is_ok(), adlx.gpus().is_ok());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_read_plainly() {
        assert_eq!(error("ADLXInitialize", result::ADL_INIT_ERROR), AdlxError::NoAmd);
        assert_eq!(
            error("VSync SetMode", result::NOT_SUPPORTED).to_string(),
            "VSync SetMode failed with ADLX code 12"
        );
        assert!(succeeded(result::OK) && succeeded(result::ALREADY_ENABLED) && !succeeded(result::NOT_SUPPORTED));
    }

    #[test]
    fn settings_round_trip_through_their_keys() {
        for s in Setting::ALL {
            assert_eq!(Setting::from_key(s.key()), Some(s));
        }
        assert_eq!(Setting::from_key("boost"), None);
    }
}
