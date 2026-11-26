//! The loading menu against the map view over a session's frames: every frame where either
//! changes, as `t_ms,view,menu` (view V1/V2/V3/-, menu gunner/loader/foot/-), and a count of
//! frames with the menu under each view.
//!   cargo run --release -p mtc-vision --example menu_views <session-id>...
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::loading::{MenuSeat, read_menu};
use mtc_vision::map::{CueReader, MapView};

fn main() {
    let cues = CueReader::new();
    for id in std::env::args().skip(1) {
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        let mut frames: Vec<_> = session.frames.iter().collect();
        frames.sort_by_key(|m| m.t_ms);
        println!("== {id} ({} frames)", frames.len());
        let mut last: Option<(String, String)> = None;
        let mut counts = std::collections::BTreeMap::new();
        for meta in frames {
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            let origin = (o.x, o.y);
            let c = cues.read(&view, origin);
            let v = [(MapView::Full, "V1"), (MapView::Scope, "V2"), (MapView::Mini, "V3")].into_iter().find(|(m, _)| c.shows(*m)).map_or("-", |(_, n)| n).to_owned();
            let menu = read_menu(&view, origin).map_or("-", |m| match m.seat {
                MenuSeat::Gunner => "gunner",
                MenuSeat::Loader => "loader",
                MenuSeat::Dismounted => "foot",
            }).to_owned();
            *counts.entry((v.clone(), menu.clone())).or_insert(0u32) += 1;
            let now = (v, menu);
            if last.as_ref() != Some(&now) {
                println!("{},{},{}", meta.t_ms, now.0, now.1);
                last = Some(now);
            }
        }
        println!("counts {counts:?}");
    }
}
