//! What the HUD's GPU read-backs cost outside its own code: in play, the graphics
//! driver's threads in the HUD (`nvwgf2umx.dll`) took 0.038–0.040 cores, against 0.007 for all the
//! other threads when the capture library ran alone (`capture_cost.rs`). Captures a window that
//! redraws about 64 times a second at every frame, and alternates between doing nothing with the
//! frames and reading back the HUD's regions at the HUD's rates (readout and view cues 25 a second,
//! loading UI 4.6, seat menu 1) in several ways (`MODES`), measuring the process's threads by the
//! module they started in. Measured: the readout and cues cost 0.040 cores over the capture
//! alone, 0.036 with 8×8 px regions, 0.009 double-buffered.
//!
//! `cargo run --release -p mtc-hud --example readback_cost` spawns the target window (`capture_cost
//! --target`, a dark window at the bottom of the z-order) and closes it at the end.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use mtc_capture::readback::{Readback, Region};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
use windows::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThread, OpenThread, THREAD_QUERY_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION};
use windows::Win32::System::WindowsProgramming::{QueryProcessCycleTime, QueryThreadCycleTime};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

const TITLE: &str = "mtc-capture-cost-target";
const PHASE: Duration = Duration::from_secs(20);
const ROUNDS: usize = 2;
/// The client's top-left in the frame (the 34 px title bar of a maximised window).
const ORIGIN: (i32, i32) = (0, 34);
const CLIENT: (i32, i32, u32, u32) = (0, 34, 2560, 1334);

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQueryInformationThread(thread: HANDLE, class: u32, info: *mut core::ffi::c_void, len: u32, returned: *mut u32) -> i32;
}

fn start_module(thread: HANDLE) -> String {
    let mut address: usize = 0;
    if unsafe { NtQueryInformationThread(thread, 9, (&raw mut address).cast(), size_of::<usize>() as u32, std::ptr::null_mut()) } != 0 {
        return "?".into();
    }
    let mut module = HMODULE::default();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if unsafe { GetModuleHandleExW(flags, windows::core::PCWSTR(address as *const u16), &mut module) }.is_err() {
        return "?".into();
    }
    let mut buf = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit('\\').next().unwrap_or(&path).to_owned()
}

/// Cycles of this process's threads, summed by the module each started in.
fn by_module() -> std::collections::HashMap<String, u64> {
    let mut out = std::collections::HashMap::new();
    let pid = unsafe { GetCurrentProcessId() };
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }) else { return out };
    let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
    let mut more = unsafe { Thread32First(snap, &mut entry) }.is_ok();
    while more {
        if entry.th32OwnerProcessID == pid
            && let Ok(h) = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION | THREAD_QUERY_INFORMATION, false, entry.th32ThreadID) }
        {
            let mut c = 0u64;
            let _ = unsafe { QueryThreadCycleTime(h, &mut c) };
            *out.entry(start_module(h)).or_insert(0) += c;
            unsafe {
                let _ = CloseHandle(h);
            }
        }
        more = unsafe { Thread32Next(snap, &mut entry) }.is_ok();
    }
    unsafe {
        let _ = CloseHandle(snap);
    }
    out
}

fn cycles_per_second() -> f64 {
    let (mut a, mut b) = (0u64, 0u64);
    unsafe {
        let _ = QueryThreadCycleTime(GetCurrentThread(), &mut a);
    }
    let start = Instant::now();
    let mut x = 0u64;
    while start.elapsed() < Duration::from_millis(500) {
        x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
    }
    unsafe {
        let _ = QueryThreadCycleTime(GetCurrentThread(), &mut b);
    }
    (b - a) as f64 / start.elapsed().as_secs_f64()
}

/// What the handler reads back: nothing, all of the HUD's reads, only the readout and cues (a
/// staging texture and a map each, or packed into one texture and one map), only the loading UI,
/// only the seat menu.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Nothing,
    All,
    Main,
    MainPacked,
    Loading,
    Seat,
    /// The readout and cue rectangles cut to 8×8 px each: the same reads, almost no bytes.
    MainTiny,
    /// The readout and cues double-buffered: copy this frame into one set of staging textures, map
    /// the set copied on the previous read (no wait for this frame's copy).
    MainDouble,
}

const MODES: [(Mode, &str); 8] = [
    (Mode::Nothing, "capture only"),
    (Mode::All, "all of the HUD's readbacks"),
    (Mode::Main, "readout boxes and view hints only (as the HUD does it: 5 regions, one map each)"),
    (Mode::MainPacked, "readout boxes and view hints only, packed into one texture, one map"),
    (Mode::MainTiny, "readout boxes and view hints only, each cut to 8x8 px"),
    (Mode::MainDouble, "readout boxes and view hints only, double-buffered (mapping the previous copy)"),
    (Mode::Loading, "loading area only"),
    (Mode::Seat, "seat area only"),
];

/// The readout and cue rectangles copied into one staging texture, stacked, and mapped once.
struct Packed {
    texture: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    out: Vec<u8>,
}

impl Packed {
    fn read(this: &mut Option<Self>, frame: &Frame, rects: &[(i32, i32, u32, u32)]) -> windows::core::Result<()> {
        use windows::Win32::Graphics::Direct3D11::{D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING};
        let desc = *frame.desc();
        let width = rects.iter().map(|r| r.2).max().unwrap_or(1);
        let height: u32 = rects.iter().map(|r| r.3).sum();
        if this.is_none() {
            let d = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: desc.Format,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut t = None;
            unsafe { frame.device().CreateTexture2D(&d, None, Some(&mut t))? };
            *this = Some(Self { texture: t.expect("texture"), out: vec![0; (width * height * 4) as usize] });
        }
        let p = this.as_mut().expect("packed");
        let context = frame.device_context();
        let mut y = 0;
        for &(x0, y0, w, h) in rects {
            let b = D3D11_BOX { left: x0.max(0) as u32, top: y0.max(0) as u32, front: 0, right: (x0.max(0) as u32 + w).min(desc.Width), bottom: (y0.max(0) as u32 + h).min(desc.Height), back: 1 };
            unsafe { context.CopySubresourceRegion(&p.texture, 0, 0, y, 0, frame.as_raw_texture(), 0, Some(&b)) };
            y += h;
        }
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&p.texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row = (width * 4) as usize;
            for r in 0..height as usize {
                std::ptr::copy_nonoverlapping(mapped.pData.cast::<u8>().add(r * mapped.RowPitch as usize), p.out.as_mut_ptr().add(r * row), row);
            }
            context.Unmap(&p.texture, 0);
        }
        Ok(())
    }
}

/// Two sets of staging textures for the readout and cues, used in turn.
struct Double {
    sets: [Vec<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>; 2],
    turn: usize,
    copied: bool,
    out: Vec<u8>,
}

impl Double {
    fn read(this: &mut Option<Self>, frame: &Frame, rects: &[(i32, i32, u32, u32)]) -> windows::core::Result<()> {
        use windows::Win32::Graphics::Direct3D11::{D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING};
        let desc = *frame.desc();
        if this.is_none() {
            let make = || -> windows::core::Result<Vec<_>> {
                rects
                    .iter()
                    .map(|&(_, _, w, h)| {
                        let d = D3D11_TEXTURE2D_DESC {
                            Width: w,
                            Height: h,
                            MipLevels: 1,
                            ArraySize: 1,
                            Format: desc.Format,
                            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                            Usage: D3D11_USAGE_STAGING,
                            BindFlags: 0,
                            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                            MiscFlags: 0,
                        };
                        let mut t = None;
                        unsafe { frame.device().CreateTexture2D(&d, None, Some(&mut t))? };
                        Ok(t.expect("texture"))
                    })
                    .collect()
            };
            *this = Some(Self { sets: [make()?, make()?], turn: 0, copied: false, out: Vec::new() });
        }
        let d = this.as_mut().expect("double");
        let context = frame.device_context();
        for (t, &(x0, y0, w, h)) in d.sets[d.turn].iter().zip(rects) {
            let b = D3D11_BOX { left: x0.max(0) as u32, top: y0.max(0) as u32, front: 0, right: (x0.max(0) as u32 + w).min(desc.Width), bottom: (y0.max(0) as u32 + h).min(desc.Height), back: 1 };
            unsafe { context.CopySubresourceRegion(t, 0, 0, 0, 0, frame.as_raw_texture(), 0, Some(&b)) };
        }
        let other = 1 - d.turn;
        if d.copied {
            for (t, &(_, _, w, h)) in d.sets[other].iter().zip(rects) {
                unsafe {
                    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                    context.Map(t, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                    let row = (w * 4) as usize;
                    d.out.resize(row * h as usize, 0);
                    for r in 0..h as usize {
                        std::ptr::copy_nonoverlapping(mapped.pData.cast::<u8>().add(r * mapped.RowPitch as usize), d.out.as_mut_ptr().add(r * row), row);
                    }
                    context.Unmap(t, 0);
                }
            }
        }
        d.copied = true;
        d.turn = other;
        Ok(())
    }
}

struct Probe {
    frames: Arc<AtomicU64>,
    double: Option<Double>,
    tiny: Vec<(i32, i32, u32, u32)>,
    tiny_regions: Vec<Region>,
    mode: Arc<AtomicU8>,
    packed: Option<Packed>,
    readback: Readback,
    regions: Vec<Region>,
    loading: Region,
    seat: Region,
    rects: Vec<(i32, i32, u32, u32)>,
    last: [Option<Instant>; 3],
}

impl GraphicsCaptureApiHandler for Probe {
    type Flags = (Arc<AtomicU64>, Arc<AtomicU8>);
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let aim = mtc_vision::aim::AimReader::new().wanted_region(ORIGIN);
        let cues = mtc_vision::map::cues::CueReader::new().regions(ORIGIN);
        let rects: Vec<(i32, i32, u32, u32)> = std::iter::once(aim).chain(cues.iter().copied()).map(|(x, y, w, h)| (x, y, w as u32, h as u32)).collect();
        Ok(Self {
            frames: ctx.flags.0,
            mode: ctx.flags.1,
            packed: None,
            double: None,
            tiny: rects.iter().map(|&(x, y, _, _)| (x, y, 8, 8)).collect(),
            tiny_regions: vec![Region::default(); rects.len()],
            readback: Readback::new(),
            regions: vec![Region::default(); rects.len()],
            loading: Region::default(),
            seat: Region::default(),
            rects,
            last: [None; 3],
        })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, _control: InternalCaptureControl) -> Result<(), Self::Error> {
        self.frames.fetch_add(1, Ordering::Relaxed);
        let mode = MODES[usize::from(self.mode.load(Ordering::Relaxed))].0;
        let due = |last: Option<Instant>, every: Duration| last.is_none_or(|t| t.elapsed() >= every);
        if matches!(mode, Mode::All | Mode::Main | Mode::MainPacked | Mode::MainTiny | Mode::MainDouble) && due(self.last[0], Duration::from_millis(40)) {
            self.last[0] = Some(Instant::now());
            if mode == Mode::MainPacked {
                Packed::read(&mut self.packed, frame, &self.rects).map_err(|e| e.to_string())?;
            } else if mode == Mode::MainTiny {
                self.readback.read_regions(frame, &self.tiny, &mut self.tiny_regions).map_err(|e| e.to_string())?;
            } else if mode == Mode::MainDouble {
                Double::read(&mut self.double, frame, &self.rects).map_err(|e| e.to_string())?;
            } else {
                self.readback.read_regions(frame, &self.rects, &mut self.regions).map_err(|e| e.to_string())?;
            }
        }
        if matches!(mode, Mode::All | Mode::Loading) && due(self.last[1], Duration::from_millis(217)) {
            self.last[1] = Some(Instant::now());
            let (x, y, w, h) = mtc_vision::loading::LOADING_REGION;
            self.readback.read_region(frame, ORIGIN.0 + x, ORIGIN.1 + y, w as u32, h as u32, &mut self.loading).map_err(|e| e.to_string())?;
        }
        if matches!(mode, Mode::All | Mode::Seat) && due(self.last[2], Duration::from_millis(1000)) {
            self.last[2] = Some(Instant::now());
            let (x, y, w, h) = mtc_vision::seats::SeatReader::region(CLIENT);
            self.readback.read_region(frame, x, y, w, h, &mut self.seat).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn main() {
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let target = std::env::current_exe().expect("exe").with_file_name("capture_cost.exe");
    let mut child = std::process::Command::new(target).arg("--target").spawn().expect("target window (build the capture_cost example)");
    let window = loop {
        if let Ok(w) = Window::from_name(TITLE) {
            break w;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let hz = cycles_per_second();
    let frames = Arc::new(AtomicU64::new(0));
    let mode = Arc::new(AtomicU8::new(0));
    let settings = Settings::new(
        window,
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Custom(Duration::from_millis(4)),
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        (frames.clone(), mode.clone()),
    );
    let control = Probe::start_free_threaded(settings).expect("capture");
    std::thread::sleep(Duration::from_secs(2));
    for _ in 0..ROUNDS {
        for (i, (_, name)) in MODES.iter().enumerate() {
            mode.store(i as u8, Ordering::Relaxed);
            std::thread::sleep(Duration::from_secs(2));
            let (m0, p0, f0, t0) = (by_module(), process_cycles(), frames.load(Ordering::Relaxed), Instant::now());
            std::thread::sleep(PHASE);
            let (m1, p1, f1, dt) = (by_module(), process_cycles(), frames.load(Ordering::Relaxed), t0.elapsed().as_secs_f64());
            let mut parts: Vec<(String, f64)> = m1.iter().map(|(k, &v)| (k.clone(), v.saturating_sub(*m0.get(k).unwrap_or(&0)) as f64 / hz / dt)).filter(|(_, c)| *c >= 0.0005).collect();
            parts.sort_by(|a, b| b.1.total_cmp(&a.1));
            let parts: Vec<String> = parts.iter().map(|(k, c)| format!("{k} {c:.4}")).collect();
            println!(
                "{}: {:.0} frames/s delivered · process {:.4} cores · {}",
                name,
                (f1 - f0) as f64 / dt,
                (p1 - p0) as f64 / hz / dt,
                parts.join(" · ")
            );
        }
    }
    let _ = control.stop();
    let _ = child.kill();
}

fn process_cycles() -> u64 {
    let mut c = 0u64;
    unsafe {
        let _ = QueryProcessCycleTime(GetCurrentProcess(), &mut c);
    }
    c
}
