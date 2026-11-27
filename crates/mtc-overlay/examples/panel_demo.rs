//! Shows the fire-control panel and the aim strip at the M109A6 readout position with sample
//! values, for `seconds` (default 20): `panel_demo [seconds]`. Clicks print their action.
//!   cargo run --release -p mtc-overlay --example panel_demo
use std::time::{Duration, Instant};

use mtc_overlay::{AimRow, AimStrip, Axis, Overlay, PanelArc, PanelLoad, PanelSolution, PanelState, PanelTarget, hud_right, panel_anchor};

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

fn main() {
    mtc_capture::window::enable_per_monitor_dpi_awareness();
    let seconds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20);
    let overlay = Overlay::spawn(roblox_font_dir());
    let (right, label_y) = (hud_right(934, 1280, 1.0, 1.0), 1261);
    let target = |label: &str, objective: bool, reachable: bool| PanelTarget { label: label.into(), objective, reachable };
    let mut state = PanelState {
        anchor: panel_anchor(right, label_y, 1.0),
        targets: vec![
            target("A1", false, true),
            target("A2", false, true),
            target("A3", false, false),
            target("A", true, true),
            target("B", true, true),
            target("C", true, true),
        ],
        selected: Some("A2".into()),
        solution: Some(PanelSolution {
            distance_m: 2069.4,
            height_m: 42.7,
            azimuth: 297.77,
            low: Some(PanelArc { elevation: 12.98, time_of_flight: 9.1, active: true, beyond_reach: false }),
            high: Some(PanelArc { elevation: 71.40, time_of_flight: 44.6, active: false, beyond_reach: false }),
        }),
        shell: Some("M107".into()),
        load: PanelLoad::Loaded,
        recommended: Some("Medium".into()),
        charge: Some("Low".into()),
        gun_set: true,
        notice: None,
        precision: Some((3.6, true)),
        compact: false,
        locked: true,
        settings_open: false,
        scale: 1.0,
        menu: None,
        shell_kind: None,
        charge_menu: None,
    };
    overlay.show_panel(Some(state.clone()));
    overlay.show_aim(Some(AimStrip {
        right,
        rows: [
            AimRow { axis: Axis::Elevation, current: 12.34, target: 12.98, tolerance: 0.05, label_y, value_y: 1285, beyond_reach: false },
            AimRow { axis: Axis::Azimuth, current: 297.80, target: 297.77, tolerance: 0.05, label_y: 1317, value_y: 1341, beyond_reach: false },
        ],
        mirror: false,
        scale: 1.0,
    }));
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        for action in overlay.take_actions() {
            println!("{action:?}");
            if let mtc_overlay::PanelAction::Select(label) = action {
                state.selected = Some(label);
                overlay.show_panel(Some(state.clone()));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
