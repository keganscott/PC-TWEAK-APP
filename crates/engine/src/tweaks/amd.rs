//! AMD settings (CATALOGUE H21): AMD Software's global graphics settings,
//! changed through ADLX, AMD's published settings library (`adlx.rs`).
//!
//! The catalogue planned per-adapter registry values under the display class
//! key; those are AMD's private storage, undocumented, and AMD Software may
//! not read changes made there. ADLX is the interface AMD publishes for this,
//! so it is used instead (DECISIONS 15.24).
//!
//! Each tool sets its setting on every AMD graphics card that has it, so a
//! PC with AMD integrated graphics next to a Radeon card gets both. What each
//! card had before is journalled first and Undo puts it back; a card removed
//! since is noted and skipped.
//!
//! Radeon Anti-Lag here is plain Anti-Lag, which paces the processor from
//! the driver. Drivers since late 2023 also offer "Anti-Lag Next", which AMD
//! describes as "an advanced algorithm in supported DX11 and DX12 games";
//! the tool sets the level to plain Anti-Lag before turning it on, so it
//! never turns Next on (NOTES N87). Turning Anti-Lag on makes the driver turn
//! Radeon Chill off (AMD: the two cannot be on together), so the tool turns
//! Chill off itself first, journalled, and Undo turns it back on.
//!
//! The frame cap (`amd.framecap`) turns on Radeon Frame Rate Target Control
//! at the main display's refresh rate minus 3, the same cap as
//! `nvidia.framecap`. It turns Chill off first, journalled, as Chill sets its
//! own frame rate range. VERIFY (NOTES N144): whether AMD Software lets
//! Chill and Frame Rate Target Control be on together.

use std::borrow::Cow;

use crate::adlx::{anti_lag_level, vsync, AmdGpu, Setting};
use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::system::{SysItem, SysState};
use crate::transaction::Transaction;
use crate::tweaks::refresh::main_display_frame_cap;
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState,
};

pub struct AmdSetting {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    /// What AMD Software shows, for the technical view.
    pub target: &'static str,
    pub safety: SafetyTier,
    pub tradeoff: Option<&'static str>,
    /// (setting, value), set in this order. A card has the tool when it has
    /// the last setting, the one the tool is named for; the ones before it
    /// prepare it and are set where the card has them.
    pub values: &'static [(Setting, u32)],
}

fn item(gpu: &AmdGpu, setting: Setting) -> SysItem {
    SysItem::AmdSetting {
        gpu: gpu.id.clone(),
        setting: setting.key().into(),
    }
}

impl AmdSetting {
    fn main(&self) -> Setting {
        self.values[self.values.len() - 1].0
    }

    /// The AMD cards that have this tool's setting, or why there are none.
    fn cards(&self, res: &ContextResolver) -> Result<Vec<AmdGpu>> {
        cards_with(res, self.main())
    }
}

/// The AMD cards that have `main`, or why there are none.
fn cards_with(res: &ContextResolver, main: Setting) -> Result<Vec<AmdGpu>> {
    let all = res.amd_gpus()?;
    if all.is_empty() {
        return Err(EngineError::Blocked {
            reason: BlockedReason::new(BlockedCode::HardwareUnsupported, "This PC has no AMD graphics card."),
        });
    }
    let mut with = Vec::new();
    for g in all {
        if res.read_system(&item(&g, main))? != SysState::Absent {
            with.push(g);
        }
    }
    if with.is_empty() {
        return Err(EngineError::Blocked {
            reason: BlockedReason::new(
                BlockedCode::HardwareUnsupported,
                format!("This PC's AMD graphics does not have {}.", main.label()),
            ),
        });
    }
    Ok(with)
}

impl Tweak for AmdSetting {
    fn id(&self) -> &str {
        self.id
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(self.id),
            name: Cow::Borrowed(self.name),
            summary: Cow::Borrowed(self.summary),
            target: Cow::Borrowed(self.target),
            category: Cow::Borrowed("graphics"),
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
        self.values
            .iter()
            .map(|(setting, _)| SysItem::AmdSetting {
                gpu: "*".into(),
                setting: setting.key().into(),
            })
            .collect()
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let cards = match self.cards(res) {
            Ok(c) => c,
            Err(EngineError::Blocked { reason }) => return Ok(TweakState::Blocked { reason }),
            Err(e) => return Err(e),
        };
        for g in &cards {
            for (setting, value) in self.values {
                match res.read_system(&item(g, *setting))? {
                    SysState::Absent => {}
                    s if s == (SysState::Dword { value: *value }) => {}
                    _ => return Ok(TweakState::Default),
                }
            }
        }
        Ok(if has_journal_entry {
            TweakState::Applied
        } else {
            TweakState::Foreign
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        for g in self.cards(tx.resolver())? {
            for (setting, value) in self.values {
                let target = item(&g, *setting);
                if tx.resolver().read_system(&target)? != SysState::Absent {
                    tx.set_system(target, SysState::Dword { value: *value })?;
                }
            }
        }
        Ok(())
    }
}

pub const ANTI_LAG: AmdSetting = AmdSetting {
    id: "amd.antilag",
    name: "Radeon Anti-Lag: on",
    summary: "Turns on Radeon Anti-Lag in AMD Software's global graphics settings: the driver paces the processor \
              so it does not run ahead of the graphics card and queue up frames.",
    target: "AMD Software > Gaming > Graphics > Radeon Anti-Lag: Enabled (Anti-Lag, not Anti-Lag Next); Radeon \
             Chill: Disabled",
    safety: SafetyTier::Safe,
    tradeoff: Some("Radeon Chill is turned off if it is on: AMD does not allow both."),
    values: &[
        (Setting::AntiLagLevel, anti_lag_level::ANTI_LAG),
        (Setting::Chill, 0),
        (Setting::AntiLag, 1),
    ],
};

pub const VSYNC_OFF: AmdSetting = AmdSetting {
    id: "amd.vsyncoff",
    name: "AMD Wait for Vertical Refresh: always off",
    summary: "Sets Wait for Vertical Refresh to Always off in AMD Software's global graphics settings, so the \
              driver shows each frame as soon as it is ready instead of waiting for the screen to refresh, \
              whatever a game's own setting says.",
    target: "AMD Software > Gaming > Graphics > Wait for Vertical Refresh: Always off",
    safety: SafetyTier::Moderate,
    tradeoff: Some("Screen tearing can show, also on a FreeSync monitor."),
    values: &[(Setting::WaitForVerticalRefresh, vsync::ALWAYS_OFF)],
};

pub const SUPER_RESOLUTION: AmdSetting = AmdSetting {
    id: "amd.rsr",
    name: "Radeon Super Resolution: on",
    summary: "Turns on Radeon Super Resolution in AMD Software's graphics settings. When a game runs in exclusive \
              fullscreen at a resolution below the display's, the driver scales its picture up to the display's \
              resolution with AMD FidelityFX Super Resolution 1 and sharpens it. Choosing that lower resolution in \
              the game is up to you; at the display's own resolution nothing changes.",
    target: "AMD Software > Gaming > Graphics > Radeon Super Resolution: Enabled",
    safety: SafetyTier::Safe,
    tradeoff: Some(
        "A game at a lower resolution looks softer than at the display's own. Only exclusive fullscreen games \
         use it.",
    ),
    values: &[(Setting::SuperResolution, 1)],
};

pub const FRAME_CAP_ID: &str = "amd.framecap";

/// Radeon Frame Rate Target Control just under the main display's refresh
/// rate, on every AMD card that has it. Follows the display like
/// `nvidia::FrameCap`.
pub struct FrameCap;

impl FrameCap {
    /// The cards and the cap, or why the tool is not offered.
    fn plan(res: &ContextResolver) -> Result<(Vec<AmdGpu>, u32)> {
        let cards = cards_with(res, Setting::FrameRateTarget)?;
        let cap = main_display_frame_cap(res, 0x3ff)?.map_err(|reason| EngineError::Blocked { reason })?;
        Ok((cards, cap))
    }
}

impl Tweak for FrameCap {
    fn id(&self) -> &str {
        FRAME_CAP_ID
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(FRAME_CAP_ID),
            name: Cow::Borrowed("AMD frame rate limit: just under the refresh rate"),
            summary: Cow::Borrowed(
                "Turns on Radeon Frame Rate Target Control in AMD Software's global graphics settings, at 3 frames \
                 per second below the refresh rate the main display runs at, the margin Blur Busters' G-SYNC \
                 tests recommend: with FreeSync, games stay inside the range the display follows, and the graphics \
                 card does not draw frames the display cannot show. If the refresh rate changes later, apply it \
                 again.",
            ),
            target: Cow::Borrowed(
                "AMD Software > Gaming > Graphics > Frame Rate Target Control: Enabled, refresh rate minus 3; \
                 Radeon Chill: Disabled",
            ),
            category: Cow::Borrowed("graphics"),
            tier: Tier::Pro,
            safety: SafetyTier::Safe,
            impact: Impact::Moderate,
            tradeoff: Some(Cow::Borrowed(
                "No game runs above that frame rate. Radeon Chill is turned off if it is on.",
            )),
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
        [Setting::Chill, Setting::FrameRateTarget]
            .into_iter()
            .map(|setting| SysItem::AmdSetting {
                gpu: "*".into(),
                setting: setting.key().into(),
            })
            .collect()
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let (cards, cap) = match Self::plan(res) {
            Ok(p) => p,
            Err(EngineError::Blocked { reason }) => return Ok(TweakState::Blocked { reason }),
            Err(e) => return Err(e),
        };
        for g in &cards {
            if res.read_system(&item(g, Setting::FrameRateTarget))? != (SysState::Dword { value: cap }) {
                return Ok(TweakState::Default);
            }
        }
        Ok(if has_journal_entry {
            TweakState::Applied
        } else {
            TweakState::Foreign
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        let (cards, cap) = Self::plan(tx.resolver())?;
        for g in cards {
            let chill = item(&g, Setting::Chill);
            if !matches!(
                tx.resolver().read_system(&chill)?,
                SysState::Absent | SysState::Dword { value: 0 }
            ) {
                tx.set_system(chill, SysState::Dword { value: 0 })?;
            }
            tx.set_system(item(&g, Setting::FrameRateTarget), SysState::Dword { value: cap })?;
        }
        Ok(())
    }
}

pub fn all() -> Vec<Box<dyn Tweak>> {
    vec![
        Box::new(ANTI_LAG),
        Box::new(FrameCap),
        Box::new(SUPER_RESOLUTION),
        Box::new(VSYNC_OFF),
    ]
}
