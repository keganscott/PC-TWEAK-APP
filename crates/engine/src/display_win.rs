//! Displays and their refresh rates, through the same Win32 calls Settings >
//! Display uses (Windows only).
//!
//! Only a rate Windows itself lists for the display at the resolution and
//! colour depth it has now is ever set, and Windows is asked to test the mode
//! before it is applied (`CDS_TEST`). The change is saved for the next sign in
//! (`CDS_UPDATEREGISTRY`); VERIFY on a real PC that it survives a restart
//! (NOTES N95).

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, EnumDisplayDevicesW, EnumDisplaySettingsW, CDS_TEST, CDS_UPDATEREGISTRY, DEVMODEW,
    DISPLAY_DEVICEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_PRIMARY_DEVICE, DISP_CHANGE_SUCCESSFUL,
    DM_BITSPERPEL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE,
};

use super::error::{EngineError, Result};
use super::system::Display;

/// A driver that never ends its list must not hang the engine.
const MAX_ENTRIES: u32 = 4096;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn text(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn devmode() -> DEVMODEW {
    DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    }
}

/// 0 and 1 mean "the hardware's default", not a rate.
fn real(hz: u32) -> u32 {
    if hz <= 1 {
        0
    } else {
        hz
    }
}

/// The current mode of `device`, and every rate Windows lists at its
/// resolution and colour depth.
fn modes(device: &[u16]) -> Result<(DEVMODEW, Vec<u32>)> {
    let name = PCWSTR(device.as_ptr());
    let mut current = devmode();
    // SAFETY: a NUL-terminated name and a DEVMODEW with dmSize set.
    if !unsafe { EnumDisplaySettingsW(name, ENUM_CURRENT_SETTINGS, &mut current) }.as_bool() {
        return Err(EngineError::Win32 {
            call: "EnumDisplaySettingsW".into(),
            code: 0,
            detail: format!("no current mode for {}", text(device)),
        });
    }
    let mut rates = Vec::new();
    for i in 0..MAX_ENTRIES {
        let mut m = devmode();
        // SAFETY: as above.
        if !unsafe { EnumDisplaySettingsW(name, ENUM_DISPLAY_SETTINGS_MODE(i), &mut m) }.as_bool() {
            break;
        }
        if m.dmPelsWidth == current.dmPelsWidth
            && m.dmPelsHeight == current.dmPelsHeight
            && m.dmBitsPerPel == current.dmBitsPerPel
            && real(m.dmDisplayFrequency) > 0
        {
            rates.push(m.dmDisplayFrequency);
        }
    }
    rates.sort_unstable();
    rates.dedup();
    Ok((current, rates))
}

/// Every display attached to the desktop. One whose modes cannot be read is
/// left out rather than failing the list.
pub fn displays() -> Result<Vec<Display>> {
    let mut out = Vec::new();
    for i in 0..MAX_ENTRIES {
        let mut adapter = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: a DISPLAY_DEVICEW with cb set.
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut adapter, 0) }.as_bool() {
            break;
        }
        if adapter.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
            continue;
        }
        let device = text(&adapter.DeviceName);
        let device_w = wide(&device);
        let Ok((current, rates)) = modes(&device_w) else {
            continue;
        };
        let current_hz = real(current.dmDisplayFrequency);
        if current_hz == 0 {
            continue;
        }
        let mut monitor = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: as above; the first monitor on this display.
        let name = if unsafe { EnumDisplayDevicesW(PCWSTR(device_w.as_ptr()), 0, &mut monitor, 0) }.as_bool() {
            text(&monitor.DeviceString)
        } else {
            text(&adapter.DeviceString)
        };
        out.push(Display {
            device,
            name,
            primary: adapter.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0,
            width: current.dmPelsWidth,
            height: current.dmPelsHeight,
            current_hz,
            max_hz: rates.last().copied().unwrap_or(current_hz).max(current_hz),
        });
    }
    Ok(out)
}

/// The display's refresh rate now.
pub fn rate(device: &str) -> Result<u32> {
    let (current, _) = modes(&wide(device))?;
    Ok(real(current.dmDisplayFrequency))
}

/// Set the display's refresh rate, keeping its resolution and colour depth.
/// Only a rate Windows lists for that mode.
pub fn set_rate(device: &str, hz: u32) -> Result<()> {
    let device_w = wide(device);
    let (current, rates) = modes(&device_w)?;
    if real(current.dmDisplayFrequency) == hz {
        return Ok(());
    }
    if !rates.contains(&hz) {
        return Err(EngineError::Win32 {
            call: "EnumDisplaySettingsW".into(),
            code: 0,
            detail: format!(
                "{device} does not offer {hz} Hz at {}x{} (it offers {rates:?})",
                current.dmPelsWidth, current.dmPelsHeight
            ),
        });
    }
    let mut mode = devmode();
    mode.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_BITSPERPEL | DM_DISPLAYFREQUENCY;
    mode.dmPelsWidth = current.dmPelsWidth;
    mode.dmPelsHeight = current.dmPelsHeight;
    mode.dmBitsPerPel = current.dmBitsPerPel;
    mode.dmDisplayFrequency = hz;
    for (flags, step) in [(CDS_TEST, "test"), (CDS_UPDATEREGISTRY, "change")] {
        // SAFETY: a NUL-terminated device name and a filled DEVMODEW.
        let r =
            unsafe { ChangeDisplaySettingsExW(PCWSTR(device_w.as_ptr()), Some(&mode), HWND::default(), flags, None) };
        if r != DISP_CHANGE_SUCCESSFUL {
            return Err(EngineError::Win32 {
                call: "ChangeDisplaySettingsExW".into(),
                code: r.0 as u32,
                detail: format!("Windows refused to {step} {hz} Hz on {device}"),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads only. What a real Windows reports, for the evidence log; a CI
    /// runner may have one virtual display or none.
    #[test]
    fn lists_the_displays_on_this_pc() {
        let all = displays().expect("listing displays");
        eprintln!("displays: {all:?}");
        for d in &all {
            assert!(d.max_hz >= d.current_hz, "{d:?}");
            assert_eq!(rate(&d.device).unwrap(), d.current_hz);
        }
    }
}
