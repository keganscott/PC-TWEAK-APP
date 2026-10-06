//! Win32-level hardware facts: the system drive, the primary display's modes and
//! the graphics adapters (via DXGI, which reports VRAM above 4 GB correctly,
//! unlike `Win32_VideoController.AdapterRAM`).

use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE,
};

use super::error::{EngineError, Result};
use super::hardware::{DisplayInfo, GpuAdapter, OsFacts};

/// Microsoft's Basic Render Driver / WARP vendor id.
const VENDOR_MICROSOFT: u32 = 0x1414;

pub struct WindowsFacts;

impl OsFacts for WindowsFacts {
    fn system_drive(&self) -> String {
        std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())
    }

    fn display(&self) -> Result<DisplayInfo> {
        unsafe {
            let mut current = DEVMODEW {
                dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            if !EnumDisplaySettingsW(windows::core::PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut current).as_bool() {
                return Err(EngineError::Win32 {
                    call: "EnumDisplaySettingsW".into(),
                    code: 0,
                    detail: "no current display mode (no desktop session?)".into(),
                });
            }

            let mut max_hz = current.dmDisplayFrequency;
            let mut i = 0u32;
            loop {
                let mut m = DEVMODEW {
                    dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                    ..Default::default()
                };
                if !EnumDisplaySettingsW(windows::core::PCWSTR::null(), ENUM_DISPLAY_SETTINGS_MODE(i), &mut m).as_bool()
                {
                    break;
                }
                if m.dmPelsWidth == current.dmPelsWidth
                    && m.dmPelsHeight == current.dmPelsHeight
                    && m.dmBitsPerPel == current.dmBitsPerPel
                {
                    max_hz = max_hz.max(m.dmDisplayFrequency);
                }
                i += 1;
                if i > 4096 {
                    break; // a driver that never ends the list must not hang us
                }
            }

            // 0 and 1 mean "hardware default" rather than a real rate.
            let real = |hz: u32| if hz <= 1 { 0 } else { hz };
            if real(current.dmDisplayFrequency) == 0 {
                return Err(EngineError::Win32 {
                    call: "EnumDisplaySettingsW".into(),
                    code: 0,
                    detail: "the display reports its default refresh rate, not a number".into(),
                });
            }
            Ok(DisplayInfo {
                width: current.dmPelsWidth,
                height: current.dmPelsHeight,
                current_hz: real(current.dmDisplayFrequency),
                max_hz_at_current_resolution: real(max_hz),
            })
        }
    }

    fn gpu_adapters(&self) -> Result<Vec<GpuAdapter>> {
        unsafe {
            let factory: IDXGIFactory1 =
                CreateDXGIFactory1().map_err(|e| EngineError::win32("CreateDXGIFactory1", e))?;
            let mut out = Vec::new();
            let mut i = 0;
            // EnumAdapters1 returns DXGI_ERROR_NOT_FOUND when the list ends.
            while let Ok(adapter) = factory.EnumAdapters1(i) {
                let desc = adapter
                    .GetDesc1()
                    .map_err(|e| EngineError::win32("IDXGIAdapter1::GetDesc1", e))?;
                let len = desc
                    .Description
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(desc.Description.len());
                out.push(GpuAdapter {
                    name: String::from_utf16_lossy(&desc.Description[..len]),
                    vendor_id: desc.VendorId,
                    dedicated_vram_bytes: desc.DedicatedVideoMemory as u64,
                    shared_memory_bytes: desc.SharedSystemMemory as u64,
                    is_software: desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0
                        || desc.VendorId == VENDOR_MICROSOFT,
                });
                i += 1;
            }
            Ok(out)
        }
    }
}
