//! Overlay-plane probe: does a small window with a DWM backdrop (acrylic, mica or the old blur)
//! above the game make Windows compose the game? A backdrop needs the pixels underneath, which a
//! game on its own hardware plane does not give DWM. If it does, the game and the HUD go through
//! the same composition while V1 is open, in step without a lead. Run `plane_game` underneath.
//!   cargo run --release -p mtc-overlay --example plane_backdrop -- [--kind acrylic|mica|blur]
//!       [--size WxH] [--at X,Y] [--seconds N]

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_BB_ENABLE, DWM_BLURBEHIND, DWMSBT_MAINWINDOW, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DwmEnableBlurBehindWindow,
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, HWND_TOPMOST, MSG, PM_REMOVE, PeekMessageW, RegisterClassExW, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SetWindowPos, ShowWindow, TranslateMessage, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Result, w};

extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let pair = |s: String, sep: char| -> (i32, i32) {
        let (a, b) = s.split_once(sep).expect("pair");
        (a.trim().parse().expect("number"), b.trim().parse().expect("number"))
    };
    let kind = value("--kind").unwrap_or_else(|| "acrylic".to_owned());
    let size = value("--size").map_or((16, 16), |s| pair(s, 'x'));
    let at = value("--at").map_or((1200, 700), |s| pair(s, ','));
    let seconds: u64 = value("--seconds").map_or(13, |s| s.parse().expect("seconds"));
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszClassName: w!("PlaneBackdropProbe"),
            ..Default::default()
        };
        RegisterClassExW(&class);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("PlaneBackdropProbe"),
            w!("plane backdrop probe"),
            WS_POPUP,
            at.0,
            at.1,
            size.0,
            size.1,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        DwmExtendFrameIntoClientArea(hwnd, &MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 })?;
        match kind.as_str() {
            "acrylic" | "mica" => {
                let backdrop = if kind == "mica" { DWMSBT_MAINWINDOW } else { DWMSBT_TRANSIENTWINDOW };
                DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as *const _, 4)?;
            }
            _ => {
                let blur = DWM_BLURBEHIND { dwFlags: DWM_BB_ENABLE, fEnable: true.into(), ..Default::default() };
                DwmEnableBlurBehindWindow(hwnd, &blur)?;
            }
        }
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), at.0, at.1, size.0, size.1, SWP_NOACTIVATE);
        println!("pid {} · {kind} backdrop · {}x{} at {:?}", std::process::id(), size.0, size.1, at);
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(seconds) {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    Ok(())
}
