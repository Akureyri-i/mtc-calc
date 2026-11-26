use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetCursorPos, GetForegroundWindow};
use windows_capture::window::Window;

/// Executable of the Roblox player (Roblox Studio windows are ignored).
pub const ROBLOX_PROCESS: &str = "RobloxPlayerBeta.exe";

/// Makes every coordinate this process reads from Windows a physical pixel.
pub fn enable_per_monitor_dpi_awareness() {
    // Fails only when awareness was already set (for example by a manifest), which is the goal.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// The open Roblox player window.
pub fn find_roblox() -> Option<Window> {
    Window::enumerate().ok()?.into_iter().find(|window| {
        window
            .process_name()
            .is_ok_and(|name| name.eq_ignore_ascii_case(ROBLOX_PROCESS))
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    fn from_win32(rect: RECT) -> Self {
        Self {
            x: rect.left,
            y: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
        }
    }
}

/// Window placement in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowGeometry {
    /// Visible window bounds on screen (DWM extended frame bounds). Windows Graphics Capture
    /// frames cover exactly this area, title bar included.
    pub frame: Rect,
    /// Client area on screen.
    pub client: Rect,
    pub dpi: u32,
}

impl WindowGeometry {
    pub fn query(window: &Window) -> windows::core::Result<Self> {
        let hwnd = HWND(window.as_raw_hwnd());
        let mut frame = RECT::default();
        let mut client = RECT::default();
        let mut origin = POINT::default();
        unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&raw mut frame).cast(),
                size_of::<RECT>() as u32,
            )?;
            GetClientRect(hwnd, &mut client)?;
            let _ = ClientToScreen(hwnd, &mut origin);
        }
        Ok(Self {
            frame: Rect::from_win32(frame),
            client: Rect {
                x: origin.x,
                y: origin.y,
                width: client.right,
                height: client.bottom,
            },
            dpi: unsafe { GetDpiForWindow(hwnd) },
        })
    }

    /// Client area in captured-frame coordinates.
    pub fn client_in_frame(&self) -> Rect {
        Rect {
            x: self.client.x - self.frame.x,
            y: self.client.y - self.frame.y,
            ..self.client
        }
    }

    /// Converts a screen point to captured-frame coordinates.
    pub fn screen_to_frame(&self, x: i32, y: i32) -> (i32, i32) {
        (x - self.frame.x, y - self.frame.y)
    }
}

/// Cursor position in physical screen pixels.
pub fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.ok()?;
    Some((point.x, point.y))
}

/// Whether the given window currently has keyboard focus.
pub fn is_foreground(window: &Window) -> bool {
    unsafe { GetForegroundWindow() }.0 == window.as_raw_hwnd()
}

/// The foreground window's handle as a number (0 when none), to compare with a window's.
pub fn foreground_handle() -> isize {
    unsafe { GetForegroundWindow() }.0 as isize
}
