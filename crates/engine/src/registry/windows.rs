//! The real registry, via `winreg`.

use std::io::ErrorKind;

use winreg::enums::{
    RegType, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_CREATE_SUB_KEY, KEY_READ,
    KEY_SET_VALUE,
};
use winreg::RegKey;

use super::{components, Hive, RegistryBackend};
use crate::error::{EngineError, Result};
use crate::types::RawValue;

#[derive(Default)]
pub struct WinRegistry;

impl WinRegistry {
    pub fn new() -> Self {
        Self
    }
}

fn predef(hive: Hive) -> RegKey {
    RegKey::predef(match hive {
        Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        Hive::ClassesRoot => HKEY_CLASSES_ROOT,
        Hive::CurrentUser => HKEY_CURRENT_USER,
        Hive::Users => HKEY_USERS,
    })
}

fn display(hive: Hive, path: &str) -> String {
    format!("{}\\{}", hive.name(), path)
}

/// Only the types `SUPPORTED_VALUE_TYPES` lists can be written back. Anything
/// else is refused rather than silently mapped to `REG_NONE`.
fn reg_type(vtype: u32) -> Option<RegType> {
    use RegType::*;
    Some(match vtype {
        1 => REG_SZ,
        2 => REG_EXPAND_SZ,
        3 => REG_BINARY,
        4 => REG_DWORD,
        7 => REG_MULTI_SZ,
        11 => REG_QWORD,
        _ => return None,
    })
}

/// Store exactly `bytes`. For string types `RegSetValueExW` reads the two
/// bytes after the length it is given, and if they are zero it stores a NUL
/// terminator that was never asked for (measured: registry::contract_tests,
/// `unterminated_string_read_back_report`). Whatever follows a plain buffer in
/// memory is chance, so an unterminated string was stored with or without a
/// terminator at random. Two non-zero guard bytes inside our own buffer make
/// the result exact, and keep Windows' read past the end inside our memory.
fn set_exact(key: &RegKey, name: &str, vtype: RegType, bytes: &[u8]) -> std::io::Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegSetValueExW, HKEY, REG_VALUE_TYPE};

    let mut buf = Vec::with_capacity(bytes.len() + 2);
    buf.extend_from_slice(bytes);
    buf.extend_from_slice(&[0xFF, 0xFF]);
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: the handle is open with KEY_SET_VALUE for the life of `key`;
    // `wide` is NUL-terminated; the slice is valid for its length.
    let rc = unsafe {
        RegSetValueExW(
            HKEY(key.raw_handle() as *mut _),
            PCWSTR(wide.as_ptr()),
            0,
            REG_VALUE_TYPE(vtype as u32),
            Some(&buf[..bytes.len()]),
        )
    };
    if rc.is_ok() {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(rc.0 as i32))
    }
}

fn not_found(e: &std::io::Error) -> bool {
    e.kind() == ErrorKind::NotFound
}

fn open_read(hive: Hive, path: &str) -> Result<Option<RegKey>> {
    match predef(hive).open_subkey_with_flags(path, KEY_READ) {
        Ok(k) => Ok(Some(k)),
        Err(e) if not_found(&e) => Ok(None),
        Err(e) => Err(EngineError::registry(display(hive, path), None, e)),
    }
}

impl RegistryBackend for WinRegistry {
    fn key_exists(&self, hive: Hive, path: &str) -> Result<bool> {
        Ok(open_read(hive, path)?.is_some())
    }

    fn read_value(&self, hive: Hive, path: &str, name: &str) -> Result<Option<RawValue>> {
        let Some(key) = open_read(hive, path)? else {
            return Ok(None);
        };
        match key.get_raw_value(name) {
            Ok(v) => Ok(Some(RawValue {
                vtype: v.vtype as u32,
                bytes: v.bytes,
            })),
            Err(e) if not_found(&e) => Ok(None),
            Err(e) => Err(EngineError::registry(display(hive, path), Some(name), e)),
        }
    }

    fn value_names(&self, hive: Hive, path: &str) -> Result<Vec<String>> {
        let Some(key) = open_read(hive, path)? else {
            return Ok(Vec::new());
        };
        key.enum_values()
            .map(|v| {
                v.map(|(name, _)| name)
                    .map_err(|e| EngineError::registry(display(hive, path), None, e))
            })
            .collect()
    }

    fn write_value(&self, hive: Hive, path: &str, name: &str, value: &RawValue) -> Result<()> {
        let Some(vtype) = reg_type(value.vtype) else {
            return Err(EngineError::UnsupportedValueType {
                path: display(hive, path),
                value: name.to_owned(),
                vtype: value.vtype,
            });
        };
        let (key, _) = predef(hive)
            .create_subkey_with_flags(path, KEY_SET_VALUE | KEY_CREATE_SUB_KEY)
            .map_err(|e| EngineError::registry(display(hive, path), Some(name), e))?;
        set_exact(&key, name, vtype, &value.bytes)
            .map_err(|e| EngineError::registry(display(hive, path), Some(name), e))
    }

    fn delete_value(&self, hive: Hive, path: &str, name: &str) -> Result<bool> {
        let key = match predef(hive).open_subkey_with_flags(path, KEY_SET_VALUE) {
            Ok(k) => k,
            Err(e) if not_found(&e) => return Ok(false),
            Err(e) => return Err(EngineError::registry(display(hive, path), Some(name), e)),
        };
        match key.delete_value(name) {
            Ok(()) => Ok(true),
            Err(e) if not_found(&e) => Ok(false),
            Err(e) => Err(EngineError::registry(display(hive, path), Some(name), e)),
        }
    }

    fn create_key(&self, hive: Hive, path: &str) -> Result<()> {
        predef(hive)
            .create_subkey_with_flags(path, KEY_READ | KEY_CREATE_SUB_KEY)
            .map(|_| ())
            .map_err(|e| EngineError::registry(display(hive, path), None, e))
    }

    fn delete_key_if_empty(&self, hive: Hive, path: &str) -> Result<bool> {
        let parts = components(path);
        let Some((leaf, parent_parts)) = parts.split_last() else {
            return Ok(false); // never delete a hive root
        };
        let Some(key) = open_read(hive, path)? else {
            return Ok(false);
        };
        let info = key
            .query_info()
            .map_err(|e| EngineError::registry(display(hive, path), None, e))?;
        if info.sub_keys != 0 || info.values != 0 {
            return Ok(false);
        }
        drop(key);

        let parent_path = parent_parts.join("\\");
        let parent = if parent_path.is_empty() {
            predef(hive)
        } else {
            match open_read(hive, &parent_path)? {
                Some(p) => p,
                None => return Ok(false),
            }
        };
        // RegDeleteKey fails on a key that has subkeys, so a racing writer
        // cannot make this recursive.
        match parent.delete_subkey(leaf) {
            Ok(()) => Ok(true),
            Err(e) if not_found(&e) => Ok(false),
            Err(e) => Err(EngineError::registry(display(hive, path), None, e)),
        }
    }
}
