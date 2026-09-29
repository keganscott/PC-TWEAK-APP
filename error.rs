//! Engine error type.
//!
//! Every variant is `Serialize` because these cross the Tauri boundary and the
//! frontend renders them. Errors name what failed and what the operator can do
//! about it — a bare `io::Error` reaching the UI is a bug.

use serde::Serialize;
use std::fmt;

pub type Result<T> = std::result::Result<T, EngineError>;

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineError {
    /// The process is not running elevated. Nothing mutating can proceed.
    NotElevated,

    /// We could not work out which user's hive to write to.
    UserContextUnresolved { detail: String },

    /// The interactive user's hive is not loaded under HKEY_USERS. Happens when
    /// no one is logged on interactively; loading it needs SE_RESTORE_NAME,
    /// which we deliberately do not take.
    UserHiveNotLoaded { sid: String },

    /// A registry operation failed.
    Registry {
        path: String,
        value: Option<String>,
        detail: String,
    },

    /// A Win32 call failed. `code` is the raw GetLastError / HRESULT.
    Win32 { call: &'static str, code: u32, detail: String },

    /// Filesystem failure writing a backup or journal entry.
    Storage { path: String, detail: String },

    /// The journal has no record of this tweak, so there is nothing to restore.
    NoJournalEntry { tweak_id: String },

    /// The tweak exists but cannot run on this machine right now.
    Blocked { reason: super::types::BlockedReason },

    /// Unknown tweak id from the frontend.
    UnknownTweak { tweak_id: String },

    /// A tweak tried to mutate outside its declared execution context.
    ContextViolation { tweak_id: String, detail: String },
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotElevated => write!(f, "PeakTweaks is not running elevated"),
            Self::UserContextUnresolved { detail } => {
                write!(f, "could not resolve the interactive user: {detail}")
            }
            Self::UserHiveNotLoaded { sid } => {
                write!(f, "registry hive for {sid} is not loaded under HKEY_USERS")
            }
            Self::Registry { path, value, detail } => match value {
                Some(v) => write!(f, "registry {path}\\{v}: {detail}"),
                None => write!(f, "registry {path}: {detail}"),
            },
            Self::Win32 { call, code, detail } => write!(f, "{call} failed ({code}): {detail}"),
            Self::Storage { path, detail } => write!(f, "{path}: {detail}"),
            Self::NoJournalEntry { tweak_id } => {
                write!(f, "no journal entry for {tweak_id}")
            }
            Self::Blocked { reason } => write!(f, "blocked: {}", reason.message),
            Self::UnknownTweak { tweak_id } => write!(f, "unknown tweak: {tweak_id}"),
            Self::ContextViolation { tweak_id, detail } => {
                write!(f, "{tweak_id} violated its execution context: {detail}")
            }
        }
    }
}

impl std::error::Error for EngineError {}

impl EngineError {
    pub fn storage(path: impl Into<String>, e: std::io::Error) -> Self {
        Self::Storage {
            path: path.into(),
            detail: e.to_string(),
        }
    }

    pub fn registry(path: impl Into<String>, value: Option<&str>, e: std::io::Error) -> Self {
        Self::Registry {
            path: path.into(),
            value: value.map(str::to_owned),
            detail: e.to_string(),
        }
    }

    pub fn win32(call: &'static str, e: windows::core::Error) -> Self {
        Self::Win32 {
            call,
            code: e.code().0 as u32,
            detail: e.message(),
        }
    }
}
