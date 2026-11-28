//! Every overlay window in its typical states drawn offscreen, each layer's premultiplied BGRA
//! written to `<out dir>/<state>.bgra` (with its size in the name): rendered before and after a
//! change to the drawing code and compared byte for byte.
//!   cargo run --release -p mtc-overlay --example render_states -- <out dir>
use std::path::Path;

use mtc_overlay::{
    AimRow, AimStrip, Axis, Blast, ChargeDot, ChargeMarks, MapMark, MapMarks, PanelArc, PanelLoad, PanelSolution, PanelState, PanelTarget, PlanArc, PlanChargeBox, PlanDetail, PlanNext, PlanRow, PlanState,
    render_charge_marks, render_offscreen, render_plan, render_text_sample,
};

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

fn target(label: &str, objective: bool, reachable: bool) -> PanelTarget {
    PanelTarget { label: label.into(), objective, reachable }
}

fn panels() -> Vec<(&'static str, PanelState)> {
    let full = PanelState {
        anchor: (1700, 1220),
        targets: vec![target("A1", false, true), target("A2", false, true), target("A3", false, false), target("A", true, true), target("B", true, true), target("C", true, true)],
        selected: Some("A2".into()),
        solution: Some(PanelSolution {
            distance_m: 2069.4,
            height_m: 42.7,
            azimuth: 297.77,
            low: Some(PanelArc { elevation: 12.98, time_of_flight: 9.1, active: true, beyond_reach: false }),
            high: Some(PanelArc { elevation: 71.40, time_of_flight: 44.6, active: false, beyond_reach: true }),
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
    let empty = PanelState {
        targets: vec![target("A", true, true), target("B", true, true), target("C", true, true)],
        selected: None,
        solution: None,
        shell: None,
        load: PanelLoad::Empty,
        recommended: None,
        charge: None,
        precision: None,
        ..full.clone()
    };
    vec![
        ("panel_full", full.clone()),
        ("panel_unlocked", PanelState { locked: false, ..full.clone() }),
        ("panel_compact", PanelState { compact: true, ..full.clone() }),
        ("panel_empty", empty.clone()),
        ("panel_empty_compact", PanelState { compact: true, ..empty.clone() }),
        ("panel_notice", PanelState { notice: Some("Map not identified".into()), ..empty }),
        ("panel_high_active", PanelState {
            solution: Some(PanelSolution {
                distance_m: 812.0,
                height_m: -35.6,
                azimuth: 264.78,
                low: Some(PanelArc { elevation: 3.5, time_of_flight: 3.0, active: false, beyond_reach: true }),
                high: Some(PanelArc { elevation: 84.2, time_of_flight: 61.3, active: true, beyond_reach: false }),
            }),
            recommended: None,
            precision: Some((12.4, false)),
            ..full
        }),
    ]
}

fn strips() -> Vec<(&'static str, AimStrip)> {
    let rows = |de: f64, da: f64, beyond: bool| {
        [
            AimRow { axis: Axis::Elevation, current: 12.34, target: 12.34 + de, tolerance: 0.05, label_y: 1262, value_y: 1285, beyond_reach: beyond },
            AimRow { axis: Axis::Azimuth, current: 213.03, target: 213.03 + da, tolerance: 0.05, label_y: 1318, value_y: 1341, beyond_reach: false },
        ]
    };
    vec![
        ("strip_off", AimStrip { right: 1180, rows: rows(0.64, -1.27, false), mirror: false, scale: 1.0 }),
        ("strip_on", AimStrip { right: 1180, rows: rows(0.02, -0.03, false), mirror: false, scale: 1.0 }),
        ("strip_beyond", AimStrip { right: 1180, rows: rows(6.07, 0.40, true), mirror: false, scale: 1.0 }),
        ("strip_mirror", AimStrip { right: 1180, rows: rows(0.64, -1.27, false), mirror: true, scale: 1.0 }),
    ]
}

fn maps() -> Vec<(&'static str, MapMarks)> {
    let mark = |label: &str, x: f32, y: f32, selected: bool, objective: bool, precision: Option<f32>, covered: bool| MapMark { label: label.into(), x, y, selected, objective, precision_m: precision, covered };
    let big = MapMarks {
        left: 839,
        top: 260,
        size: 882,
        small: false,
        marks: vec![
            mark("A1", 1100.4, 520.7, true, false, Some(1.8), false),
            mark("A2", 1301.0, 640.2, false, false, Some(9.6), false),
            mark("A3", 1500.6, 400.3, false, false, Some(2.2), true),
            mark("A", 1200.0, 700.0, false, true, None, false),
            mark("B", 1000.0, 900.0, false, true, None, false),
        ],
        coarse_m: 5.0,
        blast: Some(Blast { reach_px: 33.1, kill_px: 13.8, reach_m: 49.7, spread_px: None }),
        planning: None,
        follow: None,
        pred: None,
        scale: 1.0,
    };
    vec![
        ("map_v1", big.clone()),
        ("map_v1_cluster", MapMarks { blast: Some(Blast { reach_px: 12.0, kill_px: 5.0, reach_m: 18.0, spread_px: Some(70.5) }), ..big.clone() }),
        ("map_v1_small_blast", MapMarks { blast: Some(Blast { reach_px: 15.0, kill_px: 6.0, reach_m: 9.8, spread_px: None }), ..big.clone() }),
        ("map_v1_objective_selected", MapMarks {
            marks: big.marks.iter().map(|m| MapMark { selected: m.label == "B", ..m.clone() }).collect(),
            ..big.clone()
        }),
        ("map_v3", MapMarks {
            left: 2183,
            top: 110,
            size: 354,
            small: true,
            marks: vec![mark("A1", 2300.0, 200.0, true, false, Some(6.4), false), mark("A2", 2400.5, 300.5, false, false, Some(6.4), false)],
            blast: Some(Blast { reach_px: 7.8, kill_px: 3.2, reach_m: 49.7, spread_px: None }),
            ..big
        }),
    ]
}

fn charges() -> ChargeMarks {
    let mut dots = Vec::new();
    for card in 0..4 {
        for k in 0..3 {
            dots.push(ChargeDot { x: 1300.0 + 180.0 * card as f32, y: 1180.0 + 28.0 * k as f32, reaches: (card + k) % 2 == 0 });
        }
    }
    ChargeMarks { left: 1290, top: 1170, width: 560, height: 80, dots, scale: 1.0 }
}

fn write(dir: &Path, name: &str, width: u32, height: u32, bgra: &[u8]) {
    std::fs::write(dir.join(format!("{name}_{width}x{height}.bgra")), bgra).expect("write");
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("out dir"));
    std::fs::create_dir_all(&dir).expect("out dir");
    let fonts = roblox_font_dir();
    let mut n = 0;
    for (name, panel) in panels() {
        for layer in render_offscreen(&fonts, None, Some(&panel), None).expect("panel") {
            write(&dir, name, layer.width, layer.height, &layer.bgra);
            n += 1;
        }
    }
    for (name, strip) in strips() {
        for layer in render_offscreen(&fonts, Some(&strip), None, None).expect("strip") {
            write(&dir, name, layer.width, layer.height, &layer.bgra);
            n += 1;
        }
    }
    for (name, marks) in maps() {
        for layer in render_offscreen(&fonts, None, None, Some(&marks)).expect("map") {
            write(&dir, name, layer.width, layer.height, &layer.bgra);
            n += 1;
        }
    }
    let layer = render_charge_marks(&fonts, &charges()).expect("charges");
    write(&dir, "charges", layer.width, layer.height, &layer.bgra);
    for (name, text, size, medium) in [("text_label", "Elevation", 13.0, false), ("text_value", "+12.98°", 18.0, true)] {
        write(&dir, name, 160, 40, &render_text_sample(&fonts, text, size, medium, 160, 40).expect("text"));
    }
    let plan_row = |label: &str, objective: bool, charge: Option<(&str, u8)>, t: Option<f64>, reachable: bool| PlanRow {
        label: label.into(),
        objective,
        distance_m: Some(1500.0),
        charge: charge.map(|(n, i)| (n.into(), i)),
        time_s: t,
        high_arc: false,
        reachable,
        precision: (!objective).then_some((2.4, true)),
    };
    let plan = PlanState {
        at: (1785, 260),
        rows: vec![
            plan_row("A1", false, Some(("Low", 0)), Some(9.1), true),
            plan_row("A2", false, Some(("High", 2)), Some(30.2), true),
            plan_row("A3", false, None, None, false),
            plan_row("B", true, Some(("Low", 0)), Some(4.6), true),
        ],
        selected: Some("A2".into()),
        hover: Some("A1".into()),
        shell: Some("M107".into()),
        loaded_charge: Some(("Low".into(), 0)),
        shell_kind: None,
        charge_manual: None,
        menu: None,
        load: PanelLoad::Loaded,
        next: PlanNext::Load("High".into(), 2),
        detail: Some(PlanDetail {
            label: "A2".into(),
            height_m: 42.7,
            azimuth: 297.81,
            precision: Some((2.4, true)),
            blast_m: Some((49.7, 20.7)),
            low: None,
            high: Some(PlanArc { elevation: 71.4, time_s: 30.2, beyond_reach: true, blocked_m: None, hint_m: None }),
            high_chosen: true,
            auto: true,
            charges: vec![
                PlanChargeBox { name: "Low".into(), index: 0, band_km: Some((0.1, 2.4)), time_s: None, loaded: true, planned: false },
                PlanChargeBox { name: "High".into(), index: 2, band_km: Some((0.4, 9.8)), time_s: Some(30.2), loaded: false, planned: true },
            ],
            terrain: None,
        }),
        gun_set: true,
        gun_source: Some(format!("{} · ±6.1 m", mtc_overlay::lang::t("gun.minimap"))),
        notice: None,
        terrain_open: false,
        settings_open: false,
        targets_open: true,
        squad: None,
        scale: 1.0,
    };
    for (name, state) in [("plan", plan.clone()), ("plan_notice", PlanState { rows: Vec::new(), shell: None, loaded_charge: None, detail: None, next: PlanNext::Nothing, notice: Some("Load a shell for charges and flight times".into()), gun_set: false, gun_source: None, ..plan })] {
        let layer = render_plan(&fonts, &state).expect("plan");
        write(&dir, name, layer.width, layer.height, &layer.bgra);
        n += 1;
    }
    // All three windows in one call (the HUD composite path).
    let all = render_offscreen(&fonts, Some(&strips()[0].1), Some(&panels()[0].1), Some(&maps()[0].1)).expect("all");
    for (k, layer) in all.iter().enumerate() {
        write(&dir, &format!("together_{k}"), layer.width, layer.height, &layer.bgra);
    }
    println!("{} layers written to {}", n + 3 + all.len(), dir.display());
}
