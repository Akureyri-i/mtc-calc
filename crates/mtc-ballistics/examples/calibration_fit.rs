//! Fits each gun's launch point to measured shots: the shell leaves `height` m above the ground
//! under the player's arrow and `ahead` m in front of it along the line of fire, where the model
//! launches it from the ground at the arrow. For a height the best offset is the mean of the
//! measured minus the modelled distances; the height with the least RMS wins.
//!   cargo run --release -p mtc-ballistics --example calibration_fit -- <shots.csv> [--max-residual M] [--only COLUMN=VALUE] [--check HEIGHT,AHEAD]
//! CSV columns: gun, shell, velocity, drag, lifetime, elevation, distance_m, dz_m, source (dz: the
//! X's ground height over the gun's), then optional columns named in the header (`--only gun_view=V1`
//! keeps the rows whose column has that value). Shots the model cannot bring down are left out;
//! with `--max-residual`, shots off the first fit by more than that (an arc into a tree or a house)
//! are left out and the gun fitted again.

use mtc_ballistics::{Projectile, trajectory};

struct Shot {
    gun: String,
    shell: String,
    projectile: Projectile,
    elevation: f64,
    distance: f64,
    dz: f64,
    source: String,
}

/// Where the shot comes down through the X's height, interpolated between frames as the HUD's
/// arcs and `impact` are (`landing_range` stops at the frame after: up to a frame's flight long).
fn modelled(s: &Shot, height: f64) -> f64 {
    let h = s.dz - height;
    let path = trajectory(&s.projectile, s.elevation, 1.0e6, 1.0e4 - h);
    path.windows(2)
        .find(|w| w[1].1 < w[0].1 && w[0].1 >= h && w[1].1 < h)
        .map_or(f64::NAN, |w| w[0].0 + (w[1].0 - w[0].0) * (w[0].1 - h) / (w[0].1 - w[1].1))
}

/// (offset, rms) for `height`.
fn fit_offset(shots: &[&Shot], height: f64) -> (f64, f64) {
    let residuals: Vec<f64> = shots.iter().map(|s| s.distance - modelled(s, height)).collect();
    let offset = residuals.iter().sum::<f64>() / residuals.len() as f64;
    let rms = (residuals.iter().map(|r| (r - offset).powi(2)).sum::<f64>() / residuals.len() as f64).sqrt();
    (offset, rms)
}

/// (height, offset) with the least RMS over 0–8 m.
fn best_fit(shots: &[&Shot]) -> (f64, f64) {
    (0..=160)
        .map(|i| {
            let h = f64::from(i) * 0.05;
            let (offset, rms) = fit_offset(shots, h);
            (h, offset, rms)
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map_or((0.0, 0.0), |(h, o, _)| (h, o))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("CSV path");
    let max_residual: Option<f64> = args.iter().position(|a| a == "--max-residual").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok());
    let only: Option<(String, String)> = args.iter().position(|a| a == "--only").and_then(|i| args.get(i + 1)).and_then(|v| v.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned()));
    let check: Option<(f64, f64)> = args.iter().position(|a| a == "--check").and_then(|i| args.get(i + 1)).and_then(|v| v.split_once(',')).and_then(|(h, a)| Some((h.parse().ok()?, a.parse().ok()?)));
    let text = std::fs::read_to_string(path).expect("read CSV");
    let header: Vec<&str> = text.lines().next().unwrap_or("").split(',').collect();
    let only_column = only.as_ref().map(|(k, v)| (header.iter().position(|h| h == k).expect("--only column"), v.clone()));
    let shots: Vec<Shot> = text
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .filter(|l| only_column.as_ref().is_none_or(|(i, v)| l.split(',').nth(*i) == Some(v.as_str())))
        .filter(|l| {
            let f: Vec<&str> = l.split(',').collect();
            let p = Projectile { velocity: f[2].parse().unwrap_or(0.0), gravity: Some(9.81), drag_constant: f[3].parse().ok(), lifetime: f[4].parse().ok(), ..Projectile::default() };
            let s = Shot { gun: String::new(), shell: String::new(), projectile: p, elevation: f[5].parse().unwrap_or(0.0), distance: 0.0, dz: f[7].parse().unwrap_or(0.0), source: String::new() };
            !modelled(&s, 0.0).is_nan() && !modelled(&s, 8.0).is_nan()
        })
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            let num = |i: usize| f[i].trim().parse::<f64>().unwrap_or_else(|_| panic!("column {i} of {l}"));
            Shot {
                gun: f[0].to_owned(),
                shell: f[1].to_owned(),
                projectile: Projectile { velocity: num(2), gravity: Some(9.81), drag_constant: Some(num(3)), lifetime: Some(num(4)), ..Projectile::default() },
                elevation: num(5),
                distance: num(6),
                dz: num(7),
                source: f[8].to_owned(),
            }
        })
        .collect();
    let mut guns: Vec<&str> = shots.iter().map(|s| s.gun.as_str()).collect();
    guns.dedup();
    guns.sort_unstable();
    guns.dedup();
    for gun in guns {
        let mut own: Vec<&Shot> = shots.iter().filter(|s| s.gun == gun).collect();
        if let Some(limit) = max_residual {
            let (height, offset) = best_fit(&own);
            let before = own.len();
            own.retain(|s| (s.distance - modelled(s, height) - offset).abs() <= limit);
            println!("{gun}: {} of {before} shots within {limit} m of the first fit", own.len());
        }
        if let Some((h, a)) = check {
            let misses: Vec<f64> = own.iter().map(|s| s.distance - modelled(s, h) - a).collect();
            let mean = misses.iter().sum::<f64>() / misses.len() as f64;
            let rms = (misses.iter().map(|m| m * m).sum::<f64>() / misses.len() as f64).sqrt();
            println!("{gun}: with {h} m up and {a} m ahead: mean miss {mean:+.1} m, RMS {rms:.1} m over {} shots", misses.len());
        }
        let (base_offset, base_rms) = fit_offset(&own, 0.0);
        let mut best = (0.0, base_offset, base_rms);
        let mut curve = Vec::new();
        for i in 0..=160 {
            let height = f64::from(i) * 0.05;
            let (offset, rms) = fit_offset(&own, height);
            curve.push((height, rms));
            if rms < best.2 {
                best = (height, offset, rms);
            }
        }
        // The height range whose RMS is within 0.5 m of the best: how well the shots pin it.
        let near: Vec<f64> = curve.iter().filter(|(_, r)| *r <= best.2 + 0.5).map(|(h, _)| *h).collect();
        println!(
            "{gun}: {} shots · launch {:.2} m up (RMS within 0.5 m for {:.2}–{:.2}), {:+.1} m ahead · RMS {:.1} m (from the ground at the arrow: mean miss {:+.1} m, spread {:.1} m)",
            own.len(),
            best.0,
            near.first().unwrap_or(&best.0),
            near.last().unwrap_or(&best.0),
            best.1,
            best.2,
            base_offset,
            base_rms
        );
        for s in &own {
            let fitted = modelled(s, best.0) + best.1;
            println!(
                "    {:<22} {:>6.2}°  dz {:+5.1}  measured {:>7.1}  model from ground {:>7.1} ({:+6.1})  fitted {:>7.1} ({:+5.1})  {}",
                s.shell,
                s.elevation,
                s.dz,
                s.distance,
                modelled(s, 0.0),
                s.distance - modelled(s, 0.0),
                fitted,
                s.distance - fitted,
                s.source
            );
        }
    }
}
