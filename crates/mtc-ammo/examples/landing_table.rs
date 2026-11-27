//! Development: where one shell lands, as JSON rows of elevation (°), distance (m) and time of
//! flight (s), on flat ground `height` metres relative to the gun, with the weapon's launch point.
//! With `path` at the end, the flight at the first elevation instead: one [distance, height] a frame.
//!   cargo run --release -p mtc-ammo --example landing_table -- <shells.json> <weapon> <shell> <height> <from°> <to°> <step°> [path]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = mtc_ammo::AmmoData::from_json(&std::fs::read_to_string(&args[1]).expect("shells.json")).expect("parse");
    let (weapon, name) = (&args[2], &args[3]);
    let number = |i: usize| args[i].parse::<f64>().expect("number");
    let (height, from, to, step) = (number(4), number(5), number(6), number(7));
    let shell = data.shells(weapon).iter().find(|s| s.name == *name).expect("shell");
    let projectile = shell.projectile();
    let (launch, _) = mtc_ammo::launch_point(weapon);
    if args.get(8).is_some_and(|a| a == "path") {
        let points = launch.trajectory(&projectile, from, f64::INFINITY, -height);
        println!("[{}]", points.iter().map(|(x, y)| format!("[{x:.2},{y:.2}]")).collect::<Vec<_>>().join(","));
        return;
    }
    let mut rows = Vec::new();
    let mut elevation = from;
    while elevation <= to + 1e-9 {
        if let Some((distance, time)) = launch.impact(&projectile, elevation, |_| height) {
            rows.push(format!("[{elevation:.3},{distance:.2},{time:.3}]"));
        }
        elevation += step;
    }
    println!("[{}]", rows.join(","));
}
