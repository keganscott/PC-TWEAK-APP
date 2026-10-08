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
use crate::types::{
    BlockedCode, BlockedReason, ExecutionContext, Impact, PredicateOutcome, RegRoot, RegTarget, SafetyTier, SystemEnv,
    Tier, Tweak, TweakMetadata, TweakState,
};

#[derive(Debug, Clone, Copy)]
pub enum Data {
    Dword(u32),
    /// Written as `REG_SZ`.
    Str(&'static str),
    /// A `REG_SZ` holding a decimal number, of which only the `clear` bits are
    /// turned off; every other bit keeps the user's own setting. `default` is
    /// what Windows assumes when the value is absent or not a number.
    StrClearBits {
        clear: u32,
        default: u32,
    },
    /// One `Name=Value;` entry in a `REG_SZ` list of them, the form DirectX
    /// keeps its settings in; every other entry is kept as it is.
    ListEntry {
        name: &'static str,
        value: &'static str,
    },
}

/// The value of `name` in a `Name=Value;` list (any case), if it has one.
pub(crate) fn list_get<'a>(list: &'a str, name: &str) -> Option<&'a str> {
    list.split(';').find_map(|item| {
        let (k, v) = item.split_once('=')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// `list` with `name` set to `value`: replaced where it is, added at the end
/// where it is not. Every other entry keeps its own text and place.
pub(crate) fn list_set(list: &str, name: &str, value: &str) -> String {
    let mut found = false;
    let mut items: Vec<String> = Vec::new();
    for item in list.split(';').filter(|i| !i.trim().is_empty()) {
        let ours = item
            .split_once('=')
            .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(name));
        if ours && !found {
            items.push(format!("{name}={value}"));
            found = true;
        } else if !ours {
            items.push(item.to_owned());
        }
    }
    if !found {
        items.push(format!("{name}={value}"));
    }
    items.iter().map(|i| format!("{i};")).collect()
}

impl Data {
    /// The number a decimal-string value holds, or `default` when it is absent
    /// or unreadable.
    fn flags(raw: Option<String>, default: u32) -> u32 {
        raw.and_then(|v| v.trim().parse().ok()).unwrap_or(default)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub key: &'static str,
    pub value: &'static str,
    pub data: Data,
    /// True when an absent value already behaves like `data`.
    pub absent_matches: bool,
}

pub(crate) const fn dword(key: &'static str, value: &'static str, data: u32) -> Setting {
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

const fn list_entry(key: &'static str, value: &'static str, name: &'static str, data: &'static str) -> Setting {
    Setting {
        key,
        value,
        data: Data::ListEntry { name, value: data },
        absent_matches: false,
    }
}

const fn clear_bits(key: &'static str, value: &'static str, clear: u32, default: u32) -> Setting {
    Setting {
        key,
        value,
        data: Data::StrClearBits { clear, default },
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
                Data::StrClearBits { clear, .. } => format!(r"{root}\{}\{} &= ~{clear}", s.key, s.value),
                Data::ListEntry { name, value } => format!(r"{root}\{}\{}: {name}={value};", s.key, s.value),
            };
            parts.push(shown);
        }
        parts.join("; ")
    }

    fn matches(&self, res: &ContextResolver, s: &Setting) -> Result<bool> {
        if let Data::StrClearBits { clear, default } = s.data {
            let current = Data::flags(res.read_string(self.root, s.key, s.value)?, default);
            return Ok(current & clear == 0);
        }
        if let Data::ListEntry { name, value } = s.data {
            let list = res.read_string(self.root, s.key, s.value)?.unwrap_or_default();
            return Ok(list_get(&list, name) == Some(value));
        }
        Ok(match res.read_raw(self.root, s.key, s.value)? {
            None => s.absent_matches,
            Some(raw) => match s.data {
                Data::Dword(n) => raw.as_dword() == Some(n),
                Data::Str(v) => raw.as_sz().as_deref() == Some(v),
                Data::StrClearBits { .. } | Data::ListEntry { .. } => unreachable!("handled above"),
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
                Data::StrClearBits { clear, default } => {
                    let current = Data::flags(tx.resolver().read_string(self.root, s.key, s.value)?, default);
                    tx.set_string(self.root, s.key, s.value, &(current & !clear).to_string())?
                }
                Data::ListEntry { name, value } => {
                    let list = tx
                        .resolver()
                        .read_string(self.root, s.key, s.value)?
                        .unwrap_or_default();
                    tx.set_string(self.root, s.key, s.value, &list_set(&list, name, value))?
                }
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
/// SKF_HOTKEYACTIVE, FKF_HOTKEYACTIVE and TKF_HOTKEYACTIVE share this bit.
const HOTKEY_ACTIVE: u32 = 0x4;
const MMCSS_GAMES: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile\Tasks\Games";
const ADVERTISING: &str = r"Software\Microsoft\Windows\CurrentVersion\AdvertisingInfo";
const CONTENT_DELIVERY: &str = r"Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager";
const PRIVACY: &str = r"Software\Microsoft\Windows\CurrentVersion\Privacy";
const SYSTEM_POLICY: &str = r"SOFTWARE\Policies\Microsoft\Windows\System";
const DATA_COLLECTION: &str = r"SOFTWARE\Policies\Microsoft\Windows\DataCollection";
const EXPLORER_ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const EXPLORER_POLICY: &str = r"Software\Policies\Microsoft\Windows\Explorer";
const SEARCH: &str = r"Software\Microsoft\Windows\CurrentVersion\Search";
const DESKTOP: &str = r"Control Panel\Desktop";
const WINDOW_METRICS: &str = r"Control Panel\Desktop\WindowMetrics";
const MOUSE_CLASS: &str = r"SYSTEM\CurrentControlSet\Services\mouclass\Parameters";
const KEYBOARD_CLASS: &str = r"SYSTEM\CurrentControlSet\Services\kbdclass\Parameters";
const SESSION_POWER: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Power";
const KERNEL: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\kernel";

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
/// Filter and Toggle Keys. Only the hotkey bit (0x4: SKF_, FKF_ and
/// TKF_HOTKEYACTIVE) is cleared, so a user who has one of these features on
/// keeps it on, along with any other option they set. Windows' defaults are
/// 510, 126 and 62.
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
        clear_bits(STICKY_KEYS, "Flags", HOTKEY_ACTIVE, 510),
        clear_bits(FILTER_KEYS, "Flags", HOTKEY_ACTIVE, 126),
        clear_bits(TOGGLE_KEYS, "Flags", HOTKEY_ACTIVE, 62),
    ],
};

/// Settings > Personalization > Colors > Transparency effects.
pub const TRANSPARENCY: ValueTweak = ValueTweak {
    id: "display.transparency",
    name: "Transparency effects",
    summary: "Turns off the see-through effect on the taskbar, Start and window frames, so Windows draws them as \
              solid colours.",
    category: "appearance",
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

// ---- Privacy (catalogue H14, registry half) --------------------------------

/// Settings > Privacy & security > General > "Let apps show me personalized
/// ads by using my advertising ID".
pub const ADVERTISING_ID: ValueTweak = ValueTweak {
    id: "privacy.advertisingid",
    name: "Advertising ID",
    summary: "Turns off the advertising ID Windows gives apps to show personalised ads.",
    category: "privacy",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: false,
    settings: &[dword(ADVERTISING, "Enabled", 0)],
};

/// Settings > Personalization > Start and Settings > System > Notifications:
/// suggestions, tips and the "welcome experience". Four switches in one key.
pub const TIPS: ValueTweak = ValueTweak {
    id: "privacy.tips",
    name: "Tips and suggestions",
    summary: "Turns off Windows tips, suggested apps in Start and the suggestions shown after updates.",
    category: "privacy",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: false,
    settings: &[
        dword(CONTENT_DELIVERY, "SubscribedContent-338389Enabled", 0),
        dword(CONTENT_DELIVERY, "SubscribedContent-338388Enabled", 0),
        dword(CONTENT_DELIVERY, "SystemPaneSuggestionsEnabled", 0),
        dword(CONTENT_DELIVERY, "SoftLandingEnabled", 0),
        // "Show me the Windows welcome experience after updates". VERIFY.
        dword(CONTENT_DELIVERY, "SubscribedContent-310093Enabled", 0),
    ],
};

/// Settings > Privacy & security > Diagnostics & feedback > Tailored experiences.
pub const TAILORED: ValueTweak = ValueTweak {
    id: "privacy.tailored",
    name: "Tailored experiences",
    summary: "Stops Microsoft using your diagnostic data to pick tips, ads and recommendations for you.",
    category: "privacy",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: false,
    settings: &[dword(PRIVACY, "TailoredExperiencesWithDiagnosticDataEnabled", 0)],
};

/// Group Policy "Allow publishing of User Activities" and "Allow upload of User
/// Activities" (Computer Configuration > Administrative Templates > System >
/// OS Policies).
pub const ACTIVITY_HISTORY: ValueTweak = ValueTweak {
    id: "privacy.activityhistory",
    name: "Activity history",
    summary: "Stops Windows keeping and uploading a history of the apps, files and websites you open.",
    category: "privacy",
    root: RegRoot::LocalMachine,
    safety: SafetyTier::Safe,
    tradeoff: Some("Settings shows \"Some settings are managed by your organization\" while this is on."),
    requires_reboot: false,
    settings: &[
        dword(SYSTEM_POLICY, "PublishUserActivities", 0),
        dword(SYSTEM_POLICY, "UploadUserActivities", 0),
    ],
};

/// Group Policy "Allow Diagnostic Data" (AllowTelemetry). 1 = Required, the
/// lowest level Home and Pro accept (0 is treated as 1 there).
pub const TELEMETRY: ValueTweak = ValueTweak {
    id: "privacy.telemetry",
    name: "Diagnostic data",
    summary: "Sets Windows diagnostic data to Required, the lowest level Windows Home and Pro send.",
    category: "privacy",
    root: RegRoot::LocalMachine,
    safety: SafetyTier::Safe,
    tradeoff: Some(
        "Settings shows \"Some settings are managed by your organization\" while this is on, and Windows Insider \
         builds stop arriving.",
    ),
    requires_reboot: false,
    settings: &[dword(DATA_COLLECTION, "AllowTelemetry", 1)],
};

// ---- Explorer and quality of life (catalogue H27) --------------------------

/// File Explorer > View > Show > File name extensions.
pub const FILE_EXTENSIONS: ValueTweak = ValueTweak {
    id: "explorer.fileextensions",
    name: "File name extensions",
    summary: "Shows file name extensions such as .exe and .txt in File Explorer.",
    category: "appearance",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: false,
    settings: &[dword(EXPLORER_ADVANCED, "HideFileExt", 0)],
};

/// Group Policy "Turn off display of recent search entries in the File
/// Explorer search box" (DisableSearchBoxSuggestions) plus the per-user Bing
/// switch: Start and the taskbar search show local results only.
pub const WEB_SEARCH: ValueTweak = ValueTweak {
    id: "explorer.websearch",
    name: "Web results in Start search",
    summary: "Makes Start and taskbar search show results from this PC only, without Bing web results.",
    category: "appearance",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: true,
    settings: &[
        dword(EXPLORER_POLICY, "DisableSearchBoxSuggestions", 1),
        dword(SEARCH, "BingSearchEnabled", 0),
    ],
};

/// Windows re-compresses JPEG wallpapers to about 85 quality unless this says
/// otherwise. Takes effect the next time a wallpaper is set.
pub const WALLPAPER_QUALITY: ValueTweak = ValueTweak {
    id: "display.wallpaperquality",
    name: "Full-quality wallpaper",
    summary: "Stops Windows re-compressing JPEG wallpapers, from the next time a wallpaper is set.",
    category: "appearance",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: false,
    settings: &[dword(DESKTOP, "JPEGImportQuality", 100)],
};

// ---- Visual effects (catalogue H11) ----------------------------------------

/// System > Advanced system settings > Performance: "Animate windows when
/// minimizing and maximizing" and "Animations in the taskbar".
pub const ANIMATIONS: ValueTweak = ValueTweak {
    id: "display.animations",
    name: "Window and taskbar animations",
    summary: "Turns off the animations when windows open, minimise and maximise, and in the taskbar.",
    category: "appearance",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: true,
    settings: &[
        string(WINDOW_METRICS, "MinAnimate", "0"),
        dword(EXPLORER_ADVANCED, "TaskbarAnimations", 0),
    ],
};

// ---- Input (catalogue H4) --------------------------------------------------

/// The mouse and keyboard class drivers' input buffers. Windows' default is
/// 100 events each. VERIFY: 50 is our choice (Hone does not publish its
/// value); smaller buffers can drop input on very high polling rates.
pub const INPUT_QUEUE: ValueTweak = ValueTweak {
    id: "input.queuesize",
    name: "Mouse and keyboard buffer size",
    summary: "Lowers the number of mouse and keyboard events Windows' input drivers hold in their buffers from 100 \
              to 50.",
    category: "input",
    root: RegRoot::LocalMachine,
    safety: SafetyTier::Moderate,
    tradeoff: Some("Needs a restart."),
    requires_reboot: true,
    settings: &[
        dword(MOUSE_CLASS, "MouseDataQueueSize", 50),
        dword(KEYBOARD_CLASS, "KeyboardDataQueueSize", 50),
    ],
};

// ---- Power (catalogue H10, registry half) -----------------------------------

/// Control Panel > Power Options > "Turn on fast startup". Hibernation itself
/// (powercfg /h) is a separate tool with its own undo type.
pub const FAST_STARTUP: ValueTweak = ValueTweak {
    id: "system.faststartup",
    name: "Fast startup",
    summary: "Turns off fast startup, so shutting down fully closes Windows instead of saving it to disk.",
    category: "system",
    root: RegRoot::LocalMachine,
    safety: SafetyTier::Safe,
    tradeoff: Some("Starting the PC from shut down takes a little longer."),
    requires_reboot: false,
    settings: &[dword(SESSION_POWER, "HiberbootEnabled", 0)],
};

// ---- Timer (catalogue H2, registry half) ------------------------------------

/// Windows 11 limits a program's timer-resolution request to that program.
/// This value restores the system-wide behaviour of Windows 10; earlier
/// versions ignore it. The timer itself is held by the timer-resolution tool.
pub const TIMER_REQUESTS: ValueTweak = ValueTweak {
    id: "scheduling.timerrequests",
    name: "System-wide timer requests",
    summary: "Lets a program's timer-resolution request apply to the whole system again, as before Windows 11.",
    category: "scheduling",
    root: RegRoot::LocalMachine,
    safety: SafetyTier::Moderate,
    tradeoff: Some("Needs a restart."),
    requires_reboot: true,
    settings: &[dword(KERNEL, "GlobalTimerResolutionRequests", 1)],
};

/// First Windows 11 build.
pub const WINDOWS_11_BUILD: u32 = 22000;

/// A setting only Windows 11 has. Blocked on Windows 10, where it would be a
/// change that does nothing; allowed when the build is not known, since
/// Windows ignores the value where it has no such setting.
pub struct Windows11(pub ValueTweak);

impl Tweak for Windows11 {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn metadata(&self) -> TweakMetadata {
        self.0.metadata()
    }

    fn execution_context(&self) -> ExecutionContext {
        self.0.execution_context()
    }

    fn touches(&self) -> Vec<RegTarget> {
        self.0.touches()
    }

    fn evaluate_predicate(&self, env: &SystemEnv) -> PredicateOutcome {
        match env.os_build() {
            Some(build) if build < WINDOWS_11_BUILD => PredicateOutcome::Block(BlockedReason::new(
                BlockedCode::OsVersionUnsupported,
                "Only Windows 11 has this setting.",
            )),
            _ => PredicateOutcome::Allow,
        }
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        self.0.read_state(res, has_journal_entry)
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        self.0.apply(tx)
    }
}

// ---- Windows 11 (catalogue H1 windowed games, H27 right-click menu) --------

const DIRECTX_PREFS: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";
/// The new right-click menu's COM class. A per-user registration with an
/// empty server makes File Explorer fall back to the full menu.
pub const NEW_MENU_SERVER: &str = r"Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}\InprocServer32";

/// Settings > System > Display > Graphics > Optimizations for windowed games.
pub const WINDOWED_GAMES: Windows11 = Windows11(ValueTweak {
    id: "gaming.windowedgames",
    name: "Optimizations for windowed games",
    summary: "Turns on Windows' setting for games that use DirectX 10 or 11 in a window or a borderless window, \
              so they hand their frames to Windows the way full-screen games do.",
    category: "gaming",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: None,
    requires_reboot: false,
    settings: &[list_entry(
        DIRECTX_PREFS,
        "DirectXUserGlobalSettings",
        "SwapEffectUpgradeEnable",
        "1",
    )],
});

/// Windows 11's short right-click menu, replaced by the full one.
pub const FULL_CONTEXT_MENU: Windows11 = Windows11(ValueTweak {
    id: "explorer.fullmenu",
    name: "Full right-click menu",
    summary: "Shows the full right-click menu in File Explorer and on the desktop straight away, as in Windows \
              10, instead of the short menu with Show more options.",
    category: "appearance",
    root: RegRoot::InteractiveUser,
    safety: SafetyTier::Safe,
    tradeoff: Some("Takes effect after you sign out and back in."),
    requires_reboot: false,
    settings: &[string(NEW_MENU_SERVER, "", "")],
});

/// Every value tweak, in display order within the catalogue.
pub fn all() -> Vec<Box<dyn Tweak>> {
    vec![
        Box::new(GAME_MODE),
        Box::new(BACKGROUND_CAPTURE),
        Box::new(WINDOWED_GAMES),
        Box::new(ACCESSIBILITY_SHORTCUTS),
        Box::new(TRANSPARENCY),
        Box::new(GPU_SCHEDULING),
        Box::new(BACKGROUND_APPS),
        Box::new(POWER_THROTTLING_OFF),
        Box::new(SYSTEM_RESPONSIVENESS),
        Box::new(GAMES_TASK),
        Box::new(NETWORK_THROTTLING),
        Box::new(TIMER_REQUESTS),
        Box::new(INPUT_QUEUE),
        Box::new(FAST_STARTUP),
        Box::new(ANIMATIONS),
        Box::new(WALLPAPER_QUALITY),
        Box::new(FILE_EXTENSIONS),
        Box::new(FULL_CONTEXT_MENU),
        Box::new(WEB_SEARCH),
        Box::new(ADVERTISING_ID),
        Box::new(TIPS),
        Box::new(TAILORED),
        Box::new(ACTIVITY_HISTORY),
        Box::new(TELEMETRY),
    ]
}
