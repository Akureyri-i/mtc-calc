//! Development: the minimap's and the full map's containers found on recorded frames
//! (`mtc_vision::locate`, from the read-back probes only, as the HUD reads them)
//! against the rules, and what a try costs. Per configuration and view: frames tried, found, the
//! median found box and the rule's, the median time per try.
//!   cargo run --release -p mtc-vision --example locate_cost -- <session> [...]
use std::collections::BTreeMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::layout::{UiLayout, picture_shift, shift_probes};
use mtc_vision::locate;
use mtc_vision::map::geometry::MapView;

fn main() {
    // (dpi, w, h, view) -> (tries, found boxes, microseconds)
    let mut seen: BTreeMap<(u32, u32, u32, &str), (usize, Vec<(f64, f64, f64)>, Vec<f64>)> = BTreeMap::new();
    for id in std::env::args().skip(1) {
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        let mut shift = (0, 0);
        for meta in &session.frames {
            let g = &meta.geometry;
            let (cw, ch) = (g.client.width as u32, g.client.height as u32);
            let layout = UiLayout { width: cw, height: ch, dpi: g.dpi };
            if layout.is_reference() {
                continue;
            }
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let frame = FrameView::rgb(&image.data, image.width, image.height);
            let c = g.client_in_frame();
            if image.width > cw {
                let [(bx, by, bw, bh), (rx, ry, rw, rh)] = shift_probes((image.width, image.height));
                shift = picture_shift(&frame.sub(bx, by, bw, bh), &frame.sub(rx, ry, rw, rh)).unwrap_or(shift);
            } else {
                shift = (0, 0);
            }
            let origin = (c.x - shift.0, c.y - shift.1);
            for (view, name) in [(MapView::Mini, "mini"), (MapView::Full, "full")] {
                // Only the probes, as the HUD reads them back.
                let t = std::time::Instant::now();
                let views: Vec<FrameView> = locate::probes(&layout, view)
                    .into_iter()
                    .filter(|&(x, y, w, h)| frame.covers(x + origin.0, y + origin.1, w, h))
                    .map(|(x, y, w, h)| frame.sub(x + origin.0, y + origin.1, w as u32, h as u32))
                    .collect();
                let found = locate::container(&views, origin, &layout, view);
                let us = t.elapsed().as_secs_f64() * 1e6;
                let e = seen.entry((g.dpi, cw, ch, name)).or_default();
                e.0 += 1;
                e.2.push(us);
                if let Some(b) = found {
                    e.1.push(b);
                }
            }
        }
    }
    let med = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
    };
    for ((dpi, w, h, name), (tries, found, us)) in seen {
        let rule = UiLayout { width: w, height: h, dpi }.map_container(if name == "mini" { MapView::Mini } else { MapView::Full });
        let n = found.len();
        let m = (med(found.iter().map(|b| b.0).collect()), med(found.iter().map(|b| b.1).collect()), med(found.iter().map(|b| b.2).collect()));
        let spread = found.iter().map(|b| (b.0 - m.0).abs().max((b.1 - m.1).abs()).max((b.2 - m.2).abs())).fold(0.0, f64::max);
        println!(
            "{}% {w}x{h} {name}: {n}/{tries} found, box ({:.1}, {:.1}, {:.1}) rule ({:.1}, {:.1}, {:.1}) diff ({:+.1}, {:+.1}, {:+.1}), spread {spread:.1}, {:.0} us/try",
            dpi * 100 / 96, m.0, m.1, m.2, rule.0, rule.1, rule.2, m.0 - rule.0, m.1 - rule.1, m.2 - rule.2, med(us)
        );
    }
}
