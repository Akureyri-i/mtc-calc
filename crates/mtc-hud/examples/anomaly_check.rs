//! Development: the shots of the statistics database whose X landed far from the prediction,
//! recomputed over the map's terrain layer and full heightmap. For each: where the arc stands at
//! the X (above the terrain, inside something the heightmap has above it — trees, houses — or
//! under the terrain), and the elevation that would have landed at the X with its time of flight
//! against the one seen.
//!   cargo run --release -p mtc-hud --example anomaly_check -- <shells.json> <records.json> <min |along| m>
//! records.json: the `record` objects of the `shots` table (outcome "x"), as a JSON array.
use std::collections::HashMap;

use serde_json::Value;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let ammo = mtc_ammo::AmmoData::from_json(&std::fs::read_to_string(&a[0]).expect("shells json")).expect("ammo data");
    let recs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&a[1]).expect("records")).expect("records json");
    let min_along: f64 = a[2].parse().expect("min along");
    let mut layers: HashMap<String, Option<(mtc_maps::Heightmap, mtc_maps::Heightmap)>> = HashMap::new();
    println!("id,map,weapon,charge,elevation,azimuth,predicted_m,seen_m,along_m,terrain_land_m,seen_s,predicted_s,arc_at_x_m,terrain_at_x_m,objects_top_at_x_m,verdict,elevation_for_x,time_for_x_s,slope_along_deg,slope_across_deg");
    for r in &recs {
        let num = |k: &str| r.get(k).and_then(Value::as_f64);
        let text = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
        let (Some(along), Some(el), Some(az), Some(pred_m), Some(pred_s)) = (num("along_m"), num("elevation"), num("azimuth"), num("predicted_m"), num("predicted_s")) else { continue };
        if along.abs() < min_along {
            continue;
        }
        let pair = |k: &str| r.get(k).and_then(Value::as_array).and_then(|v| Some((v.first()?.as_f64()?, v.get(1)?.as_f64()?)));
        let (Some(gun), Some(x)) = (pair("gun"), pair("x")) else { continue };
        let (map_id, weapon, shell_name) = (text("map"), text("weapon"), text("shell"));
        let Some(map) = mtc_maps::by_id(&map_id) else { continue };
        let Some(shell) = ammo.shells(&weapon).iter().find(|s| s.name == shell_name).map(|s| s.projectile()) else { continue };
        let Some((full, terrain)) = layers
            .entry(map_id.clone())
            .or_insert_with(|| Some((mtc_maps::load_heightmap(map).ok()??, mtc_maps::load_terrain(map).ok()??)))
            .as_ref()
        else {
            continue;
        };
        let size = map.size_m();
        let (du, dv) = (az.to_radians().sin(), -az.to_radians().cos());
        let base = terrain.height_m(gun.0, gun.1);
        let at = |layer: &mtc_maps::Heightmap, d: f64| layer.height_m(gun.0 + du * d / size, gun.1 + dv * d / size) - base;
        let launch = mtc_ammo::launch_point(&weapon).0;
        let ground = |d: f64| if d < 30.0 { f64::NEG_INFINITY } else { at(terrain, d) };
        let terrain_land = launch.impact(&shell, el, ground).map_or(f64::NAN, |(d, _)| d);
        let seen_m = ((x.0 - gun.0) * size).hypot((x.1 - gun.1) * size);
        let seen_s = pred_s + num("late_s").unwrap_or(f64::NAN);
        // The arc's height where the X is, the terrain there and the tallest thing the heightmap
        // has above the terrain within 15 m of it along the line.
        let arc = launch.trajectory(&shell, el, seen_m + 50.0, 500.0);
        let arc_at = arc.windows(2).find(|w| w[0].0 <= seen_m && seen_m <= w[1].0).map(|w| w[0].1 + (w[1].1 - w[0].1) * (seen_m - w[0].0) / (w[1].0 - w[0].0));
        let t_x = at(terrain, seen_m);
        let top = (-15..=15).map(|k| at(full, seen_m + f64::from(k))).fold(f64::NEG_INFINITY, f64::max);
        let verdict = match arc_at {
            None => "arc ends before the X",
            Some(h) if h > top + 3.0 => "arc above everything at the X",
            Some(h) if h >= t_x - 3.0 && top - t_x > 3.0 => "X inside trees or houses on the arc",
            Some(h) if (h - t_x).abs() <= 3.0 => "X on the terrain under the arc",
            Some(_) => "arc under the terrain at the X",
        };
        // The elevation that lands at the X over the terrain layer, and its time of flight.
        let (mut lo, mut hi) = (el - 6.0, el + 6.0);
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            match launch.impact(&shell, mid, ground) {
                Some((d, _)) if d < seen_m => lo = mid,
                _ => hi = mid,
            }
        }
        let e = (lo + hi) / 2.0;
        let t_e = launch.impact(&shell, e, ground).map_or(f64::NAN, |(_, t)| t);
        // The ground's slope under the gun (terrain layer, over 12 m each way): along the line of
        // fire (rising ahead positive) and across it (rising to the right positive).
        let slope = |dx: f64, dy: f64| {
            let h = |k: f64| terrain.height_m(gun.0 + dx * k / size, gun.1 + dy * k / size);
            ((h(12.0) - h(-12.0)) / 24.0).atan().to_degrees()
        };
        let (along_slope, across_slope) = (slope(du, dv), slope(-dv, du));
        let id = r.get("id").and_then(Value::as_i64).unwrap_or(0);
        println!(
            "{id},{map_id},{weapon},{},{el:.2},{az:.2},{pred_m:.1},{seen_m:.1},{along:.1},{terrain_land:.1},{seen_s:.2},{pred_s:.2},{},{t_x:.1},{top:.1},{verdict},{e:.3},{t_e:.2},{along_slope:.2},{across_slope:.2}",
            text("charge"),
            arc_at.map_or(String::new(), |h| format!("{h:.1}"))
        );
    }
}
