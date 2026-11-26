//! What the aim tolerance means on the ground for the measured guns:
//! per charge and distance (flat ground, from the gun's launch point), the low arc's elevation, its
//! range change per degree, the miss one readout step (0.01°) and ±0.05° make, and the elevation
//! tolerance a ±5 m miss along the line allows.
//!   cargo run --release -p mtc-hud --example tolerance_table

use mtc_ballistics::{Ballistics, Projectile};

fn main() {
    let guns: [(&str, &str, [(&str, f64, f64, f64); 3]); 4] = [
        ("D-30", "122mm D-30", [("Low", 175.0, 9.652183153767759e-05, 50.0), ("Medium", 350.0, 9.652183153767759e-05, 50.0), ("High", 690.0, 9.652183153767759e-05, 20.0)]),
        ("2S33", "152mm 2A64M", [("Low", 207.0, 7.4982806475763e-05, 45.0), ("Medium", 414.0, 7.4982806475763e-05, 45.0), ("High", 828.0, 7.4982806475763e-05, 45.0)]),
        ("M109A6", "155mm M284", [("Low", 171.0, 3.449643574347571e-05, 45.0), ("Medium", 342.0, 6.020904164719427e-05, 30.0), ("High", 684.0, 7.954243009446473e-05, 20.0)]),
        ("60mm M2", "60mm M2", [("Low", 125.0, 0.0006971753560021186, 30.0), ("Medium", 172.0, 0.0006971753560021186, 30.0), ("High", 225.0, 0.0002544690049407733, 30.0)]),
    ];
    for (gun, weapon, shells) in guns {
        let launch = mtc_ammo::launch_point(weapon).0;
        let (lo, hi) = mtc_ammo::elevation_limits(weapon).unwrap_or((-90.0, 90.0));
        println!("{gun} ({weapon}, elevation {lo}..{hi}°)");
        for (charge, v, drag, life) in shells {
            let p = Projectile { velocity: v, gravity: Some(9.81), drag_constant: Some(drag), lifetime: Some(life), ..Projectile::default() };
            let b = Ballistics::new(p.clone());
            for d in [500.0, 1000.0, 1500.0, 2000.0, 3000.0] {
                let (td, th) = launch.target(d, 0.0);
                let (low, high) = b.arcs(td, th);
                for (name, arc) in [("low", low), ("high", high)] {
                    let Some(arc) = arc.filter(|a| (lo..=hi).contains(&a.elevation)) else { continue };
                    let e = arc.elevation;
                    let slope = ((launch.landing_range(&p, e + 0.05, 0.0) - launch.landing_range(&p, e - 0.05, 0.0)) / 0.1).abs();
                    if !slope.is_finite() {
                        continue;
                    }
                    println!(
                        "  {charge:<6} {d:>5.0} m {name:<4} {e:>6.2}°  {slope:>6.0} m/°  0.01° = {:>5.1} m  ±0.05° = ±{:>5.1} m  ±5 m = ±{:.3}°  azimuth ±0.05° = ±{:.1} m",
                        slope * 0.01,
                        slope * 0.05,
                        5.0 / slope,
                        d * 0.05f64.to_radians()
                    );
                }
            }
        }
    }
}
