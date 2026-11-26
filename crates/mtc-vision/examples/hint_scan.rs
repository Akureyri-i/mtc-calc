//! V1 marker hint over a session: t_ms,cursor_x,cursor_y,remove_lead,remove
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::map::{CueReader, MapView};

fn main() {
    let id = std::env::args().nth(1).expect("session id");
    let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
    let cues = CueReader::new();
    let mut frames: Vec<_> = session.frames.iter().collect();
    frames.sort_by_key(|m| m.t_ms);
    println!("t_ms,cursor_x,cursor_y,remove_lead,remove");
    for meta in frames {
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let c = cues.read(&view, (o.x, o.y));
        if !c.shows(MapView::Full) {
            continue;
        }
        let (cx, cy) = meta.cursor.unwrap_or((-1, -1));
        println!("{},{cx},{cy},{:.3},{}", meta.t_ms, c.remove_hint_lead, u8::from(c.remove_hint()));
    }
}
