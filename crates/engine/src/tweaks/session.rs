//! Gaming Mode (CATALOGUE H5, H15): changes made while a game runs and put
//! back when it closes (`play.rs`, `Engine::start_play_session`).
//!
//! They are internal tweaks: not listed in Tools, but journalled like every
//! change, with a `.reg` backup first, listed in Backups while in effect, and
//! reached by Undo all. A session a crash or power cut left open is put back
//! the next time PeakTweaks starts and sees no game running.
//!
//! Hone's Gaming Mode also pauses Windows Update and raises the game's
//! priority. The first is left out (Kegan's brief: nothing that touches
//! Windows Update); the second is the per-game tool H31.
//!
//! VERIFY: that a changed `ToastEnabled` takes effect without signing out, as
//! the switch in Settings does (NOTES N80).

use std::borrow::Cow;

use super::registry_values::{dword, ValueTweak};
use crate::context::ContextResolver;
use crate::error::Result;
use crate::system::{SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{ExecutionContext, Impact, RegRoot, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState};

pub const PUSH_NOTIFICATIONS: &str = r"Software\Microsoft\Windows\CurrentVersion\PushNotifications";

pub const NOTIFICATIONS_ID: &str = "session.notifications";
pub const SEARCH_PAUSE_ID: &str = "session.searchpause";

/// Every Gaming Mode change, in the order they are made.
pub const SESSION_IDS: &[&str] = &[NOTIFICATIONS_ID, SEARCH_PAUSE_ID];

/// H15. The switch under Settings > System > Notifications.
pub const QUIET_NOTIFICATIONS: ValueTweak = ValueTweak {
    id: NOTIFICATIONS_ID,
    name: "Notifications off while playing",
    summary: "Turns off pop-up notifications while a game runs and back on when it closes.",
    category: "session",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: false,
    settings: &[dword(PUSH_NOTIFICATIONS, "ToastEnabled", 0)],
};

/// H5, the indexing part. The Windows Search service is stopped while a game
/// runs and started again when it closes; how it starts with Windows is kept.
pub struct PauseSearchIndexing;

fn wsearch() -> SysItem {
    SysItem::Service {
        name: "WSearch".to_owned(),
    }
}

impl Tweak for PauseSearchIndexing {
    fn id(&self) -> &str {
        SEARCH_PAUSE_ID
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(SEARCH_PAUSE_ID),
            name: Cow::Borrowed("Search indexing paused while playing"),
            summary: Cow::Borrowed(
                "Stops the Windows Search service while a game runs and starts it again when the game closes.",
            ),
            target: Cow::Borrowed("Service WSearch: stopped while a game runs"),
            category: Cow::Borrowed("session"),
            tier: Tier::Pro,
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
        Vec::new()
    }

    fn system_targets(&self) -> Vec<SysItem> {
        vec![wsearch()]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        Ok(match res.read_system(&wsearch())? {
            SysState::Service { running: true, .. } => TweakState::Default,
            _ if has_journal_entry => TweakState::Applied,
            // Stopped already, or not installed: nothing to pause.
            _ => TweakState::Foreign,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        if let SysState::Service { start, running: true } = tx.resolver().read_system(&wsearch())? {
            tx.set_system(wsearch(), SysState::Service { start, running: false })?;
        }
        Ok(())
    }
}

pub fn all() -> Vec<Box<dyn Tweak>> {
    vec![Box::new(QUIET_NOTIFICATIONS), Box::new(PauseSearchIndexing)]
}
