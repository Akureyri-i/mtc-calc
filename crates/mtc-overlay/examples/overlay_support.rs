//! What the display hardware reports about overlay planes (MPO), per output of the adapter the
//! HUD renders on:
//!   cargo run --release -p mtc-overlay --example overlay_support
//!
//! `SupportsOverlays` (IDXGIOutput2) says whether the output has overlay planes at all;
//! `CheckOverlaySupport` (IDXGIOutput3) whether a format can be shown on one (DIRECT: unscaled,
//! SCALING: scaled); `CheckHardwareCompositionSupport` (IDXGIOutput6) whether the hardware can
//! compose fullscreen or windowed swap chains (and stretch the cursor). Support does not mean a
//! given window gets a plane: Windows decides that at run time.

use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R10G10B10A2_UNORM,
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_YUY2,
};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIOutput, IDXGIOutput2, IDXGIOutput3, IDXGIOutput6};
use windows::core::Interface;

const FORMATS: [(&str, DXGI_FORMAT); 6] = [
    ("B8G8R8A8_UNORM (HUD)", DXGI_FORMAT_B8G8R8A8_UNORM),
    ("R8G8B8A8_UNORM", DXGI_FORMAT_R8G8B8A8_UNORM),
    ("R10G10B10A2_UNORM", DXGI_FORMAT_R10G10B10A2_UNORM),
    ("R16G16B16A16_FLOAT", DXGI_FORMAT_R16G16B16A16_FLOAT),
    ("NV12", DXGI_FORMAT_NV12),
    ("YUY2", DXGI_FORMAT_YUY2),
];

fn overlay_flags(f: u32) -> String {
    let mut s = Vec::new();
    if f & 1 != 0 {
        s.push("DIRECT");
    }
    if f & 2 != 0 {
        s.push("SCALING");
    }
    if s.is_empty() { "none".to_owned() } else { s.join("|") }
}

fn composition_flags(f: u32) -> String {
    let mut s = Vec::new();
    if f & 1 != 0 {
        s.push("FULLSCREEN");
    }
    if f & 2 != 0 {
        s.push("WINDOWED");
    }
    if f & 4 != 0 {
        s.push("CURSOR_STRETCHED");
    }
    if s.is_empty() { "none".to_owned() } else { s.join("|") }
}

fn main() -> windows::core::Result<()> {
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, Default::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, None)?;
        let device = device.expect("D3D11 device");
        let adapter = device.cast::<IDXGIDevice>()?.GetAdapter()?;
        let desc = adapter.GetDesc()?;
        let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_owned();
        println!("adapter: {name}");
        let mut i = 0;
        while let Ok(output) = adapter.EnumOutputs(i) {
            i += 1;
            report(&output, &device)?;
        }
        if i == 0 {
            println!("no outputs on this adapter");
        }
    }
    Ok(())
}

unsafe fn report(output: &IDXGIOutput, device: &ID3D11Device) -> windows::core::Result<()> {
    unsafe {
        let d = output.GetDesc()?;
        let name = String::from_utf16_lossy(&d.DeviceName).trim_end_matches('\0').to_owned();
        let r = d.DesktopCoordinates;
        println!("\noutput {name}: {}x{} at ({}, {}), attached {}", r.right - r.left, r.bottom - r.top, r.left, r.top, d.AttachedToDesktop.as_bool());
        match output.cast::<IDXGIOutput2>() {
            Ok(o2) => println!("  SupportsOverlays: {}", o2.SupportsOverlays().as_bool()),
            Err(e) => println!("  IDXGIOutput2 unavailable: {e}"),
        }
        match output.cast::<IDXGIOutput3>() {
            Ok(o3) => {
                for (label, format) in FORMATS {
                    match o3.CheckOverlaySupport(format, device) {
                        Ok(flags) => println!("  CheckOverlaySupport {label}: {} (0x{flags:x})", overlay_flags(flags)),
                        Err(e) => println!("  CheckOverlaySupport {label}: error {e}"),
                    }
                }
            }
            Err(e) => println!("  IDXGIOutput3 unavailable: {e}"),
        }
        match output.cast::<IDXGIOutput6>() {
            Ok(o6) => match o6.CheckHardwareCompositionSupport() {
                Ok(flags) => println!("  CheckHardwareCompositionSupport: {} (0x{flags:x})", composition_flags(flags)),
                Err(e) => println!("  CheckHardwareCompositionSupport: error {e}"),
            },
            Err(e) => println!("  IDXGIOutput6 unavailable: {e}"),
        }
    }
    Ok(())
}
