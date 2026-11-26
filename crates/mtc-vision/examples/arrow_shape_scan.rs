//! Shape scores of white blobs: the labelled player arrows against every other white blob in the
//! same frames (any size from 60 px), or every blob in the V1 container of given screenshots.
//!   cargo run --release -p mtc-vision --example arrow_shape_scan
//!   cargo run --release -p mtc-vision --example arrow_shape_scan -- --png [--mini] <frame.png>...
//! Screenshots must be full 2560×1368 frames (title bar included) showing V1 (or the V3 minimap
//! with `--mini`).
use std::collections::HashMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::labels::FrameLabel;
use mtc_dataset::{Session, datasets_dir, labels};
use mtc_vision::FrameView;
use mtc_vision::map::arrow::{ARROW_AREA, ARROW_SIDE, WhiteBlob, template, white_blobs_sized};
use mtc_vision::map::{CueReader, MapTransform, MapView};

const SESSIONS: [&str; 2] = ["20261117-014241", "20261117-160854"];
const WIDE_AREA: (u32, u32) = (60, 600);
const WIDE_SIDE: (u32, u32) = (10, 45);

fn arrow_sized(b: &WhiteBlob) -> bool {
    let a = b.pixels.len() as u32;
    (ARROW_AREA.0..=ARROW_AREA.1).contains(&a)
        && (ARROW_SIDE.0..=ARROW_SIDE.1).contains(&b.size.0)
        && (ARROW_SIDE.0..=ARROW_SIDE.1).contains(&b.size.1)
}

fn describe(b: &WhiteBlob, score: f32) -> String {
    format!(
        "score {score:.3} at ({:.1}, {:.1}) area {} box {}x{} mounted {} {}",
        b.pivot.0,
        b.pivot.1,
        b.pixels.len(),
        b.size.0,
        b.size.1,
        b.mounted,
        if arrow_sized(b) { "[arrow-sized]" } else { "" }
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tpl = template();
    if args.first().is_some_and(|a| a == "--png") {
        let map_view = if args.iter().any(|a| a == "--mini") { MapView::Mini } else { MapView::Full };
        for path in args[1..].iter().filter(|a| !a.starts_with("--")) {
            let image = RgbImage::load_png(std::path::Path::new(path)).expect("png");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let mut scored: Vec<_> = white_blobs_sized(&view, &MapTransform::fixed(map_view, (0, 34)), WIDE_AREA, WIDE_SIDE)
                .into_iter()
                .map(|b| (tpl.score(&b), b))
                .collect();
            scored.sort_by(|a, b| b.0.total_cmp(&a.0));
            println!("{path}: {} blobs", scored.len());
            for (s, b) in &scored {
                println!("  {}", describe(b, *s));
            }
        }
        return;
    }
    let cues = CueReader::new();
    let mut positives: Vec<(f32, String)> = Vec::new();
    let mut negatives: Vec<(f32, String)> = Vec::new();
    for id in SESSIONS {
        let session = Session::open(datasets_dir().join("raw").join(id)).expect("session");
        let labels: HashMap<String, FrameLabel> = labels(id).expect("labels").into_iter().map(|l| (l.file.clone(), l)).collect();
        for meta in &session.frames {
            let Some(truth) = labels.get(&meta.file).and_then(|l| l.player) else { continue };
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            let Some(primary) = cues.read(&view, (o.x, o.y)).primary() else { continue };
            for b in white_blobs_sized(&view, &MapTransform::fixed(primary, (o.x, o.y)), WIDE_AREA, WIDE_SIDE) {
                let s = tpl.score(&b);
                let text = format!("{id}/{} {primary:?} {}", meta.file, describe(&b, s));
                if (b.pivot.0 - truth.x).hypot(b.pivot.1 - truth.y) <= 1.0 {
                    positives.push((s, text));
                } else {
                    negatives.push((s, text));
                }
            }
        }
    }
    positives.sort_by(|a, b| a.0.total_cmp(&b.0));
    negatives.sort_by(|a, b| b.0.total_cmp(&a.0));
    let q = |v: &[(f32, String)], f: f64| v[((v.len() - 1) as f64 * f) as usize].0;
    println!(
        "positives {}: min {:.3}, 1% {:.3}, median {:.3}, max {:.3}",
        positives.len(),
        positives[0].0,
        q(&positives, 0.01),
        q(&positives, 0.5),
        positives[positives.len() - 1].0
    );
    for (_, t) in positives.iter().take(5) {
        println!("  lowest: {t}");
    }
    println!("negatives {} (area {}-{}, sides {}-{})", negatives.len(), WIDE_AREA.0, WIDE_AREA.1, WIDE_SIDE.0, WIDE_SIDE.1);
    for (_, t) in negatives.iter().take(15) {
        println!("  highest: {t}");
    }
}
