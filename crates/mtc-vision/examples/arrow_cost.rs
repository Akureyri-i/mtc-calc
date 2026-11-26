//! Development: the player arrow on the minimap of a session's frames in a time window: whole or
//! clipped, how many clipped blobs, and the time of each clipped fit from the last pose and from
//! scratch, to find where the fitting thread's time goes.
//!   cargo run --release -p mtc-vision --example arrow_cost <session> <from-ms> <to-ms>
use mtc_dataset::{Session, datasets_dir, image::RgbImage};
use mtc_vision::FrameView;
use mtc_vision::map::arrow::{fit_clipped_blob, fit_clipped_blob_near, player_arrow_candidates};
use mtc_vision::map::{MapTransform, MapView};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::open(datasets_dir().join("raw").join(&args[1])).expect("session");
    let (from, to): (u64, u64) = (args[2].parse().expect("from"), args[3].parse().expect("to"));
    let mut last: Option<((f64, f64), f64)> = None;
    for meta in session.frames.iter().filter(|m| (from..=to).contains(&m.t_ms)) {
        let Ok(image) = RgbImage::load_png(&session.frame_path(meta)) else { continue };
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let t = MapTransform::fixed(MapView::Mini, (o.x, o.y));
        let c = player_arrow_candidates(&view, &t);
        if let Some(a) = c.complete {
            last = Some((a.map, a.heading_deg));
            println!("{} whole arrow", meta.file);
            continue;
        }
        for b in &c.clipped {
            let started = std::time::Instant::now();
            let seeded = fit_clipped_blob_near(b, last);
            let seeded_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = std::time::Instant::now();
            let scratch = fit_clipped_blob(b);
            let scratch_ms = started.elapsed().as_secs_f64() * 1000.0;
            println!(
                "{} clipped ({} blobs): from last pose {seeded_ms:.1} ms {:?}, from scratch {scratch_ms:.1} ms {:?}",
                meta.file,
                c.clipped.len(),
                seeded.map(|s| (s.frame.0.round(), s.frame.1.round(), (s.shape * 100.0).round() / 100.0)),
                scratch.map(|s| (s.frame.0.round(), s.frame.1.round(), (s.shape * 100.0).round() / 100.0))
            );
            if let Some(s) = seeded {
                last = Some((s.map, s.heading_deg));
            }
        }
    }
}
