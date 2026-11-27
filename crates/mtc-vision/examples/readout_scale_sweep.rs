//! Development: how well the angle readout reads on a recording at each multiple of the text scale
//! the layout model gives (`UiLayout::readout_text`), labels and digits scaled alike: frames read,
//! their mean cost, and the mean "Elevation" label score. Shows the true scale of a configuration
//! whose rule is off, or of a game whose text size setting was raised.
//!   cargo run --release -p mtc-vision --example readout_scale_sweep -- <session> [from_s to_s] [lo hi step]
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::aim::{AimReader, AimResult, TextScale};
use mtc_vision::layout::{Placement, UiLayout, remap};
use mtc_vision::FrameView;

/// The readout's area in the reference client (as `AimReader` reads it, with room above for
/// larger text).
const AREA: (i32, i32, i32, i32) = (330, 1150, 1340, 184);

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let session = Session::open(datasets_dir().join("raw").join(&a[0])).expect("session");
    let (from, to) = if a.len() >= 3 { (a[1].parse::<f64>().unwrap() * 1000.0, a[2].parse::<f64>().unwrap() * 1000.0) } else { (0.0, f64::MAX) };
    let (lo, hi, step) = if a.len() >= 6 { (a[3].parse::<f32>().unwrap(), a[4].parse::<f32>().unwrap(), a[5].parse::<f32>().unwrap()) } else { (0.85, 1.6, 0.025) };
    let virtual_origin = Placement::VIRTUAL_ORIGIN;
    let mut views = Vec::new();
    let mut layout = None;
    for meta in session.frames.iter().filter(|m| (m.t_ms as f64) >= from && (m.t_ms as f64) <= to) {
        let g = &meta.geometry;
        let l = UiLayout { width: g.client.width as u32, height: g.client.height as u32, dpi: g.dpi };
        // One configuration: the first frame's (a recording can end on another).
        if layout.is_some_and(|first| first != l) {
            continue;
        }
        layout = Some(l);
        let c = g.client_in_frame();
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let (ax, ay, aw, ah) = AREA;
        let r = remap(&view, (c.x, c.y), &l.bottom(), (ax, ay, aw, ah), virtual_origin);
        views.push((r.x, r.y, r.width, r.height, r.data));
    }
    let layout = layout.expect("frames in the window");
    let base = layout.readout_text();
    println!("{} frames, {:?}, rule scale {:?}", views.len(), layout, base);
    println!("factor,read,unreadable,absent,mean_cost,mean_label_score,read_best_dy,mean_cost_best_dy,dy_counts");
    let mut f = lo;
    while f <= hi + 1e-4 {
        let scale = TextScale { label: (base.label.0 * f, base.label.1 * f), digits: (base.digits.0 * f, base.digits.1 * f) };
        let mut reader = AimReader::with_scale(scale);
        let (mut read, mut unread, mut absent, mut cost, mut score, mut scored) = (0, 0, 0, 0u64, 0.0f32, 0);
        let (mut best_read, mut best_cost, mut dys) = (0, 0u64, std::collections::BTreeMap::new());
        for (x, y, w, h, data) in &views {
            let v = FrameView { data, format: mtc_vision::view::PixelFormat::BgraPeak, origin: (*x, *y), width: *w, height: *h, stride: *w as usize * 4 };
            if let Some((s, lx, ly, _)) = reader.probe_label(&v, virtual_origin) {
                score += s;
                scored += 1;
                // The value lines decoded at each vertical offset under the found label: the best.
                let best = (-3..=5).filter_map(|dy| reader.probe_lines(&v, (virtual_origin.0, virtual_origin.1 + dy), lx, ly).map(|(_, _, c)| (c, dy))).min();
                if let Some((c, dy)) = best {
                    best_read += 1;
                    best_cost += u64::from(c);
                    *dys.entry(dy).or_insert(0) += 1;
                }
            }
            match reader.read(&v, virtual_origin) {
                AimResult::Read(r) => {
                    read += 1;
                    cost += u64::from(r.cost);
                }
                AimResult::Unreadable { .. } => unread += 1,
                AimResult::Absent => absent += 1,
            }
        }
        println!("{f:.3},{read},{unread},{absent},{:.1},{:.3},{best_read},{:.1},{dys:?}", cost as f64 / f64::from(read.max(1)), score / scored.max(1) as f32, best_cost as f64 / f64::from(best_read.max(1)));
        f += step;
    }
}
