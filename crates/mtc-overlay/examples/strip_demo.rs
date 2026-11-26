//! Shows the aim strip at the M109A6 readout position with sample values.
//!   cargo run --release -p mtc-overlay --example strip_demo

use std::time::Duration;

use mtc_overlay::{AimRow, AimStrip, Axis, Overlay, hud_right};

fn roblox_font_dir() -> std::path::PathBuf {
    let versions = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Roblox/Versions");
    std::fs::read_dir(versions)
        .expect("Roblox versions")
        .filter_map(Result::ok)
        .map(|e| e.path().join("content/fonts"))
        .filter(|p| p.join("Montserrat-Medium.ttf").is_file())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .expect("Montserrat in a Roblox install")
}

fn strip(elevation: f64, azimuth: f64) -> AimStrip {
    AimStrip {
        right: hud_right(934, 1280, 1.0, 1.0),
        rows: [
            AimRow { axis: Axis::Elevation, current: elevation, target: 12.98, tolerance: 0.05, label_y: 1261, value_y: 1285, beyond_reach: false },
            AimRow { axis: Axis::Azimuth, current: azimuth, target: 56.78, tolerance: 0.05, label_y: 1317, value_y: 1341, beyond_reach: false },
        ],
        mirror: false,
        scale: 1.0,
    }
}

fn main() {
    mtc_capture::window::enable_per_monitor_dpi_awareness();
    let overlay = Overlay::spawn(roblox_font_dir());
    overlay.show_aim(Some(strip(12.34, 57.40)));
    std::thread::sleep(Duration::from_secs(5));
    overlay.show_aim(Some(strip(12.97, 56.12)));
    std::thread::sleep(Duration::from_secs(4));
}
