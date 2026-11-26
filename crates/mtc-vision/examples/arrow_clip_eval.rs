//! Development: the player's arrow cut by a map edge, on the labelled arrows. Each labelled arrow
//! is cut from each side (left, right, top, bottom) at several depths, as the container's edge
//! cuts it near the map's edge; the old estimate (centroid and axis of what shows, behind the
//! size and shape checks) and the clipped fit are compared with the labelled pivot.
//!   cargo run --release -p mtc-vision --example arrow_clip_eval
use mtc_dataset::eval::labelled_frames;
use mtc_dataset::labels::MapView as LabelView;
use mtc_vision::FrameView;
use mtc_vision::map::arrow::{ARROW_AREA, ARROW_SIDE, MIN_SHAPE_SCORE, WhiteBlob, template, white_blobs};
use mtc_vision::map::{MapTransform, MapView};

const CUTS: [f64; 4] = [0.25, 0.4, 0.55, 0.7];
const PIVOT_AHEAD: f64 = 1.45;

/// The old estimate on a set of pixels: centroid moved ahead along the axis, after the checks.
fn old_estimate(pixels: &[(i32, i32)]) -> Option<(f64, f64)> {
    let n = pixels.len() as f64;
    let (minx, maxx) = pixels.iter().fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (miny, maxy) = pixels.iter().fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    let (bw, bh) = ((maxx - minx + 1) as u32, (maxy - miny + 1) as u32);
    if !(ARROW_AREA.0..=ARROW_AREA.1).contains(&(pixels.len() as u32)) || !(ARROW_SIDE.0..=ARROW_SIDE.1).contains(&bw) || !(ARROW_SIDE.0..=ARROW_SIDE.1).contains(&bh) {
        return None;
    }
    let cx = pixels.iter().map(|p| f64::from(p.0)).sum::<f64>() / n;
    let cy = pixels.iter().map(|p| f64::from(p.1)).sum::<f64>() / n;
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in pixels {
        let (dx, dy) = (f64::from(p.0) - cx, f64::from(p.1) - cy);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (ax, ay) = (angle.cos(), angle.sin());
    let skew: f64 = pixels.iter().map(|p| ((f64::from(p.0) - cx) * ax + (f64::from(p.1) - cy) * ay).powi(3)).sum();
    let (fx, fy) = if skew >= 0.0 { (ax, ay) } else { (-ax, -ay) };
    let pivot = (cx + 0.5 + PIVOT_AHEAD * fx, cy + 0.5 + PIVOT_AHEAD * fy);
    let blob = WhiteBlob { pixels: pixels.to_vec(), pivot, forward: (fx, fy), heading_deg: 0.0, mounted: true, size: (bw, bh), clipped: false };
    (template().score(&blob) >= MIN_SHAPE_SCORE).then_some(pivot)
}

fn summary(errors: &mut Vec<f64>, cases: usize) -> String {
    errors.sort_by(f64::total_cmp);
    if errors.is_empty() {
        return format!("found 0/{cases}");
    }
    let q = |f: f64| errors[((errors.len() - 1) as f64 * f).round() as usize];
    format!("found {}/{cases}, error median {:.2} px, p90 {:.2}, max {:.2}", errors.len(), q(0.5), q(0.9), q(1.0))
}

fn main() {
    let t = template();
    let mut uncut: Vec<f64> = Vec::new();
    let mut rows: Vec<(f64, Vec<f64>, Vec<f64>, Vec<f32>, usize)> = CUTS.iter().map(|&c| (c, Vec::new(), Vec::new(), Vec::new(), 0)).collect();
    let mut arrows = 0;
    // Time of each fit of a cut arrow: the HUD's fit worker peaks on these.
    let mut fit_ms: Vec<f64> = Vec::new();
    // The same cut arrows searched from a last pose (the uncut fit, as the previous read): close
    // (1 px off, turned 8°), and wrong (6 px off, turned 90° and 180°: must fall back to the global
    // search). Errors against the truth, and times.
    let mut seeded: Vec<(&str, Vec<f64>, Vec<f64>, usize)> = vec![("from the last pose (off by 1 px, turned 8°)", Vec::new(), Vec::new(), 0), ("from a wrong last pose (off by 6 px, turned 90°)", Vec::new(), Vec::new(), 0), ("from a wrong last pose (off by 6 px, turned 180°)", Vec::new(), Vec::new(), 0)];
    let mut global_err: Vec<f64> = Vec::new();
    for frame in labelled_frames().expect("dataset") {
        let (Some(map), Some(truth)) = (&frame.label.map, &frame.label.player) else { continue };
        let o = frame.meta.geometry.client_in_frame();
        let client = (o.x, o.y);
        let transform = match (map.view, map.zoom, map.origin) {
            (LabelView::V1, Some(z), Some(origin)) => MapTransform::zoomed(MapView::Full, client, z, origin),
            (LabelView::V1, _, _) => MapTransform::fixed(MapView::Full, client),
            (LabelView::V2, _, _) => MapTransform::fixed(MapView::Scope, client),
            (LabelView::V3, _, _) => MapTransform::fixed(MapView::Mini, client),
        };
        let image = frame.image().expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let Some(blob) = white_blobs(&view, &transform).into_iter().filter(|b| !b.clipped).min_by(|a, b| {
            let d = |b: &WhiteBlob| (b.pivot.0 - truth.x).hypot(b.pivot.1 - truth.y);
            d(a).total_cmp(&d(b))
        }) else {
            continue;
        };
        if (blob.pivot.0 - truth.x).hypot(blob.pivot.1 - truth.y) > 3.0 {
            continue;
        }
        arrows += 1;
        // The fit on the whole arrow (no cut): how it compares with the centroid method.
        let uncut_pose = t.fit_clipped(&blob.pixels, (f64::MIN, f64::MIN, f64::MAX, f64::MAX)).map(|(p, f, _)| (p, f.0.atan2(-f.1)));
        if let Some((p, f, s)) = t.fit_clipped(&blob.pixels, (f64::MIN, f64::MIN, f64::MAX, f64::MAX)) {
            let e = (p.0 - truth.x).hypot(p.1 - truth.y);
            uncut.push(e);
            if e > 1.0 {
                let turn = (f.0 * blob.forward.0 + f.1 * blob.forward.1).clamp(-1.0, 1.0).acos().to_degrees();
                println!("  uncut fit off {e:.2} px: {} {} fit turned {turn:.0}° from the blob's axis, score {s:.2}, full-arrow score {:.2}", frame.session, frame.meta.file, t.score(&blob));
            }
        }
        let (minx, maxx) = blob.pixels.iter().fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let (miny, maxy) = blob.pixels.iter().fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        for row in &mut rows {
            for side in 0..4 {
                let (keep, bounds): (Box<dyn Fn(&(i32, i32)) -> bool>, (f64, f64, f64, f64)) = match side {
                    0 => {
                        let cut = minx + (row.0 * f64::from(maxx - minx + 1)).round() as i32;
                        (Box::new(move |p| p.0 >= cut), (f64::from(cut), f64::MIN, f64::MAX, f64::MAX))
                    }
                    1 => {
                        let cut = maxx + 1 - (row.0 * f64::from(maxx - minx + 1)).round() as i32;
                        (Box::new(move |p| p.0 < cut), (f64::MIN, f64::MIN, f64::from(cut), f64::MAX))
                    }
                    2 => {
                        let cut = miny + (row.0 * f64::from(maxy - miny + 1)).round() as i32;
                        (Box::new(move |p| p.1 >= cut), (f64::MIN, f64::from(cut), f64::MAX, f64::MAX))
                    }
                    _ => {
                        let cut = maxy + 1 - (row.0 * f64::from(maxy - miny + 1)).round() as i32;
                        (Box::new(move |p| p.1 < cut), (f64::MIN, f64::MIN, f64::MAX, f64::from(cut)))
                    }
                };
                let pixels: Vec<(i32, i32)> = blob.pixels.iter().copied().filter(|p| keep(p)).collect();
                row.4 += 1;
                if let Some(p) = old_estimate(&pixels) {
                    row.1.push((p.0 - truth.x).hypot(p.1 - truth.y));
                }
                let started = std::time::Instant::now();
                let fit = if pixels.len() >= 40 { t.fit_clipped(&pixels, bounds) } else { None };
                if pixels.len() >= 40 {
                    fit_ms.push(started.elapsed().as_secs_f64() * 1000.0);
                }
                if let Some((p, _, s)) = fit {
                    row.3.push(s);
                    if s >= MIN_SHAPE_SCORE {
                        row.2.push((p.0 - truth.x).hypot(p.1 - truth.y));
                        global_err.push((p.0 - truth.x).hypot(p.1 - truth.y));
                    }
                }
                if pixels.len() >= 40
                    && let Some((p0, h0)) = uncut_pose
                {
                    let deg = std::f64::consts::PI / 180.0;
                    for (k, (dx, dy, turn)) in [(1.0, -1.0, 8.0), (6.0, 0.0, 90.0), (0.0, 6.0, 180.0)].into_iter().enumerate() {
                        let started = std::time::Instant::now();
                        let fit = t.fit_clipped_near(&pixels, bounds, Some(((p0.0 + dx, p0.1 + dy), h0 + turn * deg)));
                        seeded[k].2.push(started.elapsed().as_secs_f64() * 1000.0);
                        seeded[k].3 += 1;
                        if let Some((p, _, s)) = fit
                            && s >= MIN_SHAPE_SCORE
                        {
                            seeded[k].1.push((p.0 - truth.x).hypot(p.1 - truth.y));
                        }
                    }
                }
            }
        }
    }
    println!("{arrows} labelled arrows");
    let stat = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        if v.is_empty() { "none".to_owned() } else { format!("median {:.2}, p90 {:.2}, max {:.2}", v[v.len() / 2], v[v.len() * 9 / 10], v[v.len() - 1]) }
    };
    println!("global search (current): found {}, error px {}", global_err.len(), stat(&mut global_err));
    for (name, err, ms, cases) in &mut seeded {
        println!("{name}: {cases} tries, found {}, error px {}; ms each {}", err.len(), stat(err), stat(ms));
    }
    fit_ms.sort_by(f64::total_cmp);
    if !fit_ms.is_empty() {
        println!("fit of a cut arrow: {} fits, median {:.1} ms, p90 {:.1} ms, max {:.1} ms", fit_ms.len(), fit_ms[fit_ms.len() / 2], fit_ms[fit_ms.len() * 9 / 10], fit_ms[fit_ms.len() - 1]);
    }
    println!("uncut, fit: {}", summary(&mut uncut, arrows));
    for (cut, old, new, scores, cases) in &mut rows {
        scores.sort_by(f32::total_cmp);
        let low = scores.first().copied().unwrap_or(0.0);
        let med = scores.get(scores.len() / 2).copied().unwrap_or(0.0);
        println!("cut {:.0} %:\n  old  {}\n  fit  {} (scores min {low:.2}, median {med:.2})", *cut * 100.0, summary(old, *cases), summary(new, *cases));
    }
}
