//! Reads text boxes of one frame with the UI text reader:
//!   cargo run --release -p mtc-vision --example text_probe <png> <style weapon|name|value> <x0> <y0> <x1> <y1> [min_ink]
use mtc_dataset::image::RgbImage;
use mtc_vision::FrameView;
use mtc_vision::text::{TextReader, TextStyle};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let image = RgbImage::load_png(std::path::Path::new(&a[1])).expect("png");
    let view = FrameView::rgb(&image.data, image.width, image.height);
    let style = match a[2].as_str() {
        "weapon" => TextStyle::Weapon,
        "name" => TextStyle::Name,
        _ => TextStyle::Value,
    };
    let n = |i: usize| a[i].parse::<i32>().expect("number");
    let min_ink = a.get(7).map_or(140, |s| s.parse().expect("min ink"));
    let reader = TextReader::new();
    let started = std::time::Instant::now();
    let text = reader.read(&view, style, (n(3), n(4), n(5), n(6)), min_ink);
    println!("{text:?}  ({} us)", started.elapsed().as_micros());
}
