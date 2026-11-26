//! Loads every cached heightmap: load time, size, and heights at a fixed set of points (for
//! comparison with an independent full-resolution read), as CSV lines `map,u,v,metres`.
//!   heightmap_check [points per map, default 100]
//!   cargo run --release -p mtc-maps --example heightmap_check
fn main() {
    let points: usize = std::env::args().nth(1).map_or(100, |n| n.parse().expect("points"));
    for map in mtc_maps::all().iter().filter(|m| m.heightmap.is_some()) {
        let started = std::time::Instant::now();
        let Ok(Some(h)) = mtc_maps::load_heightmap(map) else {
            eprintln!("{}: no heightmap in the cache", map.id);
            continue;
        };
        eprintln!("{:<22} {}x{} loaded in {} ms", map.id, h.width, h.height, started.elapsed().as_millis());
        for i in 0..points {
            let (u, v) = ((i as f64 * 0.6180339887).fract(), (i as f64 * 0.7548776662).fract());
            println!("{},{u},{v},{}", map.slug, h.height_m(u, v));
        }
    }
}
