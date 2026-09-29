//! Reference `User` (interactive hive) tweak: 1:1 pointer movement.
//!
//! Two details this implementation exists to get right:
//!
//! **These are strings, not DWORDs.** `MouseSpeed`, `MouseThreshold1` and
//! `MouseThreshold2` are `REG_SZ`. Scripts that write them as `REG_DWORD` look
//! like they worked and change nothing, which is why "I disabled mouse accel
//! and it still feels floaty" is such a common complaint.
//!
//! **We do not touch `SmoothMouseXCurve`/`SmoothMouseYCurve`.** Those blobs
//! only take effect while Enhance Pointer Precision is *on*. With `MouseSpeed`
//! at 0 they are inert, so writing a "linear curve" blob is theatre. Shipping
//! it would mean three more values to back up and restore for no behavioural
//! change.
//!
//! Live application is the interesting part. `SystemParametersInfoW(SPI_SETMOUSE)`
//! acts on the calling process's session and, with `SPIF_UPDATEINIFILE`, writes
//! the caller's own `HKEY_CURRENT_USER`. That is exactly what we want when the
//! interactive user is us — the common case under UAC elevation. When we
//! resolved a *different* SID, calling it would apply the setting to the wrong
//! profile, so we skip it and let the registry write take effect at next sign-in.

use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_SETMOUSE,
};

use crate::engine::context::ContextResolver;
use crate::engine::error::Result;
use crate::engine::journal::Transaction;
use crate::engine::types::{
    ExecutionContext, Impact, RegRoot, SafetyTier, Tier, Tweak, TweakMetadata, TweakState,
};

const KEY: &str = r"Control Panel\Mouse";
const SPEED: &str = "MouseSpeed";
const T1: &str = "MouseThreshold1";
const T2: &str = "MouseThreshold2";

pub struct MouseAcceleration;

impl Tweak for MouseAcceleration {
    fn id(&self) -> &'static str {
        "input.mouseaccel"
    }

    fn metadata(&self) -> TweakMetadata {
        TweakMetadata {
            id: self.id(),
            name: "Pointer precision",
            summary: "Turns off mouse acceleration so the same hand movement always moves the same distance.",
            target: r"HKEY_USERS\<sid>\Control Panel\Mouse\MouseSpeed, MouseThreshold1, MouseThreshold2",
            category: "input",
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

    fn read_state(&self, res: &ContextResolver, has_journal_entry: bool) -> Result<TweakState> {
        let key = match res.open_read(RegRoot::InteractiveUser, KEY) {
            Ok(k) => k,
            // No Mouse key at all is a fresh profile; Windows defaults apply.
            Err(_) => return Ok(TweakState::Default),
        };

        let read = |name: &str| key.get_value::<String, _>(name).unwrap_or_else(|_| "0".into());
        let off = read(SPEED) == "0" && read(T1) == "0" && read(T2) == "0";

        Ok(match (off, has_journal_entry) {
            (true, true) => TweakState::Applied,
            // Already 1:1 without us. Nothing to do and nothing to claim credit
            // for — reporting Applied here would be a lie the UI acts on.
            (true, false) => TweakState::Default,
            (false, _) => TweakState::Default,
        })
    }

    fn apply(&self, tx: &mut Transaction) -> Result<()> {
        tx.set_string(RegRoot::InteractiveUser, KEY, SPEED, "0")?;
        tx.set_string(RegRoot::InteractiveUser, KEY, T1, "0")?;
        tx.set_string(RegRoot::InteractiveUser, KEY, T2, "0")?;
        push_live(0)?;
        Ok(())
    }

    fn revert(&self, tx: &mut Transaction) -> Result<()> {
        tx.restore_journalled(self.id())?;
        // Read back what the restore actually produced rather than assuming
        // Windows' default of 1 — the user may have had acceleration off
        // already, or set to something unusual.
        push_live(1)?;
        Ok(())
    }
}

/// Apply the setting to the running session so it takes effect without a
/// sign-out. Best-effort: a failure here is cosmetic, because the registry
/// write already persisted and will apply at next sign-in. We deliberately do
/// not fail the transaction over it.
///
/// `SPI_SETMOUSE` takes a 3-element `[threshold1, threshold2, acceleration]`
/// array. Acceleration 0 disables it entirely.
fn push_live(acceleration: u32) -> Result<()> {
    let params: [u32; 3] = if acceleration == 0 { [0, 0, 0] } else { [6, 10, 1] };

    unsafe {
        // SPIF_UPDATEINIFILE targets the *calling* user's hive. Harmless when
        // that is the interactive user (the normal case) and skipped by the
        // caller when it is not.
        let _ = SystemParametersInfoW(
            SPI_SETMOUSE,
            0,
            Some(params.as_ptr() as *mut core::ffi::c_void),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        );
    }
    Ok(())
}
