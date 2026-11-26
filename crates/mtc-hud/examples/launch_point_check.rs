//! Where shots measured on Fulvia Gap (2S33) land by the model over the heightmap,
//! launched from the ground at the arrow and from the measured launch point.
//!   cargo run --release -p mtc-hud --example launch_point_check

use mtc_ballistics::{LaunchPoint, Projectile, impact};

fn main() {
    let map = mtc_maps::by_id_or_debug("fulvia_gap").expect("fulvia_gap");
    let heights = mtc_maps::load_heightmap(map).expect("heightmap").expect("Fulvia Gap has one");
    let size = map.size_m();
    let launch = mtc_ammo::launch_point("152mm 2A64M").0;
    let projectile = |v: f64| Projectile { velocity: v, gravity: Some(9.81), drag_constant: Some(7.4982806475763e-05), lifetime: Some(45.0), ..Projectile::default() };
    // (gun x, y in m; elevation, azimuth; muzzle velocity; measured distance to the X)
    let shots = [
        ((171.1, 1393.7), 6.46, 97.57, 207.0, 897.8),
        ((170.3, 1393.6), 11.37, 93.89, 207.0, 1563.7),
        ((170.8, 1393.5), 15.79, 95.89, 207.0, 2078.6),
        ((170.3, 1393.6), 2.89, 93.89, 414.0, 1584.0),
        ((170.8, 1393.5), 3.78, 95.88, 414.0, 2116.0),
    ];
    for ((gx, gy), elevation, azimuth, v, measured) in shots {
        let (du, dv) = (f64::to_radians(azimuth).sin(), -f64::to_radians(azimuth).cos());
        let gun = (gx / size, gy / size);
        let at = |x: f64| (gun.0 + du * x / size, gun.1 + dv * x / size);
        let base = heights.height_m(gun.0, gun.1);
        let ground = |x: f64| {
            let p = at(x);
            heights.height_m(p.0, p.1) - base
        };
        let p = projectile(v);
        let from_ground = impact(&p, elevation, ground).map_or(f64::NAN, |(x, _)| x);
        let from_launch = launch.impact(&p, elevation, ground).map_or(f64::NAN, |(x, _)| x);
        let at_arrow = LaunchPoint::default().impact(&p, elevation, ground).map_or(f64::NAN, |(x, _)| x);
        println!(
            "{v:>4} m/s {elevation:>5.2}°: measured {measured:>7.1}  from the ground {from_ground:>7.1} ({:+6.1})  from the launch point {from_launch:>7.1} ({:+5.1})",
            measured - from_ground,
            measured - from_launch
        );
        assert_eq!(from_ground.to_bits(), at_arrow.to_bits());
    }
}
