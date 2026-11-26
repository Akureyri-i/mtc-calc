//! Development: the V1 registrar's no-grid fallback on the labelled V1 frames (zoom and origin
//! known). For each frame the previous registration is set to the true view at the same zoom
//! (`d` = 0: no wheel since) or `d` wheel notches away about the container centre (a zoom the
//! grid did not catch), optionally also panned by `pan` px since (in eight directions), and the
//! fallback's zoom is compared with the label.
//!   v1_fallback [pan px ...]
//!   cargo run --release -p mtc-vision --example v1_fallback
use std::collections::HashMap;

use mtc_dataset::eval::labelled_frames;
use mtc_dataset::labels::MapView as LabelView;
use mtc_vision::FrameView;
use mtc_vision::map::zoom::{Method, Registration, V1Registrar};
use mtc_vision::map::{MapReference, MapTransform, MapView};

fn main() {
    let pans: Vec<f64> = std::env::args().skip(1).map(|a| a.parse().expect("pan px")).collect();
    let pans = if pans.is_empty() { vec![0.0] } else { pans };
    let mut registrars: HashMap<String, V1Registrar> = HashMap::new();
    // Per previous-zoom offset d: frames, right zoom, wrong zoom, none.
    let mut tally: HashMap<(i64, i32), (u32, u32, u32, u32)> = HashMap::new();
    let mut wrong_at_d0: Vec<String> = Vec::new();
    for frame in labelled_frames().expect("dataset") {
        let Some(map) = &frame.label.map else { continue };
        let (LabelView::V1, Some(zoom), Some(origin)) = (map.view, map.zoom, map.origin) else { continue };
        if zoom < 1.1 {
            continue;
        }
        let registrar = registrars.entry(map.id.clone()).or_insert_with(|| {
            let info = mtc_maps::by_id(&map.id).expect("map");
            let img = mtc_maps::load_reference(info).expect("reference");
            V1Registrar::new(&MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data), info.grid_cell_m)
        });
        let image = frame.image().expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = frame.meta.geometry.client_in_frame();
        let client = (o.x, o.y);
        let truth = MapTransform::zoomed(MapView::Full, client, zoom, origin);
        let centre = (truth.left + truth.size / 2.0, truth.top + truth.size / 2.0);
        let (mu, mv) = truth.to_map(centre.0, centre.1);
        for (&pan, d) in pans.iter().flat_map(|p| (-2..=2).map(move |d| (p, d))) {
            let z = zoom + 0.2 * f64::from(d);
            if !(1.0..=5.0 + 1e-9).contains(&z) {
                continue;
            }
            // The previous view at zoom z with the same map point under the container centre.
            let s = truth.size * z;
            for k in 0..if pan > 0.0 { 8 } else { 1 } {
            let a = std::f64::consts::TAU * f64::from(k) / 8.0;
            let (px, py) = (pan * a.cos() / s, pan * a.sin() / s);
            let prev = MapTransform::zoomed(MapView::Full, client, z, (mu - (centre.0 - truth.left) / s + px, mv - (centre.1 - truth.top) / s + py));
            let last = Registration { transform: prev, method: Method::GridLabels, score: 1.0, grid: None };
            let result = registrar.without_grid_from(Some(last), &view, client, Some(centre));
            let e = tally.entry((pan as i64, d)).or_default();
            e.0 += 1;
            match result {
                Some(r) if (r.transform.zoom - zoom).abs() < 1e-6 => e.1 += 1,
                Some(r) => {
                    e.2 += 1;
                    if d == 0 {
                        wrong_at_d0.push(format!("{}/{} label {zoom} got {:.1} (score {:.3})", frame.session, frame.meta.file, r.transform.zoom, r.score));
                    }
                }
                None => e.3 += 1,
            }
            }
        }
    }
    let mut ds: Vec<_> = tally.keys().copied().collect();
    ds.sort();
    for key in ds {
        let (n, right, wrong, none) = tally[&key];
        println!("panned {:>4} px, previous zoom {:+} notches: {n} cases, right {right}, wrong zoom {wrong}, none {none}", key.0, key.1);
    }
    for w in wrong_at_d0.iter().take(20) {
        println!("  d=0 wrong: {w}");
    }
}
