//! Plan section 6.2, item 3: a display set below the highest refresh rate it
//! offers, put at the highest. The scanner's "Display refresh rate" finding
//! names this as its fix.
//!
//! Each display keeps its resolution and colour depth; only a rate Windows
//! itself lists for that mode is set (`display_win.rs`), the same list
//! Settings > System > Display > Advanced display offers. Every display's
//! rate is journalled before it changes, and Undo sets each back.

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::system::{SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState,
};

pub const ID: &str = "display.refresh_max";

pub struct HighestRefreshRate;

/// Below its highest rate by more than rounding: 59.94 Hz shows as 59 or 60
/// depending on the driver, as the scanner allows (`scanner::refresh_rate`).
fn below_highest(d: &crate::system::Display) -> bool {
    d.current_hz + 1 < d.max_hz
}

fn no_display() -> BlockedReason {
    BlockedReason::new(BlockedCode::HardwareUnsupported, "Windows lists no display on this PC.")
}

impl Tweak for HighestRefreshRate {
    fn id(&self) -> &str {
        ID
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: ID.into(),
            name: "Highest refresh rate".into(),
            summary: "Sets each display to the highest refresh rate Windows offers for it at the resolution it \
                      has now, the same choice as in Settings > System > Display > Advanced display."
                .into(),
            target: "Each display's refresh rate (Settings > System > Display > Advanced display)".into(),
            category: "display".into(),
            tier: Tier::Pro,
            safety: SafetyTier::Safe,
            impact: Impact::Moderate,
            tradeoff: Some("On a laptop running on battery, a higher refresh rate uses more power.".into()),
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
        vec![SysItem::RefreshRate { display: "*".into() }]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let displays = res.displays()?;
        if displays.is_empty() {
            return Ok(TweakState::Blocked { reason: no_display() });
        }
        Ok(match (displays.iter().any(below_highest), has_journal_entry) {
            (true, _) => TweakState::Default,
            (false, true) => TweakState::Applied,
            (false, false) => TweakState::Foreign,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let displays = tx.resolver().displays()?;
        if displays.is_empty() {
            return Err(EngineError::Blocked { reason: no_display() });
        }
        for d in displays {
            if below_highest(&d) {
                tx.set_system(
                    SysItem::RefreshRate { display: d.device },
                    SysState::Dword { value: d.max_hz },
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::Display;
    use crate::testutil::Harness;

    fn display(device: &str, current_hz: u32, max_hz: u32) -> Display {
        Display {
            device: device.into(),
            name: "Generic PnP Monitor".into(),
            primary: device.ends_with('1'),
            width: 1920,
            height: 1080,
            current_hz,
            max_hz,
        }
    }

    fn state(h: &Harness) -> TweakState {
        h.engine
            .list()
            .unwrap()
            .into_iter()
            .find(|t| t.metadata.id == ID)
            .unwrap()
            .state
    }

    fn rates(h: &Harness) -> Vec<u32> {
        h.sys.displays_now().iter().map(|d| d.current_hz).collect()
    }

    #[test]
    fn each_display_below_its_highest_rate_is_raised_and_undo_puts_each_back() {
        let mut h = Harness::new(vec![Box::new(HighestRefreshRate)]);
        h.sys.set_displays(vec![
            display(r"\\.\DISPLAY1", 60, 144),
            display(r"\\.\DISPLAY2", 60, 60),
            display(r"\\.\DISPLAY3", 120, 165),
        ]);
        assert_eq!(state(&h), TweakState::Default);

        h.engine.apply(ID).unwrap();
        assert_eq!(rates(&h), vec![144, 60, 165]);
        assert_eq!(state(&h), TweakState::Applied);

        h.engine.revert(ID).unwrap();
        assert_eq!(rates(&h), vec![60, 60, 120]);
        assert_eq!(state(&h), TweakState::Default);
    }

    #[test]
    fn displays_already_at_their_highest_rate_read_as_already_set() {
        let h = Harness::new(vec![Box::new(HighestRefreshRate)]);
        h.sys.set_displays(vec![
            display(r"\\.\DISPLAY1", 144, 144),
            display(r"\\.\DISPLAY2", 59, 60),
        ]);
        assert_eq!(state(&h), TweakState::Foreign);
    }

    #[test]
    fn set_back_by_windows_or_the_user_reads_as_changed_and_can_be_applied_again() {
        let mut h = Harness::new(vec![Box::new(HighestRefreshRate)]);
        h.sys.set_displays(vec![display(r"\\.\DISPLAY1", 60, 144)]);
        h.engine.apply(ID).unwrap();
        h.sys.set_displays(vec![display(r"\\.\DISPLAY1", 60, 144)]);
        assert_eq!(state(&h), TweakState::Drifted);
        h.engine.apply(ID).unwrap();
        assert_eq!(rates(&h), vec![144]);
    }

    #[test]
    fn without_a_display_there_is_nothing_to_set() {
        let mut h = Harness::new(vec![Box::new(HighestRefreshRate)]);
        assert!(matches!(
            state(&h),
            TweakState::Blocked { reason } if reason.code == BlockedCode::HardwareUnsupported
        ));
        assert!(h.engine.apply(ID).is_err());
        assert!(h.engine.applied_tweak_ids().is_empty());
    }

    #[test]
    fn a_rate_windows_refuses_changes_no_display() {
        let mut h = Harness::new(vec![Box::new(HighestRefreshRate)]);
        h.sys.set_displays(vec![
            display(r"\\.\DISPLAY1", 60, 144),
            display(r"\\.\DISPLAY2", 60, 75),
        ]);
        h.sys.fail_writes(true);
        assert!(h.engine.apply(ID).is_err());
        h.sys.fail_writes(false);
        assert_eq!(rates(&h), vec![60, 60]);
        // Whatever is still on record can be undone.
        if !h.engine.applied_tweak_ids().is_empty() {
            h.engine.revert(ID).unwrap();
        }
        assert_eq!(rates(&h), vec![60, 60]);
    }
}
