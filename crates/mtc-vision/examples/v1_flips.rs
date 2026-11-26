//! Development: the V1 registrar over every V1 frame of the recordings in time order (as the live
//! HUD runs it), printing each frame's zoom and method and flagging short-lived zooms (a zoom held
//! for at most `FLIP_FRAMES` frames between two equal ones) — the registrar jumping away and back.
//!   cargo run --release -p mtc-vision --example v1_flips [session ...]
use std::collections::HashMap;

use mtc_dataset::eval::labelled_frames;
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::map::zoom::V1Registrar;
use mtc_vision::map::{CueReader, MapReference, MapView};

const FLIP_FRAMES: usize = 3;

fn main() {
    let wanted: Vec<String> = std::env::args().skip(1).collect();
    // Map of each labelled frame, by session and time.
    let mut maps: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    for f in labelled_frames().expect("dataset") {
        if let Some(m) = &f.label.map {
            maps.entry(f.session.clone()).or_default().push((f.meta.t_ms as u64, m.id.clone()));
        }
    }
    let cues = CueReader::new();
    let mut references: HashMap<String, MapReference> = HashMap::new();
    let mut sessions: Vec<_> = maps.keys().cloned().collect();
    sessions.sort();
    for id in sessions.into_iter().filter(|s| wanted.is_empty() || wanted.contains(s)) {
        let mut labels = maps[&id].clone();
        labels.sort();
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        let mut metas: Vec<_> = session.frames.iter().collect();
        metas.sort_by_key(|m| m.t_ms);
        let mut registrars: HashMap<String, V1Registrar> = HashMap::new();
        let mut seq: Vec<(u64, String, String, f64)> = Vec::new();
        for meta in metas {
            let t = meta.t_ms as u64;
            // The map of the labelled frames on both sides, when they agree.
            let before = labels.iter().rev().find(|(lt, _)| *lt <= t).map(|(_, m)| m);
            let after = labels.iter().find(|(lt, _)| *lt >= t).map(|(_, m)| m);
            let map_id = match (before, after) {
                (Some(a), Some(b)) if a == b => a.clone(),
                (Some(a), None) | (None, Some(a)) => a.clone(),
                _ => continue,
            };
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            if !cues.read(&view, (o.x, o.y)).shows(MapView::Full) {
                continue;
            }
            let reference = references.entry(map_id.clone()).or_insert_with(|| {
                let info = mtc_maps::by_id(&map_id).expect("map");
                let img = mtc_maps::load_reference(info).expect("reference");
                MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data)
            });
            let registrar = registrars.entry(map_id.clone()).or_insert_with(|| V1Registrar::new(reference, mtc_maps::by_id(&map_id).and_then(|m| m.grid_cell_m)));
            let cursor = meta.cursor.map(|(x, y)| (f64::from(x) + 0.5, f64::from(y) + 0.5));
            let (method, zoom) = match registrar.register(&view, (o.x, o.y), cursor) {
                Some(r) => (format!("{:?}", r.method), r.transform.zoom),
                None => ("none".to_owned(), f64::NAN),
            };
            seq.push((t, meta.file.clone(), method, zoom));
        }
        let mut flips = 0;
        for i in 1..seq.len() {
            // A run of a different zoom starting at i, ended by a return to the zoom before it.
            let z0 = seq[i - 1].3;
            if (seq[i].3 - z0).abs() < 1e-6 || seq[i].3.is_nan() && z0.is_nan() {
                continue;
            }
            if let Some(back) = (i + 1..(i + 1 + FLIP_FRAMES).min(seq.len())).find(|&j| (seq[j].3 - z0).abs() < 1e-6)
                && (i..back).all(|k| (seq[k].3 - z0).abs() >= 1e-6)
            {
                flips += 1;
                let run: Vec<String> = (i - 1..=back).map(|k| format!("{} {:.1}×{} @{}", seq[k].1, seq[k].3, seq[k].2, seq[k].0)).collect();
                println!("{id} flip: {}", run.join("  →  "));
            }
        }
        let nones = seq.iter().filter(|s| s.3.is_nan()).count();
        println!("{id}: {} V1 frames, {flips} flips, {nones} unregistered", seq.len());
    }
}
