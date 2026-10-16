//! Scheduled tasks a player can do without (CATALOGUE H14, the scheduled-task
//! half; the telemetry service is in `services.rs`, the policy and user values
//! in `registry_values.rs`).
//!
//! Each task is disabled through `Transaction::set_system`, so whether it was
//! enabled is journalled first and Undo puts it back. Tasks this PC does not
//! have are skipped; a tool whose tasks are all missing is not offered.
//!
//! Left out on purpose: the Microsoft Compatibility Appraiser, which Windows
//! Update uses to check a PC for the next version of Windows (Kegan's brief:
//! nothing that touches Windows Update). `never_do_audit` refuses any task in
//! Windows Update's or Defender's own folders.
//!
//! VERIFY: the task paths are Windows' own as recalled (NOTES N78). The Windows
//! CI step "Power and service tools on this runner" disables and re-enables the
//! ones the runner has.

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::system::{SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState,
};

pub struct TaskTweak {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub category: &'static str,
    /// Full paths from the Task Scheduler root, `\Folder\Name`.
    pub tasks: &'static [&'static str],
    pub safety: SafetyTier,
    pub tradeoff: Option<&'static str>,
}

fn item(path: &str) -> SysItem {
    SysItem::ScheduledTask { path: path.to_owned() }
}

impl TaskTweak {
    /// The tasks of this tool that exist on this PC, with whether each is on.
    fn present(&self, res: &ContextResolver) -> Result<Vec<(&'static str, bool)>> {
        let mut out = Vec::new();
        for path in self.tasks {
            match res.read_system(&item(path))? {
                SysState::Absent => {}
                SysState::Bool { on } => out.push((*path, on)),
                other => {
                    return Err(EngineError::Internal {
                        detail: format!("scheduled task {path} read as {other:?}"),
                    })
                }
            }
        }
        Ok(out)
    }

    fn not_here() -> BlockedReason {
        BlockedReason::new(
            BlockedCode::OsVersionUnsupported,
            "These scheduled tasks are not part of this edition of Windows.",
        )
    }
}

impl Tweak for TaskTweak {
    fn id(&self) -> &str {
        self.id
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(self.id),
            name: Cow::Borrowed(self.name),
            summary: Cow::Borrowed(self.summary),
            target: Cow::Owned(format!("Scheduled tasks {}: disabled", self.tasks.join(", "))),
            category: Cow::Borrowed(self.category),
            tier: Tier::Pro,
            safety: self.safety,
            impact: Impact::Moderate,
            tradeoff: self.tradeoff.map(Cow::Borrowed),
            requires_reboot: false,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Service
    }

    fn touches(&self) -> Vec<RegTarget> {
        Vec::new()
    }

    fn system_targets(&self) -> Vec<SysItem> {
        self.tasks.iter().map(|p| item(p)).collect()
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let present = self.present(res)?;
        // Our own change stays shown, so Undo stays in reach.
        if present.is_empty() && !has_journal_entry {
            return Ok(TweakState::Blocked {
                reason: Self::not_here(),
            });
        }
        Ok(if present.iter().any(|(_, on)| *on) {
            TweakState::Default
        } else if has_journal_entry {
            TweakState::Applied
        } else {
            TweakState::Foreign
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let present = self.present(tx.resolver())?;
        if present.is_empty() {
            return Err(EngineError::Blocked {
                reason: Self::not_here(),
            });
        }
        for (path, _) in present {
            tx.set_system(item(path), SysState::Bool { on: false })?;
        }
        Ok(())
    }
}

/// H14. Tasks that collect usage and diagnostic data for Microsoft.
pub const TELEMETRY_TASKS: TaskTweak = TaskTweak {
    id: "privacy.telemetrytasks",
    name: "Telemetry scheduled tasks",
    summary: "Turns off the scheduled tasks that collect usage and diagnostic data about this PC for Microsoft: \
              the Customer Experience Improvement Program, the Application Experience inventory, the disk \
              diagnostic data collector and the Autochk proxy.",
    category: "privacy",
    tasks: &[
        r"\Microsoft\Windows\Customer Experience Improvement Program\Consolidator",
        r"\Microsoft\Windows\Customer Experience Improvement Program\UsbCeip",
        r"\Microsoft\Windows\Application Experience\ProgramDataUpdater",
        r"\Microsoft\Windows\DiskDiagnostic\Microsoft-Windows-DiskDiagnosticDataCollector",
        r"\Microsoft\Windows\Autochk\Proxy",
    ],
    safety: SafetyTier::Safe,
    tradeoff: None,
};

pub fn all() -> Vec<Box<dyn Tweak>> {
    vec![Box::new(TELEMETRY_TASKS)]
}
