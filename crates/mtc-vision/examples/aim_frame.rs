//! What the angle reader makes of single frames: the raw symbols of both value lines and the result.
//!   cargo run --release -p mtc-vision --example aim_frame <frame.png>...
//! `MTC_AIM_SCALE="lx,ly,dx,dy"` gives the configuration's text scale (default the reference).

use mtc_dataset::image::RgbImage;
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, TextScale};

fn main() {
    let scale = std::env::var("MTC_AIM_SCALE").ok().map_or(TextScale::REFERENCE, |v| {
        let f: Vec<f32> = v.split(',').map(|x| x.trim().parse().expect("MTC_AIM_SCALE: four numbers")).collect();
        TextScale { label: (f[0], f[1]), digits: (f[2], f[3]) }
    });
    let mut reader = AimReader::with_scale(scale);
    for path in std::env::args().skip(1) {
        let image = RgbImage::load_png(std::path::Path::new(&path)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        // Recorder frames: 34 px title bar above the client area.
        let origin = (0, 34);
        let raw = reader.raw_lines(&view, origin);
        let result = reader.read(&view, origin);
        println!("{path}: raw {raw:?} → {result:?}");
    }
}
