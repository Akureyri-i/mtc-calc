//! Markers covered by objective icons, on real frames: real objective icons (cut from unzoomed
//! Fulvia Gap frames at objective A's table position, orange and green) are pasted over every
//! labelled marker of the labelled full-map and scope-map frames at several offsets.
//!
//! - Diamond covered (label visible): the marker must be found by its label, with the right
//!   number (letter `?` when the icon covers it), within `LABEL_TOLERANCE` (unzoomed px) of the
//!   labelled point.
//! - Letter covered (diamond whole): the diamond must be found within `DIAMOND_TOLERANCE`.
//!
//! Markers within `REAL_OBJECTIVE_PX` of a real objective icon are left out (already covered).
//! Needs the recorded frames (datasets/raw).

use mtc_dataset::eval::{Tally, labelled_frames};
use mtc_dataset::image::RgbImage;
use mtc_dataset::labels::MapView as LabelView;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::map::markers::MarkerReader;
use mtc_vision::map::{MapTransform, MapView};

/// Label-placed point to labelled point, in unzoomed px (frame px / zoom).
const LABEL_TOLERANCE: f64 = 2.0;
/// Diamond-placed point to labelled point, frame px.
const DIAMOND_TOLERANCE: f64 = 1.0;
/// Objective icon: 41 px diamond, half-diagonal 20.5 px.
const ICON: usize = 41;
const REAL_OBJECTIVE_PX: f64 = 70.0;

/// Objective A of Fulvia Gap in two colours: session and frame it is cut from.
const ICONS: [(&str, &str, &str); 2] = [("orange icon", "20261117-014241", "frames/000302041.png"), ("green icon", "20261117-222558", "frames/000127235.png")];

/// The icon pixels and mask, cut from a real frame around objective A of Fulvia Gap.
fn objective_icon(session_id: &str, file: &str) -> (Vec<u8>, Vec<bool>) {
    let session = Session::open(datasets_dir().join("raw").join(session_id)).expect("session");
    let meta = session.frames.iter().find(|m| m.file == file).expect("frame");
    let image = RgbImage::load_png(&session.frame_path(meta)).expect("png");
    let a = mtc_maps::objectives("fulvia_gap").iter().find(|o| o.letter == "A").expect("objective A");
    let o = meta.geometry.client_in_frame();
    let t = MapTransform::fixed(MapView::Full, (o.x, o.y));
    let (cx, cy) = t.to_frame(a.u, a.v);
    let (x0, y0) = ((cx - ICON as f64 / 2.0).round() as usize, (cy - ICON as f64 / 2.0).round() as usize);
    let mut rgb = Vec::with_capacity(ICON * ICON * 3);
    let mut mask = Vec::with_capacity(ICON * ICON);
    for y in 0..ICON {
        for x in 0..ICON {
            let i = ((y0 + y) * image.width as usize + x0 + x) * 3;
            rgb.extend_from_slice(&image.data[i..i + 3]);
            let l1 = (x as f64 + 0.5 - ICON as f64 / 2.0).abs() + (y as f64 + 0.5 - ICON as f64 / 2.0).abs();
            mask.push(l1 <= ICON as f64 / 2.0);
        }
    }
    (rgb, mask)
}

fn paste(image: &mut RgbImage, icon: &(Vec<u8>, Vec<bool>), centre: (f64, f64)) {
    let (x0, y0) = ((centre.0 - ICON as f64 / 2.0).round() as i64, (centre.1 - ICON as f64 / 2.0).round() as i64);
    for y in 0..ICON {
        for x in 0..ICON {
            let (fx, fy) = (x0 + x as i64, y0 + y as i64);
            if icon.1[y * ICON + x] && fx >= 0 && fy >= 0 && (fx as u32) < image.width && (fy as u32) < image.height {
                let i = (fy as usize * image.width as usize + fx as usize) * 3;
                image.data[i..i + 3].copy_from_slice(&icon.0[(y * ICON + x) * 3..(y * ICON + x) * 3 + 3]);
            }
        }
    }
}

fn percentile(values: &mut [f64], q: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(((values.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(f64::NAN)
}

#[test]
fn covered_markers_are_found_by_label() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let icons: Vec<(&str, (Vec<u8>, Vec<bool>))> = ICONS.iter().map(|&(name, session, file)| (name, objective_icon(session, file))).collect();
    let reader = MarkerReader::new();
    // Icon centre relative to the marked point (frame px); `true` = the label must place it.
    let cases: [(&str, (f64, f64), bool); 4] = [
        ("diamond covered", (0.0, 0.0), true),
        ("diamond covered, icon off-centre", (3.0, -2.0), true),
        ("diamond and letter covered", (4.0, -8.0), true),
        ("letter covered, diamond whole", (12.0, -30.0), false),
    ];
    let combos: Vec<(usize, usize)> = (0..icons.len()).flat_map(|i| (0..cases.len()).map(move |k| (i, k))).collect();
    let mut tallies: Vec<Tally> = combos.iter().map(|&(i, k)| Tally::new(format!("covered markers, {}: {}", icons[i].0, cases[k].0))).collect();
    let mut errors: Vec<Vec<f64>> = vec![Vec::new(); combos.len()];
    for frame in labelled_frames().expect("dataset") {
        let (Some(map), Some(markers)) = (&frame.label.map, &frame.label.markers) else { continue };
        let view = match map.view {
            LabelView::V1 => MapView::Full,
            LabelView::V2 => MapView::Scope,
            LabelView::V3 => continue,
        };
        let o = frame.meta.geometry.client_in_frame();
        let t = match (view, map.zoom, map.origin) {
            (MapView::Full, Some(zoom), Some(origin)) => MapTransform::zoomed(view, (o.x, o.y), zoom, origin),
            (MapView::Full, Some(_), None) => MapTransform::fixed(view, (o.x, o.y)),
            (MapView::Full, None, _) => continue,
            _ => MapTransform::fixed(view, (o.x, o.y)),
        };
        let objectives: Vec<(f64, f64)> = mtc_maps::objectives(&map.id).iter().map(|ob| t.to_frame(ob.u, ob.v)).collect();
        let image = frame.image().expect("frame image");
        for truth in markers.iter().filter(|m| m.id.is_some()) {
            if objectives.iter().any(|ob| (ob.0 - truth.x).hypot(ob.1 - truth.y) < REAL_OBJECTIVE_PX) {
                continue;
            }
            let label = truth.id.clone().expect("labelled");
            for (c, &(i, k)) in combos.iter().enumerate() {
                let (_, d, by_label) = &cases[k];
                let icon = &icons[i].1;
                let centre = (truth.x + d.0, truth.y + d.1);
                if markers.iter().any(|m| m != truth && (m.x - centre.0).hypot(m.y - centre.1) < 60.0) || !t.contains_frame(centre.0, centre.1) {
                    continue;
                }
                let mut covered = image.clone();
                paste(&mut covered, icon, centre);
                let found = reader.read(&FrameView::rgb(&covered.data, covered.width, covered.height), &t);
                let near = |m: &&mtc_vision::map::markers::MarkerSighting| (m.frame.0 - truth.x).hypot(m.frame.1 - truth.y);
                // With the letter covered the reader reports `?` and the number (whose marker it is
                // is the target book's call).
                let cut = format!("?{}", &label[1..]);
                let hit = if *by_label {
                    found
                        .iter()
                        .filter(|m| m.label.as_deref() == Some(label.as_str()) || m.label.as_deref() == Some(cut.as_str()))
                        .map(|m| near(&m) / t.zoom)
                        .fold(f64::INFINITY, f64::min)
                } else {
                    found.iter().filter(|m| !m.from_label).map(|m| near(&m)).fold(f64::INFINITY, f64::min)
                };
                let ok = hit <= if *by_label { LABEL_TOLERANCE } else { DIAMOND_TOLERANCE };
                if hit.is_finite() {
                    errors[c].push(hit);
                }
                tallies[c].check(&format!("{} {label}", frame.id()), ok, || {
                    format!("error {hit:.2}; found {:?}, truth ({:.1}, {:.1})", found.iter().map(|m| (m.label.clone(), m.frame, m.from_label)).collect::<Vec<_>>(), truth.x, truth.y)
                });
            }
        }
    }
    for (c, t) in tallies.iter().enumerate() {
        println!("{}", t.report());
        let mut e = errors[c].clone();
        println!("  error median {:.2}, p95 {:.2} ({})", percentile(&mut e, 0.5), percentile(&mut e, 0.95), if cases[combos[c].1].2 { "unzoomed px" } else { "frame px" });
    }
    for t in &tallies {
        t.require(0.95);
    }
}
