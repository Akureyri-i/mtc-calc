//! Development: the player's arrow on every map-view frame of recorded sessions,
//! each view read as the HUD reads it at the frame's display configuration (cues placed for it,
//! the container resampled onto the reference one, icons at `Placement::map_icon_scale`). One CSV
//! line per frame with a view: session, file, dpi, width, height, view, found (complete, clipped
//! or none), map position. With `MTC_LOCATE` set, the minimap and full map containers found on
//! screen (`locate::container`, the first found per configuration) replace the rules, as the HUD
//! does once it has taken them.
//!   cargo run --release -p mtc-vision --example arrow_scan -- <session> [...] > arrows.csv
use std::collections::BTreeMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::layout::{Located, Placement, UiLayout, picture_shift, shift_probes};
use mtc_vision::map::arrow::{best_shape_at, player_arrow_candidates_at};
use mtc_vision::map::{CueReader, MapTransform};

fn main() {
    let mut cue_readers: BTreeMap<(u32, u32, u32), CueReader> = BTreeMap::new();
    let locate = std::env::var_os("MTC_LOCATE").is_some();
    let mut found_boxes: BTreeMap<(u32, u32, u32), Located> = BTreeMap::new();
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
            let mut placement = Placement::new((c.x - shift.0, c.y - shift.1), layout);
            if locate && !placement.is_reference() {
                let boxes = found_boxes.entry((g.dpi, cw, ch)).or_insert(Located::NONE);
                for view in [mtc_vision::map::MapView::Mini, mtc_vision::map::MapView::Full] {
                    if boxes.get(view).is_none() {
                        let found = mtc_vision::locate::container(&[frame], placement.origin, &layout, view);
                        match view {
                            mtc_vision::map::MapView::Mini => boxes.mini = found,
                            _ => boxes.full = found,
                        }
                    }
                }
                placement.located = *boxes;
            }
            let cues = cue_readers.entry((g.dpi, cw, ch)).or_insert_with(|| CueReader::for_placement(&placement));
            let Some(v) = cues.read(&frame, placement.origin).primary() else { continue };
            let origin = placement.bottom_origin();
            let t = MapTransform::fixed(v, origin);
            let rect = v.frame_rect(origin);
            let (found, best) = if placement.is_reference() {
                (player_arrow_candidates_at(&frame, &t, 1.0), best_shape_at(&frame, &t, 1.0))
            } else {
                let (x, y, w, h) = placement.map_source(v, rect, (image.width, image.height));
                let remapped = placement.map_remap(v, &frame.sub(x, y, w as u32, h as u32), rect);
                let k = placement.map_icon_scale(v);
                (player_arrow_candidates_at(&remapped.view(), &t, k), best_shape_at(&remapped.view(), &t, k))
            };
            let (kind, at) = match (found.complete, found.clipped.is_empty()) {
                (Some(a), _) => ("complete", format!("{:.4},{:.4}", a.map.0, a.map.1)),
                (None, false) => ("clipped", ",".to_owned()),
                (None, true) => ("none", ",".to_owned()),
            };
            println!("{id},{},{},{cw},{ch},{v:?},{kind},{at},{:.3}", meta.file, g.dpi, best.unwrap_or(-1.0));
        }
    }
}
