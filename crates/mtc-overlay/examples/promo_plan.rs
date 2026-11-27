//! Development: the V1 plan panel for one logged match (Rohkstov, 3OF61 Low loaded), with its
//! terrain profile open, rendered on a dark ground at `scale`. Target A7 (the arc clears the
//! ground), or objective B with `b` (the low arc hits terrain `stop_m` out, the high arc is used):
//!   promo_plan <ground.json> <arc.json> <out.png> [scale] [b <low.json> <stop_m>]
//! The JSON files hold [distance m, height m] pairs from the gun (`arc.json`: B's high arc).
use std::path::Path;

use mtc_dataset::image::RgbImage;
use mtc_overlay::{PanelLoad, PlanArc, PlanChargeBox, PlanDetail, PlanNext, PlanRow, PlanState, PlanTerrain, ProfileArc, TerrainNote, render_plan};

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

/// `[[x, y], ...]` as pairs.
fn pairs(path: &str) -> Vec<(f32, f32)> {
    let text = std::fs::read_to_string(path).expect("json");
    let numbers: Vec<f32> = text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e')).filter(|s| !s.is_empty()).map(|s| s.parse().expect("number")).collect();
    numbers.chunks(2).map(|p| (p[0], p[1])).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scale: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let objective_b = args.get(4).is_some_and(|a| a == "b");
    let ground = pairs(&args[0]);
    let ground_at = |x: f32| {
        ground.windows(2).find(|w| w[0].0 <= x && x <= w[1].0).map_or(ground[ground.len() - 1].1, |w| w[0].1 + (w[1].1 - w[0].1) * (x - w[0].0) / (w[1].0 - w[0].0))
    };
    let row = |label: &str, objective: bool, d: f64, t: f64, high_arc: bool| PlanRow {
        label: label.into(),
        objective,
        distance_m: Some(d),
        charge: Some(("Low".into(), 0)),
        time_s: Some(t),
        high_arc,
        reachable: true,
        precision: if objective { None } else { Some((2.3, true)) },
    };
    let boxed = |name: &str, index: u8, band: (f32, f32), t: Option<f64>, loaded: bool, planned: bool| PlanChargeBox { name: name.into(), index, band_km: Some(band), time_s: t, loaded, planned };
    let state = PlanState {
        at: (0, 0),
        rows: vec![
            row("A7", false, 1279.3, 6.5, false),
            row("A", true, 1330.7, 6.7, false),
            row("B", true, 1292.8, 38.8, true),
            row("C", true, 2064.3, 11.0, false),
            row("D", true, 2404.7, 13.3, false),
            row("E", true, 3490.2, 24.7, false),
            row("F", true, 3493.0, 24.8, false),
        ],
        selected: Some(if objective_b { "B" } else { "A7" }.into()),
        hover: None,
        shell: Some("3OF61".into()),
        loaded_charge: Some(("Low".into(), 0)),
        shell_kind: None,
        menu: None,
        charge_manual: None,
        load: PanelLoad::Loaded,
        next: PlanNext::Ready,
        detail: Some(if objective_b {
            let stop: f32 = args[6].parse().expect("stop_m");
            PlanDetail {
                label: "B".into(),
                height_m: -31.7,
                azimuth: 136.41,
                precision: None,
                blast_m: Some((55.4, 25.6)),
                low: Some(PlanArc { elevation: 7.52, time_s: 6.5, beyond_reach: false, blocked_m: Some(stop), hint_m: None }),
                high: Some(PlanArc { elevation: 79.36, time_s: 38.8, beyond_reach: true, blocked_m: None, hint_m: None }),
                high_chosen: true,
                auto: true,
                charges: vec![boxed("Low", 0, (0.1, 3.5), Some(38.8), true, true), boxed("Medium", 1, (0.1, 9.3), Some(3.2), false, false), boxed("High", 2, (0.1, 16.7), Some(1.6), false, false)],
                terrain: Some(PlanTerrain {
                    distance_m: 1292.8,
                    target_m: -31.7,
                    ground: ground.clone(),
                    arcs: vec![
                        ProfileArc { points: pairs(&args[1]), high: true, charge: Some(0), used: true, stopped: None },
                        ProfileArc { points: pairs(&args[5]), high: false, charge: Some(0), used: false, stopped: Some((stop, ground_at(stop))) },
                    ],
                    skims: None,
                    note: TerrainNote::OtherArc { high: false, at_m: stop, auto: true, hard: true },
                }),
            }
        } else {
            PlanDetail {
                label: "A7".into(),
                height_m: -22.7,
                azimuth: 161.24,
                precision: Some((2.3, true)),
                blast_m: Some((55.4, 25.6)),
                low: Some(PlanArc { elevation: 7.81, time_s: 6.5, beyond_reach: false, blocked_m: None, hint_m: None }),
                high: Some(PlanArc { elevation: 79.46, time_s: 38.7, beyond_reach: true, blocked_m: None, hint_m: None }),
                high_chosen: false,
                auto: true,
                charges: vec![boxed("Low", 0, (0.1, 3.5), Some(6.5), true, true), boxed("Medium", 1, (0.1, 9.3), Some(3.2), false, false), boxed("High", 2, (0.1, 16.7), Some(1.6), false, false)],
                terrain: Some(PlanTerrain {
                    distance_m: 1279.3,
                    target_m: -22.7,
                    ground: ground.clone(),
                    arcs: vec![ProfileArc { points: pairs(&args[1]), high: false, charge: Some(0), used: true, stopped: None }],
                    skims: None,
                    note: TerrainNote::Clear,
                }),
            }
        }),
        gun_set: true,
        gun_source: Some("V1 arrow  ·  ±1.2 m".into()),
        notice: None,
        terrain_open: true,
        settings_open: false,
        targets_open: true,
        scale,
    };
    let layer = render_plan(&roblox_font_dir(), &state).expect("render");
    let mut image = RgbImage::new(layer.width, layer.height);
    for (i, px) in image.data.chunks_mut(3).enumerate() {
        let (b, g, r, a) = (layer.bgra[i * 4], layer.bgra[i * 4 + 1], layer.bgra[i * 4 + 2], layer.bgra[i * 4 + 3]);
        let keep = 255 - u32::from(a);
        let ground = [11u32, 16, 14];
        for (k, src) in [r, g, b].into_iter().enumerate() {
            px[k] = (u32::from(src) + (ground[k] * keep + 127) / 255).min(255) as u8;
        }
    }
    image.save_png(Path::new(&args[2])).expect("save");
    println!("{} x {}", layer.width, layer.height);
}
