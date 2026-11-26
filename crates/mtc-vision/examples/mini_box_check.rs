//! Development: the minimap container the rule gives against the one found on
//! screen (`locate::container`, median of the frames it was found on), per configuration.
//!   cargo run --release -p mtc-vision --example mini_box_check -- <session> [...]
use std::collections::BTreeMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::layout::{Placement, UiLayout, picture_shift, shift_probes};
use mtc_vision::map::MapView;

fn main() {
    let mut found: BTreeMap<(u32, u32, u32), Vec<(f64, f64, f64)>> = BTreeMap::new();
    for id in std::env::args().skip(1) {
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        let mut shift = (0, 0);
        for meta in &session.frames {
            let g = &meta.geometry;
            let (cw, ch) = (g.client.width as u32, g.client.height as u32);
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let frame = FrameView::rgb(&image.data, image.width, image.height);
            let c = g.client_in_frame();
            if image.width > cw {
                let [(bx, by, bw, bh), (rx, ry, rw, rh)] = shift_probes((image.width, image.height));
                shift = picture_shift(&frame.sub(bx, by, bw, bh), &frame.sub(rx, ry, rw, rh)).unwrap_or(shift);
            } else {
                shift = (0, 0);
            }
            let layout = UiLayout { width: cw, height: ch, dpi: g.dpi };
            let placement = Placement::new((c.x - shift.0, c.y - shift.1), layout);
            if let Some(b) = mtc_vision::locate::container(&[frame], placement.origin, &layout, MapView::Mini) {
                found.entry((g.dpi, cw, ch)).or_default().push(b);
            }
        }
    }
    for ((dpi, w, h), boxes) in found {
        let layout = UiLayout { width: w, height: h, dpi };
        let med = |i: usize| {
            let mut v: Vec<f64> = boxes.iter().map(|b| [b.0, b.1, b.2][i]).collect();
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let (l, t, s) = layout.map_container(MapView::Mini);
        let capped = 354.35 * layout.hud_scale() > 0.32 * f64::from(h);
        println!(
            "{dpi} {w}x{h} n={} capped={capped} s={:.4} rule ({l:.1}, {t:.1}, {s:.1}) found ({:.1}, {:.1}, {:.1}) diff ({:+.1}, {:+.1}, {:+.1})",
            boxes.len(),
            layout.hud_scale(),
            med(0),
            med(1),
            med(2),
            med(0) - l,
            med(1) - t,
            med(2) - s
        );
    }
}
