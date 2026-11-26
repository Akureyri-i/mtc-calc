//! Development: the view cues on every frame of recorded sessions, read where
//! `CueReader::with_layout` puts them for each frame's display configuration (the client found in
//! the frame as the HUD finds it). One CSV line per frame: session, file, t_ms, dpi, width,
//! height, primary view, ping, M key, scope black, scope lit.
//!   cargo run --release -p mtc-vision --example cue_scan -- <session> [...] > cues.csv
use std::collections::BTreeMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::layout::{UiLayout, picture_shift, shift_probes};
use mtc_vision::map::CueReader;

fn main() {
    let mut readers: BTreeMap<(u32, u32, u32), CueReader> = BTreeMap::new();
    for id in std::env::args().skip(1) {
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        let mut shift = (0, 0);
        for meta in &session.frames {
            let g = &meta.geometry;
            let (cw, ch) = (g.client.width as u32, g.client.height as u32);
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let c = g.client_in_frame();
            if image.width > cw {
                let [(bx, by, bw, bh), (rx, ry, rw, rh)] = shift_probes((image.width, image.height));
                shift = picture_shift(&view.sub(bx, by, bw, bh), &view.sub(rx, ry, rw, rh)).unwrap_or(shift);
            } else {
                shift = (0, 0);
            }
            let origin = (c.x - shift.0, c.y - shift.1);
            let layout = UiLayout { width: cw, height: ch, dpi: g.dpi };
            let reader = readers.entry((g.dpi, cw, ch)).or_insert_with(|| CueReader::with_layout(&layout));
            let cues = reader.read(&view, origin);
            let primary = cues.primary().map_or("-".to_owned(), |v| format!("{v:?}"));
            println!("{id},{},{},{},{cw},{ch},{primary},{:.3},{:.3},{:.3},{:.3}", meta.file, meta.t_ms, g.dpi, cues.ping_score, cues.m_key_score, cues.scope_black, cues.scope_lit);
        }
    }
}
