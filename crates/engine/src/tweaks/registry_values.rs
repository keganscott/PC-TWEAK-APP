//! Tweaks that are nothing more than a fixed set of registry values.
//!
//! Most Windows settings a gamer changes by hand are one or a few values under
//! one key. Rather than a file per setting, each is a `ValueTweak`: a list of
//! `Setting`s plus its card text. Apply writes every value through the
//! transaction; revert is the default journal replay; state reads every value
//! and compares.
//!
//! **State.** All values at the wanted data: `Applied` when the journal has our
//! apply, otherwise `Foreign` (already set on this PC, shown as done rather than
//! hidden). Anything else, including a value of another type, is `Default`.
//! `absent_matches` covers settings whose Windows default, with no value
//! written, is already the wanted one (Game Mode).
//!
//! **Evidence.** Kegan's `tweak-dictionary.md` (NOTES N2) has never been in the
//! repository, so none of these has an evidence grade from it. Each one is a
//! setting Windows documents or exposes in Settings, and the card text says what
//! the setting does, never what it gains (copy lint). NOTES N64.
//!
//! VERIFY: the key paths, value names and data below are from Microsoft's
//! documentation and Windows' own Settings pages as recalled, not yet checked
//! against a primary source or a real PC. The field check reads them.

use std::borrow::Cow;

use crate::context::ContextResolver;
use crate::error::Result;
use crate::transaction::Transaction;
use crate::types::{ExecutionContext, Impact, RegRoot, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState};

#[derive(Debug, Clone, Copy)]
pub enum Data {
    Dword(u32),
    /// Written as `REG_SZ`.
    Str(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub key: &'static str,
    pub value: &'static str,
    pub data: Data,
    /// True when an absent value already behaves like `data`.
    pub absent_matches: bool,
}

const fn dword(key: &'static str, value: &'static str, data: u32) -> Setting {
    Setting {
        key,
        value,
        data: Data::Dword(data),
        absent_matches: false,
    }
}

const fn string(key: &'static str, value: &'static str, data: &'static str) -> Setting {
    Setting {
        key,
        value,
        data: Data::Str(data),
        absent_matches: false,
    }
}

pub struct ValueTweak {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub category: &'static str,
    pub root: RegRoot,
    pub safety: SafetyTier,
    pub tradeoff: Option<&'static str>,
    pub requires_reboot: bool,
    pub settings: &'static [Setting],
}

impl ValueTweak {
    fn target(&self) -> String {
        let root = match self.root {
            RegRoot::InteractiveUser => r"HKEY_USERS\<sid>",
            RegRoot::LocalMachine => "HKLM",
            RegRoot::ClassesRoot => "HKCR",
        };
        let mut parts: Vec<String> = Vec::new();
        for s in self.settings {
            let shown = match s.data {
                Data::Dword(n) => format!(r"{root}\{}\{} = {n}", s.key, s.value),
                Data::Str(v) => format!(r#"{root}\{}\{} = "{v}""#, s.key, s.value),
            };
            parts.push(shown);
        }
        parts.join("; ")
    }

    fn matches(&self, res: &ContextResolver, s: &Setting) -> Result<bool> {
        Ok(match res.read_raw(self.root, s.key, s.value)? {
            None => s.absent_matches,
            Some(raw) => match s.data {
                Data::Dword(n) => raw.as_dword() == Some(n),
                Data::Str(v) => raw.as_sz().as_deref() == Some(v),
            },
        })
    }
}

impl Tweak for ValueTweak {
    fn id(&self) -> &str {
        self.id
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: Cow::Borrowed(self.id),
            name: Cow::Borrowed(self.name),
            summary: Cow::Borrowed(self.summary),
            target: self.target().into(),
            category: Cow::Borrowed(self.category),
            tier: Tier::Pro,
            safety: self.safety,
            impact: Impact::Moderate,
            tradeoff: self.tradeoff.map(Cow::Borrowed),
            requires_reboot: self.requires_reboot,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        self.root.required_context()
    }

    fn touches(&self) -> Vec<RegTarget> {
        let mut out: Vec<RegTarget> = Vec::new();
        for s in self.settings {
            match out.iter_mut().find(|t| t.key.eq_ignore_ascii_case(s.key)) {
                Some(t) => t.values.push(s.value.to_owned()),
                None => out.push(RegTarget::new(self.root, s.key, &[s.value])),
            }
        }
        out
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        for s in self.settings {
            if !self.matches(res, s)? {
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
        for s in self.settings {
            match s.data {
                Data::Dword(n) => tx.set_dword(self.root, s.key, s.value, n)?,
                Data::Str(v) => tx.set_string(self.root, s.key, s.value, v)?,
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The settings
// ---------------------------------------------------------------------------

const GAME_BAR: &str = r"Software\Microsoft\GameBar";
const GAME_CONFIG: &str = r"System\GameConfigStore";
const GAME_DVR: &str = r"Software\Microsoft\Windows\CurrentVersion\GameDVR";
const STICKY_KEYS: &str = r"Control Panel\Accessibility\StickyKeys";
const FILTER_KEYS: &str = r"Control Panel\Accessibility\Keyboard Response";
const TOGGLE_KEYS: &str = r"Control Panel\Accessibility\ToggleKeys";
const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const BG_APPS: &str = r"Software\Microsoft\Windows\CurrentVersion\BackgroundAccessApplications";
const GRAPHICS: &str = r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers";
const POWER_THROTTLING: &str = r"SYSTEM\CurrentControlSet\Control\Power\PowerThrottling";
const MMCSS: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile";
const MMCSS_GAMES: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile\Tasks\Games";

/// Settings > Gaming > Game Mode. On by default since Windows 10 1903, so an
/// absent value counts as on and most PCs show this as already done.
pub const GAME_MODE: ValueTweak = ValueTweak {
    id: "gaming.gamemode",
    name: "Game Mode",
    summary: "Keeps Windows Game Mode on. While a game runs, Windows Update holds off installing drivers and \
              showing restart notifications.",
    category: "gaming",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: false,
    settings: &[Setting {
        absent_matches: true,
        ..dword(GAME_BAR, "AutoGameModeEnabled", 1)
    }],
};

/// Settings > Gaming > Captures > "Record what happened". Two values: the
/// per-user capture switch and the Game DVR switch Game Bar reads.
pub const BACKGROUND_CAPTURE: ValueTweak = ValueTweak {
    id: "gaming.capture",
    name: "Background game recording",
    summary: "Turns off Xbox Game Bar's game capture, which can keep recording the last few minutes of play in \
              the background.",
    category: "gaming",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: Some("Xbox Game Bar stops recording clips and screenshots of games until you undo this."),
    requires_reboot: false,
    settings: &[
        dword(GAME_CONFIG, "GameDVR_Enabled", 0),
        dword(GAME_DVR, "AppCaptureEnabled", 0),
    ],
};

/// Settings > Accessibility > Keyboard: the keyboard shortcuts for Sticky,
/// Filter and Toggle Keys. The Flags strings are Windows' defaults (510, 126,
/// 62) with the hotkey bit cleared.
pub const ACCESSIBILITY_SHORTCUTS: ValueTweak = ValueTweak {
    id: "input.accessibilitykeys",
    name: "Sticky Keys pop-ups",
    summary: "Stops the Sticky Keys, Filter Keys and Toggle Keys prompts that open when Shift is pressed five \
              times or held down during a game. The features stay available in Settings.",
    category: "input",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: true,
    settings: &[
        string(STICKY_KEYS, "Flags", "506"),
        string(FILTER_KEYS, "Flags", "122"),
        string(TOGGLE_KEYS, "Flags", "58"),
    ],
};

/// Settings > Personalization > Colors > Transparency effects.
pub const TRANSPARENCY: ValueTweak = ValueTweak {
    id: "display.transparency",
    name: "Transparency effects",
    summary: "Turns off the see-through effect on the taskbar, Start and window frames, so Windows draws them as \
              solid colours.",
    category: "display",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: false,
    settings: &[dword(PERSONALIZE, "EnableTransparency", 0)],
};

/// The per-user switch that stops Store apps running in the background.
pub const BACKGROUND_APPS: ValueTweak = ValueTweak {
    id: "system.backgroundapps",
    name: "Background Store apps",
    summary: "Stops apps from the Microsoft Store from running in the background when they are closed.",
    category: "system",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Moderate,
    tradeoff: Some(
        "Store apps such as Mail, Calendar and the Xbox app stop updating and showing notifications while they are \
         closed.",
    ),
    requires_reboot: true,
    settings: &[dword(BG_APPS, "GlobalUserDisabled", 1)],
};

/// Settings > Display > Graphics > Hardware-accelerated GPU scheduling.
/// 2 = on, 1 = off.
pub const GPU_SCHEDULING: ValueTweak = ValueTweak {
    id: "display.gpuscheduling",
    name: "Hardware-accelerated GPU scheduling",
    summary: "Turns on the Windows setting that lets the graphics card manage its own memory and work queue.",
    category: "display",
    root: RegRoot::LocalMachine,
    safety: SafetyTier::Moderate,
    tradeoff: Some(
        "Windows ignores it unless the graphics card and its driver support it. Some older games and recording \
         tools have had problems with it on; Undo puts it back.",
    ),
    requires_reboot: true,
    settings: &[dword(GRAPHICS, "HwSchMode", 2)],
};

/// Power throttling (EcoQoS) for background programs, system-wide.
pub const POWER_THROTTLING_OFF: ValueTweak = ValueTweak {
    id: "system.powerthrottling",
    name: "Power throttling",
    summary: "Stops Windows from running background programs in its low-power mode to save energy.",
    category: "system",
    root: RegRoot::LocalMachine,
    safety: SafetyTier::Moderate,
    tradeoff: Some(
        "On a laptop running on battery, background programs use more power and the battery runs down sooner.",
    ),
    requires_reboot: true,
    settings: &[dword(POWER_THROTTLING, "PowerThrottlingOff", 1)],
};

/// Multimedia Class Scheduler: CPU share held back for background work.
/// Windows' default is 20.
pub const SYSTEM_RESPONSIVENESS: ValueTweak = ValueTweak {
    id: "scheduling.systemresponsiveness",
    name: "Multimedia scheduler reserve",
    summary: "Lowers the share of processor time Windows' multimedia scheduler holds back for background work, \
              from 20 to 10 percent.",
    category: "scheduling",
    root: RegRoot::LocalMachine,
    safety: SafetyTier::Moderate,
    tradeoff: Some(
        "Background work such as downloads and updates gets less reserved processor time while media or a game plays.",
    ),
    requires_reboot: true,
    settings: &[dword(MMCSS, "SystemResponsiveness", 10)],
};

/// Multimedia Class Scheduler: network packet limit while media plays.
/// Windows' default is 10; 0xFFFFFFFF turns the limit off.
pub const NETWORK_THROTTLING: ValueTweak = ValueTweak {
    id: "network.throttling",
    name: "Network throttling",
    summary: "Turns off the limit Windows' multimedia scheduler puts on network packet handling while audio or \
              video plays.",
    category: "network",
    root: RegRoot::LocalMachine,
    safety: SafetyTier::Moderate,
    tradeoff: Some("Network traffic can use more processor time while media plays. Microsoft added the limit to keep audio playback steady."),
    requires_reboot: true,
    settings: &[dword(MMCSS, "NetworkThrottlingIndex", 0xFFFF_FFFF)],
};

/// Multimedia Class Scheduler: the "Games" task profile. Windows' defaults are
/// Priority 2, Scheduling Category "Medium", SFIO Priority "Normal".
pub const GAMES_TASK: ValueTweak = ValueTweak {
    id: "scheduling.gamestask",
    name: "Game task priority",
    summary: "Raises the priority of Windows' multimedia scheduler profile for games, used by games that register \
              with it.",
    category: "scheduling",
    root: RegRoot::LocalMachine,
    safety: SafetyTier::Moderate,
    tradeoff: Some(
        "Only games that register with the multimedia scheduler use this profile; others ignore it. Other programs \
         using the scheduler get lower priority while such a game runs.",
    ),
    requires_reboot: true,
    settings: &[
        dword(MMCSS_GAMES, "Priority", 6),
        string(MMCSS_GAMES, "Scheduling Category", "High"),
        string(MMCSS_GAMES, "SFIO Priority", "High"),
    ],
};

/// Every value tweak, in display order within the catalogue.
pub fn all() -> Vec<Box<dyn Tweak>> {
    vec![
        Box::new(GAME_MODE),
        Box::new(BACKGROUND_CAPTURE),
        Box::new(ACCESSIBILITY_SHORTCUTS),
        Box::new(TRANSPARENCY),
        Box::new(GPU_SCHEDULING),
        Box::new(BACKGROUND_APPS),
        Box::new(POWER_THROTTLING_OFF),
        Box::new(SYSTEM_RESPONSIVENESS),
        Box::new(GAMES_TASK),
        Box::new(NETWORK_THROTTLING),
    ]
}
