//! Development: the view cues on every frame of recorded sessions, read where
//! `CueReader::with_layout` puts them for each frame's display configuration (the client found in
//! the frame as the HUD finds it). One CSV line per frame: session, file, t_ms, dpi, width,
//! height, primary view, ping, M key, scope black, scope lit.
//! `--text-factor F FROM_MS TO_MS`: frames in that span are read as with the text factor F measured
//! on screen (`CueReader::set_text_factor`).
//!   cargo run --release -p mtc-vision --example cue_scan -- <session> [...] [--text-factor F FROM_MS TO_MS] > cues.csv
use std::collections::BTreeMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::layout::{UiLayout, picture_shift, shift_probes};
use mtc_vision::map::CueReader;

fn main() {
    let mut readers: BTreeMap<(u32, u32, u32, bool), CueReader> = BTreeMap::new();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let text = args.iter().position(|a| a == "--text-factor").map(|i| {
        let n = |k: usize| args[i + k].parse::<f64>().expect("--text-factor F FROM_MS TO_MS");
        (n(1) as f32, n(2) as u64, n(3) as u64)
    });
    let ids: Vec<&String> = match args.iter().position(|a| a == "--text-factor") {
        Some(i) => args[..i].iter().chain(&args[i + 4..]).collect(),
        None => args.iter().collect(),
    };
    for id in ids {
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
            let raised = text.filter(|&(_, from, to)| (from..=to).contains(&meta.t_ms)).map(|t| t.0);
            let reader = readers.entry((g.dpi, cw, ch, raised.is_some())).or_insert_with(|| {
                let mut reader = CueReader::with_layout(&layout);
                if let Some(factor) = raised {
                    reader.set_text_factor(factor);
                }
                reader
            });
            let cues = reader.read(&view, origin);
            let primary = cues.primary().map_or("-".to_owned(), |v| format!("{v:?}"));
            println!("{id},{},{},{},{cw},{ch},{primary},{:.3},{:.3},{:.3},{:.3},{}", meta.file, meta.t_ms, g.dpi, cues.ping_score, cues.m_key_score, cues.scope_black, cues.scope_lit, u8::from(cues.remove_hint()));
        }
    }
}
