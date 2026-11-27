//! Development: every overlay window shown far off screen, then the graphics devices and surfaces
//! made again as after a device loss, and the windows shown again; the overlay reports each step
//! (or its errors) on stderr.
//!   cargo run --release -p mtc-overlay --example rebuild_check
use std::time::Duration;

use mtc_overlay::{AimRow, AimStrip, Axis, ChargeDot, ChargeMarks, DragMark, MapMark, MapMarks, Overlay, PanelLoad, PanelState, PanelTarget};

fn roblox_font_dir() -> std::path::PathBuf {
    let versions = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Roblox/Versions");
    std::fs::read_dir(versions)
        .expect("Roblox versions")
        .filter_map(Result::ok)
        .map(|e| e.path().join("content/fonts"))
        .find(|p| p.join("Montserrat-Medium.ttf").is_file())
        .expect("Montserrat in a Roblox install")
}

/// Far from any monitor.
const OFF: i32 = -20_000;

fn show(overlay: &Overlay, k: i32) {
    let mark = MapMark { label: "A1".into(), x: (OFF + 300 + k) as f32, y: (OFF + 300) as f32, selected: true, objective: false, precision_m: Some(1.8), covered: false };
    overlay.show_panel(Some(PanelState {
        anchor: (OFF + 400, OFF + 400),
        targets: vec![PanelTarget { label: "A".into(), objective: true, reachable: k % 2 == 0 }],
        selected: None,
        solution: None,
        shell: None,
        load: PanelLoad::Empty,
        recommended: None,
        charge: None,
        gun_set: false,
        notice: None,
        precision: None,
        compact: false,
        locked: true,
        settings_open: false,
        scale: 1.0,
        menu: None,
        shell_kind: None,
        charge_menu: None,
    }));
    overlay.show_aim(Some(AimStrip {
        right: OFF + 400,
        rows: [
            AimRow { axis: Axis::Elevation, current: 10.0, target: 10.5 + f64::from(k), tolerance: 0.05, label_y: OFF + 500, value_y: OFF + 523, beyond_reach: false },
            AimRow { axis: Axis::Azimuth, current: 200.0, target: 201.0, tolerance: 0.05, label_y: OFF + 556, value_y: OFF + 579, beyond_reach: false },
        ],
        mirror: false,
        scale: 1.0,
    }));
    overlay.show_map(Some(MapMarks { left: OFF, top: OFF, size: 882, small: false, marks: vec![mark.clone()], coarse_m: 5.0, blast: None, planning: None, follow: None, pred: None, scale: 1.0 }));
    overlay.show_charges(Some(ChargeMarks { left: OFF, top: OFF, width: 100, height: 40, dots: vec![ChargeDot { x: (OFF + 20) as f32, y: (OFF + 20) as f32, reaches: k % 2 == 0 }], scale: 1.0 }));
    overlay.show_drag(Some(DragMark { mark, coarse_m: 5.0, grab: (OFF, OFF), released_at: Some((0, 0)), snap: None, press: (0.0, 0.0), pressed_at: std::time::Instant::now(), scale: 1.0 }));
}

fn main() {
    let overlay = Overlay::spawn(roblox_font_dir());
    show(&overlay, 0);
    std::thread::sleep(Duration::from_millis(500));
    eprintln!("check: asking for a rebuild");
    overlay.rebuild_graphics();
    std::thread::sleep(Duration::from_millis(500));
    show(&overlay, 1);
    std::thread::sleep(Duration::from_millis(300));
    eprintln!("check: second rebuild");
    overlay.rebuild_graphics();
    std::thread::sleep(Duration::from_millis(500));
    show(&overlay, 2);
    std::thread::sleep(Duration::from_millis(300));
    overlay.show_drag(None);
    eprintln!("check: done");
}
