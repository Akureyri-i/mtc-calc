//! Development: the angle reader on every labelled frame, each with a fresh reader (no cached
//! place), listing the frames read from the shadow-corrected plane (with the label when there is
//! one) and the time of reads that found nothing, to check that plane adds no wrong readings.
//!   cargo run --release -p mtc-vision --example aim_shadow_check
use mtc_dataset::eval::labelled_frames;
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};

fn main() {
    let (mut frames, mut plain, mut shadow, mut nothing) = (0, 0, 0, 0);
    let mut nothing_ms: Vec<f64> = Vec::new();
    let mut read_ms: Vec<f64> = Vec::new();
    for frame in labelled_frames().expect("dataset") {
        let Ok(image) = frame.image() else { continue };
        frames += 1;
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let origin = frame.meta.geometry.client_in_frame();
        let mut reader = AimReader::new();
        let started = std::time::Instant::now();
        let result = reader.read(&view, (origin.x, origin.y));
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        match result {
            AimResult::Read(r) if reader.last_from_shadow() => {
                shadow += 1;
                let truth = frame.label.aim.as_ref().map_or("no label".to_owned(), |t| format!("label {} {}", t.elevation, t.azimuth));
                println!("{}: shadow read {} {} (cost {}) | {truth}", frame.id(), r.elevation_text, r.azimuth_text, r.cost);
            }
            AimResult::Read(_) => {
                plain += 1;
                read_ms.push(ms);
            }
            _ => {
                nothing += 1;
                nothing_ms.push(ms);
            }
        }
    }
    let pct = |v: &mut Vec<f64>, q: f64| {
        v.sort_by(f64::total_cmp);
        v.get(((v.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0)
    };
    println!(
        "{frames} frames: {plain} read plainly ({:.2} ms median), {shadow} from the shadow, {nothing} nothing ({:.2} ms median, p90 {:.2})",
        pct(&mut read_ms, 0.5),
        pct(&mut nothing_ms.clone(), 0.5),
        pct(&mut nothing_ms, 0.9)
    );
}
