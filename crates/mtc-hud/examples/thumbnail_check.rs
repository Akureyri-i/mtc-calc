//! Whether every saved identification thumbnail is exactly the one made from its reference now
//! (the HUD reads them instead of making them at start).
//!
//! `MTC_MAP_CACHE=scratch/map-cache cargo run --release -p mtc-hud --example thumbnail_check`

use mtc_vision::map::{MapReference, Thumbnail};

fn main() {
    let (mut same, mut checked) = (0, 0);
    for info in mtc_maps::all() {
        let Ok(reference) = std::fs::read(mtc_maps::reference_path(info)) else { continue };
        let key = mtc_maps::content_hash(&reference);
        let saved = std::fs::read(mtc_maps::thumbnail_path(info)).ok().and_then(|b| Thumbnail::from_bytes(&b, key));
        let img = mtc_maps::load_reference(info).expect("reference");
        let fresh = Thumbnail::of(&MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data));
        checked += 1;
        let ok = saved.as_ref() == Some(&fresh);
        same += usize::from(ok);
        if !ok {
            println!("{}: {}", info.id, if saved.is_some() { "the saved one differs from a fresh one" } else { "none saved" });
        }
    }
    println!("{same}/{checked} saved identification thumbnails are byte-identical to fresh ones");
}
