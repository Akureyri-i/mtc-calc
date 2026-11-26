//! Overlay-plane probe, the overlay side: a top-most click-through window like the HUD's,
//! redrawn every refresh, to see with PresentMon which present path Windows gives it. Run
//! `plane_game` underneath to stand in for the game.
//!   cargo run --release -p mtc-overlay --example plane_overlay -- [options]
//!
//! Options (default: the HUD's way — layered, click-through, DirectComposition swap chain with
//! premultiplied alpha, FLIP_SEQUENTIAL, 2 buffers):
//!   --size WxH      window size (physical px, default 800x600)
//!   --at X,Y        window position (default 200,200)
//!   --full          cover the whole primary monitor
//!   --no-layered    no WS_EX_LAYERED
//!   --interactive   no WS_EX_TRANSPARENT (takes clicks)
//!   --opaque        alpha ignored (diagnostic: is transparency the blocker?)
//!   --discard       FLIP_DISCARD
//!   --buffers N     buffer count
//!   --hwnd          HWND swap chain instead of DirectComposition (opaque)
//!   --presentation  composition swap chain API (IPresentationManager, presentation.h) instead of a
//!                   DXGI swap chain; paced by DwmFlush
//!   --sync N        Present sync interval (default 1)
//!   --seconds N     run time (default 20)
//!   --toggle MS     show and hide the window every MS ms (does a plane coming and going make the
//!                   game underneath skip a frame?)
//!   --shrink        with --toggle: shrink the window to 1x1 px instead of hiding it (the swap chain
//!                   keeps its size; the plane may stay allocated)

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::CompositionSwapchain::{CreatePresentationFactory, IPresentationBuffer, IPresentationFactory, IPresentationManager, IPresentationSurface};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_RESOURCE_MISC_SHARED, D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::DirectComposition::DCompositionCreateSurfaceHandle;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11DeviceContext1,
    ID3D11RenderTargetView, ID3D11Texture2D,
};
use windows::Win32::Graphics::DirectComposition::{DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGISwapChain1,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetSystemMetrics, HWND_TOPMOST, LWA_ALPHA, MSG, PM_REMOVE, PeekMessageW, RegisterClassExW,
    SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetLayeredWindowAttributes, SetWindowPos, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{Interface, Result, w};

struct Options {
    size: (i32, i32),
    at: (i32, i32),
    layered: bool,
    click_through: bool,
    opaque: bool,
    discard: bool,
    buffers: u32,
    hwnd_chain: bool,
    presentation: bool,
    sync: u32,
    seconds: u64,
    toggle_ms: Option<u64>,
    shrink: bool,
}

fn options() -> Options {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let pair = |s: String, sep: char| {
        let (a, b) = s.split_once(sep).expect("pair");
        (a.trim().parse().expect("number"), b.trim().parse().expect("number"))
    };
    let mut size = value("--size").map_or((800, 600), |s| pair(s, 'x'));
    let mut at = value("--at").map_or((200, 200), |s| pair(s, ','));
    if flag("--full") {
        size = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        at = (0, 0);
    }
    Options {
        size,
        at,
        layered: !flag("--no-layered"),
        click_through: !flag("--interactive"),
        opaque: flag("--opaque"),
        discard: flag("--discard"),
        buffers: value("--buffers").map_or(2, |s| s.parse().expect("buffers")),
        hwnd_chain: flag("--hwnd"),
        presentation: flag("--presentation"),
        sync: value("--sync").map_or(1, |s| s.parse().expect("sync")),
        seconds: value("--seconds").map_or(20, |s| s.parse().expect("seconds")),
        toggle_ms: value("--toggle").map(|s| s.parse().expect("toggle ms")),
        shrink: flag("--shrink"),
    }
}

/// The overlay window as `o` asks (no redirection bitmap: DirectComposition content).
unsafe fn overlay_window(o: &Options, name: windows::core::PCWSTR) -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: name,
            ..Default::default()
        };
        RegisterClassExW(&class);
        let mut ex = WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP;
        if o.layered {
            ex |= WS_EX_LAYERED;
        }
        if o.click_through {
            ex |= WS_EX_TRANSPARENT;
        }
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(ex.0), name, w!("plane overlay probe"), WS_POPUP, o.at.0, o.at.1, o.size.0, o.size.1, None, None, Some(instance.into()), None)?;
        if o.layered {
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
        }
        Ok(hwnd)
    }
}

/// The overlay drawn with the composition swap chain API: textures added as presentation
/// buffers, a presentation surface bound to a DirectComposition visual, one present per DwmFlush.
fn presentation(o: &Options) -> Result<()> {
    unsafe {
        let hwnd = overlay_window(o, w!("PlaneOverlayPresentation"))?;
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
        let (device, context) = (device.expect("device"), context.expect("context"));
        let context1: ID3D11DeviceContext1 = context.cast()?;
        let factory: IPresentationFactory = CreatePresentationFactory(&device)?;
        println!("presentation supported {}, with independent flip {}", factory.IsPresentationSupported(), factory.IsPresentationSupportedWithIndependentFlip());
        let manager: IPresentationManager = factory.CreatePresentationManager()?;
        // COMPOSITIONOBJECT_READ | COMPOSITIONOBJECT_WRITE.
        let handle = DCompositionCreateSurfaceHandle(3, None)?;
        let surface: IPresentationSurface = manager.CreatePresentationSurface(handle)?;
        surface.SetAlphaMode(if o.opaque { DXGI_ALPHA_MODE_IGNORE } else { DXGI_ALPHA_MODE_PREMULTIPLIED })?;
        surface.SetSourceRect(&RECT { left: 0, top: 0, right: o.size.0, bottom: o.size.1 })?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: o.size.0 as u32,
            Height: o.size.1 as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
        };
        let mut buffers: Vec<(ID3D11Texture2D, IPresentationBuffer)> = Vec::new();
        for _ in 0..o.buffers.max(2) {
            let mut texture: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&desc, None, Some(&mut texture))?;
            let texture = texture.expect("texture");
            let buffer = manager.AddBufferFromResource(&texture)?;
            buffers.push((texture, buffer));
        }
        let dcomp: IDCompositionDevice = DCompositionCreateDevice(&device.cast::<IDXGIDevice>()?)?;
        let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
        let visual = dcomp.CreateVisual()?;
        let content = dcomp.CreateSurfaceFromHandle(handle)?;
        visual.SetContent(&content)?;
        target.SetRoot(&visual)?;
        dcomp.Commit()?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), o.at.0, o.at.1, o.size.0, o.size.1, SWP_NOACTIVATE);
        println!(
            "pid {} · {}x{} at {:?} · layered {} · click-through {} · presentation API · {} · {} buffers",
            std::process::id(),
            o.size.0,
            o.size.1,
            o.at,
            o.layered,
            o.click_through,
            if o.opaque { "opaque" } else { "premultiplied alpha" },
            buffers.len()
        );
        let started = Instant::now();
        let mut frame = 0u64;
        while started.elapsed() < Duration::from_secs(o.seconds) {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let (texture, buffer) = &buffers[frame as usize % buffers.len()];
            WaitForSingleObject(buffer.GetAvailableEvent()?, INFINITE);
            let mut view: Option<ID3D11RenderTargetView> = None;
            device.CreateRenderTargetView(texture, None, Some(&mut view))?;
            let view = view.expect("view");
            context.ClearRenderTargetView(&view, &[0.0, 0.0, 0.0, 0.0]);
            let x = (frame * 4 % (o.size.0.max(80) as u64 - 60)) as i32;
            context1.ClearView(&view, &[0.9, 0.15, 0.15, 1.0], Some(&[RECT { left: x, top: 40, right: x + 60, bottom: 100 }]));
            context1.ClearView(&view, &[0.0, 0.3, 0.0, 0.5], Some(&[RECT { left: 0, top: o.size.1 / 2, right: o.size.0, bottom: o.size.1 / 2 + 30 }]));
            context.Flush();
            surface.SetBuffer(buffer)?;
            manager.Present()?;
            DwmFlush()?;
            frame += 1;
        }
        println!("{frame} frames in {:.1} s", started.elapsed().as_secs_f64());
    }
    Ok(())
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

fn main() -> Result<()> {
    // Before `options`: `--full` reads the screen size in physical pixels.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let o = options();
    if o.presentation {
        return presentation(&o);
    }
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("PlaneOverlayProbe"),
            ..Default::default()
        };
        RegisterClassExW(&class);
        let mut ex = WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
        if !o.hwnd_chain {
            ex |= WS_EX_NOREDIRECTIONBITMAP;
        }
        if o.layered {
            ex |= WS_EX_LAYERED;
        }
        if o.click_through {
            ex |= WS_EX_TRANSPARENT;
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(ex.0),
            w!("PlaneOverlayProbe"),
            w!("plane overlay probe"),
            WS_POPUP,
            o.at.0,
            o.at.1,
            o.size.0,
            o.size.1,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        if o.layered {
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
        }

        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
        let (device, context) = (device.expect("device"), context.expect("context"));
        let context1: ID3D11DeviceContext1 = context.cast()?;
        let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: o.size.0 as u32,
            Height: o.size.1 as u32,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: o.buffers,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: if o.discard { DXGI_SWAP_EFFECT_FLIP_DISCARD } else { DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL },
            AlphaMode: if o.opaque || o.hwnd_chain { DXGI_ALPHA_MODE_IGNORE } else { DXGI_ALPHA_MODE_PREMULTIPLIED },
            ..Default::default()
        };
        // Kept alive for the run.
        let mut _composition: Option<(IDCompositionDevice, IDCompositionTarget, IDCompositionVisual)> = None;
        let chain: IDXGISwapChain1 = if o.hwnd_chain {
            factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)?
        } else {
            let chain = factory.CreateSwapChainForComposition(&device, &desc, None)?;
            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&device.cast::<IDXGIDevice>()?)?;
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            visual.SetContent(&chain)?;
            target.SetRoot(&visual)?;
            dcomp.Commit()?;
            _composition = Some((dcomp, target, visual));
            chain
        };
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), o.at.0, o.at.1, o.size.0, o.size.1, SWP_NOACTIVATE);
        println!(
            "pid {} · {}x{} at {:?} · layered {} · click-through {} · {} · {} · {} buffers · {} · sync {}",
            std::process::id(),
            o.size.0,
            o.size.1,
            o.at,
            o.layered,
            o.click_through,
            if o.hwnd_chain { "HWND swap chain" } else { "DirectComposition swap chain" },
            if o.opaque || o.hwnd_chain { "opaque" } else { "premultiplied alpha" },
            o.buffers,
            if o.discard { "FLIP_DISCARD" } else { "FLIP_SEQUENTIAL" },
            o.sync
        );

        let started = Instant::now();
        let mut frame = 0u64;
        let mut shown = true;
        while started.elapsed() < Duration::from_secs(o.seconds) {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if let Some(ms) = o.toggle_ms {
                let want = (started.elapsed().as_millis() as u64 / ms) % 2 == 0;
                if want != shown {
                    shown = want;
                    let at = started.elapsed().as_secs_f64() * 1000.0;
                    if o.shrink {
                        let (w, h) = if shown { o.size } else { (1, 1) };
                        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), o.at.0, o.at.1, w, h, SWP_NOACTIVATE);
                    } else {
                        let _ = ShowWindow(hwnd, if shown { SW_SHOWNOACTIVATE } else { SW_HIDE });
                    }
                    println!("{} at {at:.1} ms", if shown { "show" } else { "hide" });
                }
            }
            let buffer: ID3D11Texture2D = chain.GetBuffer(0)?;
            let mut view: Option<ID3D11RenderTargetView> = None;
            device.CreateRenderTargetView(&buffer, None, Some(&mut view))?;
            let view = view.expect("view");
            // Transparent background, an opaque moving block and a half-transparent band, like
            // the HUD's marks.
            context.ClearRenderTargetView(&view, &[0.0, 0.0, 0.0, 0.0]);
            let x = (frame * 4 % (o.size.0.max(80) as u64 - 60)) as i32;
            let block = RECT { left: x, top: 40, right: x + 60, bottom: 100 };
            context1.ClearView(&view, &[0.9, 0.15, 0.15, 1.0], Some(&[block]));
            let band = RECT { left: 0, top: o.size.1 / 2, right: o.size.0, bottom: o.size.1 / 2 + 30 };
            context1.ClearView(&view, &[0.0, 0.3, 0.0, 0.5], Some(&[band]));
            chain.Present(o.sync, DXGI_PRESENT(0)).ok()?;
            frame += 1;
        }
        println!("{frame} frames in {:.1} s", started.elapsed().as_secs_f64());
    }
    Ok(())
}
