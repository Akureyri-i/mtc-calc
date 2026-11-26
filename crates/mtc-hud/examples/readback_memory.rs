//! What the readback's staging textures hold in memory, on a real capture: reads the HUD's regions
//! from a captured window as the HUD does (readout and view cues 25 a second, the readout's
//! columns drifting in width as its place moves; the map container of the open view) through
//! three phases: the minimap only, then the full map open, then the minimap again for longer than
//! the readback keeps an unused texture. After each phase: the working set, the private bytes and
//! the staging textures held. The driver keeps a released texture's memory pooled until the device
//! is trimmed (`memory_probe`), so the third phase shows whether the full map's texture really goes.
//!
//! `cargo run --release -p mtc-hud --example readback_memory` spawns the target window
//! (`capture_cost --target`, a dark window at the bottom of the z-order) and closes it at the end.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mtc_capture::readback::{Readback, Region};
use mtc_vision::map::MapView;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

const TITLE: &str = "mtc-capture-cost-target";
/// The client's top-left in the frame (the 34 px title bar of a maximised window).
const ORIGIN: (i32, i32) = (0, 34);
/// Phases: the view whose container is read, and how long.
const PHASES: [(MapView, &str, u64); 3] = [(MapView::Mini, "minimap", 8), (MapView::Full, "full map open", 8), (MapView::Mini, "after closing the full map", 15)];

fn memory() -> (f64, f64) {
    let mut c = PROCESS_MEMORY_COUNTERS_EX { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(), c.cb);
    }
    (c.WorkingSetSize as f64 / 1e6, c.PrivateUsage as f64 / 1e6)
}

struct Reader {
    phase: Arc<AtomicU8>,
    staging: Arc<Mutex<(usize, usize)>>,
    readback: Readback,
    rects: Vec<(i32, i32, u32, u32)>,
    regions: Vec<Region>,
    map: Region,
    last: Option<Instant>,
    turn: u32,
}

impl GraphicsCaptureApiHandler for Reader {
    type Flags = (Arc<AtomicU8>, Arc<Mutex<(usize, usize)>>);
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let aim = mtc_vision::aim::AimReader::new().wanted_region(ORIGIN);
        let cues = mtc_vision::map::cues::CueReader::new().regions(ORIGIN);
        let rects: Vec<(i32, i32, u32, u32)> = std::iter::once(aim).chain(cues.iter().copied()).map(|(x, y, w, h)| (x, y, w as u32, h as u32)).collect();
        Ok(Self { phase: ctx.flags.0, staging: ctx.flags.1, readback: Readback::new(), regions: vec![Region::default(); rects.len()], rects, map: Region::default(), last: None, turn: 0 })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, _control: InternalCaptureControl) -> Result<(), Self::Error> {
        if self.last.is_some_and(|t| t.elapsed() < Duration::from_millis(40)) {
            return Ok(());
        }
        self.last = Some(Instant::now());
        self.turn += 1;
        // The readout's columns: about 300 px, a pixel or two wider or narrower as its place moves.
        let mut rects = self.rects.clone();
        rects[0] = (rects[0].0 + 400, rects[0].1, 296 + self.turn % 5, rects[0].3);
        self.readback.read_regions(frame, &rects, &mut self.regions).map_err(|e| e.to_string())?;
        let view = PHASES[usize::from(self.phase.load(Ordering::Relaxed))].0;
        let (x, y, w, h) = view.frame_rect(ORIGIN);
        self.readback.read_region(frame, x, y, w as u32, h as u32, &mut self.map).map_err(|e| e.to_string())?;
        *self.staging.lock().expect("staging") = self.readback.staging_bytes();
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
    let phase = Arc::new(AtomicU8::new(0));
    let staging = Arc::new(Mutex::new((0, 0)));
    let settings = Settings::new(
        window,
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Custom(Duration::from_millis(4)),
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        (phase.clone(), staging.clone()),
    );
    let before = memory();
    let control = Reader::start_free_threaded(settings).expect("capture");
    for (i, (view, name, seconds)) in PHASES.iter().enumerate() {
        phase.store(i as u8, Ordering::Relaxed);
        std::thread::sleep(Duration::from_secs(*seconds));
        let (ws, private) = memory();
        let (n, bytes) = *staging.lock().expect("staging");
        let (x, y, w, h) = view.frame_rect(ORIGIN);
        println!(
            "{name} (reading {w}x{h} at {x},{y}) after {seconds} s: working set {ws:.1} MB ({:+.1}) · private {private:.1} MB ({:+.1}) · staging textures {n}, {:.1} MB",
            ws - before.0,
            private - before.1,
            bytes as f64 / 1e6
        );
    }
    let _ = control.stop();
    let _ = child.kill();
}
