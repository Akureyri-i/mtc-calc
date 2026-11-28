//! Ground height at map positions, from the heightmap and from the terrain layer when the cache
//! has one: `height_probe <map-id> <u> <v> [<u> <v> …]`.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let map = mtc_maps::by_id(&args[0]).expect("map id");
    let heightmap = mtc_maps::load_heightmap(map).expect("heightmap file").expect("map has a heightmap");
    let terrain = mtc_maps::load_terrain(map).ok().flatten();
    println!("{}: heightmap {}×{}, reference canvas {:.0} m, terrain layer {}", map.name, heightmap.width, heightmap.height, map.size_m(), if terrain.is_some() { "cached" } else { "not cached" });
    for pair in args[1..].chunks(2) {
        let (u, v): (f64, f64) = (pair[0].parse().expect("u"), pair[1].parse().expect("v"));
        let t = terrain.as_ref().map_or("—".to_owned(), |t| format!("{:.1} m", t.height_m(u, v)));
        println!("  ({u:.4}, {v:.4}) -> heightmap {:.1} m, terrain {t}", heightmap.height_m(u, v));
    }
}
