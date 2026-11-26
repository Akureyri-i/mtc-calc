//! What loading the map library at start costs, step by step (the start-up CPU peak: the
//! `map-library` thread took 0.47–0.61 cores in the first 5 s). For every map:
//! decoding the reference picture, building the working planes, making the identification
//! thumbnail (all the library keeps).
//!
//! `MTC_MAP_CACHE=scratch/map-cache cargo run --release -p mtc-hud --example library_cost`

use std::time::Instant;

use mtc_vision::map::{MapIdentifier, Objective};
use mtc_vision::map::MapReference;

fn main() {
    let mut identifier = MapIdentifier::new(&[]);
    let (mut decode, mut planes, mut thumb) = (0.0, 0.0, 0.0);
    let mut pixels = Vec::new();
    for info in mtc_maps::all() {
        let t = Instant::now();
        let Ok(img) = mtc_maps::load_reference(info) else {
            println!("{}: not in the cache", info.id);
            continue;
        };
        let d = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let reference = MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data);
        let p = t.elapsed().as_secs_f64() * 1000.0;
        let layout: Vec<Objective> = mtc_maps::objectives(&info.id).iter().map(|o| Objective { letter: o.letter.chars().next().expect("letter"), u: o.u, v: o.v }).collect();
        let t = Instant::now();
        identifier.push(&reference, &layout);
        let th = t.elapsed().as_secs_f64() * 1000.0;
        println!("{:<22} {:>5}x{:<5} decode {d:6.1} ms · working planes {p:6.1} ms · thumbnails {th:5.1} ms", info.id, img.width, img.height);
        (decode, planes, thumb) = (decode + d, planes + p, thumb + th);
        pixels.push(u64::from(img.width) * u64::from(img.height));
    }
    println!("{} maps in all: decode {decode:.0} ms · working planes {planes:.0} ms · thumbnails {thumb:.0} ms · total {:.0} ms", pixels.len(), decode + planes + thumb);
}
