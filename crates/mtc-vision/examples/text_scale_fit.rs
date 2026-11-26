//! Development: the weapon label's text scale on a recording of another display configuration,
//! mapped to the reference geometry: reads the label on every frame with the
//! reader's templates resampled to each scale of a grid and counts the frames read as `expected`.
//!   cargo run --release -p mtc-vision --example text_scale_fit -- <session> <expected> [from to step]
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::loading::read_weapon_label;
use mtc_vision::text::TextReader;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::open(datasets_dir().join("raw").join(&args[1])).expect("session");
    let expected = &args[2];
    let num = |i: usize, d: f32| args.get(i).map_or(d, |v| v.parse().expect("number"));
    let (from, to, step) = (num(3, 0.80), num(4, 1.20), num(5, 0.02));
    let frames: Vec<(RgbImage, (i32, i32))> = session
        .frames
        .iter()
        .map(|m| {
            let o = m.geometry.client_in_frame();
            (RgbImage::load_png(&session.frame_path(m)).expect("frame"), (o.x, o.y))
        })
        .collect();
    let steps = ((to - from) / step).round() as usize;
    let mut results = Vec::new();
    for i in 0..=steps {
        for j in 0..=steps {
            let scale = (from + step * i as f32, from + step * j as f32);
            let reader = TextReader::with_scales([scale, (1.0, 1.0), (1.0, 1.0)]);
            let (mut right, mut read) = (0, 0);
            for (image, origin) in &frames {
                let view = FrameView::rgb(&image.data, image.width, image.height);
                if let Some(text) = read_weapon_label(&view, *origin, &reader) {
                    read += 1;
                    right += usize::from(&text == expected);
                }
            }
            results.push((right, read, scale));
        }
    }
    results.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (right, read, (sx, sy)) in results.iter().take(12) {
        println!("{sx:.2} {sy:.2}: {right} right of {read} read ({} frames)", frames.len());
    }
}
