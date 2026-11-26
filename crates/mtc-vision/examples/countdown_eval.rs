//! Development: loading countdowns over the recordings. Per run of countdown frames, the end
//! predicted from its progress (a straight line through the readings so far, at each reading) is
//! compared with the last frame the countdown showed.
//!   cargo run --release -p mtc-vision --example countdown_eval
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir, sessions};
use mtc_vision::FrameView;
use mtc_vision::loading::{countdown_progress, countdown_shown};

fn main() {
    let mut errors: Vec<f64> = Vec::new();
    for info in sessions().expect("sessions") {
        let id = info.info.id.clone();
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        let mut metas: Vec<_> = session.frames.iter().collect();
        metas.sort_by_key(|m| m.t_ms);
        let mut run: Vec<(f64, f64)> = Vec::new();
        let mut flush = |run: &mut Vec<(f64, f64)>, id: &str| {
            if run.len() >= 3 {
                let end = run.last().expect("run").0;
                let mut line = String::new();
                for k in 1..run.len() - 1 {
                    // Least squares through readings 0..=k.
                    let pts = &run[..=k];
                    let n = pts.len() as f64;
                    let (mt, mp) = (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n);
                    let slope = pts.iter().map(|p| (p.0 - mt) * (p.1 - mp)).sum::<f64>() / pts.iter().map(|p| (p.0 - mt).powi(2)).sum::<f64>().max(1e-9);
                    if slope <= 0.0 {
                        continue;
                    }
                    let predicted = mt + (1.0 - mp) / slope;
                    errors.push(predicted - end);
                    line += &format!(" {:+.2}", predicted - end);
                }
                println!("{id} {:.1}-{end:.1}s ({} readings, progress {:.2}→{:.2}): predicted end minus last frame:{line}", run[0].0, run.len(), run[0].1, run.last().expect("run").1);
            }
            run.clear();
        };
        for meta in metas {
            let t = meta.t_ms as f64 / 1000.0;
            if run.last().is_some_and(|p| t - p.0 > 1.5) {
                flush(&mut run, &id);
            }
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            if countdown_shown(&view, (o.x, o.y)) {
                if let Some(p) = countdown_progress(&view, (o.x, o.y)) {
                    run.push((t, p));
                }
            } else {
                flush(&mut run, &id);
            }
        }
        flush(&mut run, &id);
    }
    errors.sort_by(f64::total_cmp);
    let q = |f: f64| errors[((errors.len() - 1) as f64 * f).round() as usize];
    if !errors.is_empty() {
        println!("{} predictions: predicted end minus last countdown frame median {:+.2} s, p10 {:+.2}, p90 {:+.2}, min {:+.2}, max {:+.2}", errors.len(), q(0.5), q(0.1), q(0.9), q(0.0), q(1.0));
    }
}
