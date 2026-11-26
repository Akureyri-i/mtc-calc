//! The player's own position: the white arrow drawn on the vehicle icon (alone when dismounted).
//!
//! The arrow is drawn at screen scale in every view (≈197 px blob in a 24–25 px box). Its pixel
//! centroid sits behind the point it marks: fitting stationary vehicles across zoom levels (the
//! offset shrinks with zoom) gives 1.3–1.6 px along the arrow's heading, so the pivot is the
//! centroid moved 1.45 px forward. The hull icon is not used for position: its visible part
//! moves when the hull turns under the arrow.
//!
//! In multiplayer the map also shows white icons of empty vehicles, towed guns and trucks, some
//! close to the arrow's size. Every white blob of arrow size is therefore compared with the
//! arrow's shape (a template harvested from labelled frames, rotated to the blob's heading) and
//! the best match above [`MIN_SHAPE_SCORE`] is the player.
//!
//! Near the map's edge the arrow runs off the container and only part of it shows: its centroid
//! and axis then lean into the map. A blob touching the container's edge is instead fitted with
//! the template at every pose, compared only where the template falls inside the container
//! (`ArrowTemplate::fit_clipped`), and the sighting is marked `clipped`.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::map::geometry::MapTransform;
use crate::view::FrameView;

const WHITE_MIN_VALUE: u8 = 235;
const WHITE_MAX_SATURATION: f32 = 30.0;
pub const ARROW_AREA: (u32, u32) = (150, 240);
/// Bounding-box side range. The arrow is about 25 × 17 px: pointing along an axis its box is that
/// (seen in game on a V3 minimap, heading east), diagonally about 22 × 22.
pub const ARROW_SIDE: (u32, u32) = (14, 28);
/// Off the reference configuration (a view resampled to the reference geometry) the size range
/// scaled by these (lengths): the arrow's white core measured 15-30 % under its scaled area
/// (smaller windows less), and the resampling blurs it again; the shape score keeps other white
/// blobs out.
const ELSEWHERE_SIZE: (f64, f64) = (0.7, 1.1);
/// And the shape score from this: the blurred small arrows of 100 % and of minimaps held to
/// 0.32 H scored 0.59-0.65 against the template scaled for them.
const ELSEWHERE_MIN_SHAPE: f32 = 0.58;
/// Vehicle-icon pixels (cyan, or green in a squad) within this many px mark the arrow as mounted.
const ICON_REACH: i32 = 4;
/// Centroid-to-pivot distance along the heading (px).
const PIVOT_AHEAD: f64 = 1.45;
/// Sub-pixel samples per pixel side when a blob is laid over the template.
const SUBSAMPLES: i32 = 4;
/// Shape score (soft IoU with the template) a blob needs to count as the arrow. Measured
/// (`examples/arrow_shape_scan.rs`): the 320 labelled arrows score 0.79–0.91 (0.75 in lossy,
/// rescaled multiplayer screenshots); every other white blob of 60–600 px in those frames scores
/// at most 0.40, and the white empty-vehicle, towed-gun and truck icons of the multiplayer
/// screenshots, cut out as solid shapes, at most 0.59.
pub const MIN_SHAPE_SCORE: f32 = 0.65;
/// A blob touching the container's edge needs this many px (the rest may be cut off).
const CLIPPED_MIN_AREA: u32 = 40;
/// A clipped fit needs this share of the template's mass inside the container.
const CLIPPED_MIN_VISIBLE: f32 = 0.25;
/// Coarse poses refined in a clipped fit.
const CLIPPED_SEEDS: usize = 6;
/// Searching from the last pose: the global search costs 167 ms a fit (median of 2856 cut
/// arrows) and runs back to back while the arrow stays cut by the edge. Between
/// two reads the arrow moves under a pixel and turns a few degrees, so a search from the last pose
/// starts a little wider than a coarse pose's refinement, then refines as it does.
const SEEDED_STAGES: [(f64, f64, f64, f64); 3] = [(3.0, 1.0, 20.0, 4.0), (1.0, 0.25, 4.0, 1.0), (0.5, 0.125, 2.0, 0.5)];
/// The search from the last pose is taken at this score; below it (a turn, a respawn) the global
/// search runs.
const SEEDED_MIN_SCORE: f32 = 0.70;
/// Pivot reach (px) of the every-heading search around the last pivot.
const TURN_REACH: i32 = 3;
/// Refinement of a coarse pose: (reach px, step px, turn °, turn step °).
const REFINE_STAGES: [(f64, f64, f64, f64); 2] = [(2.0, 0.5, 10.0, 2.0), (0.5, 0.125, 2.0, 0.5)];

/// The best pose around `start` over `stages` (reach px, step px, turn °, turn step °).
fn search_around(score: &mut impl FnMut((f64, f64), f64) -> f32, start: ((f64, f64), f64, f32), stages: &[(f64, f64, f64, f64)]) -> ((f64, f64), f64, f32) {
    let deg = std::f64::consts::PI / 180.0;
    let mut best = start;
    for &(reach, step, turn, turn_step) in stages {
        let (around, heading, _) = best;
        let k = (reach / step).round() as i32;
        let m = (turn / turn_step).round() as i32;
        for a in -m..=m {
            let h = heading + f64::from(a) * turn_step * deg;
            for j in -k..=k {
                for i in -k..=k {
                    let p = (around.0 + f64::from(i) * step, around.1 + f64::from(j) * step);
                    let s = score(p, h);
                    if s > best.2 {
                        best = (p, h, s);
                    }
                }
            }
        }
    }
    best
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrowSighting {
    /// Continuous frame coordinates of the arrow's pivot.
    pub frame: (f64, f64),
    pub map: (f64, f64),
    /// Arrow direction, degrees clockwise from map north (up).
    pub heading_deg: f64,
    /// Drawn on a vehicle icon (in a vehicle) rather than alone (on foot).
    pub mounted: bool,
    /// Soft IoU with the arrow template, 0..1.
    pub shape: f32,
    /// Cut by the container's edge: fitted from the visible part (less sure).
    pub clipped: bool,
}

/// A white blob of plausible arrow size, with the pose it would have if it were the arrow.
#[derive(Clone, Debug)]
pub struct WhiteBlob {
    /// Frame pixel coordinates.
    pub pixels: Vec<(i32, i32)>,
    pub pivot: (f64, f64),
    /// Unit vector along the heading, frame coordinates (y down).
    pub forward: (f64, f64),
    pub heading_deg: f64,
    pub mounted: bool,
    pub size: (u32, u32),
    /// Touches the container's edge (part of it may be cut off).
    pub clipped: bool,
}

#[inline]
fn is_white(p: [u8; 3]) -> bool {
    let max = p[0].max(p[1]).max(p[2]);
    let min = p[0].min(p[1]).min(p[2]);
    max >= WHITE_MIN_VALUE && f32::from(max - min) * 255.0 <= WHITE_MAX_SATURATION * f32::from(max)
}

/// Vehicle-icon cyan (hue 160–200°) or squad green (hue ≈ 120°), saturated and bright.
#[inline]
fn is_icon_colour(p: [u8; 3]) -> bool {
    let [r, g, b] = p;
    let max = g.max(b);
    let cyan = max > 140 && r < g && r < b && f32::from(max - r) * 255.0 > 90.0 * f32::from(max) && g.abs_diff(b) < max / 3;
    let green = g > 140 && g > r.saturating_add(60) && g > b.saturating_add(60);
    cyan || green
}

/// 8-connected white blobs inside the transform's container with area and bounding-box sides in
/// the given ranges.
pub fn white_blobs_sized(view: &FrameView, t: &MapTransform, area: (u32, u32), side: (u32, u32)) -> Vec<WhiteBlob> {
    white_blobs_at(view, t, area, side, 1.0)
}

/// As `white_blobs_sized` for icons drawn `k` times their 150 % size (`player_arrow_candidates_at`):
/// the cut blob's least area, the vehicle-icon reach and the pivot's lead scale with them.
fn white_blobs_at(view: &FrameView, t: &MapTransform, area: (u32, u32), side: (u32, u32), k: f64) -> Vec<WhiteBlob> {
    let clipped_min_area = (f64::from(CLIPPED_MIN_AREA) * k * k).round() as u32;
    let icon_reach = (f64::from(ICON_REACH) * k).round() as i32;
    let pivot_ahead = PIVOT_AHEAD * k;
    let x0 = t.left.floor() as i32;
    let y0 = t.top.floor() as i32;
    let n = t.size.ceil() as i32;
    if !view.covers(x0, y0, n, n) {
        return Vec::new();
    }
    let (w, h) = (n as usize, n as usize);
    let white: Vec<bool> = (0..w * h).map(|i| is_white(view.px(x0 + (i % w) as i32, y0 + (i / w) as i32))).collect();
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    let mut pixels = Vec::new();
    let mut blobs = Vec::new();
    for start in 0..w * h {
        if !white[start] || seen[start] {
            continue;
        }
        pixels.clear();
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            pixels.push(i);
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                        let j = ny as usize * w + nx as usize;
                        if white[j] && !seen[j] {
                            seen[j] = true;
                            stack.push(j);
                        }
                    }
                }
            }
        }
        let count = pixels.len() as u32;
        let xs = || pixels.iter().map(|&i| (i % w) as f64);
        let ys = || pixels.iter().map(|&i| (i / w) as f64);
        let (minx, maxx) = xs().fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(x), b.max(x)));
        let (miny, maxy) = ys().fold((f64::MAX, f64::MIN), |(a, b), y| (a.min(y), b.max(y)));
        let (bw, bh) = ((maxx - minx) as u32 + 1, (maxy - miny) as u32 + 1);
        // Touching the container's edge: part of it may lie outside, smaller than a whole arrow.
        let clipped = minx == 0.0 || miny == 0.0 || maxx == (w - 1) as f64 || maxy == (h - 1) as f64;
        let sized = if clipped {
            (clipped_min_area..=area.1).contains(&count) && bw <= side.1 && bh <= side.1
        } else {
            (area.0..=area.1).contains(&count) && (side.0..=side.1).contains(&bw) && (side.0..=side.1).contains(&bh)
        };
        if !sized {
            continue;
        }
        let cx = xs().sum::<f64>() / f64::from(count);
        let cy = ys().sum::<f64>() / f64::from(count);
        // Heading: the arrow is a mirror-symmetric pointer, longer than wide, so its axis is the
        // major principal axis; the point is the thin end, i.e. the side the third moment
        // along the axis leans to.
        let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
        for &i in &pixels {
            let (dx, dy) = ((i % w) as f64 - cx, (i / w) as f64 - cy);
            sxx += dx * dx;
            syy += dy * dy;
            sxy += dx * dy;
        }
        let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        let (ax, ay) = (angle.cos(), angle.sin());
        let skew: f64 = pixels
            .iter()
            .map(|&i| {
                let p = ((i % w) as f64 - cx) * ax + ((i / w) as f64 - cy) * ay;
                p * p * p
            })
            .sum();
        let (fx, fy) = if skew >= 0.0 { (ax, ay) } else { (-ax, -ay) };
        let mounted = pixels.iter().any(|&i| {
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            (-icon_reach..=icon_reach).any(|dy| {
                (-icon_reach..=icon_reach).any(|dx| {
                    let (px, py) = (x0 + x + dx, y0 + y + dy);
                    view.covers(px, py, 1, 1) && is_icon_colour(view.px(px, py))
                })
            })
        });
        blobs.push(WhiteBlob {
            pixels: pixels.iter().map(|&i| (x0 + (i % w) as i32, y0 + (i / w) as i32)).collect(),
            pivot: (
                f64::from(x0) + cx + 0.5 + pivot_ahead * fx,
                f64::from(y0) + cy + 0.5 + pivot_ahead * fy,
            ),
            forward: (fx, fy),
            heading_deg: fx.atan2(-fy).to_degrees().rem_euclid(360.0),
            mounted,
            size: (bw, bh),
            clipped,
        });
    }
    blobs
}

/// White blobs of arrow size.
pub fn white_blobs(view: &FrameView, t: &MapTransform) -> Vec<WhiteBlob> {
    white_blobs_sized(view, t, ARROW_AREA, ARROW_SIDE)
}

/// Mean arrow mask in the arrow's own frame: origin at the pivot, `u` to the arrow's right, `v`
/// forward. Cell (row, col) covers u ∈ [col·cell − half·cell, …), v ∈ (half·cell − row·cell, …].
#[derive(Deserialize)]
pub struct ArrowTemplate {
    pub cell: f64,
    pub half: usize,
    pub values: Vec<f32>,
}

impl ArrowTemplate {
    pub fn side(&self) -> usize {
        2 * self.half
    }

    /// Cell index of a point given relative to the pivot in frame coordinates.
    pub fn cell_of(&self, d: (f64, f64), forward: (f64, f64)) -> Option<usize> {
        let (fx, fy) = forward;
        let u = -d.0 * fy + d.1 * fx;
        let v = d.0 * fx + d.1 * fy;
        let r = self.half as f64 * self.cell;
        let col = ((u + r) / self.cell).floor();
        let row = ((r - v) / self.cell).floor();
        let n = self.side() as f64;
        (col >= 0.0 && row >= 0.0 && col < n && row < n).then(|| row as usize * self.side() + col as usize)
    }

    /// Cells covered by the blob's pixels (sub-sampled) when laid over the template at its pose.
    pub fn occupancy(&self, blob: &WhiteBlob) -> Vec<bool> {
        let mut occ = vec![false; self.values.len()];
        for &(x, y) in &blob.pixels {
            for sy in 0..SUBSAMPLES {
                for sx in 0..SUBSAMPLES {
                    let px = f64::from(x) + (f64::from(sx) + 0.5) / f64::from(SUBSAMPLES);
                    let py = f64::from(y) + (f64::from(sy) + 0.5) / f64::from(SUBSAMPLES);
                    if let Some(c) = self.cell_of((px - blob.pivot.0, py - blob.pivot.1), blob.forward) {
                        occ[c] = true;
                    }
                }
            }
        }
        occ
    }

    /// The pose of an arrow cut by the edge of the visible area `bounds` (frame px: left, top,
    /// right, bottom) from its visible pixels: the template at every pose around the pixels,
    /// scored by soft IoU only where it falls inside `bounds`, coarse to fine. The pivot, the unit
    /// forward vector and the score, `None` when no pose keeps `CLIPPED_MIN_VISIBLE` of the
    /// template inside.
    pub fn fit_clipped(&self, pixels: &[(i32, i32)], bounds: (f64, f64, f64, f64)) -> Option<((f64, f64), (f64, f64), f32)> {
        self.fit_clipped_near(pixels, bounds, None)
    }

    /// As `fit_clipped`, searching first around `seed` (the last pose: pivot in frame px, heading
    /// in radians clockwise from up); the global search only when that finds no good fit.
    pub fn fit_clipped_near(&self, pixels: &[(i32, i32)], bounds: (f64, f64, f64, f64), seed: Option<((f64, f64), f64)>) -> Option<((f64, f64), (f64, f64), f32)> {
        if pixels.is_empty() {
            return None;
        }
        let n = self.side();
        let r = self.half as f64 * self.cell;
        // Template cells with mass, at their centres in the arrow's frame.
        let cells: Vec<(f64, f64, f32)> = (0..self.values.len())
            .filter(|&i| self.values[i] > 0.0)
            .map(|i| {
                let (row, col) = ((i / n) as f64, (i % n) as f64);
                ((col + 0.5) * self.cell - r, r - (row + 0.5) * self.cell, self.values[i])
            })
            .collect();
        let total: f32 = cells.iter().map(|c| c.2).sum();
        // Two samples per pixel side: one per template cell.
        let points: Vec<(f64, f64)> = pixels
            .iter()
            .flat_map(|&(x, y)| [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)].map(|(dx, dy)| (f64::from(x) + dx, f64::from(y) + dy)))
            .collect();
        let inside = |x: f64, y: f64| x >= bounds.0 && y >= bounds.1 && x < bounds.2 && y < bounds.3;
        let mut occupied = vec![false; self.values.len()];
        let mut score = |pivot: (f64, f64), heading: f64| -> f32 {
            let forward = (heading.sin(), -heading.cos());
            let mut mass = 0.0f32;
            for &(u, v, t) in &cells {
                let (x, y) = (pivot.0 - u * forward.1 + v * forward.0, pivot.1 + u * forward.0 + v * forward.1);
                if inside(x, y) {
                    mass += t;
                }
            }
            if mass < CLIPPED_MIN_VISIBLE * total {
                return 0.0;
            }
            occupied.fill(false);
            let (mut inter, mut count) = (0.0f32, 0.0f32);
            for &(x, y) in &points {
                if let Some(c) = self.cell_of((x - pivot.0, y - pivot.1), forward)
                    && !occupied[c]
                {
                    occupied[c] = true;
                    inter += self.values[c];
                    count += 1.0;
                }
            }
            inter / (count + mass - inter).max(f32::EPSILON)
        };
        if let Some((pivot, heading)) = seed {
            let start = (pivot, heading, score(pivot, heading));
            let (p, h, s) = search_around(&mut score, start, &SEEDED_STAGES);
            if s >= SEEDED_MIN_SCORE {
                return Some((p, (h.sin(), -h.cos()), s));
            }
            // Turned further than the seeded search reaches (a vehicle turning between two reads),
            // still about where it was: every heading near the last pivot, a fifth of the global
            // search (that one ran on most reads of an arrow cut by the edge while turning: 115–165
            // ms each, 0.29–0.34 core, measured on Fulvia Gap).
            let deg = std::f64::consts::PI / 180.0;
            let mut best = (pivot, heading, 0.0f32);
            for a in 0..36 {
                let h = f64::from(a) * 10.0 * deg;
                for j in -TURN_REACH..=TURN_REACH {
                    for i in -TURN_REACH..=TURN_REACH {
                        let p = (pivot.0 + f64::from(i), pivot.1 + f64::from(j));
                        let s = score(p, h);
                        if s > best.2 {
                            best = (p, h, s);
                        }
                    }
                }
            }
            let (p, h, s) = search_around(&mut score, best, &REFINE_STAGES);
            if s >= SEEDED_MIN_SCORE {
                return Some((p, (h.sin(), -h.cos()), s));
            }
        }
        let centre = {
            let k = pixels.len() as f64;
            (pixels.iter().map(|p| f64::from(p.0)).sum::<f64>() / k + 0.5, pixels.iter().map(|p| f64::from(p.1)).sum::<f64>() / k + 0.5)
        };
        // (pivot, heading in radians clockwise from up, score). The coarse grid's best few
        // distinct poses are each refined: the best coarse pose alone can be a wrong turn (a
        // whole arrow fitted 135° off scored 0.66 against 0.87 at the right pose).
        let deg = std::f64::consts::PI / 180.0;
        let mut coarse: Vec<((f64, f64), f64, f32)> = Vec::new();
        for a in 0..36 {
            let h = f64::from(a) * 10.0 * deg;
            for j in -7..=7 {
                for i in -7..=7 {
                    let p = (centre.0 + f64::from(i) * 2.0, centre.1 + f64::from(j) * 2.0);
                    let s = score(p, h);
                    if s > 0.0 {
                        coarse.push((p, h, s));
                    }
                }
            }
        }
        coarse.sort_by(|a, b| b.2.total_cmp(&a.2));
        let mut seeds: Vec<((f64, f64), f64, f32)> = Vec::new();
        for c in coarse {
            let distinct = seeds.iter().all(|s| {
                let turn = ((c.1 - s.1).rem_euclid(std::f64::consts::TAU)).min((s.1 - c.1).rem_euclid(std::f64::consts::TAU));
                turn > 25.0 * deg || (c.0.0 - s.0.0).hypot(c.0.1 - s.0.1) > 4.0
            });
            if distinct {
                seeds.push(c);
                if seeds.len() == CLIPPED_SEEDS {
                    break;
                }
            }
        }
        let best = seeds.into_iter().map(|s| search_around(&mut score, s, &REFINE_STAGES)).fold(((0.0, 0.0), 0.0, 0.0f32), |a, b| if b.2 > a.2 { b } else { a });
        let (pivot, heading, s) = best;
        (s > 0.0).then(|| (pivot, (heading.sin(), -heading.cos()), s))
    }

    /// Soft IoU between the blob (laid over the template at its pose) and the mean arrow mask.
    pub fn score(&self, blob: &WhiteBlob) -> f32 {
        let occ = self.occupancy(blob);
        let (mut inter, mut occupied, mut mass) = (0.0f32, 0.0f32, 0.0f32);
        for (&o, &t) in occ.iter().zip(&self.values) {
            if o {
                inter += t;
                occupied += 1.0;
            }
            mass += t;
        }
        inter / (occupied + mass - inter).max(f32::EPSILON)
    }
}

/// `fit_clipped`, the last result kept per thread: a stationary arrow shows the same pixels frame
/// after frame, and a fit costs some tens of milliseconds.
fn fit_clipped_cached(template: &ArrowTemplate, pixels: &[(i32, i32)], bounds: (f64, f64, f64, f64), seed: Option<((f64, f64), f64)>) -> Option<((f64, f64), (f64, f64), f32)> {
    type Fit = Option<((f64, f64), (f64, f64), f32)>;
    thread_local! {
        static LAST: std::cell::RefCell<Option<(Vec<(i32, i32)>, (f64, f64, f64, f64), f64, Fit)>> = const { std::cell::RefCell::new(None) };
    }
    LAST.with(|last| {
        if let Some((p, b, cell, fit)) = last.borrow().as_ref()
            && p.as_slice() == pixels
            && *b == bounds
            && *cell == template.cell
        {
            return *fit;
        }
        let fit = template.fit_clipped_near(pixels, bounds, seed);
        *last.borrow_mut() = Some((pixels.to_vec(), bounds, template.cell, fit));
        fit
    })
}

pub fn template() -> &'static ArrowTemplate {
    static TEMPLATE: OnceLock<ArrowTemplate> = OnceLock::new();
    TEMPLATE.get_or_init(|| serde_json::from_str(include_str!("../../assets/arrow_template.json")).expect("arrow template asset"))
}

impl ArrowTemplate {
    /// The template for an arrow drawn `k` times its 150 % size: the same mask over cells `k`
    /// times as large.
    pub fn scaled(&self, k: f64) -> Self {
        Self { cell: self.cell * k, half: self.half, values: self.values.clone() }
    }
}

/// The player's arrow inside the transform's container, if visible: the arrow-sized white blob
/// that best matches the arrow's shape.
pub fn find_player_arrow(view: &FrameView, t: &MapTransform) -> Option<ArrowSighting> {
    find_player_arrow_at(view, t, 1.0)
}

/// As `find_player_arrow` with icons drawn `icon_scale` times their 150 % size in the transform's
/// frame (`player_arrow_candidates_at`).
pub fn find_player_arrow_at(view: &FrameView, t: &MapTransform, icon_scale: f64) -> Option<ArrowSighting> {
    let candidates = player_arrow_candidates_at(view, t, icon_scale);
    candidates.clipped.iter().filter_map(fit_clipped_blob).chain(candidates.complete).max_by(|a, b| a.shape.total_cmp(&b.shape))
}

/// A white blob cut by the container's edge, to fit with the arrow template (`fit_clipped_blob`:
/// 150–200 ms when the cache misses, as it does while the arrow turns; the live HUD fits it away
/// from its frame loop).
#[derive(Clone, Debug)]
pub struct ClippedBlob {
    pixels: Vec<(i32, i32)>,
    bounds: (f64, f64, f64, f64),
    mounted: bool,
    transform: MapTransform,
    icon_scale: f64,
}

/// The player arrow if a whole one shows, and the blobs cut by the edge that may be it.
#[derive(Clone, Debug, Default)]
pub struct ArrowCandidates {
    pub complete: Option<ArrowSighting>,
    pub clipped: Vec<ClippedBlob>,
}

/// `find_player_arrow` without the clipped fits: whole arrows scored (well under a millisecond),
/// clipped blobs handed back.
pub fn player_arrow_candidates(view: &FrameView, t: &MapTransform) -> ArrowCandidates {
    player_arrow_candidates_at(view, t, 1.0)
}

/// Development: the best shape score among the sized whole blobs (`player_arrow_candidates_at`'s
/// view), whatever the threshold.
#[doc(hidden)]
pub fn best_shape_at(view: &FrameView, t: &MapTransform, icon_scale: f64) -> Option<f32> {
    let template = template().scaled(icon_scale);
    let k2 = icon_scale * icon_scale;
    let (lo, hi) = if view.format == crate::view::PixelFormat::BgraPeak { ELSEWHERE_SIZE } else { (1.0, 1.0) };
    let area = ((f64::from(ARROW_AREA.0) * k2 * lo * lo).floor() as u32, (f64::from(ARROW_AREA.1) * k2 * hi * hi).ceil() as u32);
    let side = ((f64::from(ARROW_SIDE.0) * icon_scale * lo).floor() as u32, (f64::from(ARROW_SIDE.1) * icon_scale * hi).ceil() as u32);
    white_blobs_at(view, t, area, side, icon_scale).iter().filter(|b| !b.clipped).map(|b| template.score(b)).fold(None, |m: Option<f32>, s| Some(m.map_or(s, |m| m.max(s))))
}

/// As `player_arrow_candidates` with the arrow drawn `icon_scale` times its 150 % size in the
/// transform's frame: on V1 and V2 of another configuration mapped to the reference geometry by the
/// map frame, the HUD's scale over the map frame's (e.g. 0.885 at 125 % and 0.772 at 100 % display
/// scaling). The size ranges, the template and the pivot's lead scale with it.
pub fn player_arrow_candidates_at(view: &FrameView, t: &MapTransform, icon_scale: f64) -> ArrowCandidates {
    let scaled;
    let template = if icon_scale == 1.0 {
        template()
    } else {
        scaled = template().scaled(icon_scale);
        &scaled
    };
    let k2 = icon_scale * icon_scale;
    // Resampled from another display configuration: the white core of a smaller arrow is a
    // smaller share of it (anti-aliased edge), then blurred again by the resampling.
    let (lo, hi) = if view.format == crate::view::PixelFormat::BgraPeak { ELSEWHERE_SIZE } else { (1.0, 1.0) };
    let area = ((f64::from(ARROW_AREA.0) * k2 * lo * lo).floor() as u32, (f64::from(ARROW_AREA.1) * k2 * hi * hi).ceil() as u32);
    let side = ((f64::from(ARROW_SIDE.0) * icon_scale * lo).floor() as u32, (f64::from(ARROW_SIDE.1) * icon_scale * hi).ceil() as u32);
    let (x0, y0) = (t.left.floor(), t.top.floor());
    let side_px = t.size.ceil();
    let bounds = (x0, y0, x0 + side_px, y0 + side_px);
    let mut out = ArrowCandidates::default();
    for b in white_blobs_at(view, t, area, side, icon_scale) {
        if b.clipped {
            out.clipped.push(ClippedBlob { pixels: b.pixels, bounds, mounted: b.mounted, transform: *t, icon_scale });
            continue;
        }
        let shape = template.score(&b);
        let min_shape = if view.format == crate::view::PixelFormat::BgraPeak { ELSEWHERE_MIN_SHAPE } else { MIN_SHAPE_SCORE };
        if shape >= min_shape && out.complete.as_ref().is_none_or(|c| shape > c.shape) {
            out.complete = Some(ArrowSighting { frame: b.pivot, map: t.to_map(b.pivot.0, b.pivot.1), heading_deg: b.heading_deg, mounted: b.mounted, shape, clipped: false });
        }
    }
    out
}

/// The arrow fitted to a clipped blob, if it fits well enough.
pub fn fit_clipped_blob(b: &ClippedBlob) -> Option<ArrowSighting> {
    fit_clipped_blob_near(b, None)
}

/// As `fit_clipped_blob`, searching first around `seed`: the last sighting's map position
/// (normalised) and heading (degrees clockwise from north).
pub fn fit_clipped_blob_near(b: &ClippedBlob, seed: Option<((f64, f64), f64)>) -> Option<ArrowSighting> {
    let seed = seed.map(|(map, heading_deg)| (b.transform.to_frame(map.0, map.1), heading_deg.to_radians()));
    let scaled;
    let template = if b.icon_scale == 1.0 {
        template()
    } else {
        scaled = template().scaled(b.icon_scale);
        &scaled
    };
    let (pivot, forward, shape) = fit_clipped_cached(template, &b.pixels, b.bounds, seed)?;
    (shape >= MIN_SHAPE_SCORE).then(|| ArrowSighting {
        frame: pivot,
        map: b.transform.to_map(pivot.0, pivot.1),
        heading_deg: forward.0.atan2(-forward.1).to_degrees().rem_euclid(360.0),
        mounted: b.mounted,
        shape,
        clipped: true,
    })
}
