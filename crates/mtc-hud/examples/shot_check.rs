//! Development: each logged shot with a landing, from a live HUD log: the X seen against the
//! landing over the terrain layer (what the HUD predicts: trees and small houses left out) and over
//! the full heightmap (trees as their crowns' tops, houses); whether the arc passes something
//! taller than the terrain on the way.
//!   cargo run --release -p mtc-hud --example shot_check -- <hud.log> <shells.json>
//! CSV: shot,map,weapon,shell,elevation,azimuth,gun_x,gun_y,seen_m,logged_pred_m,terrain_m,full_m,lateral_m,elevation_for_seen
use std::collections::HashMap;

fn shot_number(line: &str) -> u32 {
    line.trim_start_matches("Shot ").split(|c: char| !c.is_ascii_digit()).next().and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn num_after(line: &str, key: &str) -> Option<f64> {
    let rest = &line[line.find(key)? + key.len()..];
    let s: String = rest.trim_start().chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+').collect();
    s.parse().ok()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let log = std::fs::read_to_string(&args[1]).expect("hud log");
    let ammo = mtc_ammo::AmmoData::from_json(&std::fs::read_to_string(&args[2]).expect("shells json")).expect("ammo data");
    let mut map: Option<&'static mtc_maps::MapInfo> = None;
    let mut weapon: Option<String> = None;
    let mut gun: Option<(f64, f64)> = None;
    // shot -> (map, weapon, shell, elevation, azimuth, gun, logged prediction)
    let mut shots: HashMap<u32, (&'static mtc_maps::MapInfo, String, String, f64, f64, (f64, f64), f64)> = HashMap::new();
    let mut heights: HashMap<String, (mtc_maps::Heightmap, Option<mtc_maps::Heightmap>)> = HashMap::new();
    println!("shot,map,weapon,shell,elevation,azimuth,gun_x,gun_y,seen_m,logged_pred_m,terrain_m,full_m,lateral_m,elevation_for_seen");
    for line in log.lines() {
        if let Some(rest) = line.strip_prefix("Map: ") {
            let name = rest.split(" (").next().unwrap_or("").trim();
            map = mtc_maps::all().iter().find(|m| m.name == name);
        } else if let Some(rest) = line.strip_prefix("Weapon: ") {
            weapon = Some(rest.split(" (").next().unwrap_or("").trim().to_owned());
        } else if line.starts_with("Gun (") {
            if let (Some(x), Some(y)) = (num_after(line, "x "), num_after(line, "y ")) {
                gun = Some((x, y));
            }
        } else if line.starts_with("Shot ") && line.contains("fired between") {
            let n = shot_number(line);
            let shell = line.split(" (").nth(1).and_then(|s| s.split(')').next()).unwrap_or("").to_owned();
            if let (Some(m), Some(w), Some(g), Some(el), Some(az), Some(pred)) = (map, weapon.clone(), gun, num_after(line, "elevation "), num_after(line, "azimuth "), num_after(line, "predicted ")) {
                shots.insert(n, (m, w, shell, el, az, g, pred));
            }
        } else if line.starts_with("Shot ") && line.contains(" impact #") {
            let n = shot_number(line);
            let seen = line.split(": ").nth(1).and_then(|s| num_after(&format!(" {s}"), " "));
            let lateral = num_after(line, " m along, ");
            let (Some((m, w, shell, el, az, g, pred)), Some(seen)) = (shots.get(&n), seen) else { continue };
            let Some(s) = ammo.shells(w).iter().find(|s| &s.name == shell) else { continue };
            let (h, t) = heights.entry(m.id.clone()).or_insert_with(|| {
                (mtc_maps::load_heightmap(m).expect("heightmap").expect("a heightmap"), mtc_maps::load_terrain(m).ok().flatten())
            });
            let size = m.size_m();
            let (du, dv) = (az.to_radians().sin(), -az.to_radians().cos());
            let gn = (g.0 / size, g.1 / size);
            let launch = mtc_ammo::launch_point(w).0;
            // Over one layer, as the HUD: not the ground right in front of the gun (30 m).
            let land = |layer: &mtc_maps::Heightmap| {
                let base = layer.height_m(gn.0, gn.1);
                launch
                    .impact(&s.projectile(), *el, |x: f64| if x < 30.0 { f64::NEG_INFINITY } else { layer.height_m(gn.0 + du * x / size, gn.1 + dv * x / size) - base })
                    .map(|(x, _)| x)
            };
            let full = land(h);
            let terrain = t.as_ref().and_then(|t| land(t));
            let f = |v: Option<f64>| v.map_or(String::from(""), |x| format!("{x:.1}"));
            // The elevation that lands at the seen distance over the terrain layer.
            let seen_el = t.as_ref().map(|t| {
                let base = t.height_m(gn.0, gn.1);
                let ground = |x: f64| if x < 30.0 { f64::NEG_INFINITY } else { t.height_m(gn.0 + du * x / size, gn.1 + dv * x / size) - base };
                let (mut lo, mut hi) = (el - 6.0, el + 6.0);
                for _ in 0..60 {
                    let mid = (lo + hi) / 2.0;
                    match launch.impact(&s.projectile(), mid, ground) {
                        Some((d, _)) if d < seen => lo = mid,
                        _ => hi = mid,
                    }
                }
                (lo + hi) / 2.0
            });
            println!("{n},{},{w},{shell},{el:.2},{az:.2},{:.1},{:.1},{seen:.1},{pred:.1},{},{},{},{}", m.id, g.0, g.1, f(terrain), f(full), lateral.map_or(String::new(), |l| format!("{l:.1}")), seen_el.map_or(String::new(), |e| format!("{e:.3}")));
        }
    }
}
