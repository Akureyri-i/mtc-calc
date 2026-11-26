//! Overlay-plane probe, the game side: a stand-in for Roblox — a normal window covering the
//! primary monitor's work area (like the maximized game, title bar included), an opaque flip-model
//! swap chain redrawn every refresh. PresentMon showed Roblox as `Hardware Composed: Independent
//! Flip`; this checks the stand-in gets the same before `plane_overlay` goes on top.
//!   cargo run --release -p mtc-overlay --example plane_game -- [--seconds N] [--tearing] [--fps N] [--gpu-copies N]
//!       [--cpu-ms X] [--focus] [--log FILE]
//!
//! The window is shown without taking focus, at the top of the normal (not top-most) windows.
//!
//! Roblox's frames go to the screen at once, not at the next vertical blank (per PresentMon:
//! sync interval 0, AllowsTearing 1, shown 0.2–3.5 ms after rendering ends, 60 fps cap):
//! `--tearing` presents the same way, `--fps N` caps the frame rate, and `--gpu-copies N` adds GPU
//! work to every frame (N copies of a 4096×4096 texture; Roblox's frames took 6.6 ms of GPU time).
//! `--cpu-ms X` spends X ms between a frame's start (where a game reads its input) and its
//! drawing, `--focus` brings the window to the foreground like the game being played (a variable
//! refresh rate follows the foreground window), and `--log FILE` writes every frame's start and
//! present time (µs of the performance counter) for offline analysis.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11DeviceContext1, ID3D11RenderTargetView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory2, IDXGISwapChain1,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, HWND_TOP, MSG, PM_REMOVE, PeekMessageW, RegisterClassExW,
    SPI_GETWORKAREA, SW_SHOW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetForegroundWindow, SetWindowPos, ShowWindow,
    SystemParametersInfoW, TranslateMessage, WINDOW_EX_STYLE, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{Interface, Result, w};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    let seconds: u64 = value("--seconds").map_or(60, |s| s.parse().expect("seconds"));
    let tearing = args.iter().any(|a| a == "--tearing");
    let fps: Option<f64> = value("--fps").map(|s| s.parse().expect("fps"));
    let gpu_copies: u32 = value("--gpu-copies").map_or(0, |s| s.parse().expect("gpu copies"));
    let cpu_ms: f64 = value("--cpu-ms").map_or(0.0, |s| s.parse().expect("cpu ms"));
    let focus = args.iter().any(|a| a == "--focus");
    let log = value("--log").cloned();
    let mut frames: Vec<(i64, i64)> = Vec::new();
    let mut frequency = 0i64;
    // SAFETY: an out-parameter.
    unsafe { QueryPerformanceFrequency(&mut frequency) };
    let qpc = || {
        let mut c = 0i64;
        // SAFETY: an out-parameter.
        unsafe { QueryPerformanceCounter(&mut c) };
        c
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("PlaneGameProbe"),
            ..Default::default()
        };
        RegisterClassExW(&class);
        let mut work = RECT::default();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut work as *mut RECT as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))?;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("PlaneGameProbe"),
            w!("plane game probe"),
            WS_OVERLAPPEDWINDOW,
            work.left,
            work.top,
            work.right - work.left,
            work.bottom - work.top,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        if focus {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), work.left, work.top, work.right - work.left, work.bottom - work.top, SWP_NOACTIVATE);
        }
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client)?;
        let (width, height) = ((client.right - client.left) as u32, (client.bottom - client.top) as u32);

        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
        let (device, context) = (device.expect("device"), context.expect("context"));
        let context1: ID3D11DeviceContext1 = context.cast()?;
        let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            // DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.
            Flags: if tearing { 2048 } else { 0 },
            ..Default::default()
        };
        let chain: IDXGISwapChain1 = factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)?;
        println!("pid {} · {width}x{height} client · HWND swap chain, FLIP_DISCARD, opaque · tearing {tearing} · fps cap {fps:?} · gpu copies {gpu_copies}", std::process::id());
        // GPU work: copies between two large textures.
        let load = if gpu_copies > 0 {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: 4096,
                Height: 4096,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let (mut a, mut b): (Option<ID3D11Texture2D>, Option<ID3D11Texture2D>) = (None, None);
            device.CreateTexture2D(&desc, None, Some(&mut a))?;
            device.CreateTexture2D(&desc, None, Some(&mut b))?;
            Some((a.expect("texture"), b.expect("texture")))
        } else {
            None
        };

        let started = Instant::now();
        let mut frame = 0u64;
        while started.elapsed() < Duration::from_secs(seconds) {
            if let Some(fps) = fps {
                // The frame cap: busy-waits to the frame's start (sleeping is too coarse).
                let due = started + Duration::from_secs_f64(frame as f64 / fps);
                while Instant::now() < due {
                    std::hint::spin_loop();
                }
            }
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            // The frame's start: where a game reads its input.
            let sampled = qpc();
            while qpc() < sampled + (cpu_ms / 1000.0 * frequency as f64) as i64 {
                std::hint::spin_loop();
            }
            let buffer: ID3D11Texture2D = chain.GetBuffer(0)?;
            let mut view: Option<ID3D11RenderTargetView> = None;
            device.CreateRenderTargetView(&buffer, None, Some(&mut view))?;
            let view = view.expect("view");
            context.ClearRenderTargetView(&view, &[0.12, 0.16, 0.12, 1.0]);
            let x = (frame * 6 % (width as u64 - 120)) as i32;
            context1.ClearView(&view, &[0.8, 0.8, 0.2, 1.0], Some(&[RECT { left: x, top: 200, right: x + 120, bottom: 320 }]));
            if let Some((a, b)) = &load {
                for i in 0..gpu_copies {
                    if i % 2 == 0 { context.CopyResource(b, a) } else { context.CopyResource(a, b) }
                }
            }
            frames.push((sampled, qpc()));
            // DXGI_PRESENT_ALLOW_TEARING with sync interval 0.
            if tearing { chain.Present(0, DXGI_PRESENT(0x200)).ok()? } else { chain.Present(1, DXGI_PRESENT(0)).ok()? }
            frame += 1;
        }
        println!("{frame} frames in {:.1} s", started.elapsed().as_secs_f64());
    }
    if let Some(path) = log {
        use std::io::Write;
        let mut f = std::fs::File::create(path).expect("log");
        writeln!(f, "sampled_us,present_us").expect("write");
        for (s, p) in frames {
            writeln!(f, "{:.1},{:.1}", s as f64 * 1e6 / frequency as f64, p as f64 * 1e6 / frequency as f64).expect("write");
        }
    }
    Ok(())
}
