//! The yellow X reader on recorded frames: `impact_eval [session…]`.
//!
//! Every frame of the given sessions (all local sessions when none is given) is read in all three
//! map containers; frames with impact labels are scored (found, position error), every other
//! sighting is listed as a false one, with the time per container read.

use std::time::Instant;

use mtc_dataset::{Session, datasets_dir, image::RgbImage};
use mtc_vision::FrameView;
use mtc_vision::map::impacts::read_impacts;
use mtc_vision::map::{MapTransform, MapView};

fn main() {
    let wanted: Vec<String> = std::env::args().skip(1).collect();
    let raw = datasets_dir().join("raw");
    let mut sessions: Vec<_> = std::fs::read_dir(&raw).expect("raw").filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    sessions.sort();
    let (mut frames, mut reads, mut false_hits) = (0usize, 0usize, 0usize);
    let mut spent = std::time::Duration::ZERO;
    let mut errors: Vec<f64> = Vec::new();
    let (mut labelled, mut found) = (0usize, 0usize);
    for id in sessions.iter().filter(|s| wanted.is_empty() || wanted.contains(s)) {
        let Ok(session) = Session::open(raw.join(id)) else { continue };
        let labels: std::collections::HashMap<String, Vec<(f64, f64)>> = mtc_dataset::labels(id)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|l| Some((l.file, l.impacts?.iter().map(|p| (p.x, p.y)).collect())))
            .collect();
        for meta in &session.frames {
            let Ok(image) = RgbImage::load_png(&session.frame_path(meta)) else { continue };
            let frame = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            frames += 1;
            let mut seen = Vec::new();
            for view in [MapView::Full, MapView::Scope, MapView::Mini] {
                let t = MapTransform::fixed(view, (o.x, o.y));
                let started = Instant::now();
                let s = read_impacts(&frame, &t);
                spent += started.elapsed();
                reads += 1;
                seen.extend(s.into_iter().map(|s| (view, s)));
            }
            let truth = labels.get(&meta.file);
            for &(x, y) in truth.into_iter().flatten() {
                labelled += 1;
                let near = seen.iter().map(|(_, s)| (s.frame.0 - x).hypot(s.frame.1 - y)).fold(f64::INFINITY, f64::min);
                if near <= 3.0 {
                    found += 1;
                    errors.push(near);
                } else {
                    println!("missed {id}/{} at ({x:.1}, {y:.1})", meta.file);
                }
            }
            for (view, s) in &seen {
                let expected = truth.into_iter().flatten().any(|&(x, y)| (s.frame.0 - x).hypot(s.frame.1 - y) <= 3.0);
                if !expected {
                    false_hits += 1;
                    println!("false {id}/{} {view:?} ({:.1}, {:.1}) alpha {:.2} arms {:.2}", meta.file, s.frame.0, s.frame.1, s.alpha, s.arms);
                }
            }
        }
        println!("{id}: done");
    }
    errors.sort_by(f64::total_cmp);
    let pick = |q: f64| errors.get(((errors.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(f64::NAN);
    println!(
        "{frames} frames, {reads} container reads ({:.2} ms each); labelled impacts {found}/{labelled} found, error median {:.2} px, max {:.2} px; false sightings {false_hits}",
        spent.as_secs_f64() * 1000.0 / reads.max(1) as f64,
        pick(0.5),
        pick(1.0)
    );
}
