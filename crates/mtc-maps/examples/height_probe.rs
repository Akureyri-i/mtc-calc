//! Ground height at map positions: `height_probe <map-id> <u> <v> [<u> <v> …]`.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let map = mtc_maps::by_id(&args[0]).expect("map id");
    let heightmap = mtc_maps::load_heightmap(map).expect("heightmap file").expect("map has a heightmap");
    println!("{}: heightmap {}×{}, reference canvas {:.0} m", map.name, heightmap.width, heightmap.height, map.size_m());
    for pair in args[1..].chunks(2) {
        let (u, v): (f64, f64) = (pair[0].parse().expect("u"), pair[1].parse().expect("v"));
        println!("  ({u:.4}, {v:.4}) -> {:.1} m", heightmap.height_m(u, v));
    }
}
