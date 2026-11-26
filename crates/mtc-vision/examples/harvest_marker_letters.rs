//! Letter templates for marker labels, harvested from recorded V1 frames whose markers were read
//! by eye -> crates/mtc-vision/assets/marker_letters.json
//!   cargo run --release -p mtc-vision --example harvest_marker_letters
//!
//! One recording, 289–318 s (Roinburg, V1 unzoomed, map still): a squad of three, the recording
//! player third — A11–A16, B21–B26 (B23, B24 under other labels and left out), C1–C4. Each marker
//! is found where it was read; the first glyph of its label is averaged per letter at the most
//! common width. Rebuild the crate afterwards (the asset is compiled in).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::map::markers::MarkerReader;
use mtc_vision::map::{MapTransform, MapView};

const SESSION: &str = "20261120-000720";
const FROM_MS: u64 = 289_000;
const TO_MS: u64 = 318_000;
/// Marked points (frame px) and letters, read by eye on frames/000293854.png.
const MARKERS: [(char, f64, f64); 14] = [
    ('A', 1379.0, 582.0),
    ('A', 1325.0, 454.0),
    ('A', 1301.0, 435.0),
    ('A', 1181.0, 629.0),
    ('A', 1447.0, 582.0),
    ('A', 1487.0, 379.0),
    ('B', 1495.0, 732.0),
    ('B', 1595.0, 794.0),
    ('B', 1546.0, 639.0),
    ('B', 1606.0, 525.0),
    ('C', 1236.0, 819.0),
    ('C', 1188.0, 961.0),
    ('C', 1404.0, 1015.0),
    ('C', 1020.0, 835.0),
];
const FIND_PX: f64 = 3.0;
const HEIGHT: usize = 11;

fn main() {
    let session = Session::open(datasets_dir().join("raw").join(SESSION)).expect("session");
    let reader = MarkerReader::new();
    let mut samples: BTreeMap<char, Vec<(usize, Vec<f32>)>> = BTreeMap::new();
    let mut frames = 0;
    for meta in session.frames.iter().filter(|m| (FROM_MS..=TO_MS).contains(&m.t_ms)) {
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("png");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let found = reader.read(&view, &MapTransform::fixed(MapView::Full, (o.x, o.y)));
        frames += 1;
        for &(letter, x, y) in &MARKERS {
            let Some(m) = found.iter().find(|m| m.complete && (m.frame.0 - x).hypot(m.frame.1 - y) <= FIND_PX) else { continue };
            let Some(glyphs) = reader.label_glyphs(&view, m.frame) else { continue };
            if glyphs.len() >= 2 {
                samples.entry(letter).or_default().push(glyphs[0].clone());
            }
        }
    }
    let mut json = format!("{{\n  \"source\": \"{SESSION} {}-{} s, {frames} frames (examples/harvest_marker_letters.rs)\",\n  \"height\": {HEIGHT},\n  \"glyphs\": {{\n", FROM_MS / 1000, TO_MS / 1000);
    let mut first = true;
    for (letter, list) in &samples {
        let mut widths: BTreeMap<usize, usize> = BTreeMap::new();
        for (w, _) in list {
            *widths.entry(*w).or_default() += 1;
        }
        let width = widths.iter().max_by_key(|(_, n)| **n).map(|(w, _)| *w).expect("samples");
        let kept: Vec<&Vec<f32>> = list.iter().filter(|(w, _)| *w == width).map(|(_, g)| g).collect();
        let mut mean = vec![0.0f32; width * HEIGHT];
        for g in &kept {
            for (m, v) in mean.iter_mut().zip(g.iter()) {
                *m += v / kept.len() as f32;
            }
        }
        eprintln!("{letter}: {} samples, width {width} ({} kept; widths {widths:?})", list.len(), kept.len());
        let rows: Vec<String> = mean.iter().map(|v| format!("{v:.1}")).collect();
        if !first {
            json.push_str(",\n");
        }
        first = false;
        let _ = write!(json, "    \"{letter}\": {{\"width\": {width}, \"samples\": {}, \"rows\": [{}]}}", kept.len(), rows.join(", "));
    }
    json.push_str("\n  }\n}\n");
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/marker_letters.json");
    std::fs::write(&out, json).expect("write asset");
    eprintln!("wrote {}", out.display());
}
