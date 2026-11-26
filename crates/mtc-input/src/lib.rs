//! Keyboard and mouse events through Raw Input: a thread with a hidden
//! message-only window receives the system's copies of input events (`RIDEV_INPUTSINK`, so they
//! arrive while another window — the game — has focus). Nothing is hooked or intercepted: the
//! game receives its input as usual, and nothing is sent to it.
//!
//! Only the keys in the list given at start and the mouse buttons and wheel are passed on; the
//! consumer handles events only while Roblox is in the foreground and keeps none of them. Screen
//! recognition stays the authority on game state: events only say when to look.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
    RegisterRawInputDevices,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetCursorPos, GetMessageW, HWND_MESSAGE, MSG, PostThreadMessageW, RegisterClassExW, WINDOW_EX_STYLE,
    WM_INPUT, WM_QUIT, WNDCLASSEXW, WS_OVERLAPPED,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::core::w;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputEvent {
    /// A listed key (virtual-key code) went down or up.
    Key { vk: u16, down: bool, at: Instant },
    /// A mouse button went down or up with the cursor at `pos` (screen px).
    Button { button: MouseButton, down: bool, pos: (i32, i32), at: Instant },
    /// Wheel notches, positive away from the user, with the cursor position then (screen px).
    Wheel { notches: f32, pos: (i32, i32), at: Instant },
}

/// Raw Input button flags (winuser.h).
const BUTTONS: [(u16, MouseButton, bool); 6] = [
    (0x0001, MouseButton::Left, true),
    (0x0002, MouseButton::Left, false),
    (0x0004, MouseButton::Right, true),
    (0x0008, MouseButton::Right, false),
    (0x0010, MouseButton::Middle, true),
    (0x0020, MouseButton::Middle, false),
];
const WHEEL: u16 = 0x0400;
/// `RI_KEY_BREAK`: the key went up.
const KEY_BREAK: u16 = 0x0001;

/// Handle to the input thread.
pub struct Input {
    rx: Receiver<InputEvent>,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

thread_local! {
    static SINK: std::cell::RefCell<Option<(Sender<InputEvent>, Vec<u16>)>> = const { std::cell::RefCell::new(None) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_INPUT {
        let mut raw = RAWINPUT::default();
        let mut size = size_of::<RAWINPUT>() as u32;
        let read = unsafe { GetRawInputData(HRAWINPUT(lparam.0 as *mut _), RID_INPUT, Some((&raw mut raw).cast()), &mut size, size_of::<RAWINPUTHEADER>() as u32) };
        if read != u32::MAX && read > 0 {
            SINK.with(|sink| {
                if let Some((tx, keys)) = sink.borrow().as_ref() {
                    for event in decode(&raw, keys) {
                        let _ = tx.send(event);
                    }
                }
            });
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn decode(raw: &RAWINPUT, keys: &[u16]) -> Vec<InputEvent> {
    let at = Instant::now();
    let mut events = Vec::new();
    if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
        let k = unsafe { raw.data.keyboard };
        if keys.contains(&k.VKey) {
            events.push(InputEvent::Key { vk: k.VKey, down: k.Flags & KEY_BREAK == 0, at });
        }
    } else if raw.header.dwType == RIM_TYPEMOUSE.0 {
        let flags = unsafe { raw.data.mouse.Anonymous.Anonymous };
        let mut pos = POINT::default();
        if flags.usButtonFlags & (0x003F | WHEEL) != 0 {
            unsafe {
                let _ = GetCursorPos(&mut pos);
            }
        }
        for (bit, button, down) in BUTTONS {
            if flags.usButtonFlags & bit != 0 {
                events.push(InputEvent::Button { button, down, pos: (pos.x, pos.y), at });
            }
        }
        if flags.usButtonFlags & WHEEL != 0 {
            events.push(InputEvent::Wheel { notches: f32::from(flags.usButtonData as i16) / 120.0, pos: (pos.x, pos.y), at });
        }
    }
    events
}

impl Input {
    /// Starts the input thread, passing on `keys` (virtual-key codes), mouse buttons and wheel.
    pub fn spawn(keys: Vec<u16>) -> windows::core::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = std::thread::Builder::new().name("input".into()).spawn(move || {
            let result = (|| unsafe {
                let instance = GetModuleHandleW(None)?.into();
                let class = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: w!("MtcInput"), ..Default::default() };
                RegisterClassExW(&class);
                let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("MtcInput"), w!("MTC input"), WS_OVERLAPPED, 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(instance), None)?;
                // Generic desktop: mouse (usage 2) and keyboard (usage 6).
                let devices = [2u16, 6].map(|usage| RAWINPUTDEVICE { usUsagePage: 1, usUsage: usage, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd });
                RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32)?;
                Ok::<_, windows::core::Error>(GetCurrentThreadId())
            })();
            let started = result.is_ok();
            let _ = ready_tx.send(result);
            if !started {
                return;
            }
            SINK.with(|sink| *sink.borrow_mut() = Some((tx, keys)));
            let mut msg = MSG::default();
            unsafe {
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
        }).expect("input thread");
        let thread_id = ready_rx.recv().expect("input thread start")?;
        Ok(Self { rx, thread_id, thread: Some(thread) })
    }

    /// Events received since the last call.
    pub fn drain(&self) -> Vec<InputEvent> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
