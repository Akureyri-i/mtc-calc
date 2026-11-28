//! Development: one shot's arc against the ground along its line, over the terrain layer and the
//! full heightmap, both relative to the terrain under the gun, every `STEP` metres in a range.
//!   cargo run --release -p mtc-hud --example shot_profile -- <shells.json> MAP WEAPON SHELL X_M Y_M ELEVATION AZIMUTH FROM_M TO_M STEP_M
use mtc_ballistics::trajectory;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize| a[i].parse::<f64>().unwrap_or_else(|_| panic!("argument {i}"));
    let ammo = mtc_ammo::AmmoData::from_json(&std::fs::read_to_string(&a[0]).expect("shells json")).expect("ammo data");
    let map = mtc_maps::by_id(&a[1]).expect("map id");
    let shell = ammo.shells(&a[2]).iter().find(|s| s.name == a[3]).expect("shell").projectile();
    let (gx, gy, elevation, azimuth, from, to, step) = (num(4), num(5), num(6), num(7), num(8), num(9), num(10));
    let full = mtc_maps::load_heightmap(map).expect("heightmap").expect("a heightmap");
    let terrain = mtc_maps::load_terrain(map).ok().flatten().expect("terrain layer in the cache");
    let size = map.size_m();
    let (du, dv) = (azimuth.to_radians().sin(), -azimuth.to_radians().cos());
    let gun = (gx / size, gy / size);
    let base = terrain.height_m(gun.0, gun.1);
    let launch = mtc_ammo::launch_point(&a[2]).0;
    let arc = launch.trajectory(&shell, elevation, to + step, 200.0);
    let arc_at = |x: f64| {
        arc.windows(2).find(|w| w[0].0 <= x && x <= w[1].0).map(|w| w[0].1 + (w[1].1 - w[0].1) * (x - w[0].0) / (w[1].0 - w[0].0))
    };
    println!("gun ({gx}, {gy}) terrain {base:.1} m, launch {:.2} m up {:.1} m ahead; heights relative to the terrain under the gun", launch.height_m, launch.ahead_m);
    println!("distance_m,arc_m,terrain_m,full_m,arc_minus_terrain_m");
    let mut x = from;
    while x <= to + 1e-9 {
        let (u, v) = (gun.0 + du * x / size, gun.1 + dv * x / size);
        let (t, f) = (terrain.height_m(u, v) - base, full.height_m(u, v) - base);
        let arc = arc_at(x);
        println!("{x:.0},{},{t:.1},{f:.1},{}", arc.map_or(String::new(), |h| format!("{h:.1}")), arc.map_or(String::new(), |h| format!("{:.1}", h - t)));
        x += step;
    }
    // Where the arc lands over the terrain layer (past 30 m) with the gun raised by each offset.
    for dz in [0.0, 5.0, 10.0, 15.0, 20.0, 25.0] {
        let ground = |x: f64| {
            if x < 30.0 {
                return f64::NEG_INFINITY;
            }
            terrain.height_m(gun.0 + du * x / size, gun.1 + dv * x / size) - base - dz
        };
        let land = launch.impact(&shell, elevation, ground).map_or("none".to_owned(), |(x, t)| format!("{x:.1} m after {t:.2} s"));
        println!("gun {dz:+.0} m above the terrain layer: lands {land}");
    }
    // The elevation that lands at `to` over the terrain layer, and its time of flight.
    let terrain_ground = |x: f64| if x < 30.0 { f64::NEG_INFINITY } else { terrain.height_m(gun.0 + du * x / size, gun.1 + dv * x / size) - base };
    let (mut lo, mut hi) = (elevation - 3.0, elevation + 3.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        match launch.impact(&shell, mid, terrain_ground) {
            Some((x, _)) if x < to => lo = mid,
            _ => hi = mid,
        }
    }
    let e = (lo + hi) / 2.0;
    let (x, t) = launch.impact(&shell, e, terrain_ground).unwrap_or((f64::NAN, f64::NAN));
    println!("to land at {to:.0} m: elevation {e:.3}° (lands {x:.1} m after {t:.2} s)");
    // The muzzle velocity that lands at `to` at the given elevation, and its time of flight.
    let (mut lo, mut hi) = (shell.velocity * 0.8, shell.velocity * 1.3);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        let faster = mtc_ballistics::Projectile { velocity: mid, ..shell.clone() };
        match launch.impact(&faster, elevation, terrain_ground) {
            Some((x, _)) if x < to => lo = mid,
            _ => hi = mid,
        }
    }
    let v = (lo + hi) / 2.0;
    let faster = mtc_ballistics::Projectile { velocity: v, ..shell.clone() };
    let (x, t) = launch.impact(&faster, elevation, terrain_ground).unwrap_or((f64::NAN, f64::NAN));
    println!("to land at {to:.0} m at {elevation}°: velocity {v:.1} m/s instead of {:.0} (lands {x:.1} m after {t:.2} s)", shell.velocity);
    let _ = trajectory;
}
