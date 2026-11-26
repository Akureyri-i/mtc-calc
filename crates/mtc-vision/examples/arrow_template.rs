//! Harvests the player-arrow template from the player-labelled frames: every labelled arrow is
//! laid over a grid in its own frame (pivot at the origin, heading up) and the occupancy is
//! averaged.
//!   cargo run --release -p mtc-vision --example arrow_template -- crates/mtc-vision/assets/arrow_template.json
use std::collections::HashMap;
use std::fmt::Write as _;

use mtc_dataset::image::RgbImage;
use mtc_dataset::labels::FrameLabel;
use mtc_dataset::{Session, datasets_dir, labels};
use mtc_vision::FrameView;
use mtc_vision::map::arrow::{ArrowTemplate, white_blobs};
use mtc_vision::map::{CueReader, MapTransform};

const SESSIONS: [&str; 2] = ["20261117-014241", "20261117-160854"];
const CELL: f64 = 0.5;
const HALF: usize = 32;
/// The labelled pivot and the blob's pivot must agree this closely (frame px).
const MATCH_PX: f64 = 1.0;

fn main() {
    let out = std::env::args().nth(1).expect("output path");
    let grid = ArrowTemplate { cell: CELL, half: HALF, values: vec![0.0; 4 * HALF * HALF] };
    let cues = CueReader::new();
    let mut sum = vec![0u32; grid.values.len()];
    let mut used = 0u32;
    let mut per_view: HashMap<String, u32> = HashMap::new();
    for id in SESSIONS {
        let session = Session::open(datasets_dir().join("raw").join(id)).expect("session");
        let labels: HashMap<String, FrameLabel> = labels(id).expect("labels").into_iter().map(|l| (l.file.clone(), l)).collect();
        for meta in &session.frames {
            let Some(truth) = labels.get(&meta.file).and_then(|l| l.player) else { continue };
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            let origin = (o.x, o.y);
            let Some(primary) = cues.read(&view, origin).primary() else {
                println!("{id}/{}: no map view", meta.file);
                continue;
            };
            let blobs = white_blobs(&view, &MapTransform::fixed(primary, origin));
            let Some(blob) = blobs.iter().find(|b| (b.pivot.0 - truth.x).hypot(b.pivot.1 - truth.y) <= MATCH_PX) else {
                println!("{id}/{}: no blob at the labelled pivot", meta.file);
                continue;
            };
            for (s, o) in sum.iter_mut().zip(grid.occupancy(blob)) {
                *s += u32::from(o);
            }
            used += 1;
            *per_view.entry(format!("{primary:?}")).or_default() += 1;
        }
    }
    let values: Vec<f32> = sum.iter().map(|&s| s as f32 / used as f32).collect();
    let mut json = format!("{{\"cell\": {CELL}, \"half\": {HALF}, \"frames\": {used}, \"values\": [");
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            json.push_str(", ");
        }
        write!(json, "{}", (v * 1000.0).round() / 1000.0).unwrap();
    }
    json.push_str("]}\n");
    std::fs::write(&out, json).expect("write template");
    println!("{used} arrows {per_view:?} -> {out}");
    let side = 2 * HALF;
    for row in (0..side).step_by(2) {
        let line: String = (0..side)
            .map(|col| match values[row * side + col] {
                v if v >= 0.9 => '#',
                v if v >= 0.5 => '+',
                v if v >= 0.1 => '.',
                _ => ' ',
            })
            .collect();
        if line.trim().is_empty() {
            continue;
        }
        println!("{line}");
    }
}
