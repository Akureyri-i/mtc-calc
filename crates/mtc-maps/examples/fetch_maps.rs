//! Development: downloads map pictures (and optionally a heightmap) into an empty cache the way
//! the HUD does, from both hosts at once, and prints how long each took.
//!   MTC_MAP_CACHE=<empty dir> cargo run --release -p mtc-maps --example fetch_maps -- <slug> [...] [--heightmap]
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let heightmap = args.iter().any(|a| a == "--heightmap");
    for slug in args.iter().filter(|a| !a.starts_with("--")) {
        let map = mtc_maps::all().iter().find(|m| &m.slug == slug).expect("map slug");
        let started = Instant::now();
        match mtc_maps::download(map, mtc_maps::REFERENCE_FILE) {
            Ok(path) => println!("{slug} picture: {} bytes in {:.1} s", std::fs::metadata(&path).map_or(0, |m| m.len()), started.elapsed().as_secs_f64()),
            Err(e) => println!("{slug} picture failed after {:.1} s: {e}", started.elapsed().as_secs_f64()),
        }
        if heightmap {
            let started = Instant::now();
            match mtc_maps::download(map, mtc_maps::HEIGHTMAP_FILE) {
                Ok(path) => println!("{slug} heightmap: {} bytes in {:.1} s", std::fs::metadata(&path).map_or(0, |m| m.len()), started.elapsed().as_secs_f64()),
                Err(e) => println!("{slug} heightmap failed after {:.1} s: {e}", started.elapsed().as_secs_f64()),
            }
        }
    }
}
