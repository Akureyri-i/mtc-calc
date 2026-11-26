//! Development: the objective letters on every map-view frame of recorded sessions,
//! each view read as the HUD reads it at the frame's display configuration (cues placed for it,
//! the container resampled onto the reference one, icons at `Placement::map_icon_scale`). One CSV
//! line per letter blob that passed the size and fill checks: session, file, dpi, width, height,
//! view, best letter and overlap, second letter and overlap, map position.
//!   cargo run --release -p mtc-vision --example objective_scan -- <session> [...] > letters.csv
use std::collections::BTreeMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::layout::{Placement, UiLayout, picture_shift, shift_probes};
use mtc_vision::map::objectives::ObjectiveReader;
use mtc_vision::map::{CueReader, MapTransform};

fn main() {
    let reader = ObjectiveReader::new();
    let mut cue_readers: BTreeMap<(u32, u32, u32), CueReader> = BTreeMap::new();
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
            let placement = Placement::new((c.x - shift.0, c.y - shift.1), UiLayout { width: cw, height: ch, dpi: g.dpi });
            let cues = cue_readers.entry((g.dpi, cw, ch)).or_insert_with(|| CueReader::for_placement(&placement));
            let Some(v) = cues.read(&frame, placement.origin).primary() else { continue };
            let origin = placement.bottom_origin();
            let t = MapTransform::fixed(v, origin);
            let rect = v.frame_rect(origin);
            let scores = if placement.is_reference() {
                reader.scored_at(&frame, &t, 1.0)
            } else {
                let (x, y, w, h) = placement.map_source(v, rect, (image.width, image.height));
                let remapped = placement.map_remap(v, &frame.sub(x, y, w as u32, h as u32), rect);
                reader.scored_at(&remapped.view(), &t, placement.map_icon_scale(v))
            };
            for s in scores {
                let (u, w) = t.to_map(s.frame.0, s.frame.1);
                println!("{id},{},{},{cw},{ch},{v:?},{},{:.3},{},{:.3},{u:.4},{w:.4}", meta.file, g.dpi, s.best.0, s.best.1, s.second.0, s.second.1);
            }
        }
    }
}
