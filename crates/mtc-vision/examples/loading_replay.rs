//! Loading state over a session in time order: prints the tracker's events (loading started,
//! loaded with slot and charge).
//!   cargo run --release -p mtc-vision --example loading_replay <session-id>
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};
use mtc_vision::bar::BarReader;
use mtc_vision::loading::{countdown_shown, gunner_bar_highlight, read_loader_bar, read_menu};
use mtc_vision::loading_state::{LoadingFrame, LoadingTracker};

fn main() {
    let id = std::env::args().nth(1).expect("session id");
    let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
    let mut frames: Vec<_> = session.frames.iter().collect();
    frames.sort_by_key(|m| m.t_ms);
    let mut aim = AimReader::new();
    let bar = BarReader::new();
    let mut tracker = LoadingTracker::default();
    for meta in frames {
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let origin = (o.x, o.y);
        let menu = read_menu(&view, origin);
        let loader_bar = read_loader_bar(&view, origin);
        let label_x = match aim.read(&view, origin) {
            AimResult::Read(r) => Some(r.label_x),
            AimResult::Unreadable { label_x, .. } => Some(label_x),
            AimResult::Absent => None,
        };
        let gunner_slot = label_x.and_then(|x| bar.layout(&view, origin, x)).and_then(|l| gunner_bar_highlight(&view, origin, &l));
        let frame = LoadingFrame { t_ms: meta.t_ms, menu: menu.as_ref(), countdown: countdown_shown(&view, origin), progress: None, gunner_slot, loader_bar: loader_bar.as_ref() };
        for e in tracker.observe(&frame) {
            println!("{:>8.1}s {} {:?}   charges {:?}", meta.t_ms as f64 / 1000.0, meta.file, e, tracker.charges());
        }
    }
    println!("final loaded {:?}", tracker.loaded());
}
