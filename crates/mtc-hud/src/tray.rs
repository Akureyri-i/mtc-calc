//! The notification-area icon. Outside the game the calculator has no window of its own, so the
//! icon shows it is running and its menu makes a problem report, opens the log folder and the
//! licenses, or quits.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, HICON, IDI_APPLICATION, LoadIconW, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, MSG, PostMessageW, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CONTEXTMENU, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

/// The message the icon's mouse events come back as.
const CALLBACK: u32 = WM_APP + 1;
const ICON_ID: u32 = 1;
const ID_LOGS: usize = 1;
const ID_LICENSES: usize = 2;
const ID_QUIT: usize = 3;
const ID_REPORT: usize = 4;

const LICENSES: &str = concat!(include_str!("../../../LICENSE"), "\n\n", include_str!("../../../THIRD_PARTY_NOTICES.md"));

static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();
/// Explorer's "TaskbarCreated" message: the icon is added again after Explorer restarts.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// The icon, removed with `remove` when the calculator ends.
pub struct Tray {
    hwnd: Arc<AtomicIsize>,
}

/// Adds the icon from a thread of its own; Quit sets `stop`.
pub fn spawn(stop: Arc<AtomicBool>) -> Tray {
    STOP.set(stop).ok();
    let hwnd = Arc::new(AtomicIsize::new(0));
    let shared = hwnd.clone();
    std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || {
            let Some(window) = create_window() else { return };
            shared.store(window.0 as isize, Ordering::Relaxed);
            add_icon(window);
            let mut msg = MSG::default();
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        })
        .expect("tray thread");
    Tray { hwnd }
}

impl Tray {
    /// Takes the icon out of the notification area.
    pub fn remove(&self) {
        let hwnd = self.hwnd.load(Ordering::Relaxed);
        if hwnd != 0 {
            let data = NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: HWND(hwnd as *mut _), uID: ICON_ID, ..Default::default() };
            let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
        }
    }
}

fn create_window() -> Option<HWND> {
    let instance = unsafe { GetModuleHandleW(None) }.ok()?;
    let class = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance.into(), lpszClassName: w!("MTCCalculatorTray"), ..Default::default() };
    unsafe { RegisterClassW(&class) };
    TASKBAR_CREATED.store(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }, Ordering::Relaxed);
    // Never shown: it only receives the icon's messages.
    unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("MTCCalculatorTray"), w!("MTC Calculator"), WINDOW_STYLE(0), 0, 0, 0, 0, None, None, Some(instance.into()), None) }.ok()
}

fn add_icon(hwnd: HWND) {
    let instance = unsafe { GetModuleHandleW(None) }.ok();
    // The program's own icon (resource 1), else the generic one.
    let icon = instance
        .and_then(|i| unsafe { LoadIconW(Some(i.into()), PCWSTR(1 as *const u16)) }.ok())
        .or_else(|| unsafe { LoadIconW(None, IDI_APPLICATION) }.ok())
        .unwrap_or(HICON::default());
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: CALLBACK,
        hIcon: icon,
        ..Default::default()
    };
    for (slot, c) in data.szTip.iter_mut().zip("MTC Calculator".encode_utf16()) {
        *slot = c;
    }
    if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        log::warn!("Notification area icon could not be added");
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == CALLBACK {
        let event = (lparam.0 & 0xFFFF) as u32;
        if event == WM_RBUTTONUP || event == WM_LBUTTONUP || event == WM_CONTEXTMENU {
            menu(hwnd);
        }
        return LRESULT(0);
    }
    if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) {
        add_icon(hwnd);
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn menu(hwnd: HWND) {
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let title = wide(&format!("MTC Calculator {}", env!("CARGO_PKG_VERSION")));
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else { return };
    unsafe {
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(title.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        use mtc_overlay::lang::t;
        for (id, text) in [(ID_REPORT, t("tray.report")), (ID_LOGS, t("tray.logs")), (ID_LICENSES, t("tray.licenses"))] {
            let text = wide(text);
            let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(text.as_ptr()));
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let quit = wide(mtc_overlay::lang::t("tray.quit"));
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, PCWSTR(quit.as_ptr()));
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        // The menu closes on a click elsewhere only when its window is in front.
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, at.x, at.y, None, hwnd, None).0 as usize;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match chosen {
            ID_LOGS => {
                if let Some(dir) = crate::logfile::dir() {
                    let _ = std::process::Command::new("explorer").arg(dir).spawn();
                }
            }
            ID_REPORT => {
                log::info!("Problem report asked for from the notification area icon");
                crate::ask_report();
            }
            ID_LICENSES => open_licenses(),
            ID_QUIT => {
                log::info!("Quit from the notification area icon");
                if let Some(stop) = STOP.get() {
                    stop.store(true, Ordering::SeqCst);
                }
            }
            _ => {}
        }
    }
}

/// The license and the third-party notices, written out and opened in Notepad.
fn open_licenses() {
    let Some(dir) = crate::diag::dir().and_then(|d| d.parent().map(std::path::Path::to_path_buf)) else { return };
    let path = dir.join("licenses.txt");
    if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&path, LICENSES.replace('\n', "\r\n")).is_ok() {
        let _ = std::process::Command::new("notepad").arg(path).spawn();
    }
}
