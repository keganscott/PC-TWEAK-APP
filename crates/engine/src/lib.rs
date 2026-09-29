//! PeakTweaks engine: journalled, reversible system changes.
//!
//! Deliberately free of Tauri types so the logic runs and is tested on any OS.
//! The Windows-only pieces (`identity`, `registry::windows`, the ProgramData
//! ACL code) sit behind `cfg(windows)`.

pub mod context;
pub mod engine;
pub mod env;
pub mod error;
pub mod fsutil;
pub mod hardware;
pub mod journal;
pub mod probe;
pub mod profile;
pub mod proof;
pub mod reg_export;
pub mod registry;
pub mod restore;
pub mod scanner;
pub mod secure_dir;
pub mod security;
pub mod sysprobe;
pub mod timeutil;
pub mod transaction;
pub mod tweaks;
pub mod types;
pub mod wmi;

#[cfg(windows)]
pub mod identity;
#[cfg(windows)]
pub mod osfacts;
#[cfg(windows)]
pub mod restore_win;
#[cfg(windows)]
pub mod shell;

#[cfg(test)]
mod contract;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod testutil;

pub use engine::{ContextInfo, Engine, JournalView, Progress, RevertResult, TweakView};
pub use sysprobe::SystemAudit;
