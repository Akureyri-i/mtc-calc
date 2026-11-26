//! Development: the terrain layers as a player's first start gets them, from the real services:
//! each map's heightmap downloaded if missing, then `fetch_terrain`, then the layer loaded.
//!   MTC_MAP_CACHE=<empty folder> cargo run --release -p mtc-maps --example terrain_fetch -- [map ids]
use std::time::Instant;

fn main() {
    let ids: Vec<String> = std::env::args().skip(1).collect();
    let maps: Vec<&mtc_maps::MapInfo> =
        mtc_maps::all().iter().filter(|m| m.heightmap.is_some() && (ids.is_empty() || ids.contains(&m.id))).collect();
    println!("cache {}", mtc_maps::cache_dir().display());
    for map in maps {
        let t = Instant::now();
        if !mtc_maps::heightmap_path(map).is_file() {
            if let Err(e) = mtc_maps::download(map, mtc_maps::HEIGHTMAP_FILE) {
                println!("{}: heightmap not downloaded: {e}", map.id);
                continue;
            }
        }
        let heightmap_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let status = mtc_maps::fetch_terrain(map);
        let fetch_ms = t.elapsed().as_secs_f64() * 1000.0;
        let layer = (status == mtc_maps::TerrainStatus::Ready).then(|| mtc_maps::load_terrain(map));
        let loaded = match layer {
            Some(Ok(Some(h))) => format!("loaded {}x{}", h.width, h.height),
            Some(Ok(None)) => "missing after Ready".to_owned(),
            Some(Err(e)) => format!("unreadable: {e}"),
            None => "not loaded".to_owned(),
        };
        println!("{}: heightmap {heightmap_ms:.0} ms, terrain {status:?} in {fetch_ms:.0} ms, {loaded}", map.id);
    }
}
