//! Logs the game's keys (F fires, M and X open the maps, 1–6 pick shells) to `keys.jsonl` while
//! Roblox is in front, so the HUD's key-triggered reads can be replayed offline. Only
//! these keys and only in development recordings: the HUD itself logs no input.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mtc_dataset::KeyEvent;

use crate::Control;

/// F, M, X, 1–6 (virtual-key codes).
const KEYS: [u16; 9] = [0x46, 0x4D, 0x58, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36];

pub fn spawn(path: PathBuf, control: Arc<Control>) {
    std::thread::spawn(move || {
        let input = match mtc_input::Input::spawn(KEYS.to_vec()) {
            Ok(input) => input,
            Err(error) => {
                eprintln!("Key log unavailable: {error}");
                return;
            }
        };
        let mut out: Option<BufWriter<File>> = None;
        let mut roblox = None;
        while !control.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
            let events = input.drain();
            if events.is_empty() {
                continue;
            }
            if roblox.as_ref().is_none_or(|w| !mtc_capture::window::is_foreground(w)) {
                roblox = mtc_capture::window::find_roblox();
            }
            if !roblox.as_ref().is_some_and(mtc_capture::window::is_foreground) {
                continue;
            }
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            for event in events {
                let mtc_input::InputEvent::Key { vk, down, at } = event else { continue };
                let key = KeyEvent { vk, down, unix_ms: now.saturating_sub(at.elapsed().as_millis() as u64) };
                if out.is_none() {
                    out = File::create(&path).ok().map(BufWriter::new);
                }
                let Some(file) = out.as_mut() else { break };
                let _ = serde_json::to_writer(&mut *file, &key);
                let _ = writeln!(file);
            }
            if let Some(file) = out.as_mut() {
                let _ = file.flush();
            }
        }
    });
}
