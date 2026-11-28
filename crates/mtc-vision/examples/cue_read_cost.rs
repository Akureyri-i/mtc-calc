//! Development: how long reading the view cues takes on one frame, and the marker hint's
//! translucency alone, averaged over many reads.
//!
//!     cargo run --release -p mtc-vision --example cue_read_cost <session> <frame stem>

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::layout::UiLayout;
use mtc_vision::map::CueReader;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let session = Session::open(&datasets_dir().join("raw").join(&args[0])).expect("session");
    let meta = session.frames.iter().find(|m| m.file.ends_with(&format!("{}.png", args[1]))).expect("frame");
    let g = &meta.geometry;
    let reader = CueReader::with_layout(&UiLayout { width: g.client.width as u32, height: g.client.height as u32, dpi: g.dpi });
    let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
    let c = g.client_in_frame();
    let views = [FrameView::rgb(&image.data, image.width, image.height)];
    let n = 5000;
    let started = std::time::Instant::now();
    let mut seen = 0;
    for _ in 0..n {
        seen += usize::from(reader.read_views(&views, (c.x, c.y)).hint_dimmed.is_some());
    }
    let all = started.elapsed().as_secs_f64() * 1e6 / f64::from(n);
    let started = std::time::Instant::now();
    for _ in 0..n {
        seen += usize::from(reader.hint_translucency(&views, (c.x, c.y)).is_some());
    }
    let hint = started.elapsed().as_secs_f64() * 1e6 / f64::from(n);
    println!("cue read {all:.1} us per frame, of which the hint's translucency {hint:.1} us ({seen} decided)");
}
