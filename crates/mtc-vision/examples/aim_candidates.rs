//! Development: the "Elevation" label candidates on single frames, on the plain plane and the
//! shadow-corrected one (score, frame x, y, "Azimuth" score).
//!   cargo run --release -p mtc-vision --example aim_candidates <frame.png>...
use mtc_dataset::image::RgbImage;
use mtc_vision::FrameView;
use mtc_vision::aim::AimReader;

fn main() {
    let reader = AimReader::new();
    for path in std::env::args().skip(1) {
        let image = RgbImage::load_png(std::path::Path::new(&path)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        for corrected in [false, true] {
            let c: Vec<String> = reader.probe_candidates(&view, (0, 34), corrected).iter().map(|(s, x, y, az)| format!("{s:.2}@{x},{y} az {az:.2}")).collect();
            println!("{path} {}: {}", if corrected { "corrected" } else { "plain" }, c.join(" | "));
        }
    }
}
