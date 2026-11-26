//! Saves the primary monitor as composited on screen (the game with the overlay windows on top),
//! for checking the HUD against the game: `screenshot <out.png>`.
//!   cargo run --release -p mtc-capture --example screenshot -- shot.png
use windows_capture::dxgi_duplication_api::DxgiDuplicationApi;
use windows_capture::encoder::ImageFormat;
use windows_capture::monitor::Monitor;

fn main() {
    mtc_capture::window::enable_per_monitor_dpi_awareness();
    let out = std::env::args().nth(1).expect("output path");
    let mut duplication = DxgiDuplicationApi::new(Monitor::primary().expect("primary monitor")).expect("desktop duplication");
    // The first frame can come before the desktop image is ready; take the last of a few.
    let mut saved = false;
    for _ in 0..5 {
        if let Ok(mut frame) = duplication.acquire_next_frame(200) {
            let mut buffer = frame.buffer().expect("frame buffer");
            buffer.save_as_image(&out, ImageFormat::Png).expect("save");
            saved = true;
        }
    }
    assert!(saved, "no frame from desktop duplication");
    println!("{out}");
}
