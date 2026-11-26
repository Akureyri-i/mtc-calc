//! Per-map identification scores for one frame and view:
//!   cargo run --release -p mtc-vision --example map_ident_debug <png> <V1|V2|V3>
use mtc_dataset::image::RgbImage;
use mtc_vision::FrameView;
use mtc_vision::map::{MapIdentifier, MapReference, MapTransform, MapView};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let references: Vec<MapReference> = mtc_maps::all()
        .iter()
        .map(|m| {
            let img = mtc_maps::load_reference(m).expect("reference");
            MapReference::from_rgba(&m.id, m.size_m(), img.width, img.height, &img.data)
        })
        .collect();
    for r in &references {
        eprintln!("{:20} grid lines painted out {:?}", r.id, r.grid_lines);
    }
    let identifier = MapIdentifier::new(&references);
    let image = RgbImage::load_png(std::path::Path::new(&args[1])).expect("frame");
    let view = FrameView::rgb(&image.data, image.width, image.height);
    let v = match args[2].as_str() {
        "V1" => MapView::Full,
        "V2" => MapView::Scope,
        _ => MapView::Mini,
    };
    let scores = identifier.scores(&view, &MapTransform::fixed(v, (0, 34))).expect("covered");
    let mut ranked: Vec<_> = references.iter().zip(scores).map(|(r, s)| (s, r.id.clone())).collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (s, id) in ranked.iter().take(6) {
        println!("{id:20} {s:.3}");
    }
}
