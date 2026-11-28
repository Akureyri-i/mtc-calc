//! Development: the V1 plan panel listing one squad member's markers, composited onto a frame,
//! for design mockups of choosing whose markers the HUD follows:
//!   squad_plan_composite <frame.png> <out.png> <letter> <count> [selected] [--first N] [--shift PX] [--folded]
//!     [--squad A6,B6,C4 --own C]
//! `--squad`: the squad row (each letter and its markers; the listed letter followed, `--own` the
//! player's).
//! The rows are `<letter>1`…`<letter><count>` (sample distances and charges) and the map's three
//! objectives A–C; the panel docks where it does live on a 2560×1368 frame, `--shift` px lower
//! (room for a row drawn above it in a mockup).
use std::path::Path;

use mtc_dataset::image::RgbImage;
use mtc_overlay::{PanelLoad, PlanArc, PlanChargeBox, PlanDetail, PlanNext, PlanRow, PlanState, SquadLetter, SquadRow, render_plan};

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

fn composite(image: &mut RgbImage, layer: &mtc_overlay::Layer) {
    for y in 0..layer.height as i32 {
        for x in 0..layer.width as i32 {
            let (fx, fy) = (layer.x + x, layer.y + y);
            if fx < 0 || fy < 0 || fx >= image.width as i32 || fy >= image.height as i32 {
                continue;
            }
            let i = (y as usize * layer.width as usize + x as usize) * 4;
            let (b, g, r, a) = (layer.bgra[i], layer.bgra[i + 1], layer.bgra[i + 2], layer.bgra[i + 3]);
            let j = (fy as usize * image.width as usize + fx as usize) * 3;
            let keep = 255 - u32::from(a);
            for (k, src) in [r, g, b].into_iter().enumerate() {
                image.data[j + k] = (u32::from(src) + (u32::from(image.data[j + k]) * keep + 127) / 255).min(255) as u8;
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut image = RgbImage::load_png(Path::new(&args[0])).expect("frame");
    // `--zh`: the calculator's text in Chinese.
    if args.iter().any(|a| a == "--zh") {
        mtc_overlay::lang::set_language(mtc_overlay::lang::Language::Chinese);
    }
    let letter = args[2].chars().next().expect("letter");
    let count: u32 = args[3].parse().expect("count");
    let first: u32 = args.iter().position(|a| a == "--first").map_or(1, |i| args[i + 1].parse().expect("first number"));
    let selected = args.get(4).filter(|a| !a.starts_with("--")).cloned();
    let folded = args.iter().any(|a| a == "--folded");
    let shift: i32 = args.iter().position(|a| a == "--shift").map_or(0, |i| args[i + 1].parse().expect("shift px"));
    let samples = [(1846.0, 1u8, 11.8), (2210.0, 1, 13.9), (1532.0, 0, 12.4), (2690.0, 2, 10.6), (1980.0, 1, 12.6), (2402.0, 1, 14.8)];
    let names = ["Low", "Medium", "High"];
    let mut rows: Vec<PlanRow> = (0..count)
        .map(|i| {
            let (d, c, t) = samples[i as usize % samples.len()];
            PlanRow {
                label: format!("{letter}{}", first + i),
                objective: false,
                distance_m: Some(d),
                charge: Some((names[c as usize].into(), c)),
                time_s: Some(t),
                high_arc: false,
                reachable: true,
                precision: Some((1.2, false)),
            }
        })
        .collect();
    for (o, d, t) in [("A", 1204.0, 8.5), ("B", 1374.0, 9.5), ("C", 1987.0, 13.2)] {
        rows.push(PlanRow { label: o.into(), objective: true, distance_m: Some(d), charge: Some(("Low".into(), 0)), time_s: Some(t), high_arc: false, reachable: true, precision: None });
    }
    let detail = selected.as_ref().map(|label| PlanDetail {
        label: label.clone(),
        height_m: 6.2,
        azimuth: 131.08,
        precision: Some((1.2, false)),
        blast_m: Some((49.7, 20.7)),
        low: Some(PlanArc { elevation: 11.62, time_s: 13.9, beyond_reach: false, blocked_m: None, hint_m: None }),
        high: Some(PlanArc { elevation: 71.40, time_s: 41.2, beyond_reach: false, blocked_m: None, hint_m: None }),
        high_chosen: false,
        auto: true,
        charges: vec![
            PlanChargeBox { name: "Low".into(), index: 0, band_km: Some((0.3, 2.4)), time_s: None, loaded: true, planned: false },
            PlanChargeBox { name: "Medium".into(), index: 1, band_km: Some((0.9, 3.6)), time_s: Some(13.9), loaded: false, planned: true },
            PlanChargeBox { name: "High".into(), index: 2, band_km: Some((1.6, 5.0)), time_s: Some(9.8), loaded: false, planned: false },
        ],
        terrain: None,
    });
    let state = PlanState {
        at: (839 + 882 + 64, 34 + 226 + shift),
        rows,
        selected,
        hover: None,
        shell: Some("3OF45".into()),
        loaded_charge: Some(("Low".into(), 0)),
        shell_kind: Some("HE".into()),
        menu: None,
        charge_manual: None,
        load: PanelLoad::Loaded,
        next: PlanNext::Load("Medium".into(), 1),
        detail,
        gun_set: true,
        gun_source: Some(format!("{} · ±0.9 m", mtc_overlay::lang::t("gun.full_map"))),
        notice: None,
        terrain_open: false,
        settings_open: false,
        targets_open: !folded,
        squad: args.iter().position(|a| a == "--squad").map(|i| {
            let own = args.iter().position(|a| a == "--own").and_then(|j| args[j + 1].chars().next());
            SquadRow {
                letters: args[i + 1]
                    .split(',')
                    .map(|t| {
                        let l = t.chars().next().expect("letter");
                        SquadLetter { letter: l, markers: t[1..].parse().expect("markers"), own: own == Some(l) }
                    })
                    .collect(),
                following: letter,
            }
        }),
        scale: 1.0,
    };
    let layer = render_plan(&roblox_font_dir(), &state).expect("render");
    composite(&mut image, &layer);
    image.save_png(Path::new(&args[1])).expect("save");
    println!("panel at ({}, {}) {}x{}", layer.x, layer.y, layer.width, layer.height);
}
