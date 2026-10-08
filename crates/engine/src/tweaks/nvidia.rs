//! NVIDIA settings (CATALOGUE H20): NVIDIA Control Panel's global 3D settings,
//! changed in the driver's own settings store through NvAPI (`nvapi.rs`).
//!
//! Each tool sets one Control Panel setting (one or a few driver values) in
//! the base profile, the one "Manage 3D settings > Global Settings" shows.
//! What each value held before, or that it had none of its own, is
//! journalled first, and Undo puts it back or returns it to the driver's
//! default. Games pick a change up when they next start. A game's own profile
//! from NVIDIA can still set a value for that game, as in Control Panel.
//!
//! Low latency mode and power management are what Epic's Fortnite guidance
//! names (plan 8); the others are Hone's.
//!
//! Not offered on a PC without an NVIDIA card and driver. Not built: shader
//! cache size (its values are not checked; NOTES N86).
//!
//! VERIFY (NOTES N86): setting ids and values as recalled from NVIDIA's
//! `NvApiDriverSettings.h` and NVIDIA Profile Inspector; which values Control
//! Panel writes for Low Latency Mode "Ultra".

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::{EngineError, Result};
use crate::system::{SysItem, SysState};
use crate::transaction::Transaction;
use crate::types::{ExecutionContext, Impact, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState};

/// Maximum pre-rendered frames.
pub const PRERENDER_LIMIT: u32 = 0x007B_A09E;
/// Low Latency Mode as Control Panel shows it: 0 Off, 1 On, 2 Ultra.
pub const LOW_LATENCY_STATE: u32 = 0x0005_F543;
/// Ultra low latency (just-in-time frame submission): 0 off, 1 on.
pub const LOW_LATENCY_ULTRA: u32 = 0x1083_5000;
/// Power management mode (`PREFERRED_PSTATE`): 1 prefer maximum performance.
pub const POWER_MANAGEMENT: u32 = 0x1057_EB71;
/// Texture filtering - Quality (`QUALITY_ENHANCEMENTS`): 0x14 high performance.
pub const TEXTURE_QUALITY: u32 = 0x00CE_2691;
/// Threaded optimization (`OGL_THREAD_CONTROL`): 1 on, 2 off.
pub const THREADED_OPTIMIZATION: u32 = 0x20C1_221E;
/// Vertical sync (`VSYNCMODE`): 0x08416747 force off.
pub const VERTICAL_SYNC: u32 = 0x00A8_79CF;

pub struct NvidiaSetting {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    /// What Control Panel shows, for the technical view.
    pub target: &'static str,
    pub safety: SafetyTier,
    pub tradeoff: Option<&'static str>,
    /// (setting id, value), all in the base profile.
    pub values: &'static [(u32, u32)],
}

fn item(setting: u32) -> SysItem {
    SysItem::NvidiaSetting {
        profile: String::new(),
        setting,
    }
}

impl Tweak for NvidiaSetting {
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
        self.values.iter().map(|(s, _)| item(*s)).collect()
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        for (setting, value) in self.values {
            match res.read_system(&item(*setting)) {
                Ok(SysState::Dword { value: v }) if v == *value => {}
                Ok(_) => return Ok(TweakState::Default),
                // No NVIDIA card (any more): not available. Undo of an
                // earlier change still finishes (`Transaction::revert`).
                Err(EngineError::Blocked { reason }) => return Ok(TweakState::Blocked { reason }),
                Err(e) => return Err(e),
            }
        }
        Ok(if has_journal_entry {
            TweakState::Applied
        } else {
            TweakState::Foreign
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        for (setting, value) in self.values {
            tx.set_system(item(*setting), SysState::Dword { value: *value })?;
        }
        Ok(())
    }
}

pub const LOW_LATENCY: NvidiaSetting = NvidiaSetting {
    id: "nvidia.lowlatency",
    name: "NVIDIA Low Latency Mode: Ultra",
    summary: "Sets Low Latency Mode to Ultra in NVIDIA Control Panel's global settings, as Epic's Fortnite guidance \
              suggests: the driver hands each frame to the graphics card just in time instead of queuing frames \
              ahead. Games with NVIDIA Reflex turned on use Reflex instead.",
    target: "Manage 3D settings > Low Latency Mode: Ultra",
    safety: SafetyTier::Safe,
    tradeoff: None,
    values: &[(LOW_LATENCY_STATE, 2), (LOW_LATENCY_ULTRA, 1), (PRERENDER_LIMIT, 1)],
};

pub const POWER: NvidiaSetting = NvidiaSetting {
    id: "nvidia.powermax",
    name: "NVIDIA power management: prefer maximum performance",
    summary: "Sets Power management mode to Prefer maximum performance in NVIDIA Control Panel's global settings, \
              as Epic's Fortnite guidance suggests: the graphics card keeps its higher clock speeds while a game \
              runs instead of lowering them when the load drops.",
    target: "Manage 3D settings > Power management mode: Prefer maximum performance",
    safety: SafetyTier::Safe,
    tradeoff: Some("The graphics card uses more power while any program draws 3D graphics, also on battery."),
    values: &[(POWER_MANAGEMENT, 1)],
};

pub const TEXTURES: NvidiaSetting = NvidiaSetting {
    id: "nvidia.texturefiltering",
    name: "NVIDIA texture filtering: high performance",
    summary: "Sets Texture filtering - Quality to High performance in NVIDIA Control Panel's global settings: the \
              driver does less work when it filters textures.",
    target: "Manage 3D settings > Texture filtering - Quality: High performance",
    safety: SafetyTier::Moderate,
    tradeoff: Some("Textures can look blurrier or shimmer in the distance."),
    values: &[(TEXTURE_QUALITY, 0x14)],
};

pub const THREADED: NvidiaSetting = NvidiaSetting {
    id: "nvidia.threadedoptimization",
    name: "NVIDIA threaded optimization: on",
    summary: "Turns Threaded optimization on in NVIDIA Control Panel's global settings, so the driver spreads its \
              work for OpenGL games, such as Minecraft: Java Edition, across more processor cores. DirectX and \
              Vulkan games do not use it.",
    target: "Manage 3D settings > Threaded optimization: On",
    safety: SafetyTier::Moderate,
    tradeoff: Some(
        "Some older OpenGL programs stutter or close with it forced on. The default, Auto, lets the driver choose \
         per program.",
    ),
    values: &[(THREADED_OPTIMIZATION, 1)],
};

pub const VSYNC_OFF: NvidiaSetting = NvidiaSetting {
    id: "nvidia.vsyncoff",
    name: "NVIDIA vertical sync: off",
    summary: "Sets Vertical sync to Off in NVIDIA Control Panel's global settings, so the driver shows each frame \
              as soon as it is ready instead of waiting for the screen to refresh, whatever a game's own setting \
              says.",
    target: "Manage 3D settings > Vertical sync: Off",
    safety: SafetyTier::Moderate,
    tradeoff: Some("Screen tearing can show, also on a G-SYNC or FreeSync monitor."),
    values: &[(VERTICAL_SYNC, 0x0841_6747)],
};

pub fn all() -> Vec<Box<dyn Tweak>> {
    vec![
        Box::new(LOW_LATENCY),
        Box::new(POWER),
        Box::new(TEXTURES),
        Box::new(THREADED),
        Box::new(VSYNC_OFF),
    ]
}
