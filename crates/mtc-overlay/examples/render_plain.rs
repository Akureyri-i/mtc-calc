//! Development: strings drawn with DirectWrite as `mtc_overlay::render_text_plain`
//! draws them, one grey PNG each, for fitting how the game draws its UI text at other display
//! configurations. One job per stdin line:
//!   out.png|text|size|weight|mode (0 values, 1 labels, 2 no grid fitting)|x|width|height
use std::io::BufRead;

use mtc_dataset::image::RgbImage;
use mtc_overlay::{PlainText, render_text_plain};

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
    let lines: Vec<String> = std::io::stdin().lock().lines().map_while(Result::ok).filter(|l| !l.trim().is_empty()).collect();
    let jobs: Vec<Vec<String>> = lines.iter().map(|l| l.split('|').map(str::to_owned).collect()).collect();
    let texts: Vec<PlainText> = jobs
        .iter()
        .map(|j| PlainText {
            text: &j[1],
            size: j[2].parse().expect("size"),
            weight: j[3].parse().expect("weight"),
            mode: j[4].parse().expect("mode"),
            x: j[5].parse().expect("x"),
            width: j[6].parse().expect("width"),
            height: j[7].parse().expect("height"),
        })
        .collect();
    let images = render_text_plain(&roblox_font_dir(), &texts).expect("render");
    for (j, (t, bgra)) in jobs.iter().zip(texts.iter().zip(images)) {
        let grey: Vec<u8> = bgra.chunks(4).flat_map(|p| [p[0], p[0], p[0]]).collect();
        RgbImage { width: t.width, height: t.height, data: grey }.save_png(std::path::Path::new(&j[0])).expect("save");
    }
}
