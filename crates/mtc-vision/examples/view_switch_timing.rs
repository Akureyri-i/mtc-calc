//! Development: how soon the view cues see the full map (V1) open and close, against when the
//! picture started changing. For each recorded frame where `shows(Full)` flips, prints the frames
//! around it: time (ms from the flip), mean absolute change of the V1 container area against the
//! previous frame, and the cue. With no session given, lists sessions and their frame spacing.
//!   cargo run --release -p mtc-vision --example view_switch_timing [session ...]
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir, sessions};
use mtc_vision::FrameView;
use mtc_vision::map::{CueReader, MapView};

/// Frames shown before and after each flip.
const BEFORE: usize = 8;
const AFTER: usize = 3;

fn main() {
    let wanted: Vec<String> = std::env::args().skip(1).collect();
    if wanted.is_empty() {
        for s in sessions().expect("sessions") {
            let t: Vec<u64> = s.frames.iter().map(|f| f.t_ms as u64).collect();
            let mut gaps: Vec<u64> = t.windows(2).map(|w| w[1].saturating_sub(w[0])).collect();
            gaps.sort_unstable();
            let median = gaps.get(gaps.len() / 2).copied().unwrap_or(0);
            println!("{} frames {} median gap {} ms", s.info.id, s.frames.len(), median);
        }
        return;
    }
    let cues = CueReader::new();
    let c = MapView::Full.container();
    for id in wanted {
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        let mut rows: Vec<(u64, f64, bool)> = Vec::new();
        let mut previous: Option<Vec<u8>> = None;
        let mut frames: Vec<_> = session.frames.iter().collect();
        frames.sort_by_key(|f| f.t_ms as u64);
        for meta in frames {
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            let full = cues.read(&view, (o.x, o.y)).shows(MapView::Full);
            // Sample the container every 8 px.
            let mut sample = Vec::new();
            let (x0, y0, size) = (c.x.round() as i32 + o.x, c.y.round() as i32 + o.y, c.size.round() as i32);
            for y in (y0..y0 + size).step_by(8) {
                for x in (x0..x0 + size).step_by(8) {
                    let p = view.px(x, y);
                    sample.push(((u16::from(p[0]) + u16::from(p[1]) + u16::from(p[2])) / 3) as u8);
                }
            }
            let change = previous.as_ref().filter(|p| p.len() == sample.len()).map_or(0.0, |p| {
                p.iter().zip(&sample).map(|(a, b)| f64::from(a.abs_diff(*b))).sum::<f64>() / sample.len() as f64
            });
            previous = Some(sample);
            rows.push((meta.t_ms as u64, change, full));
        }
        for k in 1..rows.len() {
            if rows[k].2 == rows[k - 1].2 {
                continue;
            }
            println!("{id} {} at {} ms", if rows[k].2 { "OPEN" } else { "CLOSE" }, rows[k].0);
            for r in &rows[k.saturating_sub(BEFORE)..(k + AFTER + 1).min(rows.len())] {
                println!("  {:+6} ms  change {:5.1}  full {}", r.0 as i64 - rows[k].0 as i64, r.1, u8::from(r.2));
            }
        }
    }
}
