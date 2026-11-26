//! Development: the HUD's V1 map marks (as drawn live: selected diamond, blast, precision
//! labels, covered markers) composited onto a frame, for design mockups that build on them:
//!   v1_marks_composite <frame.png> <out.png> <spec.txt>
//! spec.txt: first line `container_left container_top container_size px_per_m selected reach_m
//! kill_m` (reach 0: no blast); then one line per target `label x y precision_m|- mark|objective [covered]` (frame px,
//! the marker point; the frame's window sits at the screen's top left). Optional V1 planning
//! lines: `gun x y`, `picture left top right bottom`, `ring radius_px charge loaded|focus|other
//! max|min`, `tag label flight_s|- charge|- [elevation distance_m]`, `fields none|time,elevation,distance,precision`
//! (what the small labels show; time and precision when not given), `hover label charge_name|- charge|- distance_m
//! low_elev|- low_s high_elev|- high_s`. `pred x y`: the predicted landing; `small`: drawn as on the
//! minimap.
use std::path::Path;

use mtc_dataset::image::RgbImage;
use mtc_overlay::{Blast, Hover, MapMark, MapMarks, Planning, RangeRing, RingKind, TargetLabel, render_offscreen};

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut image = RgbImage::load_png(Path::new(&args[0])).expect("frame");
    let spec = std::fs::read_to_string(&args[2]).expect("spec");
    let mut lines = spec.lines().filter(|l| !l.trim().is_empty());
    let head: Vec<&str> = lines.next().expect("header").split_whitespace().collect();
    let (left, top, size): (i32, i32, i32) = (head[0].parse().unwrap(), head[1].parse().unwrap(), head[2].parse().unwrap());
    let px_per_m: f32 = head[3].parse().unwrap();
    let selected = head[4];
    let (reach_m, kill_m): (f32, f32) = (head[5].parse().unwrap(), head[6].parse().unwrap());
    let lines: Vec<&str> = lines.collect();
    let word = |l: &&str, w: &str| l.split_whitespace().next() == Some(w);
    let num = |s: &str| -> Option<f32> { s.parse().ok() };
    let mut planning: Option<Planning> = None;
    for l in &lines {
        let f: Vec<&str> = l.split_whitespace().collect();
        if ["gun", "picture", "ring", "tag", "hover", "fields"].contains(&f[0]) && planning.is_none() {
            planning = Some(Planning { gun: (0.0, 0.0), picture: (0.0, 0.0, 10_000.0, 10_000.0), rings: Vec::new(), labels: Vec::new(), fields: mtc_overlay::LabelFields { time: true, precision: true, ..Default::default() }, hover: None, stopped: None });
        }
        let Some(plan) = planning.as_mut() else { continue };
        match f[0] {
            "gun" => plan.gun = (num(f[1]).unwrap(), num(f[2]).unwrap()),
            "picture" => plan.picture = (num(f[1]).unwrap(), num(f[2]).unwrap(), num(f[3]).unwrap(), num(f[4]).unwrap()),
            "ring" => plan.rings.push(RangeRing {
                radius_px: num(f[1]).unwrap(),
                charge: f[2].parse().ok(),
                kind: match f[3] { "loaded" => RingKind::Loaded, "focus" => RingKind::Focus, _ => RingKind::Other },
                min: f[4] == "min",
            }),
            "tag" => plan.labels.push(TargetLabel {
                label: f[1].to_owned(),
                flight_s: num(f[2]),
                charge: f[3].parse().ok(),
                elevation: f.get(4).and_then(|v| num(v)),
                distance_m: f.get(5).and_then(|v| num(v)).unwrap_or(0.0),
            }),
            "fields" => {
                let on = |w: &str| f[1].split(',').any(|x| x == w);
                plan.fields = mtc_overlay::LabelFields { time: on("time"), elevation: on("elevation"), distance: on("distance"), precision: on("precision"), blast: on("blast") };
            }
            "hover" => {
                plan.hover = Some(Hover {
                    label: f[1].to_owned(),
                    charge: f[3].parse().ok().map(|c| (f[2].to_owned(), c)),
                    distance_m: num(f[4]).unwrap(),
                    low: num(f[5]).zip(num(f[6])),
                    high: num(f[7]).zip(num(f[8])),
                    high_chosen: f.get(9) == Some(&"high"),
                })
            }
            _ => {}
        }
    }
    let marks: Vec<MapMark> = lines
        .iter()
        .filter(|l| !["gun", "picture", "ring", "tag", "hover", "pred", "small", "fields"].iter().any(|w| word(l, w)))
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            MapMark {
                label: f[0].to_owned(),
                x: f[1].parse().unwrap(),
                y: f[2].parse().unwrap(),
                selected: f[0] == selected,
                objective: f[4] == "objective",
                precision_m: f[3].parse().ok(),
                covered: f.get(5) == Some(&"covered"),
            }
        })
        .collect();
    let map = MapMarks {
        left,
        top,
        size,
        small: lines.iter().any(|l| word(l, "small")),
        marks,
        coarse_m: 2.0,
        // A reach of 0 draws no blast (a mockup that shows the blast its own way).
        blast: (reach_m > 0.0).then_some(Blast { reach_px: reach_m * px_per_m, kill_px: kill_m * px_per_m, reach_m, spread_px: None }),
        planning,
        follow: None,
        pred: lines.iter().find(|l| word(l, "pred")).map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (num(f[1]).unwrap(), num(f[2]).unwrap())
        }),
        scale: 1.0,
    };
    let layers = render_offscreen(&roblox_font_dir(), None, None, Some(&map)).expect("render");
    for layer in &layers {
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
    image.save_png(Path::new(&args[1])).expect("save");
}
