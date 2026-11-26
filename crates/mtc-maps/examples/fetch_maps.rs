//! Fills the map cache from the official map service: every map's heightmap and any missing
//! reference image (`--refresh` downloads the references again too).
//!   cargo run --release -p mtc-maps --example fetch_maps [--refresh]
fn main() {
    let refresh = std::env::args().any(|a| a == "--refresh");
    for map in mtc_maps::all() {
        let mut files = Vec::new();
        if map.heightmap.is_some() {
            files.push(mtc_maps::HEIGHTMAP_FILE);
        }
        if refresh || !mtc_maps::reference_path(map).is_file() {
            files.push(mtc_maps::REFERENCE_FILE);
        }
        for file in files {
            match mtc_maps::download(map, file) {
                Ok(path) => println!("{:<22} {file:<14} {} KB", map.id, std::fs::metadata(&path).map_or(0, |m| m.len() / 1024)),
                Err(e) => println!("{:<22} {file:<14} failed: {e}", map.id),
            }
        }
    }
}
