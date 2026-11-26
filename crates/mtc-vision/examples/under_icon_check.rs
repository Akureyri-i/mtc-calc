//! Runs the reader of an X under an objective icon (`read_impact_under`) on recorded frames:
//! `under_icon_check <session> <t_ms> <icon_x> <icon_y> [<t_ms> <icon_x> <icon_y>…]` (frame px).
//! Prints the X found, if any, and the minimap reader's sightings beside it.

use mtc_dataset::{Session, datasets_dir, image::RgbImage};
use mtc_vision::FrameView;
use mtc_vision::map::impacts::{read_impact_under, read_impacts_among};
use mtc_vision::map::{MapTransform, MapView};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let session = Session::open(datasets_dir().join("raw").join(&args[0])).expect("session");
    for chunk in args[1..].chunks(3) {
        let t_ms: u64 = chunk[0].parse().expect("t_ms");
        let icon = (chunk[1].parse::<f64>().expect("x"), chunk[2].parse::<f64>().expect("y"));
        let meta = session.frames.iter().find(|m| m.t_ms == t_ms).expect("frame");
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("png");
        let frame = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let t = MapTransform::fixed(MapView::Mini, (o.x, o.y));
        let started = std::time::Instant::now();
        let under = read_impact_under(&frame, &t, icon, &[icon]);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let among = read_impacts_among(&frame, &t, &[icon]);
        println!("{t_ms}: under {under:?} ({ms:.2} ms); among {among:?}");
    }
}
