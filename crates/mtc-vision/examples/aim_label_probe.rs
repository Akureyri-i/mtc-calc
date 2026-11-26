//! Development: the best "Elevation" label candidate the angle reader finds on single frames (score,
//! frame x and y, the "Azimuth" label's score under it), whether or not the lines decode.
//!   cargo run --release -p mtc-vision --example aim_label_probe -- <frame.png>...
use mtc_dataset::image::RgbImage;
use mtc_vision::FrameView;
use mtc_vision::aim::AimReader;

fn main() {
    let reader = AimReader::new();
    for path in std::env::args().skip(1) {
        let image = RgbImage::load_png(std::path::Path::new(&path)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        println!("{path}: {:?}", reader.probe_label(&view, (0, 34)));
    }
}
