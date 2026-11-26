//! Finding the minimap's and the full map's containers on screen at another display configuration:
//! the measured rules (`UiLayout::map_container`) give where to look; the container is a
//! translucent dark square, inside which the scene shows at about 0.53 of its brightness, so each
//! edge is a step of that ratio. Along rows of the translucent bands
//! above and below the map picture (left and right edges) and columns across them (top and bottom
//! edges), the step is found within `SEARCH` px of the rule's edge and placed to a sub-pixel by
//! how dark its transition pixel is. Only thin strips round the expected edges are read
//! (`probes`): about 4000 pixels, 0.06 ms a try on recorded frames (`examples/locate_cost.rs`).
//! A frame without the view, or with too dark or too busy a scene round it, gives nothing.

use crate::layout::UiLayout;
use crate::map::geometry::MapView;
use crate::view::FrameView;

/// How far round the rule's edge (px) the step is looked for.
pub const SEARCH: i32 = 30;
/// The fill's brightness ratio, inside against outside: 0.53 measured on recordings at several
/// display configurations, accepted within this range.
const RATIO: (f32, f32) = (0.42, 0.66);
const RATIO_TYPICAL: f32 = 0.53;
/// The two pixels either side of the step must be this alike (share of their level).
const FLAT: f32 = 0.10;
/// The scene outside must be at least this bright for the ratio to tell.
const MIN_OUTSIDE: f32 = 40.0;
/// Rows (columns) that must give an edge for it to count.
const MIN_HITS: usize = 5;
/// The four edges must make a square to within this (px).
const SQUARE: f64 = 2.0;

/// Where on the container the strips run: the rows' shares of the side from the top and from the
/// bottom (inside the translucent bands), the columns' shares across (clear of the minimap's M
/// box at its bottom left).
fn bands(view: MapView) -> (&'static [f64], (f64, f64)) {
    match view {
        MapView::Mini => (&[0.04, 0.06, 0.08, 0.10, 0.12, 0.14, 0.16, 0.18, 0.20], (0.3, 0.9)),
        _ => (&[0.03, 0.045, 0.06, 0.075, 0.09, 0.105, 0.12, 0.135, 0.15], (0.1, 0.9)),
    }
}

const COLUMNS: usize = 13;

/// The strips (client px rectangles: x, y, w, h) a container is found from: for each row, a strip
/// round the expected left edge and one round the right; for each column, one round the top and
/// one round the bottom; kept inside the client (the minimap's right edge is 15-22 px in).
pub fn probes(layout: &UiLayout, view: MapView) -> Vec<(i32, i32, i32, i32)> {
    let (left, top, side) = layout.map_container(view);
    let (shares, (c0, c1)) = bands(view);
    let (w, h) = (layout.width as i32, layout.height as i32);
    let clip = |a: i32, len: i32, max: i32| (a.max(0), (a + len).min(max) - a.max(0));
    let mut out = Vec::new();
    for &f in shares {
        for y in [top + f * side, top + side - f * side] {
            for x in [left, left + side] {
                let (x0, len) = clip(x as i32 - SEARCH - 3, 2 * SEARCH + 6, w);
                out.push((x0, y as i32, len, 1));
            }
        }
    }
    for k in 0..COLUMNS {
        let x = (left + side * (c0 + (c1 - c0) * k as f64 / (COLUMNS - 1) as f64)) as i32;
        for y in [top, top + side] {
            let (y0, len) = clip(y as i32 - SEARCH - 3, 2 * SEARCH + 6, h);
            out.push((x, y0, 1, len));
        }
    }
    out.retain(|r| r.2 > 0 && r.3 > 0);
    out
}

/// The container (left, top, side; client px) on these views (the full frame, or the read-back
/// `probes`, each piece from whichever view covers it), `None` when its edges do not show.
pub fn container(views: &[FrameView], client_origin: (i32, i32), layout: &UiLayout, view: MapView) -> Option<(f64, f64, f64)> {
    let (left, top, side) = layout.map_container(view);
    let (shares, (c0, c1)) = bands(view);
    let (ox, oy) = client_origin;
    // The strip last read from first: a step's pixels lie along one strip.
    let last = std::cell::Cell::new(0usize);
    let luma = |x: i32, y: i32| -> Option<f32> {
        let (fx, fy) = (x + ox, y + oy);
        let i = if views.get(last.get()).is_some_and(|v| v.covers(fx, fy, 1, 1)) { last.get() } else { views.iter().position(|v| v.covers(fx, fy, 1, 1))? };
        last.set(i);
        let p = views[i].px(fx, fy);
        Some(0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]))
    };
    let median = |mut v: Vec<f64>| -> Option<f64> {
        if v.len() < MIN_HITS {
            return None;
        }
        v.sort_by(f64::total_cmp);
        Some(v[v.len() / 2])
    };
    let mut sides = [Vec::new(), Vec::new()];
    for &f in shares {
        for y in [top + f * side, top + side - f * side] {
            let y = y as i32;
            for (i, (guess, after)) in [(left, true), (left + side, false)].into_iter().enumerate() {
                if let Some(e) = step(&|x| luma(x, y), guess, after) {
                    sides[i].push(e);
                }
            }
        }
    }
    let mut ends = [Vec::new(), Vec::new()];
    for k in 0..COLUMNS {
        let x = (left + side * (c0 + (c1 - c0) * k as f64 / (COLUMNS - 1) as f64)) as i32;
        for (i, (guess, after)) in [(top, true), (top + side, false)].into_iter().enumerate() {
            if let Some(e) = step(&|y| luma(x, y), guess, after) {
                ends[i].push(e);
            }
        }
    }
    let [l, r] = sides.map(median);
    let [t, b] = ends.map(median);
    let (l, r, t, b) = (l?, r?, t?, b?);
    ((r - l) - (b - t)).abs().le(&SQUARE).then(|| (l, t, ((r - l) + (b - t)) / 2.0))
}

/// The dark step along `line` within `SEARCH` of `guess`: the fill after it (`inside_after`) or
/// before it; its position to a sub-pixel. The candidate whose ratio is nearest the fill's.
fn step(line: &dyn Fn(i32) -> Option<f32>, guess: f64, inside_after: bool) -> Option<f64> {
    let mut best: Option<(f32, f64)> = None;
    let g = guess as i32;
    for x in g - SEARCH..g + SEARCH {
        let (Some(a0), Some(a1), Some(c), Some(b0), Some(b1)) = (line(x - 2), line(x - 1), line(x), line(x + 1), line(x + 2)) else { continue };
        let (o, i, flat) = if inside_after {
            ((a0 + a1) / 2.0, (b0 + b1) / 2.0, (a0 - a1).abs() <= FLAT * a0 && (b0 - b1).abs() <= FLAT * b0.max(8.0))
        } else {
            ((b0 + b1) / 2.0, (a0 + a1) / 2.0, (b0 - b1).abs() <= FLAT * b0 && (a0 - a1).abs() <= FLAT * a0.max(8.0))
        };
        if o < MIN_OUTSIDE || !flat || !(RATIO.0..=RATIO.1).contains(&(i / o)) {
            continue;
        }
        let f = f64::from(((o - c) / (o - i)).clamp(0.0, 1.0));
        let pos = if inside_after { f64::from(x) + 1.0 - f } else { f64::from(x) + f };
        let err = (i / o - RATIO_TYPICAL).abs();
        if best.is_none_or(|(e, _)| err < e) {
            best = Some((err, pos));
        }
    }
    best.map(|b| b.1)
}
