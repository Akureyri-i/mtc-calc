//! The Direct3D device the capture runs on: which GPU, its driver and the feature level it
//! reaches, for the log.

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_9_1, D3D_FEATURE_LEVEL_9_2,
    D3D_FEATURE_LEVEL_9_3, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::core::Interface;

/// The feature levels the capture library asks for, highest first.
const LEVELS: [D3D_FEATURE_LEVEL; 7] = [
    D3D_FEATURE_LEVEL_11_1,
    D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_9_3,
    D3D_FEATURE_LEVEL_9_2,
    D3D_FEATURE_LEVEL_9_1,
];

fn create(driver: D3D_DRIVER_TYPE) -> Option<(ID3D11Device, D3D_FEATURE_LEVEL)> {
    let mut device = None;
    let mut level = D3D_FEATURE_LEVEL::default();
    unsafe { D3D11CreateDevice(None, driver, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, Some(&LEVELS), D3D11_SDK_VERSION, Some(&mut device), Some(&mut level), None) }.ok()?;
    Some((device?, level))
}

/// "11.1" for `D3D_FEATURE_LEVEL_11_1`.
fn level_text(level: D3D_FEATURE_LEVEL) -> String {
    format!("{}.{}", (level.0 >> 12) & 0xf, (level.0 >> 8) & 0xf)
}

/// The device the capture library creates, as it creates it (the default GPU at the highest level
/// it reaches, else the software renderer), described for the log: "Graphics: NVIDIA GeForce RTX
/// 3060 · driver 32.0.15.6094 · Direct3D feature level 11.1".
pub fn describe() -> String {
    let (device, level, software) = match create(D3D_DRIVER_TYPE_HARDWARE) {
        Some((device, level)) => (device, level, false),
        None => match create(D3D_DRIVER_TYPE_WARP) {
            Some((device, level)) => (device, level, true),
            None => return "Graphics: no Direct3D 11 device could be created, hardware or software".into(),
        },
    };
    let adapter = device.cast::<IDXGIDevice>().ok().and_then(|d| unsafe { d.GetAdapter() }.ok());
    let name = adapter
        .as_ref()
        .and_then(|a| unsafe { a.GetDesc() }.ok())
        .map(|d| String::from_utf16_lossy(&d.Description).trim_end_matches('\0').trim().to_owned())
        .unwrap_or_else(|| "GPU unknown".into());
    // The user-mode driver's version, packed in four 16-bit parts.
    let driver = adapter
        .as_ref()
        .and_then(|a| unsafe { a.CheckInterfaceSupport(&IDXGIDevice::IID) }.ok())
        .map(|v| {
            let v = v as u64;
            format!(" · driver {}.{}.{}.{}", v >> 48, (v >> 32) & 0xffff, (v >> 16) & 0xffff, v & 0xffff)
        })
        .unwrap_or_default();
    let note = if software {
        " (software renderer: no hardware device could be created)"
    } else if level.0 < D3D_FEATURE_LEVEL_11_0.0 {
        " (below 11.0: capture runs on it all the same)"
    } else {
        ""
    };
    format!("Graphics: {name}{driver} · Direct3D feature level {}{note}", level_text(level))
}

#[cfg(test)]
mod tests {
    #[test]
    fn feature_levels_read_as_versions() {
        use super::*;
        assert_eq!(level_text(D3D_FEATURE_LEVEL_11_1), "11.1");
        assert_eq!(level_text(D3D_FEATURE_LEVEL_10_0), "10.0");
        assert_eq!(level_text(D3D_FEATURE_LEVEL_9_3), "9.3");
    }
}
