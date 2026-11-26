//! Global hotkeys for the recorder (a development tool; the HUD registers none).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_NOREPEAT, RegisterHotKey, VIRTUAL_KEY, VK_END, VK_F1, VK_HOME, VK_INSERT, VK_NEXT,
    VK_PAUSE, VK_PRIOR,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MB_ICONASTERISK, MB_OK, MESSAGEBOX_STYLE, MSG, WM_HOTKEY,
};

use crate::{Args, Control};

const MARK: i32 = 1;
const SNAPSHOT: i32 = 2;
const CONTINUOUS: i32 = 3;

fn virtual_key(name: &str) -> VIRTUAL_KEY {
    let upper = name.to_ascii_uppercase();
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u16>().ok())
        && (1..=24).contains(&n)
    {
        return VIRTUAL_KEY(VK_F1.0 + n - 1);
    }
    match upper.as_str() {
        "HOME" => VK_HOME,
        "END" => VK_END,
        "INSERT" => VK_INSERT,
        "PAGEUP" => VK_PRIOR,
        "PAGEDOWN" => VK_NEXT,
        "PAUSE" => VK_PAUSE,
        _ => panic!("unsupported hotkey {name}"),
    }
}

fn beep(style: MESSAGEBOX_STYLE) {
    let _ = unsafe { MessageBeep(style) };
}

pub fn spawn(args: &Args, control: Arc<Control>) {
    let keys = [
        (MARK, virtual_key(&args.mark_key), args.mark_key.clone()),
        (SNAPSHOT, virtual_key(&args.snapshot_key), args.snapshot_key.clone()),
        (CONTINUOUS, virtual_key(&args.continuous_key), args.continuous_key.clone()),
    ];
    std::thread::spawn(move || {
        for (id, key, name) in &keys {
            if let Err(error) = unsafe { RegisterHotKey(None, *id, MOD_NOREPEAT, u32::from(key.0)) } {
                eprintln!("Hotkey {name} could not be registered (another program may hold it): {error}");
            }
        }
        let mut message = MSG::default();
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
            if message.message != WM_HOTKEY {
                continue;
            }
            match message.wParam.0 as i32 {
                MARK => {
                    control.mark.store(true, Ordering::SeqCst);
                    beep(MB_OK);
                }
                SNAPSHOT => {
                    control.snapshot.store(true, Ordering::SeqCst);
                    beep(MB_OK);
                }
                CONTINUOUS => {
                    let on = !control.continuous.fetch_xor(true, Ordering::SeqCst);
                    println!("Continuous recording: {}", if on { "on" } else { "off" });
                    beep(MB_ICONASTERISK);
                }
                _ => {}
            }
        }
    });
}
