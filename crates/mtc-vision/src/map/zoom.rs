//! V1 zoom registration: the transform of the full map when it is zoomed.
//!
//! Measured game behaviour this relies on:
//! - Zoom levels are discrete, 1.0 to 5.0 in steps of 0.2; no rotation; zooming keeps the map
//!   point under the cursor fixed and the view persists when V1 is closed and reopened.
//! - The game draws a lettered grid of square cells (100 m on every rotation map except Rohkstov,
//!   200 m), an odd number per axis, centred on the map: the map centre is a cell centre and
//!   column A / row 1 is the partial cell at the left / top edge. Lines are drawn at every zoom,
//!   labels once cells are large enough.
//!
//! Two measurements, used together:
//! - [`GridFit`]: dark grid lines give the zoom level (line period = one cell at that zoom) and
//!   the sub-pixel phase, i.e. the origin up to whole cells.
//! - [`ReferenceMatcher`]: coarse-to-fine gradient correlation against the reference image at a
//!   given zoom gives the origin directly (low zoom) or picks the cell (high zoom), wherever the
//!   visible terrain has features.
//!
//! [`V1Registrar`] keeps the last registration so featureless high-zoom views stay registered
//! while the grid phase moves continuously.

use crate::map::geometry::{MapTransform, MapView};
use crate::map::labels::LabelReader;
use crate::map::raster::{Plane, area_resample};
use crate::map::reference::{MapReference, WORK_SIZE};
use crate::view::FrameView;

pub const ZOOM_LEVELS: usize = 21;
pub const ZOOM_STEP: f64 = 0.2;
/// Grid cell sizes tried for a map whose cell size is not measured.
pub const CELL_SIZES_M: [f64; 2] = [100.0, 200.0];

/// Zoom of level `k` (0 = unzoomed).
pub fn zoom_level(k: usize) -> f64 {
    1.0 + ZOOM_STEP * k as f64
}

/// Map coordinate of the left/top edge of grid column/row 0 (A / 1), in metres from the canvas
/// edge (≤ 0).
pub fn grid_origin_m(map_size_m: f64, cell_m: f64) -> f64 {
    let half = map_size_m / 2.0 - cell_m / 2.0;
    half - cell_m * (half / cell_m).ceil()
}

// ---------------------------------------------------------------------------------------------
// Grid

/// Grid periods below this many pixels are not searched (lines too dense to be drawn).
const GRID_MIN_PERIOD: f64 = 40.0;
const GRID_PHASE_STEP: f64 = 0.25;
/// Comb lead (mean line response on the teeth minus the median over phases, summed over both
/// axes) needed to accept a grid.
const GRID_MIN_SCORE: f32 = 1.2;
/// The best level must beat the runner-up comb score by this much.
const GRID_MIN_LEAD: f32 = 0.25;
/// Line response a single comb tooth may contribute at most.
const TOOTH_CAP: f32 = 4.0;
/// A tooth sits on a line at this response.
const TOOTH_HIT: f32 = 0.8;
/// Teeth on lines required on both axes, in count and share (the grid is only drawn over the
/// opaque part of the map, so maps with transparent bands lose teeth there).
const MIN_HITS: u32 = 2;
const MIN_HIT_SHARE: f32 = 0.3;
/// Periods whose ratio is this close to an integer are harmonics of each other.
const HARMONIC_TOLERANCE: f64 = 0.02;
/// A finer harmonic replaces the winner when it keeps this share of its score.
const HARMONIC_MIN_SHARE: f32 = 0.7;
/// Line response: a pixel darker than the mean of the pixels two to either side.
const LINE_OFFSET: i32 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridFit {
    /// Cell side in metres.
    pub cell_m: f64,
    /// Zoom level index.
    pub level: usize,
    pub zoom: f64,
    /// Grid period in frame pixels.
    pub period: f64,
    /// Offset of the first line's pixel from the container's (rounded) left/top edge, in
    /// [0, period).
    pub phase: (f64, f64),
    /// Continuous frame coordinates of the first vertical / horizontal line (pixel centre).
    pub first_line: (f64, f64),
    pub score: f32,
}

impl GridFit {
    /// Median line response of every container column (x) and row (y).
    fn profiles(view: &FrameView, t: &MapTransform) -> (Vec<f32>, Vec<f32>) {
        let (x0, y0) = (t.left.round() as i32, t.top.round() as i32);
        let n = t.size.round() as i32;
        let mut luma = vec![0.0f32; (n * n) as usize];
        for y in 0..n {
            for x in 0..n {
                luma[(y * n + x) as usize] = view.luma(x0 + x, y0 + y);
            }
        }
        let at = |x: i32, y: i32| luma[(y * n + x) as usize];
        let mut column = vec![0.0f32; n as usize];
        let mut row = vec![0.0f32; n as usize];
        let mut values = Vec::with_capacity(n as usize);
        for i in LINE_OFFSET..n - LINE_OFFSET {
            values.clear();
            values.extend((0..n).map(|j| (at(i - LINE_OFFSET, j) + at(i + LINE_OFFSET, j)) / 2.0 - at(i, j)));
            let mid = values.len() / 2;
            column[i as usize] = *values.select_nth_unstable_by(mid, f32::total_cmp).1;
            values.clear();
            values.extend((0..n).map(|j| (at(j, i - LINE_OFFSET) + at(j, i + LINE_OFFSET)) / 2.0 - at(j, i)));
            row[i as usize] = *values.select_nth_unstable_by(mid, f32::total_cmp).1;
        }
        (column, row)
    }

    /// Best phase of a comb with `period` over `profile`: (phase, lead over the median phase).
    /// Each tooth counts at most `TOOTH_CAP` (a road or the edge of a transparent band is a far
    /// stronger line than the grid and must not carry a comb on its own); the returned hit share
    /// is the fraction of teeth at the best phase that sit on a line.
    fn comb(profile: &[f32], period: f64) -> (f64, f32, (u32, f32)) {
        let profile: Vec<f32> = profile.iter().map(|&v| v.min(TOOTH_CAP)).collect();
        let n = profile.len() as f64;
        let sample = |x: f64| {
            let i = x.floor() as usize;
            let f = (x - x.floor()) as f32;
            profile[i] * (1.0 - f) + profile[i + 1] * f
        };
        let steps = (period / GRID_PHASE_STEP).ceil() as usize;
        let mut scores = Vec::with_capacity(steps);
        for s in 0..steps {
            let phase = s as f64 * GRID_PHASE_STEP;
            let (mut sum, mut count) = (0.0f32, 0u32);
            let mut x = phase;
            while x < n - 4.0 {
                if x >= 3.0 {
                    sum += sample(x);
                    count += 1;
                }
                x += period;
            }
            scores.push(if count == 0 { 0.0 } else { sum / count as f32 });
        }
        let best = (0..steps).max_by(|&a, &b| scores[a].total_cmp(&scores[b])).unwrap_or(0);
        let mut sorted = scores.clone();
        let mid = sorted.len() / 2;
        let median = *sorted.select_nth_unstable_by(mid, f32::total_cmp).1;
        // Parabolic refinement between neighbouring phases.
        let (l, c, r) = (scores[(best + steps - 1) % steps], scores[best], scores[(best + 1) % steps]);
        let denom = l - 2.0 * c + r;
        let shift = if denom < 0.0 { (0.5 * (l - r) / denom).clamp(-0.5, 0.5) } else { 0.0 };
        let phase = (best as f64 + f64::from(shift)) * GRID_PHASE_STEP;
        let (mut hits, mut teeth) = (0u32, 0u32);
        let mut x = best as f64 * GRID_PHASE_STEP;
        while x < n - 4.0 {
            if x >= 3.0 {
                teeth += 1;
                if sample(x) >= TOOTH_HIT {
                    hits += 1;
                }
            }
            x += period;
        }
        let share = if teeth == 0 { 0.0 } else { hits as f32 / teeth as f32 };
        (phase.rem_euclid(period), c - median, (hits, share))
    }

    /// Comb fits at every searchable zoom level and cell size, best first.
    pub fn candidates(view: &FrameView, t: &MapTransform, map_size_m: f64, cells_m: &[f64]) -> Vec<GridFit> {
        let (x0, y0) = (t.left.round() as i32, t.top.round() as i32);
        let n = t.size.round() as i32;
        if !view.covers(x0, y0, n, n) {
            return Vec::new();
        }
        let (column, row) = Self::profiles(view, t);
        let mut fits: Vec<GridFit> = cells_m
            .iter()
            .flat_map(|&cell_m| (0..ZOOM_LEVELS).map(move |level| (cell_m, level)))
            .filter_map(|(cell_m, level)| {
                let zoom = zoom_level(level);
                let period = cell_m / map_size_m * t.size * zoom;
                if period < GRID_MIN_PERIOD || period > t.size / 2.0 {
                    return None;
                }
                let (px, sx, hx) = Self::comb(&column, period);
                let (py, sy, hy) = Self::comb(&row, period);
                let enough = |(hits, share): (u32, f32)| hits >= MIN_HITS && share >= MIN_HIT_SHARE;
                if !enough(hx) || !enough(hy) {
                    return None;
                }
                Some(GridFit {
                    cell_m,
                    level,
                    zoom,
                    period,
                    phase: (px, py),
                    first_line: (f64::from(x0) + px + 0.5, f64::from(y0) + py + 0.5),
                    score: sx + sy,
                })
            })
            .collect();
        fits.sort_by(|a, b| b.score.total_cmp(&a.score));
        fits
    }

    /// Fits the grid on the full map view of a map `map_size_m` wide; `None` without a clear grid.
    ///
    /// A comb at a multiple of the true period also lands on lines (every k-th one), so the
    /// winner is replaced by the finest level whose period divides it and scores nearly as well.
    pub fn fit(view: &FrameView, t: &MapTransform, map_size_m: f64, cells_m: &[f64]) -> Option<GridFit> {
        let fits = Self::candidates(view, t, map_size_m, cells_m);
        let mut best = *fits.first()?;
        for f in &fits {
            let ratio = best.period / f.period;
            if f.cell_m == best.cell_m
                && f.period < best.period
                && (ratio - ratio.round()).abs() < HARMONIC_TOLERANCE
                && f.score >= HARMONIC_MIN_SHARE * best.score
            {
                best = *f;
            }
        }
        let lead = best.score
            - fits
                .iter()
                .filter(|f| {
                    let ratio = f.period.max(best.period) / f.period.min(best.period);
                    (ratio - ratio.round()).abs() >= HARMONIC_TOLERANCE
                })
                .map(|f| f.score)
                .fold(0.0, f32::max);
        (best.score >= GRID_MIN_SCORE && lead >= GRID_MIN_LEAD).then_some(best)
    }

    /// Container origins (map coordinates of the container's top-left corner) consistent with
    /// the grid, one per whole-cell shift, limited to views inside the canvas.
    pub fn origins(&self, t: &MapTransform, map_size_m: f64) -> (Vec<f64>, Vec<f64>) {
        let cell = self.cell_m / map_size_m;
        let first = grid_origin_m(map_size_m, self.cell_m) / map_size_m;
        let span = 1.0 / self.zoom;
        let axis = |offset: f64| {
            // The first visible line is grid line k: origin = first + k*cell - offset/scale.
            let shift = offset / (t.size * self.zoom);
            (0..=((1.0 - first) / cell).ceil() as i64 + 1)
                .map(|k| first + k as f64 * cell - shift)
                .filter(|&o| o >= -0.5 * cell && o <= 1.0 - span + 0.5 * cell)
                .collect::<Vec<_>>()
        };
        (axis(self.first_line.0 - t.left), axis(self.first_line.1 - t.top))
    }

    /// Container origin when the first visible vertical/horizontal line is grid column/row
    /// (`column`, `row`) (0 = A / 1).
    pub fn origin_for(&self, t: &MapTransform, map_size_m: f64, column: i64, row: i64) -> (f64, f64) {
        let cell = self.cell_m / map_size_m;
        let first = grid_origin_m(map_size_m, self.cell_m) / map_size_m;
        let scale = t.size * self.zoom;
        (
            first + column as f64 * cell - (self.first_line.0 - t.left) / scale,
            first + row as f64 * cell - (self.first_line.1 - t.top) / scale,
        )
    }
}

// ---------------------------------------------------------------------------------------------
// Reference correlation

/// Coarse search level: the smallest pyramid size where the visible window spans at least this
/// many pixels.
const MIN_PATCH: usize = 48;
/// A whole-canvas search always starts at the 128 level while the window spans this many pixels.
const FULL_MIN_PATCH: usize = 16;
/// Pyramid size where zoom levels are compared before the winner is refined to full resolution.
const SELECT_SIZE: usize = 256;
const COARSE_CANDIDATES: usize = 6;
/// Coarse picks followed down the pyramid.
const REFINED_CANDIDATES: usize = 3;
/// Search radius at the coarse level (grid candidates are rounded) and at each finer level.
const COARSE_REFINE_RADIUS: i64 = 2;
const FINE_REFINE_RADIUS: i64 = 1;
/// Pyramid size used to check the unzoomed hypothesis.
const FIXED_CHECK_SIZE: usize = 256;
const MASK_MIN_ALPHA: f32 = 0.8;
/// At least this fraction of the window must be opaque reference to score it.
const MIN_OPAQUE: f32 = 0.3;

struct Level {
    size: usize,
    grad: Plane,
    /// Reference pixels opaque enough to score (opacity at least `MASK_MIN_ALPHA`), a byte a pixel
    /// instead of the opacity's four.
    opaque: Vec<bool>,
}

/// Gradient pyramid of one reference (1024, 512, 256, 128).
pub struct ReferenceMatcher {
    levels: Vec<Level>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RefMatch {
    pub origin: (f64, f64),
    pub score: f32,
    /// Lead over the best refined candidate at another position.
    pub margin: f32,
}

/// Masked NCC of `patch` (n×n) against the reference window at integer (x, y).
fn window_ncc(level: &Level, patch: &Plane, x: i64, y: i64) -> Option<f32> {
    let n = patch.width as i64;
    if x < 0 || y < 0 || x + n > level.size as i64 || y + n > level.size as i64 {
        return None;
    }
    let (mut c, mut sf, mut sr, mut sff, mut srr, mut sfr) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for j in 0..n as usize {
        let row = (y as usize + j) * level.size + x as usize;
        for i in 0..n as usize {
            if !level.opaque[row + i] {
                continue;
            }
            let f = f64::from(patch.data[j * patch.width + i]);
            let r = f64::from(level.grad.data[row + i]);
            c += 1.0;
            sf += f;
            sr += r;
            sff += f * f;
            srr += r * r;
            sfr += f * r;
        }
    }
    if c < f64::from(MIN_OPAQUE) * (n * n) as f64 {
        return None;
    }
    let vf = sff - sf * sf / c;
    let vr = srr - sr * sr / c;
    if vf <= 0.0 || vr <= 0.0 {
        return None;
    }
    Some(((sfr - sf * sr / c) / (vf * vr).sqrt()) as f32)
}

impl ReferenceMatcher {
    pub fn new(reference: &MapReference) -> Self {
        Self::with_levels(reference, |_| true)
    }

    /// Only the level the unzoomed check uses (`fixed_score`), for a registrar whose zoomed
    /// searches go to another reference: 0.3 MB instead of 7.
    fn fixed_only(reference: &MapReference) -> Self {
        Self::with_levels(reference, |size| size == FIXED_CHECK_SIZE)
    }

    fn with_levels(reference: &MapReference, keep: impl Fn(usize) -> bool) -> Self {
        let mut levels = Vec::new();
        let mut size = WORK_SIZE;
        while size >= 128 {
            if keep(size) {
                let gray = reference.gray.resample(size, size);
                levels.push(Level {
                    size,
                    grad: gray.gradient(),
                    opaque: reference.alpha.resample(size, size).data.iter().map(|&a| a >= MASK_MIN_ALPHA).collect(),
                });
            }
            size /= 2;
        }
        Self { levels }
    }

    /// Gradient of the visible window sampled at pyramid size `size`: an n×n patch whose pixel
    /// (i, j) is reference pixel (origin*size + i, origin*size + j).
    fn patch(view: &FrameView, t: &MapTransform, zoom: f64, size: usize) -> Plane {
        let n = (size as f64 / zoom).floor() as usize;
        let span = n as f64 * t.size * zoom / size as f64;
        area_resample(|x, y| view.luma(x, y), (t.left, t.top), (span, span), (n, n)).gradient()
    }

    /// Correlation of the unzoomed hypothesis (the container shows the whole canvas).
    pub fn fixed_score(&self, view: &FrameView, t: &MapTransform) -> Option<f32> {
        let level = self.levels.iter().find(|l| l.size == FIXED_CHECK_SIZE)?;
        window_ncc(level, &Self::patch(view, t, 1.0, level.size), 0, 0)
    }

    /// Correlation at one origin, at the coarsest level where the window spans MIN_PATCH pixels.
    pub fn score_at(&self, view: &FrameView, t: &MapTransform, zoom: f64, origin: (f64, f64)) -> Option<f32> {
        let li = self.levels.iter().rposition(|l| (l.size as f64 / zoom) >= MIN_PATCH as f64)?;
        let l = &self.levels[li];
        let size = l.size as f64;
        window_ncc(l, &Self::patch(view, t, zoom, l.size), (origin.0 * size).round() as i64, (origin.1 * size).round() as i64)
    }

    /// Pyramid index of a level size.
    fn level_index(&self, size: usize) -> usize {
        self.levels.iter().position(|l| l.size == size).unwrap_or(0)
    }

    /// Best origin at `zoom` among the positions `search` allows, refined down to pyramid size
    /// `finest` (a cheap pass stops at 256 to compare zoom levels; the final one goes to 1024).
    pub fn locate(&self, view: &FrameView, t: &MapTransform, zoom: f64, search: &Search, finest: usize) -> Option<RefMatch> {
        let smallest = self.levels.len() - 1;
        let coarse = match search {
            Search::Full if (self.levels[smallest].size as f64 / zoom) >= FULL_MIN_PATCH as f64 => smallest,
            _ => self
                .levels
                .iter()
                .rposition(|l| (l.size as f64 / zoom) >= MIN_PATCH as f64)?,
        };
        let finest = self.level_index(finest).min(coarse);
        let level = &self.levels[coarse];
        let patch = Self::patch(view, t, zoom, level.size);
        let n = patch.width as i64;
        let size = level.size as f64;
        let last = level.size as i64 - n;
        let mut scored: Vec<(f32, i64, i64)> = Vec::new();
        let mut score_at = |x: i64, y: i64| {
            if let Some(s) = window_ncc(level, &patch, x, y) {
                scored.push((s, x, y));
            }
        };
        match search {
            Search::Full => {
                for y in 0..=last {
                    for x in 0..=last {
                        score_at(x, y);
                    }
                }
            }
            Search::Grid(us, vs) => {
                for &v in vs {
                    for &u in us {
                        score_at((u * size).round() as i64, (v * size).round() as i64);
                    }
                }
            }
            Search::Near { origin, radius } => {
                let (cx, cy) = ((origin.0 * size).round() as i64, (origin.1 * size).round() as i64);
                let r = (radius * size).ceil() as i64;
                for y in (cy - r).max(0)..=(cy + r).min(last) {
                    for x in (cx - r).max(0)..=(cx + r).min(last) {
                        score_at(x, y);
                    }
                }
            }
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        // Non-maximum suppression: candidates at least a quarter patch apart.
        let sep = (n / 4).max(2);
        let mut picks: Vec<(f32, i64, i64)> = Vec::new();
        for &c in &scored {
            if picks.iter().all(|p| (p.1 - c.1).abs() > sep || (p.2 - c.2).abs() > sep) {
                picks.push(c);
                if picks.len() == COARSE_CANDIDATES {
                    break;
                }
            }
        }
        let mut patches: Vec<Option<Plane>> = (0..self.levels.len()).map(|_| None).collect();
        patches[coarse] = Some(patch);
        let mut refined: Vec<(f32, f64, f64)> = picks
            .iter()
            .take(REFINED_CANDIDATES)
            .filter_map(|&(_, x, y)| self.refine(view, t, zoom, (coarse, finest), x, y, &mut patches))
            .collect();
        refined.sort_by(|a, b| b.0.total_cmp(&a.0));
        let best = *refined.first()?;
        // Runner-up: the best refined candidate that did not converge onto the winner.
        let apart = 2.0 / WORK_SIZE as f64;
        let second = refined
            .iter()
            .skip(1)
            .find(|r| (r.1 - best.1).abs() > apart || (r.2 - best.2).abs() > apart)
            .map_or(0.0, |r| r.0);
        Some(RefMatch {
            origin: (best.1, best.2),
            score: best.0,
            margin: best.0 - second,
        })
    }

    /// Follows a coarse position down the pyramid to `finest`, with a parabolic sub-pixel step
    /// there: (score, u, v).
    #[allow(clippy::too_many_arguments)]
    fn refine(
        &self,
        view: &FrameView,
        t: &MapTransform,
        zoom: f64,
        (coarse, finest): (usize, usize),
        x: i64,
        y: i64,
        patches: &mut [Option<Plane>],
    ) -> Option<(f32, f64, f64)> {
        let (mut x, mut y) = (x, y);
        let mut result = None;
        for li in (finest..=coarse).rev() {
            let l = &self.levels[li];
            let radius = if li == coarse {
                COARSE_REFINE_RADIUS
            } else {
                x *= 2;
                y *= 2;
                FINE_REFINE_RADIUS
            };
            let p = patches[li].get_or_insert_with(|| Self::patch(view, t, zoom, l.size));
            let p = &*p;
            let mut top = (f32::MIN, x, y);
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if let Some(s) = window_ncc(l, p, x + dx, y + dy)
                        && s > top.0
                    {
                        top = (s, x + dx, y + dy);
                    }
                }
            }
            if top.0 == f32::MIN {
                return None;
            }
            (x, y) = (top.1, top.2);
            let (sx, sy) = if li == finest {
                let at = |dx: i64, dy: i64| window_ncc(l, p, x + dx, y + dy).unwrap_or(top.0);
                (parabola(at(-1, 0), top.0, at(1, 0)), parabola(at(0, -1), top.0, at(0, 1)))
            } else {
                (0.0, 0.0)
            };
            result = Some((top.0, (x as f64 + sx) / l.size as f64, (y as f64 + sy) / l.size as f64));
        }
        result
    }
}

/// Vertex offset of the parabola through (-1, l), (0, c), (1, r), within ±0.5.
fn parabola(l: f32, c: f32, r: f32) -> f64 {
    let d = l - 2.0 * c + r;
    if d < 0.0 { f64::from((0.5 * (l - r) / d).clamp(-0.5, 0.5)) } else { 0.0 }
}

/// Which origins [`ReferenceMatcher::locate`] scores at the coarse level.
pub enum Search {
    /// Every origin on the canvas.
    Full,
    /// Grid-consistent origins (u values × v values).
    Grid(Vec<f64>, Vec<f64>),
    /// Origins within `radius` (map units, per axis) of a prediction.
    Near { origin: (f64, f64), radius: f64 },
}

// ---------------------------------------------------------------------------------------------
// Registrar

/// Below this the correlation is not trusted on its own.
const REF_MIN_SCORE: f32 = 0.15;
const REF_MIN_MARGIN: f32 = 0.05;
const FIXED_MIN_SCORE: f32 = 0.3;
/// An unzoomed-hypothesis score this high but short of `FIXED_MIN_SCORE` is a 1× view whose
/// reference fits the game's picture poorly, not a zoomed view: Zone 11 scored 0.19–0.34 at 1× and
/// at most 0.10 zoomed. Such a frame gets no search without the grid, which took about 1 s over
/// every zoom and found 5× at 0.16–0.18 there.
pub const UNZOOMED_HINT: f32 = 0.15;
/// A tracked origin may move at most this fraction of a cell between frames to keep its cell.
const TRACK_MAX_CELL_FRACTION: f64 = 0.25;
/// Label reading is trusted with at least this many agreeing labels, and this share of all read.
const LABEL_MIN_VOTES: usize = 2;
const LABEL_MIN_AGREEMENT: f64 = 0.6;
/// Wheel ticks searched either side of the previous zoom level.
const LOCAL_TICKS: i64 = 3;
/// Local search radius in map units at zoom 1 (scaled by 1/zoom).
const LOCAL_RADIUS: f64 = 0.08;
/// A tracked cell without readable labels is kept when the reference still correlates this well,
/// beats each of its eight neighbouring cells by `TRACK_NEIGHBOUR_MARGIN` (a one-cell slip fits
/// the grid exactly) and is within `TRACK_GLOBAL_SLACK` of the best grid cell the reference search
/// found (a prediction off by several cells after a fast zoom or drag).
const GRID_TRACK_MIN_SCORE: f32 = 0.2;
const TRACK_NEIGHBOUR_MARGIN: f32 = 0.03;
const TRACK_GLOBAL_SLACK: f32 = 0.02;
/// A grid-consistent cell picked by correlation needs this score.
const GRID_REF_MIN_SCORE: f32 = 0.3;
/// With a prior (the canvas the input led to), the search without the grid looks this many wheel
/// ticks either side of its zoom, and this far around its origin (map units at zoom 1, scaled by
/// 1/zoom); the whole canvas at every zoom (seconds of CPU at 5×, 0.45–0.71 core measured on
/// Normandy) only this often (s) while it does not confirm.
const PRIOR_TICKS: i64 = 1;
const PRIOR_RADIUS: f64 = 0.04;
const FULL_SEARCH_INTERVAL_S: f64 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Reference correlation without a grid.
    Reference,
    /// Grid phase, cell read from the grid labels.
    GridLabels,
    /// Grid phase, cell picked by reference correlation.
    GridReference,
    /// Grid phase, cell carried over from the previous registration.
    GridTracked,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Registration {
    pub transform: MapTransform,
    pub method: Method,
    pub score: f32,
    pub grid: Option<GridFit>,
}

/// Registers V1 frame by frame for one map.
pub struct V1Registrar {
    map_size_m: f64,
    /// Cell sizes still possible: the measured one, or all until labels settle it.
    cells_m: Vec<f64>,
    /// The reference for the unzoomed check.
    matcher: ReferenceMatcher,
    /// The reference for zoomed searches when it differs: a game-made one with its screen-size
    /// icons masked out (they do not scale with the zoom).
    zoomed: Option<ReferenceMatcher>,
    labels: LabelReader,
    last: Option<Registration>,
    /// The last frame's unzoomed-hypothesis score.
    unzoomed_score: Option<f32>,
    /// When the whole-canvas search last ran with a prior given.
    full_search_at: Option<std::time::Instant>,
}

impl V1Registrar {
    /// `cell_m`: the map's measured grid cell size (`None` tries [`CELL_SIZES_M`]).
    pub fn new(reference: &MapReference, cell_m: Option<f64>) -> Self {
        Self::new_with(ReferenceMatcher::new(reference), None, reference.size_m, cell_m)
    }

    fn new_with(matcher: ReferenceMatcher, zoomed: Option<ReferenceMatcher>, map_size_m: f64, cell_m: Option<f64>) -> Self {
        Self {
            map_size_m,
            cells_m: cell_m.map_or(CELL_SIZES_M.to_vec(), |c| vec![c]),
            matcher,
            zoomed,
            labels: LabelReader::new(),
            last: None,
            unzoomed_score: None,
            full_search_at: None,
        }
    }

    /// `new` with another reference for the zoomed searches (`zoomed`: the same picture with what
    /// does not scale with the zoom masked out).
    pub fn with_zoom_reference(reference: &MapReference, zoomed: &MapReference, cell_m: Option<f64>) -> Self {
        Self::new_with(ReferenceMatcher::fixed_only(reference), Some(ReferenceMatcher::new(zoomed)), reference.size_m, cell_m)
    }

    /// The last registered frame's unzoomed-hypothesis score (how well the reference fits it as
    /// the whole map; a 1× frame scores at least `UNZOOMED_HINT`).
    pub fn unzoomed_score(&self) -> Option<f32> {
        self.unzoomed_score
    }

    /// The matcher for zoomed searches.
    fn zoom_matcher(&self) -> &ReferenceMatcher {
        self.zoomed.as_ref().unwrap_or(&self.matcher)
    }

    pub fn last(&self) -> Option<&Registration> {
        self.last.as_ref()
    }

    pub fn forget(&mut self) {
        self.last = None;
    }

    /// Registers the full map in `view`; `cursor` is the frame-space cursor position at capture
    /// (zooming keeps the map point under it fixed).
    pub fn register(&mut self, view: &FrameView, client_origin: (i32, i32), cursor: Option<(f64, f64)>) -> Option<Registration> {
        self.register_with_prior(view, client_origin, cursor, None)
    }

    /// As `register`, with `prior`: where the view is expected to be (the canvas the wheel and
    /// drags led to). Without the grid it is checked around the prior first, and the whole canvas
    /// is searched at most every `FULL_SEARCH_INTERVAL_S` while the prior does not confirm.
    pub fn register_with_prior(&mut self, view: &FrameView, client_origin: (i32, i32), cursor: Option<(f64, f64)>, prior: Option<MapTransform>) -> Option<Registration> {
        let base = MapTransform::fixed(MapView::Full, client_origin);
        let fixed = self.matcher.fixed_score(view, &base);
        self.unzoomed_score = fixed;
        let unzoomed = fixed.filter(|s| *s >= FIXED_MIN_SCORE).map(|score| Registration {
            transform: MapTransform::fixed(MapView::Full, origin_of(&base)),
            method: Method::Reference,
            score,
            grid: None,
        });
        let looks_unzoomed = fixed.is_some_and(|s| s >= UNZOOMED_HINT);
        let result = unzoomed
            .or_else(|| GridFit::fit(view, &base, self.map_size_m, &self.cells_m).and_then(|grid| self.with_grid(view, &base, grid, cursor)))
            .or_else(|| if looks_unzoomed { None } else { self.without_grid_prior(view, &base, cursor, prior) });
        if let Some(r) = result {
            if r.method == Method::GridLabels
                && let Some(grid) = r.grid
            {
                self.cells_m = vec![grid.cell_m];
            }
            self.last = result;
        }
        result
    }

    fn with_grid(&self, view: &FrameView, base: &MapTransform, grid: GridFit, cursor: Option<(f64, f64)>) -> Option<Registration> {
        if let Some(vote) = self.labels.read(view, base, &grid) {
            let agree = |votes: usize| votes >= LABEL_MIN_VOTES && votes as f64 >= LABEL_MIN_AGREEMENT * vote.labels as f64;
            if agree(vote.column_votes) && agree(vote.row_votes) {
                let origin = grid.origin_for(base, self.map_size_m, vote.column, vote.row);
                return Some(Registration {
                    transform: MapTransform::zoomed(MapView::Full, origin_of(base), grid.zoom, origin),
                    method: Method::GridLabels,
                    score: grid.score,
                    grid: Some(grid),
                });
            }
        }
        let candidates = grid.origins(base, self.map_size_m);
        let cell = grid.cell_m / self.map_size_m;
        // Absolute first: the reference over every grid-consistent origin.
        let located = self
            .zoom_matcher()
            .locate(view, base, grid.zoom, &Search::Grid(candidates.0.clone(), candidates.1.clone()), WORK_SIZE);
        if let Some(m) = located
            && m.score >= GRID_REF_MIN_SCORE
            && m.margin >= REF_MIN_MARGIN
        {
            // Snap to the grid-consistent origin nearest to the correlation result.
            let u = nearest_any(&candidates.0, m.origin.0);
            let v = nearest_any(&candidates.1, m.origin.1);
            return Some(Registration {
                transform: MapTransform::zoomed(MapView::Full, origin_of(base), grid.zoom, (u, v)),
                method: Method::GridReference,
                score: m.score,
                grid: Some(grid),
            });
        }
        // Ambiguous reference: carry the cell over from the previous registration (same view, or a
        // cursor-centred zoom), checked against its neighbours and the best cell found.
        let predicted = self.last.map(|last| {
            let lt = last.transform;
            if (lt.zoom - grid.zoom).abs() < 1e-6 {
                lt.origin
            } else {
                let (cx, cy) = cursor.unwrap_or((base.left + base.size / 2.0, base.top + base.size / 2.0));
                let (mu, mv) = lt.to_map(cx, cy);
                let s = base.size * grid.zoom;
                (mu - (cx - base.left) / s, mv - (cy - base.top) / s)
            }
        });
        let nearest = |values: &[f64], target: f64| {
            values
                .iter()
                .copied()
                .min_by(|a, b| (a - target).abs().total_cmp(&(b - target).abs()))
                .filter(|v| (v - target).abs() <= TRACK_MAX_CELL_FRACTION * cell)
        };
        let p = predicted?;
        let (u, v) = (nearest(&candidates.0, p.0)?, nearest(&candidates.1, p.1)?);
        let tracked = self.zoom_matcher().score_at(view, base, grid.zoom, (u, v))?;
        let best_neighbour = (-1..=1i32)
            .flat_map(|dv| (-1..=1i32).map(move |du| (du, dv)))
            .filter(|&(du, dv)| (du, dv) != (0, 0))
            .filter_map(|(du, dv)| self.zoom_matcher().score_at(view, base, grid.zoom, (u + f64::from(du) * cell, v + f64::from(dv) * cell)))
            .fold(f32::MIN, f32::max);
        let keeps_up = located.is_none_or(|m| tracked >= m.score - TRACK_GLOBAL_SLACK);
        (tracked >= GRID_TRACK_MIN_SCORE && tracked - best_neighbour >= TRACK_NEIGHBOUR_MARGIN && keeps_up).then(|| Registration {
            transform: MapTransform::zoomed(MapView::Full, origin_of(base), grid.zoom, (u, v)),
            method: Method::GridTracked,
            score: grid.score,
            grid: Some(grid),
        })
    }

    /// Development: the fallback used when no grid is found, from a given previous registration.
    #[doc(hidden)]
    pub fn without_grid_from(&mut self, last: Option<Registration>, view: &FrameView, client_origin: (i32, i32), cursor: Option<(f64, f64)>) -> Option<Registration> {
        self.last = last;
        self.without_grid(view, &MapTransform::fixed(MapView::Full, client_origin), cursor)
    }

    /// `without_grid`, around `prior` first when there is one.
    fn without_grid_prior(&mut self, view: &FrameView, base: &MapTransform, cursor: Option<(f64, f64)>, prior: Option<MapTransform>) -> Option<Registration> {
        let Some(p) = prior.filter(|p| p.zoom > 1.0 + 1e-6) else { return self.without_grid(view, base, cursor) };
        let level = ((p.zoom - 1.0) / ZOOM_STEP).round() as i64;
        let searches: Vec<(usize, Search)> = ((level - PRIOR_TICKS).max(1)..=(level + PRIOR_TICKS).min(ZOOM_LEVELS as i64 - 1))
            .map(|k| {
                let zoom = zoom_level(k as usize);
                // The prior's centre kept in place at the neighbouring zooms.
                let (cu, cv) = p.to_map(base.left + base.size / 2.0, base.top + base.size / 2.0);
                let origin = (cu - 0.5 / zoom, cv - 0.5 / zoom);
                (k as usize, Search::Near { origin, radius: PRIOR_RADIUS / zoom })
            })
            .collect();
        if let Some((level, m)) = self.best_level(view, base, searches)
            && m.score >= REF_MIN_SCORE
        {
            return Some(Registration { transform: MapTransform::zoomed(MapView::Full, origin_of(base), zoom_level(level), m.origin), method: Method::Reference, score: m.score, grid: None });
        }
        if self.full_search_at.is_some_and(|t| t.elapsed().as_secs_f64() < FULL_SEARCH_INTERVAL_S) {
            return None;
        }
        self.full_search_at = Some(std::time::Instant::now());
        self.without_grid(view, base, cursor)
    }

    fn without_grid(&self, view: &FrameView, base: &MapTransform, cursor: Option<(f64, f64)>) -> Option<Registration> {
        let accept = |level: usize, m: RefMatch, needs_margin: bool| {
            (m.score >= REF_MIN_SCORE && (!needs_margin || m.margin >= REF_MIN_MARGIN)).then(|| Registration {
                transform: MapTransform::zoomed(MapView::Full, origin_of(base), zoom_level(level), m.origin),
                method: Method::Reference,
                score: m.score,
                grid: None,
            })
        };
        // Near the previous registration: a few wheel ticks either way, cursor-centred.
        if let Some(last) = self.last {
            let lt = last.transform;
            let level = ((lt.zoom - 1.0) / ZOOM_STEP).round() as i64;
            let (cx, cy) = cursor.unwrap_or((base.left + base.size / 2.0, base.top + base.size / 2.0));
            let (mu, mv) = lt.to_map(cx, cy);
            let searches: Vec<(usize, Search)> = ((level - LOCAL_TICKS).max(1)..=(level + LOCAL_TICKS).min(ZOOM_LEVELS as i64 - 1))
                .map(|k| {
                    let zoom = zoom_level(k as usize);
                    let s = base.size * zoom;
                    let origin = (mu - (cx - base.left) / s, mv - (cy - base.top) / s);
                    (k as usize, Search::Near { origin, radius: LOCAL_RADIUS / zoom })
                })
                .collect();
            if let Some((level, m)) = self.best_level(view, base, searches)
                && let Some(r) = accept(level, m, false)
            {
                return Some(r);
            }
        }
        // Whole canvas at every zoom.
        let searches = (1..ZOOM_LEVELS).map(|k| (k, Search::Full)).collect();
        let best = self.best_level(view, base, searches);
        best.and_then(|(level, m)| accept(level, m, true))
    }
}

impl V1Registrar {
    /// Compares zoom levels at the selection size, then refines the winner to full resolution.
    fn best_level(&self, view: &FrameView, base: &MapTransform, searches: Vec<(usize, Search)>) -> Option<(usize, RefMatch)> {
        let mut best: Option<(usize, RefMatch)> = None;
        for (level, search) in &searches {
            if let Some(m) = self.zoom_matcher().locate(view, base, zoom_level(*level), search, SELECT_SIZE)
                && best.is_none_or(|(_, b)| m.score > b.score)
            {
                best = Some((*level, m));
            }
        }
        let (level, coarse) = best?;
        let fine = self.zoom_matcher().locate(
            view,
            base,
            zoom_level(level),
            &Search::Near { origin: coarse.origin, radius: 2.0 / SELECT_SIZE as f64 },
            WORK_SIZE,
        )?;
        Some((level, RefMatch { margin: coarse.margin, ..fine }))
    }
}

fn nearest_any(values: &[f64], target: f64) -> f64 {
    values
        .iter()
        .copied()
        .min_by(|a, b| (a - target).abs().total_cmp(&(b - target).abs()))
        .unwrap_or(target)
}

/// Client origin recovered from a fixed V1 transform.
fn origin_of(base: &MapTransform) -> (i32, i32) {
    let c = MapView::Full.container();
    ((base.left - c.x).round() as i32, (base.top - c.y).round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_origin_matches_measured_maps() {
        // Fulvia Gap 7992 studs, Normandy 5976 studs (1 stud = 0.36 m).
        assert!((grid_origin_m(7992.0 * 0.36, 100.0) + 11.44).abs() < 0.01);
        assert!((grid_origin_m(5976.0 * 0.36, 100.0) + 74.32).abs() < 0.01);
        // Rohkstov (14996 studs) uses 200 m cells: column L starts at 2199.3 m.
        assert!((grid_origin_m(14996.0 * 0.36, 200.0) + 0.72).abs() < 0.01);
    }
}
