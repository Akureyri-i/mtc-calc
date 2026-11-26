//! Development: the unzoomed V1 hypothesis' reference score (`ReferenceMatcher::fixed_score`) on
//! every V1 frame of a session, as CSV `file,t_ms,score`; the registrar takes a frame as unzoomed
//! at `FIXED_MIN_SCORE`.
//!   cargo run --release -p mtc-vision --example v1_fixed_score -- <session-id> <map-id> [reference.png]
//! A reference image given replaces the map's (e.g. one made from the game's own V1).
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::map::zoom::ReferenceMatcher;
use mtc_vision::map::{CueReader, MapReference, MapTransform, MapView};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::open(datasets_dir().join("raw").join(&args[1])).expect("session");
    let info = mtc_maps::by_id(&args[2]).expect("map id");
    let reference = match args.get(3) {
        Some(path) => {
            let img = image_rgba(path);
            MapReference::from_rgba(&info.id, info.size_m(), img.0, img.1, &img.2)
        }
        None => {
            let img = mtc_maps::load_reference(info).expect("reference");
            MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data)
        }
    };
    let matcher = ReferenceMatcher::new(&reference);
    let cues = CueReader::new();
    let mut frames: Vec<_> = session.frames.iter().collect();
    frames.sort_by_key(|m| m.t_ms);
    println!("file,t_ms,score");
    for meta in frames {
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let origin = meta.geometry.client_in_frame();
        let origin = (origin.x, origin.y);
        if !cues.read(&view, origin).shows(MapView::Full) {
            continue;
        }
        let base = MapTransform::fixed(MapView::Full, origin);
        let score = matcher.fixed_score(&view, &base).map_or(String::from("none"), |s| format!("{s:.3}"));
        println!("{},{},{score}", meta.file, meta.t_ms);
    }
}

/// A PNG as (width, height, RGBA bytes).
fn image_rgba(path: &str) -> (u32, u32, Vec<u8>) {
    let rgb = RgbImage::load_png(std::path::Path::new(path)).expect("reference png");
    let rgba = rgb.data.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    (rgb.width as u32, rgb.height as u32, rgba)
}
