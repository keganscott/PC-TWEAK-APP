//! Internal bootstrap tweak: lift Windows' one-restore-point-per-24-hours limit.
//!
//! `SystemRestorePointCreationFrequency = 0` makes Windows create a restore point
//! on request instead of silently skipping it when one already exists from the
//! last 1440 minutes.
//!
//! This is **not** in the catalogue and is not reachable from `apply_tweak`. It
//! is the one change allowed before the restore gate opens, because it is what
//! lets the first restore point be created. Everything else about it is normal:
//! it goes through `Transaction`, gets a `.reg` backup and a journal record, and
//! `revert_all` puts the previous value back.

use crate::context::ContextResolver;
use crate::error::Result;
use crate::transaction::Transaction;
use crate::types::{ExecutionContext, Impact, RegRoot, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState};

pub const ID: &str = "system.restore.frequency";
const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore";
const VALUE: &str = "SystemRestorePointCreationFrequency";

pub struct RestoreFrequency;

impl Tweak for RestoreFrequency {
    fn id(&self) -> &str {
        ID
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: ID.into(),
            name: "Allow a restore point on demand".into(),
            summary: "Lets PeakTweaks create a restore point even if Windows made one in the last day.".into(),
            target: format!(r"HKLM\{KEY}\{VALUE}").into(),
            category: "system".into(),
            tier: Tier::Free,
            safety: SafetyTier::Safe,
            impact: Impact::Moderate,
            tradeoff: None,
            requires_reboot: false,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Service
    }

    fn touches(&self) -> Vec<RegTarget> {
        vec![RegTarget::new(RegRoot::LocalMachine, KEY, &[VALUE])]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        Ok(match res.read_dword(RegRoot::LocalMachine, KEY, VALUE)? {
            Some(0) if has_journal_entry => TweakState::Applied,
            Some(0) => TweakState::Foreign,
            _ => TweakState::Default,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        tx.set_dword(RegRoot::LocalMachine, KEY, VALUE, 0)
    }
}
