//! Development: the aim strip as the HUD draws it live, one PNG per logged readout, for a video
//! that follows a recorded aim: promo_strip <readings.json> <out dir> <scale> <target el> <target az> <tol el> <tol az>
//! `readings.json` holds [t_ms, elevation, azimuth] rows; the PNGs are `strip_0000.png`… in
//! that order, straight (not premultiplied) RGBA.
use std::path::Path;

use mtc_overlay::{AimRow, AimStrip, Axis, render_offscreen};

fn roblox_font_dir() -> std::path::PathBuf {
    let versions = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Roblox/Versions");
    std::fs::read_dir(versions)
        .expect("Roblox versions")
        .filter_map(Result::ok)
        .map(|e| e.path().join("content/fonts"))
        .filter(|p| p.join("Montserrat-Medium.ttf").is_file())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .expect("Montserrat in a Roblox install")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let text = std::fs::read_to_string(&args[0]).expect("readings");
    let numbers: Vec<f64> = text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).filter(|s| !s.is_empty()).map(|s| s.parse().expect("number")).collect();
    let out = Path::new(&args[1]);
    std::fs::create_dir_all(out).expect("out dir");
    let n = |i: usize| args[i].parse::<f64>().expect("number");
    let (scale, target_el, target_az, tol_el, tol_az) = (n(2) as f32, n(3), n(4), n(5), n(6));
    // Where the game's readout lines would be: elevation label and value, azimuth label and value.
    let at = |y: f32| (y * scale).round() as i32;
    let fonts = roblox_font_dir();
    for (i, row) in numbers.chunks(3).enumerate() {
        let (el, az) = (row[1], row[2]);
        let strip = AimStrip {
            right: at(320.0),
            rows: [
                AimRow { axis: Axis::Elevation, current: el, target: target_el, tolerance: tol_el, label_y: at(30.0), value_y: at(52.0), beyond_reach: false },
                AimRow { axis: Axis::Azimuth, current: az, target: target_az, tolerance: tol_az, label_y: at(82.0), value_y: at(104.0), beyond_reach: false },
            ],
            mirror: false,
            scale,
        };
        let layer = render_offscreen(&fonts, Some(&strip), None, None).expect("render").remove(0);
        let mut rgba = Vec::with_capacity(layer.bgra.len());
        for px in layer.bgra.chunks(4) {
            let a = u32::from(px[3]);
            let straight = |c: u8| if a == 0 { 0 } else { ((u32::from(c) * 255 + a / 2) / a).min(255) as u8 };
            rgba.extend_from_slice(&[straight(px[2]), straight(px[1]), straight(px[0]), px[3]]);
        }
        let file = std::fs::File::create(out.join(format!("strip_{i:04}.png"))).expect("png");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), layer.width, layer.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().expect("header").write_image_data(&rgba).expect("data");
        if i == 0 {
            println!("{} x {}", layer.width, layer.height);
        }
    }
}
