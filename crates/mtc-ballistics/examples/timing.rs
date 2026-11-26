//! Time per projectile to trace its trajectories (`Ballistics::new`) and to solve one target
//! (`arcs`), over every projectile of the golden fixture at half and 90 % of maximum range.
//!   cargo run --release -p mtc-ballistics --example timing
use std::time::Instant;

use mtc_ballistics::{Ballistics, Projectile};
use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    projectile: Projectile,
}

fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/ballistics/golden.json");
    let golden: Golden = serde_json::from_str(&std::fs::read_to_string(path).expect("golden")).expect("parse");
    let (mut traced, mut solved) = (Vec::new(), Vec::new());
    for case in golden.cases {
        let started = Instant::now();
        let b = Ballistics::new(case.projectile);
        traced.push(started.elapsed().as_secs_f64() * 1000.0);
        let max = b.max_range(0.0);
        for share in [0.5, 0.9] {
            let started = Instant::now();
            let _ = b.arcs(max * share, 0.0);
            solved.push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let stats = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        (v[v.len() / 2], v[v.len() * 95 / 100], v[v.len() - 1])
    };
    let (t50, t95, tmax) = stats(&mut traced);
    let (s50, s95, smax) = stats(&mut solved);
    println!("trace a projectile: median {t50:.1} ms, p95 {t95:.1} ms, max {tmax:.1} ms");
    println!("solve one target:   median {s50:.1} ms, p95 {s95:.1} ms, max {smax:.1} ms");
}
