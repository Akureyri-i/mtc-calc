//! Development: the view cues on loose frame images (exported video frames or screenshots of the
//! client area alone), read where `CueReader::with_layout` puts them for the given display
//! configuration. One line per image: file, primary view, the primary view by the earlier rules
//! (no spread floor, V1 while scoped allowed), ping, ping with no spread floor, ping window spread
//! / template spread, M key, scope black, scope lit.
//!   cargo run --release -p mtc-vision --example cue_frames -- <dpi> <png> [...]
use mtc_dataset::image::RgbImage;
use mtc_vision::FrameView;
use mtc_vision::layout::UiLayout;
use mtc_vision::map::{CueReader, ViewCues};

/// The primary view as the cues decided it before the spread floor and the scoped rule.
fn earlier_primary(c: &ViewCues, ping: f32, m_key: f32) -> &'static str {
    let mini = m_key >= c.m_key_min;
    if ping >= 0.5 {
        "Full"
    } else if c.scoped() && !mini && c.scope_lit >= 0.55 {
        "Scope"
    } else if mini {
        "Mini"
    } else {
        "-"
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dpi: u32 = args.next().expect("dpi").parse().expect("dpi");
    let mut reader: Option<((u32, u32), CueReader)> = None;
    for path in args {
        let image = RgbImage::load_png(std::path::Path::new(&path)).expect("frame");
        let size = (image.width, image.height);
        if reader.as_ref().is_none_or(|r| r.0 != size) {
            let layout = UiLayout { width: image.width, height: image.height, dpi };
            let r = CueReader::with_layout(&layout);
            eprintln!("regions {:?}", r.regions((0, 0)));
            reader = Some((size, r));
        }
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let r = &reader.as_ref().unwrap().1;
        let views = std::slice::from_ref(&view);
        let cues = r.read_views(views, (0, 0));
        let (ping, spread, template) = r.ping_detail(views, (0, 0));
        let (m_key, _, _) = r.m_key_detail(views, (0, 0));
        let primary = cues.primary().map_or("-".to_owned(), |v| format!("{v:?}"));
        let name = std::path::Path::new(&path).file_name().unwrap().to_string_lossy().into_owned();
        println!(
            "{name},{primary},{},{:.3},{:.3},{spread:.2}/{template:.2},{:.3},{:.3},{:.3}",
            earlier_primary(&cues, ping, m_key),
            cues.ping_score.max(-9.0),
            ping.max(-9.0),
            cues.m_key_score,
            cues.scope_black,
            cues.scope_lit
        );
    }
}
