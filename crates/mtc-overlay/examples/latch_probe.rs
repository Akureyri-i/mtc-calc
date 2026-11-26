//! Latch probe: how long an overlay update takes from the moment its input is sampled to the
//! composed desktop image that shows it, for the HUD's way of following a drag (wait for the
//! compositor, redraw, present) and for later and lighter ways of handing the same update over.
//! Run `plane_game` underneath to stand in for the game, and PresentMon beside it for the times the
//! desktop images reach the screen.
//!   cargo run --release -p mtc-overlay --example latch_probe -- --mode MODE [options]
//!
//! A window like the HUD's (layered, click-through, DirectComposition swap chain with premultiplied
//! alpha) shows a white block on a black strip; the block's place tells which update a desktop
//! image shows. Desktop duplication hands every composed desktop image with the time the
//! compositor made it; the compositor clock's statistics (Windows 11) give every compositor frame's
//! start, present and display times, and `IDXGIOutput::WaitForVBlank` every vertical blank.
//!
//! Modes:
//!   flush-present   the HUD now: DwmFlush, sample, redraw (`--draw-ms`), Present(0)
//!   late-present    sample `margin` before the next vertical blank, redraw, Present(0)
//!   flush-offset    DwmFlush, sample, move the visual (no redraw), Commit
//!   late-offset     sample `margin` before the next vertical blank, move the visual, Commit
//!   stream-offset   moves the visual and commits every `--every-ms` (default 1), whatever the
//!                   compositor's phase: what it takes at each pass is at most that old
//!   stream-present  the same with a redraw and Present(0) each time
//!   idle            no updates: only the vertical blanks are recorded (PREFIX-vblank.csv)
//! Options:
//!   --draw-ms X     time spent drawing before a present (default 0; the V1 marks take 2.3)
//!   --margin-ms X   late modes: a fixed margin; default sweeps 0.25–6 ms in 0.25 ms steps
//!   --seconds N     run time (default 8)
//!   --out PREFIX    writes PREFIX-submit.csv and PREFIX-screen.csv (times in µs of the performance
//!                   counter)

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{COLORREF, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11DeviceContext1, ID3D11RenderTargetView,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::DirectComposition::{
    COMPOSITION_FRAME_ID_COMPLETED, COMPOSITION_FRAME_STATS, COMPOSITION_STATS, COMPOSITION_TARGET_ID, DCompositionCreateDevice, DCompositionGetFrameId,
    DCompositionGetStatistics, DCompositionGetTargetStatistics, DCompositionWaitForCompositorClock, IDCompositionDevice,
};
use windows::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_ERROR_WAIT_TIMEOUT, DXGI_FRAME_STATISTICS, DXGI_OUTDUPL_FRAME_INFO,
    DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter1,
    IDXGIDevice, IDXGIFactory1, IDXGIFactory2, IDXGIOutput1, IDXGIResource, IDXGISwapChain1,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, HWND_TOPMOST, LWA_ALPHA, MSG, PM_REMOVE, PeekMessageW, RegisterClassExW, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SetLayeredWindowAttributes, SetWindowPos, ShowWindow, TranslateMessage, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{Interface, Result, w};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
}

/// The window on screen (physical px) and the strip row read from the desktop image.
const AT: (i32, i32) = (300, 900);
const SIZE: (i32, i32) = (700, 24);
/// The block: its width, its first place and its step per update; the place repeats every `CYCLE`.
const BLOCK: i32 = 8;
const FIRST: i32 = 4;
const STEP: i32 = 2;
const CYCLE: u64 = 256;
/// Offset modes: the block's place in the (wider) surface, black all around it.
const SURFACE_W: i32 = 1400;
const BLOCK_AT: i32 = 700;

fn qpc() -> i64 {
    let mut c = 0i64;
    // SAFETY: an out-parameter.
    unsafe { QueryPerformanceCounter(&mut c) };
    c
}

fn qpc_frequency() -> i64 {
    let mut f = 0i64;
    // SAFETY: an out-parameter.
    unsafe { QueryPerformanceFrequency(&mut f) };
    f
}

/// Busy-waits until the performance counter reaches `until` (sleeping is too coarse at 165 Hz).
fn spin_until(until: i64) {
    while qpc() < until {
        std::hint::spin_loop();
    }
}

/// The compositor's last vertical blank and refresh period (performance counter ticks).
fn vblank() -> Option<(i64, i64)> {
    let mut info = DWM_TIMING_INFO { cbSize: size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
    // SAFETY: an out-parameter sized by `cbSize`; a null window asks for the whole screen.
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut info) }.ok()?;
    let (v, p) = (info.qpcVBlank, info.qpcRefreshPeriod);
    (p > 0).then_some((v as i64, p as i64))
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

/// One composed desktop image: when the compositor made it, when it was handed over, how many
/// images it stands for, and the block's left edge in the strip (px from the window's left).
struct Screen {
    made: i64,
    acquired: i64,
    accumulated: u32,
    x: Option<i32>,
}

/// Reads the strip row of every composed desktop image until `stop`.
fn watch_desktop(stop: &AtomicBool, out: &Mutex<Vec<Screen>>) -> Result<()> {
    unsafe {
        // The adapter and output the desktop's origin is on.
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut found = None;
        'adapters: for a in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(a) else { break };
            for o in 0.. {
                let Ok(output) = adapter.EnumOutputs(o) else { break };
                let desc = output.GetDesc()?;
                let r = desc.DesktopCoordinates;
                if desc.AttachedToDesktop.as_bool() && r.left <= AT.0 && AT.0 < r.right && r.top <= AT.1 && AT.1 < r.bottom {
                    found = Some((adapter, output, r));
                    break 'adapters;
                }
            }
        }
        let (adapter, output, bounds): (IDXGIAdapter1, _, RECT) = found.expect("an output under the probe window");
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
        let (device, context) = (device.expect("device"), context.expect("context"));
        let duplication = output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: SIZE.0 as u32,
            Height: 1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        device.CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.expect("staging");
        let (x0, y) = ((AT.0 - bounds.left) as u32, (AT.1 - bounds.top + SIZE.1 / 2) as u32);
        let row = D3D11_BOX { left: x0, top: y, front: 0, right: x0 + SIZE.0 as u32, bottom: y + 1, back: 1 };
        while !stop.load(Ordering::Relaxed) {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            match duplication.AcquireNextFrame(100, &mut info, &mut resource) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => continue,
                Err(e) => return Err(e),
            }
            let acquired = qpc();
            if info.LastPresentTime != 0 {
                let texture: ID3D11Texture2D = resource.expect("desktop image").cast()?;
                context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture, 0, Some(&row));
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                let px = std::slice::from_raw_parts(mapped.pData as *const u8, SIZE.0 as usize * 4);
                let x = px.as_chunks::<4>().0.iter().position(|p| p[0] > 200 && p[1] > 200 && p[2] > 200).map(|x| x as i32);
                context.Unmap(&staging, 0);
                out.lock().expect("screens").push(Screen { made: info.LastPresentTime, acquired, accumulated: info.AccumulatedFrames, x });
            }
            duplication.ReleaseFrame()?;
        }
    }
    Ok(())
}

/// The time of every vertical blank of the output under the probe window until `stop`: with a
/// variable refresh rate they do not come at the mode's fixed period.
fn watch_vblanks(stop: &AtomicBool, out: &Mutex<Vec<i64>>) -> Result<()> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        for a in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(a) else { break };
            for o in 0.. {
                let Ok(output) = adapter.EnumOutputs(o) else { break };
                let desc = output.GetDesc()?;
                let r = desc.DesktopCoordinates;
                if desc.AttachedToDesktop.as_bool() && r.left <= AT.0 && AT.0 < r.right && r.top <= AT.1 && AT.1 < r.bottom {
                    while !stop.load(Ordering::Relaxed) {
                        output.WaitForVBlank()?;
                        out.lock().expect("vblanks").push(qpc());
                    }
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

/// One finished compositor frame, from the compositor clock's statistics (Windows 11): when the
/// compositor started it, the time it was aimed at, when it was presented and when the display
/// took it (counter ticks), with the refresh counts.
struct Frame {
    id: u64,
    start: u64,
    target: u64,
    period: u64,
    present: u64,
    vblank_duration: u64,
    presented: COMPOSITION_STATS,
    completed: COMPOSITION_STATS,
}

/// Records every finished compositor frame until `stop`, woken by the compositor clock.
fn watch_frames(stop: &AtomicBool, out: &Mutex<Vec<Frame>>) {
    let mut last = 0u64;
    while !stop.load(Ordering::Relaxed) {
        // SAFETY: no handles; returns at the compositor's next tick or after 100 ms.
        unsafe { DCompositionWaitForCompositorClock(None, 100) };
        // SAFETY: plain out-parameters.
        let Ok(newest) = (unsafe { DCompositionGetFrameId(COMPOSITION_FRAME_ID_COMPLETED) }) else { continue };
        if last == 0 {
            last = newest.saturating_sub(1);
        }
        for id in last + 1..=newest {
            let mut stats = COMPOSITION_FRAME_STATS::default();
            let mut targets = [COMPOSITION_TARGET_ID::default(); 8];
            let mut count = 0u32;
            // SAFETY: out-parameters sized as passed.
            if unsafe { DCompositionGetStatistics(id, &mut stats, targets.len() as u32, Some(targets.as_mut_ptr()), Some(&mut count)) }.is_err() || count == 0 {
                continue;
            }
            // SAFETY: a target id the call above returned.
            let Ok(target) = (unsafe { DCompositionGetTargetStatistics(id, &targets[0]) }) else { continue };
            out.lock().expect("frames").push(Frame {
                id,
                start: stats.startTime,
                target: stats.targetTime,
                period: stats.framePeriod,
                present: target.presentTime,
                vblank_duration: target.vblankDuration,
                presented: target.presentedStats,
                completed: target.completedStats,
            });
        }
        last = newest;
    }
}

/// One update handed to the compositor.
struct Submit {
    k: u64,
    sampled: i64,
    done: i64,
    vblank: i64,
    period: i64,
    margin_us: f64,
    /// The swap chain's count for this present, and the newest frame statistics then: the last
    /// present counted on screen, the refresh it showed at, and the newest refresh and its time.
    present: u32,
    stats: (u32, u32, u32, i64),
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let mode = value("--mode").unwrap_or_else(|| "flush-present".to_owned());
    let (late, offset, stream) = (mode.starts_with("late"), mode.ends_with("offset"), mode.starts_with("stream"));
    let every_ms: f64 = value("--every-ms").map_or(1.0, |s| s.parse().expect("every ms"));
    let draw_ms: f64 = value("--draw-ms").map_or(0.0, |s| s.parse().expect("draw ms"));
    let fixed_margin: Option<f64> = value("--margin-ms").map(|s| s.parse().expect("margin ms"));
    let seconds: f64 = value("--seconds").map_or(8.0, |s| s.parse().expect("seconds"));
    let out = value("--out").unwrap_or_else(|| "latch".to_owned());
    let frequency = qpc_frequency();
    let ticks = |ms: f64| (ms / 1000.0 * frequency as f64) as i64;
    let us = |t: i64| t as f64 * 1e6 / frequency as f64;
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: instance.into(), lpszClassName: w!("LatchProbe"), ..Default::default() };
        RegisterClassExW(&class);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            w!("LatchProbe"),
            w!("latch probe"),
            WS_POPUP,
            AT.0,
            AT.1,
            SIZE.0,
            SIZE.1,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;

        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
        let (device, context) = (device.expect("device"), context.expect("context"));
        let context1: ID3D11DeviceContext1 = context.cast()?;
        let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
        let width = if offset { SURFACE_W } else { SIZE.0 };
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width as u32,
            Height: SIZE.1 as u32,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            ..Default::default()
        };
        let chain: IDXGISwapChain1 = factory.CreateSwapChainForComposition(&device, &desc, None)?;
        let dcomp: IDCompositionDevice = DCompositionCreateDevice(&device.cast::<IDXGIDevice>()?)?;
        let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
        let root = dcomp.CreateVisual()?;
        let visual = dcomp.CreateVisual()?;
        visual.SetContent(&chain)?;
        root.AddVisual(&visual, false, None)?;
        target.SetRoot(&root)?;
        // The strip with the block at `x` (surface px).
        let draw = |x: i32| -> Result<()> {
            let buffer: ID3D11Texture2D = chain.GetBuffer(0)?;
            let mut view: Option<ID3D11RenderTargetView> = None;
            device.CreateRenderTargetView(&buffer, None, Some(&mut view))?;
            let view = view.expect("view");
            context.ClearRenderTargetView(&view, &[0.0, 0.0, 0.0, 1.0]);
            context1.ClearView(&view, &[1.0, 1.0, 1.0, 1.0], Some(&[RECT { left: x, top: 0, right: x + BLOCK, bottom: SIZE.1 }]));
            Ok(())
        };
        let place = |k: u64| FIRST + STEP * (k % CYCLE) as i32;
        if offset {
            draw(BLOCK_AT)?;
            chain.Present(0, DXGI_PRESENT(0)).ok()?;
            visual.SetOffsetX2((place(0) - BLOCK_AT) as f32)?;
        }
        dcomp.Commit()?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), AT.0, AT.1, SIZE.0, SIZE.1, SWP_NOACTIVATE);

        let stop = Arc::new(AtomicBool::new(false));
        let screens = Arc::new(Mutex::new(Vec::new()));
        let watcher = {
            let (stop, screens) = (stop.clone(), screens.clone());
            std::thread::spawn(move || {
                if let Err(e) = watch_desktop(&stop, &screens) {
                    eprintln!("desktop duplication: {e}");
                }
            })
        };
        let vblanks = Arc::new(Mutex::new(Vec::new()));
        let vblank_watcher = {
            let (stop, vblanks) = (stop.clone(), vblanks.clone());
            std::thread::spawn(move || {
                if let Err(e) = watch_vblanks(&stop, &vblanks) {
                    eprintln!("vertical blanks: {e}");
                }
            })
        };
        let frames = Arc::new(Mutex::new(Vec::new()));
        let frame_watcher = {
            let (stop, frames) = (stop.clone(), frames.clone());
            std::thread::spawn(move || watch_frames(&stop, &frames))
        };
        std::thread::sleep(std::time::Duration::from_millis(500));

        println!("pid {} · mode {mode} · draw {draw_ms} ms · counter frequency {frequency}", std::process::id());
        let mut submits: Vec<Submit> = Vec::new();
        let end = qpc() + ticks(seconds * 1000.0);
        let mut k = 1u64;
        while qpc() < end {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if mode == "idle" {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            let (v, p) = vblank().expect("compositor timing");
            let margin_ms = fixed_margin.unwrap_or(0.25 * (1 + (k / 16) % 24) as f64);
            let mut next = v;
            if late {
                // The first vertical blank this update can be `margin` ahead of.
                let now = qpc();
                next = v + ((now - v).div_euclid(p) + 1) * p;
                if next - ticks(margin_ms) < now {
                    next += p;
                }
                spin_until(next - ticks(margin_ms));
            } else if stream {
                // Sleeping, not spinning: the thread's CPU time below is what streaming costs.
                std::thread::sleep(std::time::Duration::from_secs_f64(every_ms / 1000.0));
            } else {
                DwmFlush()?;
            }
            let sampled = qpc();
            let mut present = 0;
            if offset {
                visual.SetOffsetX2((place(k) - BLOCK_AT) as f32)?;
                dcomp.Commit()?;
            } else {
                spin_until(sampled + ticks(draw_ms));
                draw(place(k))?;
                chain.Present(0, DXGI_PRESENT(0)).ok()?;
                present = chain.GetLastPresentCount().unwrap_or(0);
            }
            let done = qpc();
            let mut stats = DXGI_FRAME_STATISTICS::default();
            let stats = if !offset && chain.GetFrameStatistics(&mut stats).is_ok() { (stats.PresentCount, stats.PresentRefreshCount, stats.SyncRefreshCount, stats.SyncQPCTime) } else { (0, 0, 0, 0) };
            submits.push(Submit { k, sampled, done, vblank: v, period: p, margin_us: if late { margin_ms * 1000.0 } else { 0.0 }, present, stats });
            k += 1;
            if late {
                // One update per refresh.
                spin_until(next + ticks(0.3));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        stop.store(true, Ordering::Relaxed);
        let _ = watcher.join();
        let _ = vblank_watcher.join();
        let _ = frame_watcher.join();
        let mut f = std::fs::File::create(format!("{out}-frames.csv")).expect("frames csv");
        writeln!(f, "id,start_us,target_us,period_us,present_us,vblank_duration_us,presented_us,presented_refresh,presented_count,completed_us,completed_refresh,completed_count").expect("write");
        for r in frames.lock().expect("frames").iter() {
            let t = |v: u64| us(v as i64);
            writeln!(f, "{},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{},{},{:.1},{},{}", r.id, t(r.start), t(r.target), t(r.period), t(r.present), t(r.vblank_duration), t(r.presented.time), r.presented.refreshCount, r.presented.presentCount, t(r.completed.time), r.completed.refreshCount, r.completed.presentCount).expect("write");
        }
        let mut f = std::fs::File::create(format!("{out}-vblank.csv")).expect("vblank csv");
        writeln!(f, "vblank_us").expect("write");
        for &v in vblanks.lock().expect("vblanks").iter() {
            writeln!(f, "{:.1}", us(v)).expect("write");
        }

        let mut f = std::fs::File::create(format!("{out}-submit.csv")).expect("submit csv");
        writeln!(f, "k,place,sampled_us,done_us,vblank_us,period_us,margin_us,present,stat_present,stat_present_refresh,stat_sync_refresh,stat_sync_us").expect("write");
        for s in &submits {
            writeln!(f, "{},{},{:.1},{:.1},{:.1},{:.1},{:.0},{},{},{},{},{:.1}", s.k, place(s.k), us(s.sampled), us(s.done), us(s.vblank), us(s.period), s.margin_us, s.present, s.stats.0, s.stats.1, s.stats.2, us(s.stats.3)).expect("write");
        }
        let screens = screens.lock().expect("screens");
        let mut f = std::fs::File::create(format!("{out}-screen.csv")).expect("screen csv");
        writeln!(f, "made_us,acquired_us,accumulated,x").expect("write");
        for s in screens.iter() {
            writeln!(f, "{:.1},{:.1},{},{}", us(s.made), us(s.acquired), s.accumulated, s.x.map_or(String::new(), |x| x.to_string())).expect("write");
        }
        let (mut created, mut exited, mut kernel, mut user) = Default::default();
        let _ = GetThreadTimes(GetCurrentThread(), &mut created, &mut exited, &mut kernel, &mut user);
        let cpu = |t: windows::Win32::Foundation::FILETIME| ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) as f64 / 1e7;
        println!("{} updates, {} desktop images; submitting thread CPU {:.3} s over {seconds} s", submits.len(), screens.len(), cpu(kernel) + cpu(user));
    }
    Ok(())
}
