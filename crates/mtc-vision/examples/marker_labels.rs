//! Marker labels read on recorded unzoomed V1 frames (squad letters included):
//!   cargo run --release -p mtc-vision --example marker_labels <session-id> [from_s] [to_s]
//!
//! Every frame whose cues show V1 in the time range is read with the unzoomed V1 transform, so
//! only frames with the map unzoomed and centred give meaningful positions.

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::map::markers::MarkerReader;
use mtc_vision::map::{CueReader, MapTransform, MapView};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let id = args.get(1).expect("session id");
    let from = args.get(2).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) * 1000.0;
    let to = args.get(3).and_then(|s| s.parse::<f64>().ok()).unwrap_or(f64::INFINITY) * 1000.0;
    let session = Session::open(datasets_dir().join("raw").join(id)).expect("session");
    let reader = MarkerReader::new();
    let cues = CueReader::new();
    for meta in &session.frames {
        let t = meta.t_ms as f64;
        if t < from || t > to {
            continue;
        }
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("png");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        if cues.read(&view, (o.x, o.y)).primary() != Some(MapView::Full) {
            continue;
        }
        let transform = MapTransform::fixed(MapView::Full, (o.x, o.y));
        let mut found = reader.read(&view, &transform);
        found.sort_by(|a, b| a.label.cmp(&b.label));
        let labels: Vec<String> = found
            .iter()
            .map(|m| format!("{}@{:.2},{:.2}", m.label.as_deref().unwrap_or("-"), m.frame.0, m.frame.1))
            .collect();
        println!("{:.1}s {}: {}", t / 1000.0, meta.file, labels.join(" "));
    }
}
