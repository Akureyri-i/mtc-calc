//! Time to redraw the map marks window (V1 with six markers, one selected with its blast), as a
//! pan or zoom redraws it every frame: `map_draw_bench [n]`.
use mtc_overlay::{Blast, Hover, MapMark, MapMarks, Planning, RangeRing, RingKind, TargetLabel};

fn roblox_font_dir() -> std::path::PathBuf {
    let versions = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Roblox/Versions");
    std::fs::read_dir(versions)
        .expect("Roblox versions")
        .filter_map(Result::ok)
        .map(|e| e.path().join("content/fonts"))
        .find(|p| p.join("Montserrat-Medium.ttf").is_file())
        .expect("fonts")
}

fn main() {
    let n: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(300);
    let mark = |i: usize, selected: bool| MapMark {
        label: format!("A{}", i + 1),
        x: 1000.0 + 110.0 * i as f32,
        y: 500.0 + 70.0 * i as f32,
        selected,
        objective: false,
        precision_m: Some(0.4),
        covered: i == 3,
    };
    let marks = MapMarks {
        left: 839,
        top: 260,
        size: 882,
        small: false,
        marks: (0..6).map(|i| mark(i, i == 2)).collect(),
        coarse_m: 2.0,
        blast: Some(Blast { reach_px: 60.0, kill_px: 25.0, reach_m: 49.7, spread_px: None }),
        planning: None,
        follow: None,
        pred: None,
        scale: 1.0,
    };
    let fonts = roblox_font_dir();
    let t = mtc_overlay::benchmark_map_draw(&fonts, &marks, n).expect("draw");
    println!("six marks, selected with blast: {:.2} ms per drawing", t.as_secs_f64() * 1000.0);
    // V1's planning layer as live at 2.6x: rings far larger than the window, a label per target
    // (six markers, six objectives), one hovered.
    let mut planned = marks.clone();
    planned.marks.extend((0..6).map(|i| MapMark { label: ((b'A' + i as u8) as char).to_string(), x: 950.0 + 120.0 * i as f32, y: 900.0 - 50.0 * i as f32, selected: false, objective: true, precision_m: None, covered: false }));
    let rings = |scale: f32| -> Vec<RangeRing> {
        [(2800.0, 0, RingKind::Other), (7100.0, 1, RingKind::Focus), (8700.0, 2, RingKind::Loaded)]
            .into_iter()
            .flat_map(|(m, c, kind)| [RangeRing { radius_px: m * scale, charge: Some(c), kind, min: false }, RangeRing { radius_px: 300.0 * scale, charge: Some(c), kind, min: true }])
            .collect()
    };
    let labels: Vec<TargetLabel> = planned.marks.iter().map(|m| TargetLabel { label: m.label.clone(), flight_s: Some(14.2), charge: Some(1), elevation: Some(24.6), distance_m: 3467.0 }).collect();
    for (name, scale, hover) in [("1.0x", 0.163, false), ("2.6x", 0.424, false), ("2.6x hovered", 0.424, true)] {
        planned.planning = Some(Planning {
            gun: (1239.0, 300.0),
            picture: (1010.0, 260.0, 1550.0, 1142.0),
            rings: rings(scale),
            labels: labels.clone(),
            fields: mtc_overlay::LabelFields { time: true, precision: true, ..Default::default() },
            hover: hover.then(|| Hover { label: "A2".into(), charge: Some(("Medium".into(), 1)), distance_m: 3467.0, low: Some((24.6, 14.2)), high: Some((66.9, 33.8)), high_chosen: false }),
            stopped: None,
        });
        let t = mtc_overlay::benchmark_map_draw(&fonts, &planned, n).expect("draw");
        println!("V1 planning layer at {name}: {:.2} ms per drawing", t.as_secs_f64() * 1000.0);
        let no_rings = MapMarks { planning: planned.planning.clone().map(|p| Planning { rings: Vec::new(), ..p }), ..planned.clone() };
        let t = mtc_overlay::benchmark_map_draw(&fonts, &no_rings, n).expect("draw");
        println!("  without rings: {:.2} ms", t.as_secs_f64() * 1000.0);
    }
    let none = MapMarks { marks: Vec::new(), blast: None, ..marks.clone() };
    let t = mtc_overlay::benchmark_map_draw(&fonts, &none, n).expect("draw");
    println!("empty window (clear only): {:.2} ms per drawing", t.as_secs_f64() * 1000.0);
}
