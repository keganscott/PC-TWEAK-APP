//! Reference `User` (interactive hive) tweak: 1:1 pointer movement.
//!
//! Details this implementation exists to get right:
//!
//! **These are strings, not DWORDs.** `MouseSpeed`, `MouseThreshold1` and
//! `MouseThreshold2` are `REG_SZ`. Scripts that write them as `REG_DWORD` look
//! like they worked and change nothing, which is why "I disabled mouse accel
//! and it still feels floaty" is such a common complaint.
//!
//! **We do not touch `SmoothMouseXCurve`/`SmoothMouseYCurve`.** Those blobs
//! only take effect while Enhance Pointer Precision is *on*. With `MouseSpeed`
//! at 0 they are inert, so writing a "linear curve" blob is theatre.
//!
//! **Live application only ever targets the right profile.**
//! `SystemParametersInfoW(SPI_SETMOUSE)` acts on the calling process's session
//! (we never pass `SPIF_UPDATEINIFILE`, which would write the caller's own
//! `HKEY_CURRENT_USER` outside the journal). It is right only when the
//! interactive user is us. When we resolved a
//! different SID (alternate admin credentials, Administrator Protection) we skip
//! the call and the registry write takes effect at next sign-in.

use crate::context::ContextResolver;
use crate::error::Result;
use crate::transaction::Transaction;
use crate::types::{ExecutionContext, Impact, RegRoot, RegTarget, SafetyTier, Tier, Tweak, TweakMetadata, TweakState};

const KEY: &str = r"Control Panel\Mouse";
const SPEED: &str = "MouseSpeed";
const T1: &str = "MouseThreshold1";
const T2: &str = "MouseThreshold2";

/// Windows' own defaults, used only to push a live value when the registry
/// values are absent after a restore (absent means Windows uses these).
const DEFAULT_ACCEL: i32 = 1;
const DEFAULT_T1: i32 = 6;
const DEFAULT_T2: i32 = 10;

pub struct MouseAcceleration;

impl Tweak for MouseAcceleration {
    fn id(&self) -> &str {
        "input.mouseaccel"
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: self.id().to_owned().into(),
            name: "Pointer precision".into(),
            summary: "Turns off mouse acceleration so the same hand movement always moves the same distance.".into(),
            target: r"HKEY_USERS\<sid>\Control Panel\Mouse\MouseSpeed, MouseThreshold1, MouseThreshold2".into(),
            category: "input".into(),
            tier: Tier::Pro,
            safety: SafetyTier::Safe,
            impact: Impact::Moderate,
            tradeoff: None,
            requires_reboot: false,
        }
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::User
    }

    fn touches(&self) -> Vec<RegTarget> {
        vec![RegTarget::new(RegRoot::InteractiveUser, KEY, &[SPEED, T1, T2])]
    }

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        // A missing key or value means Windows defaults apply, which is
        // acceleration on. Only an explicit "0" in all three counts as off.
        let read = |name: &str| res.read_string(RegRoot::InteractiveUser, KEY, name);
        let off = [read(SPEED)?, read(T1)?, read(T2)?]
            .iter()
            .all(|v| v.as_deref() == Some("0"));

        Ok(match (off, has_journal_entry) {
            (true, true) => TweakState::Applied,
            // Already 1:1 without us: set outside PeakTweaks. Reporting
            // Applied would claim credit (and offer an Undo with nothing in
            // the journal to undo); Default would hide that it is already off.
            (true, false) => TweakState::Foreign,
            (false, _) => TweakState::Default,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        tx.set_string(RegRoot::InteractiveUser, KEY, SPEED, "0")?;
        tx.set_string(RegRoot::InteractiveUser, KEY, T1, "0")?;
        tx.set_string(RegRoot::InteractiveUser, KEY, T2, "0")?;
        if tx.user_is_self() {
            push_live(0, 0, 0);
        }
        Ok(())
    }

    fn revert(&self, tx: &mut Transaction) -> Result<()> {
        tx.restore_journalled()?;
        if tx.user_is_self() {
            // Push what the restore actually produced, not an assumed default:
            // the user may have had acceleration off already, or something odd.
            let res = tx.resolver();
            let num = |name: &str, default: i32| {
                res.read_string(RegRoot::InteractiveUser, KEY, name)
                    .ok()
                    .flatten()
                    .and_then(|s| s.trim().parse::<i32>().ok())
                    .unwrap_or(default)
            };
            push_live(num(T1, DEFAULT_T1), num(T2, DEFAULT_T2), num(SPEED, DEFAULT_ACCEL));
        }
        Ok(())
    }
}

/// Apply the setting to the running session so it takes effect without a
/// sign-out. Best-effort: a failure here is cosmetic, because the registry
/// write already persisted and applies at next sign-in.
///
/// Deliberately without `SPIF_UPDATEINIFILE`: that flag makes Windows write the
/// three values into the registry itself, which would be a change outside the
/// journal (after a revert that removed them, it would put assumed defaults
/// back). Only the transaction writes the registry.
///
/// `SPI_SETMOUSE` takes an array of three integers:
/// `[threshold1, threshold2, acceleration]`. VERIFY against Microsoft's
/// SystemParametersInfo documentation before changing the order.
#[cfg(windows)]
fn push_live(threshold1: i32, threshold2: i32, acceleration: i32) {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPIF_SENDCHANGE, SPI_SETMOUSE};

    let mut params: [i32; 3] = [threshold1, threshold2, acceleration];
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_SETMOUSE,
            0,
            Some(params.as_mut_ptr() as *mut core::ffi::c_void),
            SPIF_SENDCHANGE,
        );
    }
}

#[cfg(not(windows))]
fn push_live(_threshold1: i32, _threshold2: i32, _acceleration: i32) {}
