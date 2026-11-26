//! What the capture library costs by itself: captures a window that redraws
//! about 64 times a second, doing nothing with the frames, at several minimum update intervals,
//! each in a capture session of its own (the library sets the interval only at the start), and
//! measures the capture thread and the whole process with two clocks: the thread and process
//! times the HUD's stats use (charged per clock tick) and CPU cycle counts.
//!
//! `cargo run --release -p mtc-hud --example capture_cost` spawns the target window itself (a dark
//! window at the bottom of the z-order, behind every other window) and closes it at the end.
//! `capture_cost --target [--framed]` shows only the target window (`--framed`: with a title bar
//! and borders, to check `mtc-capture`'s `window_sizes` example).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, CreateSolidBrush, EndPaint, FillRect, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetCurrentThreadId, GetProcessTimes, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::WindowsProgramming::{QueryProcessCycleTime, QueryThreadCycleTime};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_BOTTOM, MSG, PostQuitMessage, RegisterClassW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetTimer, SetWindowPos, ShowWindow, WM_DESTROY, WM_ERASEBKGND, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP,
};
use windows::core::w;
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

const TITLE: &str = "mtc-capture-cost-target";
/// The game window's size at the reference configuration (frame px).
const SIZE: (i32, i32) = (2560, 1368);
const INTERVALS_MS: [u64; 6] = [4, 16, 33, 100, 333, 1000];
const SETTLE: Duration = Duration::from_secs(2);
const MEASURE: Duration = Duration::from_secs(15);
const ROUNDS: usize = 2;

static TICK: AtomicU32 = AtomicU32::new(0);

extern "system" fn target_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER => {
                // A small square moves across the window: every redraw changes the picture.
                let t = TICK.fetch_add(1, Ordering::Relaxed) as i32;
                let x = (t * 7) % (SIZE.0 - 40);
                let _ = InvalidateRect(Some(hwnd), Some(&RECT { left: x - 8, top: 600, right: x + 48, bottom: 640 }), false);
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                FillRect(hdc, &ps.rcPaint, CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x0020_2020)));
                let t = TICK.load(Ordering::Relaxed) as i32;
                let x = (t * 7) % (SIZE.0 - 40);
                FillRect(hdc, &RECT { left: x, top: 604, right: x + 32, bottom: 636 }, CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x0060_6060)));
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn run_target() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).expect("module");
        let class = WNDCLASSW { lpfnWndProc: Some(target_proc), hInstance: instance.into(), lpszClassName: w!("MtcCaptureCostTarget"), ..Default::default() };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("MtcCaptureCostTarget"),
            w!("mtc-capture-cost-target"),
            if std::env::args().any(|a| a == "--framed") { WS_OVERLAPPEDWINDOW } else { WS_POPUP },
            0,
            0,
            SIZE.0,
            SIZE.1,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .expect("window");
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        SetTimer(Some(hwnd), 1, 15, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    }
}

struct Probe {
    frames: Arc<AtomicU64>,
    thread_id: Arc<AtomicU32>,
}

impl GraphicsCaptureApiHandler for Probe {
    type Flags = (Arc<AtomicU64>, Arc<AtomicU32>);
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { frames: ctx.flags.0, thread_id: ctx.flags.1 })
    }

    fn on_frame_arrived(&mut self, _frame: &mut Frame, _control: InternalCaptureControl) -> Result<(), Self::Error> {
        if self.thread_id.load(Ordering::Relaxed) == 0 {
            self.thread_id.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
        }
        self.frames.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn secs(f: FILETIME) -> f64 {
    ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) as f64 / 1e7
}

#[derive(Clone, Copy)]
struct Reading {
    at: Instant,
    frames: u64,
    thread_cycles: u64,
    thread_secs: f64,
    process_cycles: u64,
    process_secs: f64,
}

fn read(thread: HANDLE, frames: &AtomicU64) -> Reading {
    unsafe {
        let (mut tc, mut pc) = (0u64, 0u64);
        let _ = QueryThreadCycleTime(thread, &mut tc);
        let _ = QueryProcessCycleTime(GetCurrentProcess(), &mut pc);
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let _ = GetThreadTimes(thread, &mut c, &mut e, &mut k, &mut u);
        let thread_secs = secs(k) + secs(u);
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        Reading { at: Instant::now(), frames: frames.load(Ordering::Relaxed), thread_cycles: tc, thread_secs, process_cycles: pc, process_secs: secs(k) + secs(u) }
    }
}

/// Cycles per second of the cycle counter, from a busy loop on this thread.
fn cycles_per_second() -> f64 {
    let thread = unsafe { GetCurrentThread() };
    let (mut a, mut b) = (0u64, 0u64);
    unsafe {
        let _ = QueryThreadCycleTime(thread, &mut a);
    }
    let start = Instant::now();
    let mut x = 0u64;
    while start.elapsed() < Duration::from_millis(500) {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
    }
    std::hint::black_box(x);
    unsafe {
        let _ = QueryThreadCycleTime(thread, &mut b);
    }
    (b - a) as f64 / start.elapsed().as_secs_f64()
}

fn main() {
    if std::env::args().any(|a| a == "--target") {
        run_target();
        return;
    }
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let mut child = std::process::Command::new(std::env::current_exe().expect("exe")).arg("--target").spawn().expect("target window");
    let window = loop {
        if let Ok(w) = Window::from_name(TITLE) {
            break w;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let hz = cycles_per_second();
    println!("Cycle counter {:.3} GHz", hz / 1e9);

    let frames = Arc::new(AtomicU64::new(0));
    println!("interval | frames/s delivered | capture thread cores (cycles) | ms per frame (cycles) | capture thread cores (ticks) | process cores (cycles) | process cores (ticks)");
    for _ in 0..ROUNDS {
        for ms in INTERVALS_MS {
            let thread_id = Arc::new(AtomicU32::new(0));
            let settings = Settings::new(
                window,
                CursorCaptureSettings::WithoutCursor,
                DrawBorderSettings::WithoutBorder,
                SecondaryWindowSettings::Default,
                MinimumUpdateIntervalSettings::Custom(Duration::from_millis(ms)),
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                (frames.clone(), thread_id.clone()),
            );
            let control = Probe::start_free_threaded(settings).expect("capture");
            while thread_id.load(Ordering::Relaxed) == 0 {
                std::thread::sleep(Duration::from_millis(20));
            }
            let thread = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, thread_id.load(Ordering::Relaxed)) }.expect("capture thread");
            std::thread::sleep(SETTLE);
            let a = read(thread, &frames);
            std::thread::sleep(MEASURE);
            let b = read(thread, &frames);
            let _ = control.stop();
            unsafe {
                let _ = CloseHandle(thread);
            }
            let dt = b.at.duration_since(a.at).as_secs_f64();
            let fps = (b.frames - a.frames) as f64 / dt;
            let thread_cores = (b.thread_cycles - a.thread_cycles) as f64 / hz / dt;
            let per_frame_ms = if fps > 0.0 { thread_cores / fps * 1000.0 } else { f64::NAN };
            println!(
                "{ms:>4} ms | {fps:6.1} | {thread_cores:.4} | {per_frame_ms:.3} | {:.4} | {:.4} | {:.4}",
                (b.thread_secs - a.thread_secs) / dt,
                (b.process_cycles - a.process_cycles) as f64 / hz / dt,
                (b.process_secs - a.process_secs) / dt,
            );
        }
    }
    // The process with no capture running, for the baseline.
    std::thread::sleep(SETTLE);
    let cur = unsafe { GetCurrentThread() };
    let a = read(cur, &frames);
    std::thread::sleep(MEASURE);
    let b = read(cur, &frames);
    let dt = b.at.duration_since(a.at).as_secs_f64();
    println!("no capture | process cores (cycles) {:.4} | process cores (ticks) {:.4}", (b.process_cycles - a.process_cycles) as f64 / hz / dt, (b.process_secs - a.process_secs) / dt);
    let _ = child.kill();
}
