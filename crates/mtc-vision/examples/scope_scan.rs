//! Development: every recorded frame the view cues call V2, with how much of a row through the
//! middle of the V2 container is not black (a black screen — death, loading — has the scope's black
//! surround and no minimap box, like V2, but no map in the container). CSV on stdout:
//! `session,file,lit_fraction,labelled_view`.
//!   cargo run --release -p mtc-vision --example scope_scan
use std::collections::HashMap;

use mtc_dataset::eval::labelled_frames;
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir, sessions};
use mtc_vision::FrameView;
use mtc_vision::map::{CueReader, MapView};

fn main() {
    let mut labelled: HashMap<(String, String), String> = HashMap::new();
    for f in labelled_frames().expect("dataset") {
        if let Some(m) = &f.label.map {
            labelled.insert((f.session.clone(), f.meta.file.clone()), format!("{:?}", m.view));
        }
    }
    let cues = CueReader::new();
    let c = MapView::Scope.container();
    println!("session,file,lit_fraction,labelled_view");
    for info in sessions().expect("sessions") {
        let id = info.info.id.clone();
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        for meta in &session.frames {
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            if cues.read(&view, (o.x, o.y)).primary() != Some(MapView::Scope) {
                continue;
            }
            let y = (c.y + c.size / 2.0).round() as i32 + o.y;
            let (x0, x1) = ((c.x + 8.0).round() as i32 + o.x, (c.x + c.size - 8.0).round() as i32 + o.x);
            let lit = (x0..x1).filter(|&x| view.px(x, y).iter().copied().max().unwrap_or(0) > 12).count();
            let label = labelled.get(&(id.clone(), meta.file.clone())).cloned().unwrap_or_default();
            println!("{id},{},{:.3},{label}", meta.file, lit as f64 / f64::from(x1 - x0));
        }
    }
}
