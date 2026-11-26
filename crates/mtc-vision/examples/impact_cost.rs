//! Development: time of the yellow X reader on the minimap of a session's frames in a time window,
//! with the sightings, to find frames that cost much.
//!   cargo run --release -p mtc-vision --example impact_cost <session> <from-ms> <to-ms>
use mtc_dataset::{Session, datasets_dir, image::RgbImage};
use mtc_vision::FrameView;
use mtc_vision::map::impacts::read_impacts;
use mtc_vision::map::{MapTransform, MapView};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::open(datasets_dir().join("raw").join(&args[1])).expect("session");
    let (from, to): (u64, u64) = (args[2].parse().expect("from"), args[3].parse().expect("to"));
    for meta in session.frames.iter().filter(|m| (from..=to).contains(&m.t_ms)) {
        let Ok(image) = RgbImage::load_png(&session.frame_path(meta)) else { continue };
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let t = MapTransform::fixed(MapView::Mini, (o.x, o.y));
        let started = std::time::Instant::now();
        let found = read_impacts(&view, &t);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        println!("{} {:.1} ms {:?}", meta.file, ms, found.iter().map(|s| (s.frame.0.round(), s.frame.1.round())).collect::<Vec<_>>());
    }
}
