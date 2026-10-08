//! PeakTweaks engine: journalled, reversible system changes.
//!
//! Deliberately free of Tauri types so the logic runs and is tested on any OS.
//! The Windows-only pieces (`identity`, `registry::windows`, the ProgramData
//! ACL code) sit behind `cfg(windows)`.

pub mod background;
pub mod cleanup;
pub mod context;
pub mod drive_optimize;
pub mod engine;
pub mod env;
pub mod error;
pub mod fsutil;
pub mod game_installs;
pub mod gpu_choice;
pub mod gpu_driver;
pub mod hardware;
pub mod ini;
pub mod instance;
pub mod journal;
pub mod memory;
pub mod offline;
pub mod play;
pub mod power;
pub mod probe;
pub mod proc;
pub mod profile;
pub mod proof;
pub mod reg_export;
pub mod registry;
pub mod restore;
pub mod scanner;
pub mod secure_dir;
pub mod security;
pub mod settings;
pub mod sysdirs;
pub mod sysprobe;
pub mod system;
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
#[cfg(windows)]
pub mod system_win;

#[cfg(test)]
mod contract;
#[cfg(test)]
mod copy_lint;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod network_audit;
#[cfg(test)]
mod never_do_audit;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod testutil;

pub use engine::{AppliedChange, ChangeKind, ContextInfo, Engine, JournalView, Progress, RevertResult, TweakView};
pub use sysprobe::SystemAudit;
