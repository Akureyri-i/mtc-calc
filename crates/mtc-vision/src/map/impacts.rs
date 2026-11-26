//! The yellow X where the player's own shell landed.
//!
//! On V1 the X is 30 px across in ~(210, 180, 5), arms about 7 px wide horizontally, drawn on whole
//! pixels; it shows at full strength from the first frame and fades linearly to nothing in about
//! 2.0 s (11 impacts on the unzoomed V1 of the debug map). Objective icons are drawn over it. It is
//! drawn at screen size, 30 px on the minimap as well (see `side`); the size on V2 is taken from V1
//! (map icons are drawn at screen size, the markers' diamonds are 30 px on both).
//!
//! A faded X is the X colour mixed over whatever is behind it, so every pixel is read as an
//! amount of X colour over a local background: alpha = (p − bg)·(X − bg) / |X − bg|², with what is
//! left over small. Seeds are pixels that are more X than their neighbours 6 px to either side
//! (outside the arm); each cluster of seeds is fitted with the X's shape, whose gaps give the
//! background, and kept when most of the arms and few of the gaps are X colour.

use super::{MapTransform, MapView};
use crate::FrameView;

/// The X's colour at full strength.
const X_COLOUR: [f32; 3] = [210.0, 180.0, 5.0];

/// The X on V1, 30 × 30 (`#` arm, `+` anti-aliased edge, `.` gap), measured at full strength.
const SHAPE: [&str; 30] = [
    "#####....................#####",
    "######..................######",
    "#######................#######",
    "#######+..............+#######",
    "########+............+########",
    ".#########..........#########.",
    "..#########........#########..",
    "...+########......########+...",
    "....+########....#########....",
    ".....########+..+########.....",
    "......##################......",
    ".......################.......",
    "........##############........",
    ".........+##########+.........",
    "..........##########..........",
    "..........##########..........",
    ".........+##########+.........",
    "........##############........",
    ".......################.......",
    "......##################......",
    ".....########+..+########.....",
    "....+########....########+....",
    "...+########......########+...",
    "..#########........#########..",
    ".#########..........#########.",
    "########+............#########",
    "#######+..............+#######",
    "#######................#######",
    "######..................######",
    "#####....................#####",
];

/// Side of the X (px) on each view: drawn at screen size, 30 px on the minimap too (eight
/// recordings show it there at 30×30, fading as on V1; the 12 px once assumed, as for the markers'
/// diamonds, never matched one).
fn side(_view: MapView) -> i32 {
    30
}

/// Seeds: X colour this much stronger than both neighbours `SEED_REACH` px to the side.
const SEED_ALPHA: f32 = 0.10;
/// An X: at least this share of its arms is X colour (an objective icon may cover the rest)...
const MIN_ARMS: f32 = 0.6;
/// ...at most this share of its gaps...
const MAX_GAPS: f32 = 0.2;
/// ...or this many when this share of its arms is X colour at this strength or more: a fresh,
/// nearly whole X over a mottled patch of the map, whose browns read partly as X colour in the
/// gaps (0.22 in one recorded case).
const STRONG_ARMS: f32 = 0.85;
const STRONG_ALPHA: f32 = 0.8;
const STRONG_MAX_GAPS: f32 = 0.25;
/// ...and its median strength at least this (the last ~0.2 s of the fade are left out).
const MIN_ALPHA: f32 = 0.08;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImpactSighting {
    /// Continuous frame coordinates of the X's centre.
    pub frame: (f64, f64),
    /// How strong the X colour is (about 1.2 when it appears, falling linearly to 0 in ~2 s).
    pub alpha: f32,
    /// Share of the X's arms seen in X colour.
    pub arms: f32,
}

/// Cells of the X's box (offsets from its top-left).
type Cells = Vec<(i32, i32)>;
/// A placement's fit: score, share of arms and of gaps in X colour, median arm strength.
type Fit = (f32, f32, f32, f32);

/// Half the diagonal of an objective icon (frame px; 40–41 px diamonds at screen size in every
/// view) plus its anti-aliased edge: cells this near its centre (as
/// |dx| + |dy|) are under the icon.
const ICON_REACH: f64 = 22.0;
/// At least this share of the X's arms must be in view (not under an icon) to read it.
const MIN_VISIBLE_ARMS: f32 = 0.4;
/// A seed cluster this many px or more narrower (each side) than the X is also fitted flush with
/// its sides: the other side is under a vehicle icon.
const SIDE_START_MIN: i32 = 3;
/// Seeds a cluster needs to be merged with others into one X (the arm ends of the covered Xs
/// recorded have 3–14; single specks of grass or icon edges are left alone).
const MIN_FRAGMENT_SEEDS: u32 = 3;

/// What lies over the map at a frame pixel and hides the X under it: an objective icon (drawn over
/// the X; targets beside an objective put the minimap's X under it), a vehicle icon or the player's
/// arrow (cyan, white; enemies red), or a death mark (red, drawn over the X). Not green: the
/// player's own markers, drawn over an X aimed at them, are thin green outlines.
fn covered(p: [u8; 3], (x, y): (i32, i32), icons: &[(f64, f64)]) -> bool {
    let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
    let cyan = b > 170 && g > 150 && r < 130;
    let white = r > 215 && g > 215 && b > 215;
    let red = r > 170 && g < 90 && b < 90;
    cyan || white || red || icons.iter().any(|&(cx, cy)| (f64::from(x) + 0.5 - cx).abs() + (f64::from(y) + 0.5 - cy).abs() <= ICON_REACH)
}

/// Pixels of the frame box `(x0, y0, w, h)` that are `covered` or beside a covered one: icons'
/// anti-aliased edges, blended with the map, take colours near the X's (a red death mark's edge
/// over grass reads orange). Outside the box: `covered` alone.
struct Hidden {
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
    mask: Vec<bool>,
}

impl Hidden {
    fn of(frame: &FrameView, (x0, y0, w, h): (i32, i32, i32, i32), icons: &[(f64, f64)]) -> Self {
        let raw: Vec<bool> = (0..(w + 2) * (h + 2)).map(|i| {
            let (x, y) = (x0 - 1 + i % (w + 2), y0 - 1 + i / (w + 2));
            frame.covers(x, y, 1, 1) && covered(frame.px(x, y), (x, y), icons)
        }).collect();
        let at = |x: i32, y: i32| raw[((y + 1) * (w + 2) + x + 1) as usize];
        let mask = (0..w * h).map(|i| {
            let (x, y) = (i % w, i / w);
            at(x, y) || at(x + 1, y) || at(x - 1, y) || at(x, y + 1) || at(x, y - 1)
        }).collect();
        Self { x0, y0, w, h, mask }
    }

    fn at(&self, frame: &FrameView, x: i32, y: i32, icons: &[(f64, f64)]) -> bool {
        let (lx, ly) = (x - self.x0, y - self.y0);
        if lx >= 0 && ly >= 0 && lx < self.w && ly < self.h { self.mask[(ly * self.w + lx) as usize] } else { covered(frame.px(x, y), (x, y), icons) }
    }
}

/// The shape scaled to `n` px: arm and gap cells, edges left out.
fn template(n: i32) -> (Cells, Cells) {
    let (mut arms, mut gaps) = (Vec::new(), Vec::new());
    for y in 0..n {
        for x in 0..n {
            // The cell of the 30 px shape under this one's centre.
            let sx = ((x as f32 + 0.5) * 30.0 / n as f32) as usize;
            let sy = ((y as f32 + 0.5) * 30.0 / n as f32) as usize;
            match SHAPE[sy].as_bytes()[sx] {
                b'#' => arms.push((x, y)),
                b'.' => gaps.push((x, y)),
                _ => {}
            }
        }
    }
    (arms, gaps)
}

fn rgb(p: [u8; 3]) -> [f32; 3] {
    [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
}

/// How much X colour `p` is over `bg`, and what is left over (px of colour).
fn alpha(p: [f32; 3], bg: [f32; 3]) -> (f32, f32) {
    let d = [X_COLOUR[0] - bg[0], X_COLOUR[1] - bg[1], X_COLOUR[2] - bg[2]];
    let e = [p[0] - bg[0], p[1] - bg[1], p[2] - bg[2]];
    let dd = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).max(1.0);
    let a = (e[0] * d[0] + e[1] * d[1] + e[2] * d[2]) / dd;
    let r = [e[0] - a * d[0], e[1] - a * d[1], e[2] - a * d[2]];
    (a, (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt())
}

/// The strength of `p` as X colour over `bg` (towards `X_COLOUR`, about 1.2 as the X appears).
fn a_of(p: [f32; 3], bg: [f32; 3]) -> f32 {
    alpha(p, bg).0
}

/// X colour by `a` over the background, not something else: the X's pixels, its anti-aliased edges
/// included, are its colour mixed over the background and leave 0–3 over; an objective icon's
/// orange read as X colour leaves ~36 at full strength (15 % of the step), a white label far more.
fn is_x(a: f32, rest: f32, step: f32) -> bool {
    rest <= 10.0 + 0.08 * a.abs() * step
}

/// The X's colours as recorded: `X_COLOUR`; brighter as it appears, (244, 209, 13); bluer on the
/// snow maps' minimap, (212, 185, 24) to (217, 192, 48), where the one colour left 21–54 over and
/// missed whole Xs.
const X_COLOURS: [[f32; 3]; 3] = [X_COLOUR, [244.0, 209.0, 13.0], [217.0, 192.0, 48.0]];

/// `alpha` towards colour `c`.
fn alpha_to(c: [f32; 3], p: [f32; 3], bg: [f32; 3]) -> (f32, f32) {
    let d = [c[0] - bg[0], c[1] - bg[1], c[2] - bg[2]];
    let e = [p[0] - bg[0], p[1] - bg[1], p[2] - bg[2]];
    let dd = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).max(1.0);
    let a = (e[0] * d[0] + e[1] * d[1] + e[2] * d[2]) / dd;
    let r = [e[0] - a * d[0], e[1] - a * d[1], e[2] - a * d[2]];
    (a, (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt())
}

/// Whether `p` is one of the X's colours over `bg` (`is_x` towards any of `X_COLOURS`).
fn x_coloured(p: [f32; 3], bg: [f32; 3]) -> bool {
    X_COLOURS.iter().any(|&c| {
        let (a, rest) = alpha_to(c, p, bg);
        let step = ((c[0] - bg[0]).powi(2) + (c[1] - bg[1]).powi(2) + (c[2] - bg[2]).powi(2)).sqrt();
        is_x(a, rest, step)
    })
}

fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let k = values.len() / 2;
    *values.select_nth_unstable_by(k, f32::total_cmp).1
}

/// The fit of the X placed with its top-left at `(x0, y0)`: score (arms' mean strength less the
/// gaps'), share of arms and of gaps in X colour, median arm strength; cells under an icon
/// (`covered`) count as neither, and too few arms in view fit nothing.
fn fit(frame: &FrameView, (x0, y0): (i32, i32), arms: &[(i32, i32)], gaps: &[(i32, i32)], hidden: &dyn Fn(i32, i32) -> bool) -> Fit {
    let in_view = |cells: &[(i32, i32)]| -> Vec<(i32, i32)> { cells.iter().copied().filter(|&(dx, dy)| !hidden(x0 + dx, y0 + dy)).collect() };
    let all_arms = arms.len().max(1);
    let (arms, gaps) = (in_view(arms), in_view(gaps));
    if (arms.len() as f32) < MIN_VISIBLE_ARMS * all_arms as f32 || gaps.is_empty() {
        return (f32::MIN, 0.0, 1.0, 0.0);
    }
    let (arms, gaps) = (&arms[..], &gaps[..]);
    let mut channel: [Vec<f32>; 3] = Default::default();
    for &(dx, dy) in gaps {
        let p = rgb(frame.px(x0 + dx, y0 + dy));
        for c in 0..3 {
            channel[c].push(p[c]);
        }
    }
    let bg = [median(&mut channel[0]), median(&mut channel[1]), median(&mut channel[2])];
    let read = |cells: &[(i32, i32)]| {
        let mut sum = 0.0;
        let mut coloured = 0;
        let mut strengths = Vec::with_capacity(cells.len());
        for &(dx, dy) in cells {
            let p = rgb(frame.px(x0 + dx, y0 + dy));
            let a = a_of(p, bg);
            // Only X colour counts (an icon over the X is neither arm nor gap).
            if a >= MIN_ALPHA * 0.5 && x_coloured(p, bg) {
                sum += a.min(1.5);
                coloured += 1;
                strengths.push(a);
            }
        }
        (sum / cells.len() as f32, coloured as f32 / cells.len() as f32, strengths)
    };
    let (arm_mean, arm_share, mut strengths) = read(arms);
    let (gap_mean, gap_share, _) = read(gaps);
    (arm_mean - gap_mean, arm_share, gap_share, median(&mut strengths))
}

/// Development: `MTC_IMPACT_DEBUG` set prints each seed cluster and its best fit (read once).
fn debug_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MTC_IMPACT_DEBUG").is_some())
}

/// The yellow Xs in the container of `t` (the frame view covers it).
pub fn read_impacts(frame: &FrameView, t: &MapTransform) -> Vec<ImpactSighting> {
    read_impacts_among(frame, t, &[])
}

/// As `read_impacts`, with the objective icons drawn at `icons` (frame px centres): an X partly
/// under one is read from the part in view.
pub fn read_impacts_among(frame: &FrameView, t: &MapTransform, icons: &[(f64, f64)]) -> Vec<ImpactSighting> {
    let n = side(t.view);
    let (arms, gaps) = template(n);
    let reach = (n / 5).max(2);
    let x_lo = (t.left.ceil() as i32).max(frame.origin.0) + reach;
    let y_lo = (t.top.ceil() as i32).max(frame.origin.1);
    let x_hi = ((t.left + t.size).floor() as i32).min(frame.origin.0 + frame.width as i32) - reach;
    let y_hi = ((t.top + t.size).floor() as i32).min(frame.origin.1 + frame.height as i32);
    if x_hi <= x_lo || y_hi <= y_lo {
        return Vec::new();
    }
    // Seeds on every other pixel (the arms are 7 px wide on V1).
    let stride = if n >= 20 { 2 } else { 1 };
    let (gw, gh) = (((x_hi - x_lo) / stride + 1) as usize, ((y_hi - y_lo) / stride + 1) as usize);
    let mut seed = vec![false; gw * gh];
    for gy in 0..gh {
        let y = y_lo + gy as i32 * stride;
        if y >= y_hi {
            break;
        }
        for gx in 0..gw {
            let x = x_lo + gx as i32 * stride;
            if x >= x_hi {
                break;
            }
            let p = rgb(frame.px(x, y));
            // Cheap reject: the X colour has far less blue than red and green at any strength.
            if p[0] - p[2] < 12.0 || p[1] - p[2] < 10.0 {
                continue;
            }
            let strong = [frame.px(x - reach, y), frame.px(x + reach, y)].iter().all(|&q| {
                let q = rgb(q);
                a_of(p, q) >= SEED_ALPHA && x_coloured(p, q)
            });
            seed[gy * gw + gx] = strong;
        }
    }
    // Clusters of seeds, linked across up to `LINK` grid cells: where the arms cross, the
    // neighbours to the side are arm too and no seed is made, which splits an X in four.
    const LINK: i32 = 3;
    let mut label = vec![0u32; gw * gh];
    let mut clusters: Vec<(i32, i32, i32, i32, u32)> = Vec::new();
    let mut stack = Vec::new();
    for start in 0..gw * gh {
        if !seed[start] || label[start] != 0 {
            continue;
        }
        let id = clusters.len() as u32 + 1;
        let mut c = (i32::MAX, i32::MAX, i32::MIN, i32::MIN, 0u32);
        label[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (gx, gy) = ((i % gw) as i32, (i / gw) as i32);
            c = (c.0.min(gx), c.1.min(gy), c.2.max(gx), c.3.max(gy), c.4 + 1);
            for ny in (gy - LINK).max(0)..=(gy + LINK).min(gh as i32 - 1) {
                for nx in (gx - LINK).max(0)..=(gx + LINK).min(gw as i32 - 1) {
                    let j = ny as usize * gw + nx as usize;
                    if seed[j] && label[j] == 0 {
                        label[j] = id;
                        stack.push(j);
                    }
                }
            }
        }
        clusters.push(c);
    }
    // A partly covered X still spans most of its box; seeds are fewer near its ends.
    let x_sized = |&(gx0, gy0, gx1, gy1, count): &(i32, i32, i32, i32, u32)| {
        let (w, h) = ((gx1 - gx0 + 1) * stride, (gy1 - gy0 + 1) * stride);
        w <= n + 8 && h <= n + 8 && w >= n / 3 && h >= n / 3 && (count as i32) * stride * stride >= n * n / 12
    };
    // An X whose middle is under vehicle icons or a death mark leaves the ends of its arms as
    // separate clusters, each too small: those that together fit in the X's box are tried as one
    // too.
    // An arm's end is at most about half the X.
    let tip_sized = |&(gx0, gy0, gx1, gy1, count): &(i32, i32, i32, i32, u32)| {
        count >= MIN_FRAGMENT_SEEDS && (gx1 - gx0 + 1) * stride <= n / 2 + 4 && (gy1 - gy0 + 1) * stride <= n / 2 + 4
    };
    // The ends of an X's arms lie on its diagonals, on two sides of its centre at least.
    let on_diagonals = |group: &(i32, i32, i32, i32, u32), parts: &[(i32, i32, i32, i32, u32)]| {
        let centre = |c: &(i32, i32, i32, i32, u32)| ((c.0 + c.2) * stride / 2, (c.1 + c.3) * stride / 2);
        let (gx, gy) = centre(group);
        let mut quadrants = [false; 4];
        let all = parts.iter().all(|p| {
            let (px, py) = centre(p);
            let (dx, dy) = (px - gx, py - gy);
            quadrants[usize::from(dx > 0) + 2 * usize::from(dy > 0)] = true;
            (dx.abs() - dy.abs()).abs() <= n / 5 && dx.abs().max(dy.abs()) >= n / 6
        });
        all && quadrants.iter().filter(|&&q| q).count() >= 2
    };
    let fragments: Vec<(i32, i32, i32, i32, u32)> = clusters.iter().copied().filter(|c| !x_sized(c) && tip_sized(c)).collect();
    let mut used = vec![false; fragments.len()];
    for i in 0..fragments.len() {
        if used[i] {
            continue;
        }
        let (mut group, mut members) = (fragments[i], vec![i]);
        for (j, &f) in fragments.iter().enumerate().skip(i + 1) {
            let union = (group.0.min(f.0), group.1.min(f.1), group.2.max(f.2), group.3.max(f.3), group.4 + f.4);
            if !used[j] && (union.2 - union.0 + 1) * stride <= n + 8 && (union.3 - union.1 + 1) * stride <= n + 8 {
                group = union;
                members.push(j);
            }
        }
        let parts: Vec<(i32, i32, i32, i32, u32)> = members.iter().map(|&m| fragments[m]).collect();
        if members.len() > 1 && x_sized(&group) && on_diagonals(&group, &parts) {
            if debug_on() {
                let parts: Vec<String> = members.iter().map(|&m| { let f = fragments[m]; format!("({},{} {}x{} {})", x_lo + f.0 * stride, y_lo + f.1 * stride, (f.2 - f.0 + 1) * stride, (f.3 - f.1 + 1) * stride, f.4) }).collect();
                eprintln!("  merged group {}", parts.join(" "));
            }
            members.iter().for_each(|&m| used[m] = true);
            clusters.push(group);
        }
    }
    let mut found: Vec<(ImpactSighting, f32)> = Vec::new();
    let half = n / 2;
    for (gx0, gy0, gx1, gy1, count) in clusters {
        let (w, h) = ((gx1 - gx0 + 1) * stride, (gy1 - gy0 + 1) * stride);
        if !x_sized(&(gx0, gy0, gx1, gy1, count)) {
            if debug_on() && count >= 3 {
                eprintln!("  cluster at ({}, {}) {w}x{h}, {count} seeds: not X-sized", x_lo + gx0 * stride, y_lo + gy0 * stride);
            }
            continue;
        }
        let centre = (x_lo + (gx0 + gx1) * stride / 2, y_lo + (gy0 + gy1) * stride / 2);
        // The X placed around the cluster's centre, and where the cluster is narrower than the X
        // (one side hidden under a vehicle icon) also flush with either of its sides: the best fit
        // within a few pixels of those.
        let (sx, sy) = (((n - w) / 2).max(0), ((n - h) / 2).max(0));
        let mut starts = vec![centre];
        for (ox, oy) in [(sx, 0), (-sx, 0), (0, sy), (0, -sy)] {
            if (ox.abs() >= SIDE_START_MIN || oy.abs() >= SIDE_START_MIN) && !starts.contains(&(centre.0 + ox, centre.1 + oy)) {
                starts.push((centre.0 + ox, centre.1 + oy));
            }
        }
        let reach_box = (centre.0 - half - sx - 4, centre.1 - half - sy - 4, n + 2 * sx + 9, n + 2 * sy + 9);
        let hidden = Hidden::of(frame, reach_box, icons);
        let hidden_at = |x: i32, y: i32| hidden.at(frame, x, y, icons);
        let mut best: Option<((i32, i32), Fit)> = None;
        let mut scores = std::collections::HashMap::new();
        for &start in &starts {
            for dy in -4..=4 {
                for dx in -4..=4 {
                    let at = (start.0 - half + dx, start.1 - half + dy);
                    if scores.contains_key(&at) || !frame.covers(at.0, at.1, n, n) {
                        continue;
                    }
                    let f = fit(frame, at, &arms, &gaps, &hidden_at);
                    scores.insert(at, f.0);
                    if best.is_none_or(|(_, b)| f.0 > b.0) {
                        best = Some((at, f));
                    }
                }
            }
        }
        let Some((at, (score, arm_share, gap_share, strength))) = best else { continue };
        if debug_on() {
            eprintln!("  cluster at ({}, {}) {w}x{h}, {count} seeds: best fit at {at:?} score {score:.2} arms {arm_share:.2} gaps {gap_share:.2} strength {strength:.2}", x_lo + gx0 * stride, y_lo + gy0 * stride);
        }
        // A faint X must show more of its arms: 3D grass through V2 once fitted 0.60 of them at
        // strength 0.14, where real Xs that faint show 0.93 or more and the half-covered ones are
        // strong.
        let min_arms = MIN_ARMS + 0.3 * (1.0 - (strength / 0.5).min(1.0));
        let max_gaps = if arm_share >= STRONG_ARMS && strength >= STRONG_ALPHA { STRONG_MAX_GAPS } else { MAX_GAPS };
        if arm_share < min_arms || gap_share > max_gaps || strength < MIN_ALPHA {
            continue;
        }
        // Half-pixel placement (an X 29 px wide sits between two 30 px placements): the peak of a
        // parabola through the neighbours' scores.
        let peak = |minus: Option<&f32>, plus: Option<&f32>| match (minus, plus) {
            (Some(&m), Some(&p)) if m + p - 2.0 * score < 0.0 => (0.5 * (m - p) / (m + p - 2.0 * score)).clamp(-0.5, 0.5),
            _ => 0.0,
        };
        let ox = peak(scores.get(&(at.0 - 1, at.1)), scores.get(&(at.0 + 1, at.1)));
        let oy = peak(scores.get(&(at.0, at.1 - 1)), scores.get(&(at.0, at.1 + 1)));
        let frame_centre = (f64::from(at.0) + f64::from(n) / 2.0 + f64::from(ox), f64::from(at.1) + f64::from(n) / 2.0 + f64::from(oy));
        found.push((ImpactSighting { frame: frame_centre, alpha: strength, arms: arm_share }, score));
    }
    // One X makes one sighting even when its seeds split.
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut kept: Vec<ImpactSighting> = Vec::new();
    for (s, _) in found {
        if kept.iter().all(|k| (k.frame.0 - s.frame.0).hypot(k.frame.1 - s.frame.1) > f64::from(n) / 3.0) {
            kept.push(s);
        }
    }
    kept
}

/// An X under an objective icon shows only the tips of its arms beyond the icon's corners (the X
/// is 30 px, the icon a 41 px diamond; seen on Rohkshort's minimap at objectives A and B):
/// the tips must show this many arm cells in X colour on each diagonal, at least...
const MIN_TIP_CELLS: [u32; 2] = [5, 25];
/// ...this share of the arm cells in view must be X colour, and this share of the X colour in and
/// around the X's box must lie on its arms.
const MIN_TIP_ARMS: f32 = 0.8;
const MIN_TIP_EXPLAINED: f32 = 0.85;
/// Margin (px) around the X's box that counts as gap: an X placed too far under the icon would
/// leave the ends of the tips in view outside its box.
const TIP_MARGIN: i32 = 4;
/// A pixel's background is the median of its neighbours this far (px) away in eight directions
/// (an arm is ~5 px wide across its run, 7 px along a row: at most two of them are on its arm).
const TIP_BG_REACH: i32 = 7;
/// A pixel is X colour at this strength over its background or more.
const TIP_ALPHA: f32 = 0.12;
/// The icon's diamond (as |dx| + |dy| from its centre, frame px): 41 px across, so 20.5.
const ICON_CORE: f64 = 20.5;
/// Its anti-aliased edge and the ring a contested objective gets (tan, 2–3 px, to ~25): out to
/// here only X colour is evidence, anything else may be the icon.
const ICON_RING: f64 = 26.0;
/// A vehicle icon (cyan or white) hides this far (px) around it: its dark outline.
const VEHICLE_OUTLINE: i32 = 2;

/// A pixel near an icon: hidden (under an icon, outside the map), in view, or X colour by a strength.
#[derive(Clone, Copy, PartialEq)]
enum Tip {
    Hidden,
    Clear,
    X(f32),
}

/// The X mostly under the objective icon at `icon` (frame px centre; `icons`: every objective
/// icon, `icon` among them), read from the tips of its arms in view: the X's shape placed with its
/// centre at every pixel under the icon, the placement whose arms best cover the X colour in view
/// and whose gaps (and a margin around its box) hold the least of it. Gaps beside an arm are left
/// out (the X is drawn 29–31 px wide, as it falls on the pixels). Needs tips on both diagonals.
/// An X less covered is `read_impacts_among`'s.
pub fn read_impact_under(frame: &FrameView, t: &MapTransform, icon: (f64, f64), icons: &[(f64, f64)]) -> Option<ImpactSighting> {
    let n = side(t.view);
    let half = n / 2;
    let (arms, gaps) = template(n);
    let mut arm_grid = vec![false; (n * n) as usize];
    for &(x, y) in &arms {
        arm_grid[(y * n + x) as usize] = true;
    }
    let is_arm = |x: i32, y: i32| (0..n).contains(&x) && (0..n).contains(&y) && arm_grid[(y * n + x) as usize];
    let mut gaps: Vec<(i32, i32)> = gaps.into_iter().filter(|&(x, y)| ![(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)].iter().any(|&(ax, ay)| is_arm(ax, ay))).collect();
    for y in -TIP_MARGIN..n + TIP_MARGIN {
        for x in -TIP_MARGIN..n + TIP_MARGIN {
            if !(0..n).contains(&x) || !(0..n).contains(&y) {
                gaps.push((x, y));
            }
        }
    }
    // Which diagonal each arm cell is on: 0 top-left to bottom-right, 1 the other.
    let diagonal: Vec<usize> = arms.iter().map(|&(x, y)| usize::from((x - y).abs() > (x + y - (n - 1)).abs())).collect();
    let reach = ICON_CORE.ceil() as i32 + 4;
    let (icx, icy) = (icon.0.floor() as i32, icon.1.floor() as i32);
    let r = reach + half + TIP_MARGIN + TIP_BG_REACH + 1;
    let (rx0, ry0, rs) = (icx - r, icy - r, 2 * r + 1);
    let (x_lo, y_lo) = ((t.left.ceil() as i32).max(frame.origin.0), (t.top.ceil() as i32).max(frame.origin.1));
    let x_hi = ((t.left + t.size).floor() as i32).min(frame.origin.0 + frame.width as i32);
    let y_hi = ((t.top + t.size).floor() as i32).min(frame.origin.1 + frame.height as i32);
    let index = |x: i32, y: i32| ((y - ry0) * rs + (x - rx0)) as usize;
    let inside = |x: i32, y: i32| (rx0..rx0 + rs).contains(&x) && (ry0..ry0 + rs).contains(&y);
    let icon_reach = |x: i32, y: i32| icons.iter().map(|&(cx, cy)| (f64::from(x) + 0.5 - cx).abs() + (f64::from(y) + 0.5 - cy).abs()).fold(f64::INFINITY, f64::min);
    // Vehicle icons and the arrow (cyan, white) with their outlines.
    let mut vehicle = vec![false; (rs * rs) as usize];
    for y in ry0..ry0 + rs {
        for x in rx0..rx0 + rs {
            if !((x_lo..x_hi).contains(&x) && (y_lo..y_hi).contains(&y)) {
                continue;
            }
            let p = frame.px(x, y);
            let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
            if (b > 170 && g > 150 && r < 130) || (r > 215 && g > 215 && b > 215) {
                for oy in -VEHICLE_OUTLINE..=VEHICLE_OUTLINE {
                    for ox in -VEHICLE_OUTLINE..=VEHICLE_OUTLINE {
                        if inside(x + ox, y + oy) {
                            vehicle[index(x + ox, y + oy)] = true;
                        }
                    }
                }
            }
        }
    }
    // Each pixel's distance to the nearest icon and whether it shows, once (they were worked out
    // again for every neighbour: 14–22 ms a minimap read with captured, coloured icons).
    let mut reach_of = vec![f64::INFINITY; (rs * rs) as usize];
    let mut shown_grid = vec![false; (rs * rs) as usize];
    for y in ry0..ry0 + rs {
        for x in rx0..rx0 + rs {
            let i = index(x, y);
            reach_of[i] = icon_reach(x, y);
            shown_grid[i] = (x_lo..x_hi).contains(&x) && (y_lo..y_hi).contains(&y) && !vehicle[i] && reach_of[i] > ICON_CORE;
        }
    }
    let shown = |x: i32, y: i32| inside(x, y) && shown_grid[index(x, y)];
    let d = TIP_BG_REACH;
    let s = (f64::from(d) / std::f64::consts::SQRT_2).round() as i32;
    let around = [(d, 0), (-d, 0), (0, d), (0, -d), (s, s), (-s, -s), (s, -s), (-s, s)];
    let mut mask = vec![Tip::Hidden; (rs * rs) as usize];
    for y in ry0..ry0 + rs {
        for x in rx0..rx0 + rs {
            if !shown(x, y) {
                continue;
            }
            let p = rgb(frame.px(x, y));
            // Cheap reject: the X colour has far less blue than red and green at any strength.
            let x_coloured = if p[0] - p[2] >= 12.0 && p[1] - p[2] >= 10.0 {
                let mut channel = [[0.0f32; 8]; 3];
                let mut k = 0;
                for &(dx, dy) in around.iter().filter(|&&(dx, dy)| shown(x + dx, y + dy)) {
                    let q = rgb(frame.px(x + dx, y + dy));
                    for c in 0..3 {
                        channel[c][k] = q[c];
                    }
                    k += 1;
                }
                (k > 0)
                    .then(|| [median(&mut channel[0][..k]), median(&mut channel[1][..k]), median(&mut channel[2][..k])])
                    .and_then(|bg| {
                        // The one colour only: under an icon the tips are few, and the icon's
                        // rim read as the other colours moved them (12 → 11 within 0.5 px).
                        let (a, rest) = alpha(p, bg);
                        let step = ((X_COLOUR[0] - bg[0]).powi(2) + (X_COLOUR[1] - bg[1]).powi(2) + (X_COLOUR[2] - bg[2]).powi(2)).sqrt();
                        (a >= TIP_ALPHA && is_x(a, rest, step)).then_some(a)
                    })
            } else {
                None
            };
            mask[index(x, y)] = match x_coloured {
                Some(a) => Tip::X(a),
                None if reach_of[index(x, y)] <= ICON_RING => Tip::Hidden,
                None => Tip::Clear,
            };
        }
    }
    // Too few pixels in X colour for the tips any placement needs: nothing to place.
    let x_pixels = mask.iter().filter(|t| matches!(t, Tip::X(_))).count() as u32;
    if x_pixels < MIN_TIP_CELLS[0] + MIN_TIP_CELLS[1] {
        return None;
    }
    let at = |x: i32, y: i32| if inside(x, y) { mask[index(x, y)] } else { Tip::Hidden };
    // A placement: score, arm cells in view, arm cells in X colour per diagonal, gap cells in X colour.
    let place = |cx: i32, cy: i32| {
        let (x0, y0) = (cx - half, cy - half);
        let (mut seen, mut x_arms, mut x_gaps) = (0u32, [0u32; 2], 0u32);
        for (k, &(dx, dy)) in arms.iter().enumerate() {
            match at(x0 + dx, y0 + dy) {
                Tip::Hidden => {}
                Tip::Clear => seen += 1,
                Tip::X(_) => {
                    seen += 1;
                    x_arms[diagonal[k]] += 1;
                }
            }
        }
        for &(dx, dy) in &gaps {
            if let Tip::X(_) = at(x0 + dx, y0 + dy) {
                x_gaps += 1;
            }
        }
        let hits = x_arms[0] + x_arms[1];
        (2 * i64::from(hits) - i64::from(seen) - 2 * i64::from(x_gaps), seen, x_arms, x_gaps)
    };
    // Every other centre, then every one around the best of those (the tips are 5-8 px across),
    // and its neighbours for the half-pixel placement.
    let mut scores: std::collections::HashMap<(i32, i32), i64> = std::collections::HashMap::new();
    let mut score_at = |c: (i32, i32)| *scores.entry(c).or_insert_with(|| place(c.0, c.1).0);
    let coarse = (icy - reach..=icy + reach)
        .step_by(2)
        .flat_map(|cy| (icx - reach..=icx + reach).step_by(2).map(move |cx| (cx, cy)))
        .filter(|&(cx, cy)| (cx - icx).abs() + (cy - icy).abs() <= reach)
        .map(|c| (c, score_at(c)))
        .max_by_key(|&(_, s)| s)?
        .0;
    let (cx, cy) = (-2..=2)
        .flat_map(|dy| (-2..=2).map(move |dx| (coarse.0 + dx, coarse.1 + dy)))
        .map(|c| (c, score_at(c)))
        .max_by_key(|&(_, s)| s)?
        .0;
    let near = [(cx - 1, cy), (cx + 1, cy), (cx, cy - 1), (cx, cy + 1)].map(|c| score_at(c));
    let (score, seen, x_arms, x_gaps) = place(cx, cy);
    let hits = x_arms[0] + x_arms[1];
    let (fewer, more) = (x_arms[0].min(x_arms[1]), x_arms[0].max(x_arms[1]));
    if fewer < MIN_TIP_CELLS[0] || more < MIN_TIP_CELLS[1] || (hits as f32) < MIN_TIP_ARMS * seen as f32 || (hits as f32) < MIN_TIP_EXPLAINED * (hits + x_gaps) as f32 {
        return None;
    }
    let mut strengths: Vec<f32> = arms
        .iter()
        .filter_map(|&(dx, dy)| match at(cx - half + dx, cy - half + dy) {
            Tip::X(a) => Some(a),
            _ => None,
        })
        .collect();
    let strength = median(&mut strengths);
    if strength < MIN_ALPHA {
        return None;
    }
    let peak = |minus: Option<&i64>, plus: Option<&i64>| match (minus, plus) {
        (Some(&m), Some(&p)) if m + p - 2 * score < 0 => (0.5 * (m - p) as f64 / (m + p - 2 * score) as f64).clamp(-0.5, 0.5),
        _ => 0.0,
    };
    let ox = peak(Some(&near[0]), Some(&near[1]));
    let oy = peak(Some(&near[2]), Some(&near[3]));
    // Cell (x, y) of the shape covers pixel (x0 + x, y0 + y): the X's centre is at x0 + n/2.
    let centre = (f64::from(cx - half) + f64::from(n) / 2.0 + ox, f64::from(cy - half) + f64::from(n) / 2.0 + oy);
    Some(ImpactSighting { frame: centre, alpha: strength, arms: hits as f32 / arms.len() as f32 })
}

/// The X's strength when it appears, and how fast it fades (per second): 11 impacts went from
/// 1.19–1.22 to 0.07–0.17 over 1.5–1.8 s, about 0.62 a second.
pub const FULL_ALPHA: f32 = 1.2;
pub const FADE_PER_S: f32 = 0.62;
/// An X not seen for this long is gone (it fades out in ~2 s).
const GONE_MS: u64 = 2500;
/// A sighting this close (frame px) to where an impact's X is drawn, and no stronger than it was
/// (it only fades), is that impact again; a stronger one on the same spot is a new shell.
const SAME_PX: f64 = 4.0;
const STRONGER: f32 = 0.2;
/// A sighting this faint only continues an impact (an X first seen in its last 0.5 s is rare; a
/// faint fit on a texture is not).
const NEW_MIN_ALPHA: f32 = 0.25;

/// One shell's landing, from its X.
#[derive(Clone, Debug, PartialEq)]
pub struct Impact {
    /// Numbered from 1 in the order they appeared.
    pub id: u32,
    /// Normalised map coordinates of the X's centre (from its sharpest sighting).
    pub map: (f64, f64),
    /// Frame px per map unit of the sighting `map` came from (the higher, the finer).
    pub scale: f64,
    /// When it was first and last seen (ms, the frames' clock).
    pub first_ms: u64,
    pub last_ms: u64,
    /// When it landed, by how faded it was when first seen (the first read can come late: V3 is
    /// read every 500 ms, or the map was opened after the landing).
    pub landed_ms: u64,
    /// Strength when first and last seen.
    pub first_alpha: f32,
    pub last_alpha: f32,
    pub sightings: u32,
}

/// The impacts whose X may still be on screen, matched across frames and views.
#[derive(Default)]
pub struct ImpactTracker {
    active: Vec<Impact>,
    next_id: u32,
}

impl ImpactTracker {
    /// Folds in the Xs read at `t_ms` on the view of `t`; returns the impacts seen for the first time.
    pub fn observe(&mut self, t_ms: u64, t: &MapTransform, sightings: &[ImpactSighting]) -> Vec<Impact> {
        self.active.retain(|i| t_ms.saturating_sub(i.last_ms) <= GONE_MS);
        let mut new = Vec::new();
        let mut taken = vec![false; self.active.len()];
        for s in sightings {
            let near = self
                .active
                .iter()
                .enumerate()
                .filter(|(k, i)| !taken[*k] && s.alpha <= i.last_alpha + STRONGER)
                .map(|(k, i)| {
                    let (x, y) = t.to_frame(i.map.0, i.map.1);
                    (k, (x - s.frame.0).hypot(y - s.frame.1))
                })
                .filter(|&(_, d)| d <= SAME_PX)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let map = t.to_map(s.frame.0, s.frame.1);
            match near {
                Some((k, _)) => {
                    taken[k] = true;
                    let i = &mut self.active[k];
                    i.last_ms = t_ms;
                    i.last_alpha = s.alpha;
                    i.sightings += 1;
                    if t.scale() > i.scale {
                        (i.map, i.scale) = (map, t.scale());
                    }
                }
                None if s.alpha < NEW_MIN_ALPHA => {}
                None => {
                    self.next_id += 1;
                    let age_ms = ((FULL_ALPHA - s.alpha).max(0.0) / FADE_PER_S * 1000.0) as u64;
                    let impact = Impact {
                        id: self.next_id,
                        map,
                        scale: t.scale(),
                        first_ms: t_ms,
                        last_ms: t_ms,
                        landed_ms: t_ms.saturating_sub(age_ms),
                        first_alpha: s.alpha,
                        last_alpha: s.alpha,
                        sightings: 1,
                    };
                    new.push(impact.clone());
                    self.active.push(impact);
                    taken.push(true);
                }
            }
        }
        new
    }

    /// The impacts whose X may still show.
    pub fn active(&self) -> &[Impact] {
        &self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sighting(x: f64, y: f64, alpha: f32) -> ImpactSighting {
        ImpactSighting { frame: (x, y), alpha, arms: 1.0 }
    }

    #[test]
    fn an_x_fading_on_one_spot_is_one_impact_and_a_fresh_one_there_is_another() {
        let t = MapTransform::fixed(MapView::Full, (0, 34));
        let mut log = ImpactTracker::default();
        let first = log.observe(1000, &t, &[sighting(1509.0, 594.0, 1.19)]);
        assert_eq!(first.len(), 1);
        assert!((984..=1000).contains(&first[0].landed_ms), "{first:?}");
        // Fading: the same impact; seen on the minimap too it would be matched by map position.
        assert!(log.observe(1200, &t, &[sighting(1509.1, 593.9, 1.05)]).is_empty());
        assert!(log.observe(1400, &t, &[sighting(1509.0, 594.0, 0.92)]).is_empty());
        // A second shell lands on the same spot while the first fades: full strength again.
        let second = log.observe(1600, &t, &[sighting(1509.0, 594.0, 0.80), sighting(1510.0, 594.5, 1.2)]);
        assert_eq!(second.len(), 1, "{second:?}");
        assert_eq!(second[0].id, 2);
        // First seen half faded (the map opened late): it landed about 1 s before.
        let late = log.observe(9000, &t, &[sighting(1200.0, 600.0, 0.58)]);
        assert!((7995..=8005).contains(&late[0].landed_ms), "{late:?}");
        // Gone after 2.5 s unseen.
        log.observe(12_000, &t, &[]);
        assert!(log.active().is_empty());
        // A faint sighting alone starts nothing.
        assert!(log.observe(13_000, &t, &[sighting(900.0, 700.0, 0.14)]).is_empty());
        assert!(log.active().is_empty());
    }

    #[test]
    fn the_shape_is_an_x() {
        let (arms, gaps) = template(30);
        assert_eq!(arms.len() + gaps.len() + SHAPE.iter().map(|r| r.matches('+').count()).sum::<usize>(), 900);
        // The centre and the four corners are arm; the middles of the edges are gap.
        for cell in [(14, 14), (0, 0), (29, 0), (0, 29), (29, 29)] {
            assert!(arms.contains(&cell), "{cell:?}");
        }
        for cell in [(14, 0), (0, 14), (29, 14), (14, 29)] {
            assert!(gaps.contains(&cell), "{cell:?}");
        }
    }

    /// A frame of terrain with an X at strength 1 whose box starts at `x_at` (if any) and a red
    /// objective icon (41 px diamond) over it centred on `icon`.
    fn under_icon(x_at: Option<(i32, i32)>, icon: (f64, f64)) -> Vec<u8> {
        let (w, h) = (200i32, 200i32);
        let mut data = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                // Terrain with a darker band, as the minimap's fields and roads.
                let c = if (x + 2 * y) % 37 < 6 { [80u8, 90, 55] } else { [96, 100, 60] };
                data[i..i + 3].copy_from_slice(&c);
            }
        }
        if let Some((x0, y0)) = x_at {
            for (dx, dy) in template(30).0 {
                let i = (((y0 + dy) * w + x0 + dx) * 3) as usize;
                data[i..i + 3].copy_from_slice(&[210, 180, 5]);
            }
        }
        for y in 0..h {
            for x in 0..w {
                if (f64::from(x) + 0.5 - icon.0).abs() + (f64::from(y) + 0.5 - icon.1).abs() <= 20.5 {
                    let i = ((y * w + x) * 3) as usize;
                    data[i..i + 3].copy_from_slice(&[196, 39, 27]);
                }
            }
        }
        data
    }

    #[test]
    fn an_x_under_an_icon_is_read_from_its_tips() {
        let t = MapTransform { view: MapView::Mini, left: 0.0, top: 0.0, size: 200.0, zoom: 1.0, origin: (0.0, 0.0) };
        // The X's box at (85, 102): centre (100, 117), 7.5 px left of and 1.5 px below the icon's,
        // as seen at an objective in a recording: tips show at the left and the bottom right.
        let icon = (107.5, 115.5);
        let data = under_icon(Some((85, 102)), icon);
        let frame = FrameView::rgb(&data, 200, 200);
        assert!(read_impacts_among(&frame, &t, &[icon]).is_empty(), "too little shows for the reader of whole Xs");
        let s = read_impact_under(&frame, &t, icon, &[icon]).expect("the X under the icon");
        assert!((s.frame.0 - 100.0).abs() < 0.3 && (s.frame.1 - 117.0).abs() < 0.3, "{s:?}");
        assert!((s.alpha - 1.0).abs() < 0.1, "{s:?}");
        // No X: nothing.
        let data = under_icon(None, icon);
        assert!(read_impact_under(&FrameView::rgb(&data, 200, 200), &t, icon, &[icon]).is_none());
    }

    #[test]
    fn tips_on_one_diagonal_only_are_not_enough() {
        let t = MapTransform { view: MapView::Mini, left: 0.0, top: 0.0, size: 200.0, zoom: 1.0, origin: (0.0, 0.0) };
        // Centred on the icon, its four tips out beyond the diamond's sides; vehicle icons (cyan)
        // over the top-right and bottom-left ones leave the "\" diagonal alone.
        let icon = (100.0, 100.0);
        let mut data = under_icon(Some((85, 85)), icon);
        let frame = FrameView::rgb(&data, 200, 200);
        let s = read_impact_under(&frame, &t, icon, &[icon]).expect("all four tips");
        assert!((s.frame.0 - 100.0).abs() < 0.3 && (s.frame.1 - 100.0).abs() < 0.3, "{s:?}");
        for (x0, y0) in [(104, 78), (78, 104)] {
            for y in y0..y0 + 18 {
                for x in x0..x0 + 18 {
                    let i = ((y * 200 + x) * 3) as usize;
                    data[i..i + 3].copy_from_slice(&[40, 200, 230]);
                }
            }
        }
        assert!(read_impact_under(&FrameView::rgb(&data, 200, 200), &t, icon, &[icon]).is_none());
    }

    /// An X drawn at strength `a` over a background with a stripe of another colour across it.
    #[test]
    fn a_faded_x_is_found_where_it_is() {
        let (w, h) = (200u32, 200u32);
        let mut data = vec![0u8; (w * h * 3) as usize];
        let bg = [60.0f32, 110.0, 50.0];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                let c = if (90..96).contains(&y) { [200.0, 200.0, 200.0] } else { bg };
                data[i..i + 3].copy_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8]);
            }
        }
        let a = 0.3f32;
        let (x0, y0) = (71, 40);
        for (dx, dy) in template(30).0 {
            let i = (((y0 + dy) as u32 * w + (x0 + dx) as u32) * 3) as usize;
            for c in 0..3 {
                data[i + c] = (a * X_COLOUR[c] + (1.0 - a) * f32::from(data[i + c])) as u8;
            }
        }
        let frame = FrameView::rgb(&data, w, h);
        let t = MapTransform { view: MapView::Full, left: 0.0, top: 0.0, size: 200.0, zoom: 1.0, origin: (0.0, 0.0) };
        let found = read_impacts(&frame, &t);
        assert_eq!(found.len(), 1, "{found:?}");
        let s = found[0];
        assert!((s.frame.0 - 86.0).abs() < 0.3 && (s.frame.1 - 55.0).abs() < 0.3, "{s:?}");
        assert!((s.alpha - a).abs() < 0.06, "{s:?}");
    }
}
