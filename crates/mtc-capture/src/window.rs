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

/// The open Roblox player window, wherever the player is installed.
pub fn find_roblox() -> Option<Window> {
    Window::enumerate().ok()?.into_iter().find(|window| {
        window
            .process_id()
            .ok()
            .and_then(|pid| process_file_name(pid).ok())
            .is_some_and(|name| name.eq_ignore_ascii_case(ROBLOX_PROCESS))
    })
}

/// A process's executable file name, from the path Windows keeps for it. Needs only the least
/// query right, which a process running as administrator grants too, and reads nothing of the
/// process itself.
pub fn process_file_name(pid: u32) -> windows::core::Result<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }?;
    let mut path = [0u16; 1024];
    let mut len = path.len() as u32;
    let result = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(path.as_mut_ptr()), &mut len) };
    let _ = unsafe { CloseHandle(process) };
    result?;
    let path = String::from_utf16_lossy(&path[..len as usize]);
    Ok(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
}

/// Windows titled like Roblox that `find_roblox` does not take, each with its program or why that
/// could not be read: for the log while the calculator waits for the game.
pub fn roblox_lookalikes() -> Vec<String> {
    Window::enumerate()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|window| {
            let title = window.title().ok().filter(|t| t.starts_with("Roblox"))?;
            let pid = window.process_id().ok()?;
            Some(match process_file_name(pid) {
                Ok(name) => format!("\"{title}\" ({name})"),
                Err(e) => format!("\"{title}\" (its program could not be read: {})", e.message()),
            })
        })
        .collect()
}

/// The display under `hwnd`: whether Windows HDR (advanced colour) is on there, and the white
/// level it gives ordinary (SDR) content, in nits (80 when HDR is off). With HDR on, a capture in
/// 8-bit colour comes out brighter by that white level over 80 nits, clipped at full brightness.
pub fn display_hdr(hwnd: HWND) -> Option<(bool, f32)> {
    use windows::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO,
        DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo,
        GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
    };
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromWindow};
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFOEXW { monitorInfo: MONITORINFO { cbSize: size_of::<MONITORINFOEXW>() as u32, ..Default::default() }, ..Default::default() };
    if !unsafe { GetMonitorInfoW(monitor, (&raw mut info).cast()) }.as_bool() {
        return None;
    }
    let device = String::from_utf16_lossy(&info.szDevice).trim_end_matches('\0').to_string();
    let (mut n_paths, mut n_modes) = (0u32, 0u32);
    unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, &mut n_modes) }.ok().ok()?;
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
    unsafe { QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, paths.as_mut_ptr(), &mut n_modes, modes.as_mut_ptr(), None) }.ok().ok()?;
    for path in &paths[..n_paths as usize] {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
        source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
        source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
        source.header.adapterId = path.sourceInfo.adapterId;
        source.header.id = path.sourceInfo.id;
        if unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } != 0 {
            continue;
        }
        if String::from_utf16_lossy(&source.viewGdiDeviceName).trim_end_matches('\0') != device {
            continue;
        }
        let mut colour = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
        colour.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
        colour.header.size = size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32;
        colour.header.adapterId = path.targetInfo.adapterId;
        colour.header.id = path.targetInfo.id;
        let hdr = unsafe { DisplayConfigGetDeviceInfo(&mut colour.header) } == 0 && unsafe { colour.Anonymous.value } & 0b10 != 0;
        let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
        white.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL;
        white.header.size = size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32;
        white.header.adapterId = path.targetInfo.adapterId;
        white.header.id = path.targetInfo.id;
        // In thousandths of 80 nits.
        let nits = if unsafe { DisplayConfigGetDeviceInfo(&mut white.header) } == 0 { white.SDRWhiteLevel as f32 / 1000.0 * 80.0 } else { 80.0 };
        return Some((hdr, if hdr { nits } else { 80.0 }));
    }
    None
}

/// Whether process `pid` runs as administrator while this one does not. Such a process's token
/// only admits administrators, so failing to open it from here counts as elevated.
pub fn elevated_above_this(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
    fn elevated(process: HANDLE) -> Option<bool> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
        let mut info = TOKEN_ELEVATION::default();
        let mut len = 0;
        let ok = unsafe { GetTokenInformation(token, TokenElevation, Some((&raw mut info).cast()), size_of::<TOKEN_ELEVATION>() as u32, &mut len) }.is_ok();
        let _ = unsafe { CloseHandle(token) };
        ok.then_some(info.TokenIsElevated != 0)
    }
    if elevated(unsafe { GetCurrentProcess() }).unwrap_or(false) {
        return false;
    }
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else { return false };
    let result = elevated(process).unwrap_or(true);
    let _ = unsafe { CloseHandle(process) };
    result
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
