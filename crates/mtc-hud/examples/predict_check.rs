//! The ground and the arc along one shot over a map's heightmap, from the weapon's launch point:
//! where the landing prediction and the terrain profile come from.
//!   cargo run --release -p mtc-hud --example predict_check -- MAP X_M Y_M ELEVATION AZIMUTH WEAPON VELOCITY DRAG LIFETIME

use mtc_ballistics::{Projectile, trajectory};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize| a[i].parse::<f64>().unwrap_or_else(|_| panic!("argument {i}"));
    let map = mtc_maps::by_id_or_debug(&a[0]).expect("map id");
    let heights = mtc_maps::load_heightmap(map).expect("heightmap").expect("a heightmap");
    let size = map.size_m();
    let (gx, gy, elevation, azimuth) = (num(1), num(2), num(3), num(4));
    let launch = mtc_ammo::launch_point(&a[5]).0;
    let p = Projectile { velocity: num(6), gravity: Some(9.81), drag_constant: Some(num(7)), lifetime: Some(num(8)), ..Projectile::default() };
    let (du, dv) = (azimuth.to_radians().sin(), -azimuth.to_radians().cos());
    let gun = (gx / size, gy / size);
    let base = heights.height_m(gun.0, gun.1);
    let ground = |x: f64| heights.height_m(gun.0 + du * x / size, gun.1 + dv * x / size) - base;
    println!("map {} ({:.0} m), gun ({gx}, {gy}) ground {base:.1} m, launch {:.2} m up {:.1} m ahead", map.name, size, launch.height_m, launch.ahead_m);
    println!("ground around the gun (m from it: height over the gun's ground): {}", [-20.0, -10.0, 0.0, 10.0, 20.0, 30.0, 50.0, 80.0, 120.0].iter().map(|x| format!("{x:.0}:{:+.1}", ground(*x))).collect::<Vec<_>>().join("  "));
    let path = trajectory(&p, elevation, 4000.0, 300.0);
    for (x, y) in path.iter().step_by(2).take(600) {
        let x = x + launch.ahead_m;
        let y = y + launch.height_m;
        println!("  {x:>7.1} m  arc {y:>7.1}  ground {:>7.1}  {}", ground(x), if y < ground(x) { "BELOW" } else { "" });
    }
    let hit = launch.impact(&p, elevation, ground);
    println!("predicted landing: {hit:?}");
}
