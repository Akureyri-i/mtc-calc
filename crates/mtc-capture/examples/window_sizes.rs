//! Development tool for resolution support: sets the Roblox window's client area to the sizes
//! other players play at, so recordings at each can be made on one machine without changing the
//! display. Changes only the window's size and position, never the game's input.
//!
//! Ctrl+Shift+N: the next size in `SIZES` (the window is restored from maximised first).
//! Ctrl+Shift+B: the base size `BASE`, the client every recognizer is tuned at (2560×1334, the
//! development machine maximised at 150 %), kept the same at every Windows display scaling
//! (maximised, the title bar and taskbar change the client with the scaling): for measuring the
//! effect of the scaling alone, from the size the UI is tuned at.
//! Ctrl+Shift+M: maximised again. Ctrl+C ends.
//!
//! Every change prints the size asked for and the client size Windows gave (the recorder also
//! records each frame's geometry). The window is looked up again at every hotkey: rejoining the
//! game (as after a change of display scaling) can open a new one.
//!
//! `cargo run --release -p mtc-capture --example window_sizes [-- --size 1920x1080 | --maximize]
//! [--sizes 1920x720,1920x760,...] [--title <window title>]`: `--size` and `--maximize` set the size
//! once and end; `--sizes` is the list Ctrl+Shift+N steps through instead of `SIZES` (such as a
//! height sweep for the readout's font sizes); `--title` takes another window than Roblox's (to
//! check the tool without the game).

use mtc_capture::window::{enable_per_monitor_dpi_awareness, find_roblox};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetClientRect, GetMessageW, GetWindowLongW, IsZoomed, MSG, SW_MAXIMIZE, SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
    ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_HOTKEY,
};

/// Client sizes (physical px): common 16:9 sizes, a 16:10 and a wide one, all within the
/// development machine's 2560×1334 maximised client.
const SIZES: [(i32, i32); 7] = [(1920, 1080), (1600, 900), (1366, 768), (1280, 720), (1600, 1000), (2560, 1080), (2240, 1260)];
/// The client the recognizers are tuned at.
const BASE: (i32, i32) = (2560, 1334);
const NEXT: i32 = 1;
const MAXIMISE: i32 = 2;
const BASE_SIZE: i32 = 3;

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    (r.right - r.left, r.bottom - r.top)
}

/// Gives the window a client area of `w`×`h` starting at the screen's left edge, the window's top
/// at the screen's top (the side borders, mostly invisible, may stand off the screen).
fn set_client_size(hwnd: HWND, (w, h): (i32, i32)) {
    unsafe {
        if IsZoomed(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
        let ex = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32);
        let mut r = RECT { left: 0, top: 0, right: w, bottom: h };
        let _ = AdjustWindowRectExForDpi(&mut r, style, false, ex, GetDpiForWindow(hwnd));
        let _ = SetWindowPos(hwnd, None, r.left, 0, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Roblox's window, or the one titled `title`.
fn find_target(title: Option<&str>) -> Option<HWND> {
    match title {
        Some(title) => windows_capture::window::Window::from_name(title).ok().map(|w| HWND(w.as_raw_hwnd())),
        None => find_roblox().map(|w| HWND(w.as_raw_hwnd())),
    }
}

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    enable_per_monitor_dpi_awareness();
    let title = arg("--title");
    let hwnd = loop {
        if let Some(hwnd) = find_target(title.as_deref()) {
            break hwnd;
        }
        println!("Waiting for the Roblox window...");
        std::thread::sleep(std::time::Duration::from_secs(2));
    };
    if let Some(size) = arg("--size") {
        let (w, h) = size.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))).expect("--size WxH");
        set_client_size(hwnd, (w, h));
        std::thread::sleep(std::time::Duration::from_millis(300));
        let (cw, ch) = client_size(hwnd);
        println!("Asked {w}x{h}, client area {cw}x{ch}");
        return;
    }
    if std::env::args().any(|a| a == "--maximize") {
        unsafe {
            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        let (cw, ch) = client_size(hwnd);
        println!("Maximised, client area {cw}x{ch}");
        return;
    }
    let sizes: Vec<(i32, i32)> = arg("--sizes").map_or(SIZES.to_vec(), |list| {
        list.split(',').map(|s| s.trim().split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))).expect("--sizes WxH,WxH,...")).collect()
    });
    unsafe {
        RegisterHotKey(None, NEXT, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, u32::from(b'N')).expect("Ctrl+Shift+N taken");
        RegisterHotKey(None, MAXIMISE, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, u32::from(b'M')).expect("Ctrl+Shift+M taken");
        RegisterHotKey(None, BASE_SIZE, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, u32::from(b'B')).expect("Ctrl+Shift+B taken");
    }
    let (w, h) = client_size(hwnd);
    println!(
        "Roblox client area now {w}x{h} (DPI {}). Ctrl+Shift+N: the next size ({} in all, then from the start again); Ctrl+Shift+B: the reference size {}x{}; Ctrl+Shift+M: maximised again.",
        unsafe { GetDpiForWindow(hwnd) },
        sizes.len(),
        BASE.0,
        BASE.1
    );
    let mut next = 0;
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        if msg.message != WM_HOTKEY {
            continue;
        }
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let Some(hwnd) = find_target(title.as_deref()) else {
            println!("{at}: no Roblox window, nothing changed");
            continue;
        };
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if msg.wParam.0 as i32 == NEXT || msg.wParam.0 as i32 == BASE_SIZE {
            let size = if msg.wParam.0 as i32 == BASE_SIZE {
                BASE
            } else {
                next += 1;
                sizes[(next - 1) % sizes.len()]
            };
            set_client_size(hwnd, size);
            std::thread::sleep(std::time::Duration::from_millis(300));
            let (w, h) = client_size(hwnd);
            println!("{at}: size {}, asked {}x{}, client area {w}x{h} (DPI {dpi})", (next - 1) % sizes.len() + 1, size.0, size.1);
        } else {
            unsafe {
                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
            let (w, h) = client_size(hwnd);
            println!("{at}: maximised again, client area {w}x{h} (DPI {dpi})");
        }
    }
}
