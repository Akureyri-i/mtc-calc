//! Prints raw readout tokens for every labelled frame the reader skips.
//!   cargo run --release -p mtc-vision --example aim_debug

use mtc_dataset::eval::labelled_frames;
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};

fn main() {
    let mut reader = AimReader::new();
    for frame in labelled_frames().expect("dataset").iter().filter(|f| f.label.aim.is_some()) {
        let image = frame.image().expect("image");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let origin = frame.meta.geometry.client_in_frame();
        let origin = (origin.x, origin.y);
        if !matches!(reader.read(&view, origin), AimResult::Read(_)) {
            let truth = frame.label.aim.as_ref().expect("filtered");
            println!("{:45} truth {:>7} {:>7}  raw {:?}", frame.id(), truth.elevation, truth.azimuth, reader.raw_lines(&view, origin));
        }
    }
}
