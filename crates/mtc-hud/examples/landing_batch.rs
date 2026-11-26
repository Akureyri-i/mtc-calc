//! Shots against the model, one per line of a file: `map gun_x gun_y elevation azimuth weapon
//! velocity drag lifetime x_x y_x label…` (metres, degrees; underscores for spaces in the weapon).
//! For each: the distance and azimuth to the X, the model's landing at the X's ground height from the
//! weapon's launch point (the ground in between left out), and the miss along the line (X minus
//! model) and across it (right positive).
//!   cargo run --release -p mtc-hud --example landing_batch -- shots.txt

use mtc_ballistics::{Projectile, trajectory};

fn main() {
    let path = std::env::args().nth(1).expect("file");
    for line in std::fs::read_to_string(path).expect("read").lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let n = |i: usize| f[i].parse::<f64>().unwrap_or_else(|_| panic!("field {i} of {line}"));
        let map = mtc_maps::by_id_or_debug(f[0]).expect("map");
        let heights = mtc_maps::load_heightmap(map).expect("heightmap");
        let size = map.size_m();
        let (gx, gy, elevation, azimuth) = (n(1), n(2), n(3), n(4));
        let launch = mtc_ammo::launch_point(&f[5].replace('_', " ")).0;
        let p = Projectile { velocity: n(6), gravity: Some(9.81), drag_constant: Some(n(7)), lifetime: Some(n(8)), ..Projectile::default() };
        let (xx, xy) = (n(9), n(10));
        let label = f[11..].join(" ");
        let h = |x: f64, y: f64| heights.as_ref().map_or(0.0, |hm| hm.height_m(x / size, y / size));
        let base = h(gx, gy);
        let (dx, dy) = (xx - gx, xy - gy);
        let dist = dx.hypot(dy);
        let az_x = dx.atan2(-dy).to_degrees().rem_euclid(360.0);
        let dz = h(xx, xy) - base;
        let target = dz - launch.height_m;
        let path = trajectory(&p, elevation, 1.0e6, 1.0e4);
        let model = path
            .windows(2)
            .find(|w| w[1].1 < w[0].1 && w[0].1 >= target && w[1].1 < target)
            .map_or(f64::NAN, |w| w[0].0 + (w[1].0 - w[0].0) * (w[0].1 - target) / (w[0].1 - w[1].1))
            + launch.ahead_m;
        let across = dist * (az_x - azimuth).to_radians().sin();
        println!(
            "{label:<30} {elevation:>6.2}° az {azimuth:>6.2}  X {dist:>7.1} m az {az_x:>6.2} (ground {dz:+5.1} m)  model {model:>7.1} m  along {:+6.1} m  across {across:+5.1} m",
            dist - model
        );
    }
}
