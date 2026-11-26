//! Non-activating top-most popup windows: click-through ones for guidance drawn over the game,
//! and interactive ones that take clicks without taking focus (the game keeps the keyboard).

use std::cell::RefCell;
use std::sync::Once;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
/// WM_MOUSELEAVE (winuser.h; not exported by the windows crate).
const WM_MOUSELEAVE: u32 = 0x02A3;
use windows::Win32::UI::WindowsAndMessaging::{
    HTCLIENT, IDC_ARROW, LoadCursorW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, HTTRANSPARENT, HWND_TOPMOST, MSG,
    PM_REMOVE, PeekMessageW, RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow, TranslateMessage, WM_MOUSEACTIVATE, WM_NCHITTEST,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, SetLayeredWindowAttributes, LWA_ALPHA,
    MA_NOACTIVATE,
};
use windows::Win32::Foundation::COLORREF;
use windows::core::w;

const CLASS: windows::core::PCWSTR = w!("MtcOverlay");
const INTERACTIVE_CLASS: windows::core::PCWSTR = w!("MtcOverlayInteractive");

/// Mouse input on an interactive window, in window pixels. Between `Down` and `Up` the window
/// holds the mouse capture: moves keep coming outside it (dragging).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mouse {
    Move(HWND, i32, i32),
    Leave(HWND),
    Down(HWND, i32, i32),
    Up(HWND, i32, i32),
}

thread_local! {
    static MOUSE: RefCell<Vec<Mouse>> = const { RefCell::new(Vec::new()) };
}

/// Mouse input received since the last call (this thread's windows).
pub fn take_mouse() -> Vec<Mouse> {
    MOUSE.with(|m| std::mem::take(&mut *m.borrow_mut()))
}

fn point(lparam: LPARAM) -> (i32, i32) {
    let v = lparam.0 as u32;
    (i32::from((v & 0xFFFF) as i16), i32::from((v >> 16) as i16))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe extern "system" fn interactive_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let (x, y) = point(lparam);
            let mut track = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
            MOUSE.with(|m| m.borrow_mut().push(Mouse::Move(hwnd, x, y)));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            MOUSE.with(|m| m.borrow_mut().push(Mouse::Leave(hwnd)));
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point(lparam);
            unsafe {
                SetCapture(hwnd);
            }
            MOUSE.with(|m| m.borrow_mut().push(Mouse::Down(hwnd, x, y)));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = point(lparam);
            unsafe {
                let _ = ReleaseCapture();
            }
            MOUSE.with(|m| m.borrow_mut().push(Mouse::Up(hwnd, x, y)));
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn register_classes() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let instance = GetModuleHandleW(None).expect("module handle").into();
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
        let interactive = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(interactive_wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: INTERACTIVE_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&interactive);
    });
}

/// The cursor position (screen px).
pub fn cursor() -> (i32, i32) {
    let mut p = windows::Win32::Foundation::POINT::default();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// The whole desktop, every monitor (screen px: left, top, width, height).
pub fn desktop() -> (i32, i32, i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN};
    // SAFETY: plain queries.
    unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) }
}

/// Creates a hidden click-through overlay window of the given size (physical pixels).
pub fn create(width: i32, height: i32) -> windows::core::Result<HWND> {
    create_window(width, height, false)
}

/// Creates a hidden overlay window that takes clicks (reported by `take_mouse`) without
/// activating.
pub fn create_interactive(width: i32, height: i32) -> windows::core::Result<HWND> {
    create_window(width, height, true)
}

fn create_window(width: i32, height: i32, interactive: bool) -> windows::core::Result<HWND> {
    register_classes();
    let click_through = if interactive { Default::default() } else { WS_EX_TRANSPARENT };
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | click_through | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            if interactive { INTERACTIVE_CLASS } else { CLASS },
            w!("MTC overlay"),
            WS_POPUP,
            0,
            0,
            width,
            height,
            None,
            None,
            Some(GetModuleHandleW(None)?.into()),
            None,
        )?;
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
        Ok(hwnd)
    }
}

/// Windows placed so far (each placement puts the window on top of the others): the settings
/// window is raised again after any (`raise`).
pub static PLACED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Puts the window back on top of the other top-most windows, where it is and as big.
pub fn raise(hwnd: HWND) {
    if crate::HIDDEN.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE);
    }
}

/// Moves and resizes the window (shown, top-most, not activated).
pub fn place_sized(hwnd: HWND, x: i32, y: i32, width: i32, height: i32) {
    if crate::HIDDEN.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    PLACED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, width, height, SWP_NOACTIVATE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

pub fn hide(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

/// Handles pending window messages for this thread's windows.
pub fn pump_messages() {
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
