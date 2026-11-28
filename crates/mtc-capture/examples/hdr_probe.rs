//! Checks the 16-bit capture against known colours: a window of grey steps and the game's UI
//! colours is captured in 8-bit and in 16-bit colour (brought back to ordinary brightness by
//! `Readback`), and each patch's centre is compared with what was drawn. The window goes under
//! every other window (a covered window is still composed and captured) on the primary display,
//! or with `--on N` on display N (0-based, in the order Windows lists them), to check a display
//! with HDR on.
//! `--list` lists the displays and their HDR state; `--hdr N on|off` turns HDR on or off on
//! display N.
//!   cargo run --release -p mtc-capture --example hdr_probe -- [--on N | --list | --hdr N on|off]
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, EnumDisplayMonitors, FillRect, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HWND_BOTTOM, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SW_SHOWNOACTIVATE, SetWindowPos, ShowWindow, TranslateMessage, WM_PAINT, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::w;
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings};
use windows_capture::window::Window;

/// Patch side (px): 21 patches fit a 800 px wide display.
const PATCH: i32 = 36;
/// Grey steps, then the game's UI yellow and pure red, green, blue.
const COLOURS: [(u8, u8, u8); 21] = [
    (0, 0, 0), (16, 16, 16), (32, 32, 32), (48, 48, 48), (64, 64, 64), (80, 80, 80), (96, 96, 96), (112, 112, 112), (128, 128, 128),
    (144, 144, 144), (160, 160, 160), (176, 176, 176), (192, 192, 192), (208, 208, 208), (224, 224, 224), (240, 240, 240), (255, 255, 255),
    (240, 183, 74), (255, 0, 0), (0, 255, 0), (0, 0, 255),
];

extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_PAINT {
        let mut ps = PAINTSTRUCT::default();
        let dc: HDC = unsafe { BeginPaint(hwnd, &mut ps) };
        for (i, &(r, g, b)) in COLOURS.iter().enumerate() {
            let rect = RECT { left: i as i32 * PATCH, top: 0, right: (i as i32 + 1) * PATCH, bottom: PATCH };
            let brush = unsafe { CreateSolidBrush(COLORREF(u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16)) };
            unsafe {
                FillRect(dc, &rect, brush);
                let _ = DeleteObject(brush.into());
            }
        }
        unsafe {
            let _ = EndPaint(hwnd, &ps);
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn pump(for_ms: u64) {
    let until = Instant::now() + Duration::from_millis(for_ms);
    while Instant::now() < until {
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A point inside display `index`, clear of its edges.
fn display_origin(index: usize) -> (i32, i32) {
    let list = displays();
    let (_, r, _, _, _) = list.get(index).unwrap_or_else(|| panic!("display {index} of {}", list.len()));
    (r.left + 20, r.top + 20)
}

struct Probe {
    readback: mtc_capture::readback::Readback,
    out: Arc<Mutex<Option<(Vec<u8>, u32, f32)>>>,
    frames: u32,
}

impl GraphicsCaptureApiHandler for Probe {
    type Flags = (Arc<Mutex<Option<(Vec<u8>, u32, f32)>>>, f32);
    type Error = Box<dyn std::error::Error + Send + Sync>;
    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let mut readback = mtc_capture::readback::Readback::new();
        readback.set_white(ctx.flags.1);
        Ok(Self { readback, out: ctx.flags.0, frames: 0 })
    }
    fn on_frame_arrived(&mut self, frame: &mut Frame, control: InternalCaptureControl) -> Result<(), Self::Error> {
        self.frames += 1;
        // The window was painted before the capture started; a still window sends one frame.
        if self.frames == 1 {
            // The cost of reading a 512x256 region (copy, map, conversion) on this frame.
            let mut region = [mtc_capture::readback::Region::default()];
            let start = Instant::now();
            for _ in 0..100 {
                self.readback.read_regions(frame, &[(0, 0, 512, 256)], &mut region)?;
            }
            println!("{:?}: a {}x{} region read back in {:.3} ms", frame.desc().Format, region[0].width, region[0].height, start.elapsed().as_secs_f64() * 10.0);
            let mut pixels = Vec::new();
            self.readback.read_full(frame, &mut pixels)?;
            let peak = self.readback.take_peak();
            *self.out.lock().unwrap() = Some((pixels, frame.width(), peak));
            control.stop();
        }
        Ok(())
    }
}

/// Each display (in `EnumDisplayMonitors` order): its GDI name, rectangle, path, whether it can
/// show HDR and whether HDR is on.
fn displays() -> Vec<(String, RECT, DISPLAYCONFIG_PATH_INFO, bool, bool)> {
    extern "system" fn each(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> windows::core::BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<(String, RECT)>) };
        let mut info = MONITORINFOEXW { monitorInfo: MONITORINFO { cbSize: size_of::<MONITORINFOEXW>() as u32, ..Default::default() }, ..Default::default() };
        if unsafe { GetMonitorInfoW(monitor, (&raw mut info).cast()) }.as_bool() {
            list.push((String::from_utf16_lossy(&info.szDevice).trim_end_matches('\0').to_owned(), info.monitorInfo.rcMonitor));
        }
        true.into()
    }
    let mut monitors: Vec<(String, RECT)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&raw mut monitors as isize));
    }
    let (mut n_paths, mut n_modes) = (0u32, 0u32);
    unsafe {
        let _ = GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, &mut n_modes);
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
    unsafe {
        let _ = QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, paths.as_mut_ptr(), &mut n_modes, modes.as_mut_ptr(), None);
    }
    monitors
        .into_iter()
        .filter_map(|(gdi, rect)| {
            let path = *paths[..n_paths as usize].iter().find(|p| {
                let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
                source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
                source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
                source.header.adapterId = p.sourceInfo.adapterId;
                source.header.id = p.sourceInfo.id;
                let found = unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } == 0;
                found && String::from_utf16_lossy(&source.viewGdiDeviceName).trim_end_matches('\0') == gdi
            })?;
            let mut colour = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
            colour.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
            colour.header.size = size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32;
            colour.header.adapterId = path.targetInfo.adapterId;
            colour.header.id = path.targetInfo.id;
            let value = if unsafe { DisplayConfigGetDeviceInfo(&mut colour.header) } == 0 { unsafe { colour.Anonymous.value } } else { 0 };
            Some((gdi, rect, path, value & 1 != 0, value & 2 != 0))
        })
        .collect()
}

/// Turns HDR on or off on a display; the Windows status code.
fn set_hdr(path: &DISPLAYCONFIG_PATH_INFO, on: bool) -> i32 {
    let mut state = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE::default();
    state.header.r#type = DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE;
    state.header.size = size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>() as u32;
    state.header.adapterId = path.targetInfo.adapterId;
    state.header.id = path.targetInfo.id;
    state.Anonymous.value = u32::from(on);
    unsafe { DisplayConfigSetDeviceInfo(&state.header) }
}

fn main() {
    mtc_capture::window::enable_per_monitor_dpi_awareness();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        for (i, (gdi, r, _, supported, enabled)) in displays().iter().enumerate() {
            println!("display {i}: {gdi} at ({}, {}) {}x{}, HDR {}", r.left, r.top, r.right - r.left, r.bottom - r.top, match (supported, enabled) {
                (_, true) => "on",
                (true, false) => "off (supported)",
                _ => "not supported",
            });
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--hdr") {
        let index: usize = args[i + 1].parse().expect("display index");
        let on = args[i + 2] == "on";
        let list = displays();
        let (gdi, _, path, _, _) = list.get(index).expect("display");
        println!("HDR {} on display {index} ({gdi}): status {}", if on { "on" } else { "off" }, set_hdr(path, on));
        return;
    }
    let on: Option<usize> = std::env::args().skip_while(|a| a != "--on").nth(1).map(|v| v.parse().expect("display index"));
    let width = PATCH * COLOURS.len() as i32;
    let (x, y) = match on {
        Some(i) => display_origin(i),
        None => (50, 50),
    };
    let instance = unsafe { GetModuleHandleW(None) }.expect("module");
    let class = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: instance.into(), lpszClassName: w!("MtcHdrProbe"), ..Default::default() };
    unsafe { RegisterClassW(&class) };
    let hwnd = unsafe {
        CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, w!("MtcHdrProbe"), w!("MTC HDR probe"), WS_POPUP, x, y, width, PATCH, None, None, Some(instance.into()), None)
    }
    .expect("window");
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
    pump(300);
    let hdr = mtc_capture::window::display_hdr(hwnd);
    println!("window at ({x}, {y}); display HDR {hdr:?}");
    let white = match hdr {
        Some((true, nits)) => nits / 80.0,
        _ => 1.0,
    };
    let mut results = Vec::new();
    for (name, format) in [("8-bit", ColorFormat::Bgra8), ("16-bit", ColorFormat::Rgba16F)] {
        let out = Arc::new(Mutex::new(None));
        let settings = Settings::new(
            Window::from_raw_hwnd(hwnd.0),
            CursorCaptureSettings::WithoutCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            format,
            (out.clone(), white),
        );
        let capture = Probe::start_free_threaded(settings).expect("capture");
        let until = Instant::now() + Duration::from_secs(3);
        while !capture.is_finished() && Instant::now() < until {
            pump(20);
        }
        let _ = capture.stop();
        let got = out.lock().unwrap().take();
        results.push((name, got));
    }
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    println!("{:>15} | {:>15} | {:>15}", "drawn", "8-bit", "16-bit");
    let mut worst = [0i32; 2];
    for (i, &(r, g, b)) in COLOURS.iter().enumerate() {
        let mut cells = Vec::new();
        for (k, (_, got)) in results.iter().enumerate() {
            match got {
                Some((px, w, _)) => {
                    let (cx, cy) = ((i as i32 * PATCH + PATCH / 2) as usize, (PATCH / 2) as usize);
                    let o = (cy * *w as usize + cx) * 4;
                    let (pb, pg, pr) = (px[o], px[o + 1], px[o + 2]);
                    let d = [(pr, r), (pg, g), (pb, b)].iter().map(|&(a, e)| (i32::from(a) - i32::from(e)).abs()).max().unwrap();
                    worst[k] = worst[k].max(d);
                    cells.push(format!("{pr:>3},{pg:>3},{pb:>3}"));
                }
                None => cells.push("no frame".into()),
            }
        }
        println!("{:>15} | {:>15} | {:>15}", format!("{r:>3},{g:>3},{b:>3}"), cells[0], cells[1]);
    }
    let peak = results[1].1.as_ref().map(|(_, _, p)| *p);
    println!("largest channel difference from what was drawn: 8-bit {}, 16-bit {}; 16-bit white used {white:.3}, brightest channel measured {peak:?}", worst[0], worst[1]);
}
