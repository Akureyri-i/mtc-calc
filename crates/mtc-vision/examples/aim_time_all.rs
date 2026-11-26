//! Development: time of the angle reader on every labelled frame (fresh reader each, no cached
//! place), split by whether it read; for comparing reader versions on the same frames.
//!   cargo run --release -p mtc-vision --example aim_time_all
use mtc_dataset::eval::labelled_frames;
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};

fn main() {
    let (mut read, mut nothing): (Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new());
    for frame in labelled_frames().expect("dataset") {
        let Ok(image) = frame.image() else { continue };
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let origin = frame.meta.geometry.client_in_frame();
        let mut reader = AimReader::new();
        let started = std::time::Instant::now();
        let r = reader.read(&view, (origin.x, origin.y));
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if matches!(r, AimResult::Read(_)) { read.push(ms) } else { nothing.push(ms) }
    }
    let q = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(f64::total_cmp);
        v.get(((v.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0.0)
    };
    println!("read {}: median {:.2} ms p90 {:.2}; nothing {}: median {:.2} ms p90 {:.2}", read.len(), q(&mut read, 0.5), q(&mut read, 0.9), nothing.len(), q(&mut nothing, 0.5), q(&mut nothing, 0.9));
}
