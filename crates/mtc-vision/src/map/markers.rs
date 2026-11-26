//! Target markers: hollow green diamonds, with an "A1"-style label in V1 and V2.
//!
//! Markers are drawn at screen scale: a 29–30 px outline (2 px thick) in V1 and V2, 12 px in V3,
//! pure green. The outline is symmetric, so its pixel centroid is the marker point. The label sits
//! about 11 px right of and 26 px above the centre in the grid-label font, so the grid glyph
//! templates read it from green ink.
//!
//! Objective icons are drawn over markers. When the diamond is hidden or cut but the label
//! shows, the label alone places the marker: its first digit's left edge and its baseline sit at
//! fixed screen offsets from the marked point (measured on complete sightings). The digit is used
//! rather than the letter because an objective icon usually covers the label's left part.
//!
//! The letter tells whose marker it is (one letter per squad member, by joining order). A letter
//! narrower than its template or not right before the first digit (`MAX_LETTER_GAP`), or missing,
//! is cut by an icon and reported as `?`. Letters differ in width (A 9 px, B and C 7 px), so where
//! the first digit starts depends on the letter (`DIGIT_LEFT` is measured on A labels). On the
//! squad recording's labelled frames (`examples/marker_label_offset.rs`) the first digit sits at
//! +18.76 (A11–A16), +19.19 (B21–B26), +18.45 (C1–C4): B and C within 0.45 px of A, inside the
//! label placement's tolerance, so one table serves every letter.

use crate::map::geometry::{MapTransform, MapView};
use crate::map::labels::LabelReader;
use crate::map::raster::area_resample;
use crate::view::FrameView;

/// Marker green: hue 100–140°, saturation and value well up (measured H≈118°, S≥0.87).
const MIN_SATURATION: f32 = 0.6;
const MIN_VALUE: u8 = 120;
/// Marker and label ink has blue at most red + 1 (55 000 pixels of labelled markers); the fill of
/// a green objective icon, (39, 127, 72), has blue − red = 33 and would join
/// a label or diamond it touches into one icon-sized blob.
const MAX_BLUE_OVER_RED: u8 = 8;
/// Diamond bounding box sides per view (nominal 29–30 px in V1/V2, 12 px in V3).
const FULL_SIZE: (u32, u32) = (25, 33);
const MINI_SIZE: (u32, u32) = (10, 15);
const FULL_NOMINAL: (u32, u32) = (28, 31);
const MINI_NOMINAL: (u32, u32) = (11, 13);
/// The outline covers at most this share of its bounding box (filled objective icons cover more).
const MAX_FILL: f32 = 0.5;
/// Fewest over most pixels among the four edges of a whole outline: median 0.90, 1st percentile
/// 0.47 over 2042 sightings; Sokolovka's A1 in V2, one edge under objective A's icon, is 0.47.
const MIN_EDGE_BALANCE: f64 = 0.55;
/// The diamond's pixel centroid sits this far from the marked point (frame px). Measured by
/// comparing markers unzoomed with the same markers at 3–5× (where the offset shrinks to a fifth):
/// (−1.1, −0.7) ± 0.2 for the 30 px diamond; the 12 px V3 diamond fits (−0.7, −0.7). Icons are
/// snapped to whole pixels, so single sightings still scatter ±0.5 px around it.
const ICON_OFFSET_FULL: (f64, f64) = (-1.1, -0.7);
const ICON_OFFSET_MINI: (f64, f64) = (-0.7, -0.7);
/// Label box relative to the diamond centre (frame px).
const LABEL_DX: i32 = 5;
const LABEL_DY: i32 = -31;
const LABEL_W: usize = 42;
/// Reaches 14 px above the centre; slivers of the diamond outline are dropped as non-glyphs.
const LABEL_H: usize = 18;
/// Rows added below the label box at another icon scale: at 125 % the label sits up to 2 px lower
/// against the diamond in V2 than in V1, and the box cut it.
const LABEL_SCALED_MARGIN: usize = 3;
/// Green ink strength: value above this on green-hued pixels.
const LABEL_INK_FLOOR: f32 = 90.0;
/// Left and top edge of the label's ink box relative to the marked point (frame px), measured on
/// complete labelled sightings in V1 at zoom 1–5 and V2 (`examples/marker_label_offset.rs`):
/// +9.32 ± 0.38, −26.96 ± 0.45.
pub const LABEL_INK_OFFSET: (f64, f64) = (9.32, -26.96);
/// First digit's left edge relative to the marked point (frame px), per digit 1–9, measured on
/// complete sightings (`examples/marker_label_offset.rs`; 7–9 not yet seen, set to the mean).
const DIGIT_LEFT: [f64; 9] = [19.46, 19.43, 19.24, 19.26, 18.91, 19.52, 19.30, 19.30, 19.30];
/// Glyph bottom edges (the baseline) relative to the marked point: −15.98 ± 0.2.
const LABEL_BASELINE: f64 = -15.98;
/// Most columns between a whole letter's right edge and the first digit (A and C: 1, B: 2).
const MAX_LETTER_GAP: f64 = 2.5;
/// Stands for a letter cut by an icon.
pub const CUT_LETTER: char = '?';
/// A label glyph has ink in at least this many rows (slivers of the diamond outline have fewer).
const LABEL_MIN_GLYPH_ROWS: usize = 6;
/// Label glyph blobs: rows and columns.
const GLYPH_ROWS: (usize, usize) = (8, 13);
const GLYPH_COLS: (usize, usize) = (2, 14);
/// Glyphs of one label: tops within this many px, horizontal gap at most this.
const WORD_TOP_TOLERANCE: usize = 2;
const WORD_MAX_GAP: usize = 5;
/// A label within this (frame px) of where a diamond's label would be belongs to that diamond.
const LABEL_MATCH_PX: f64 = 3.0;
/// A partial diamond within this (frame px) of a label-derived point is that marker.
const PARTIAL_MATCH_PX: f64 = 10.0;

/// One marker seen in one view.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkerSighting {
    /// Continuous frame coordinates of the marked point (diamond centroid, icon offset removed).
    pub frame: (f64, f64),
    /// Normalised map coordinates.
    pub map: (f64, f64),
    /// Label text ("A3") where the view shows and it could be read.
    pub label: Option<String>,
    /// Position measured from the whole outline at nominal size, or from the label when the
    /// outline is hidden or cut (`from_label`).
    pub complete: bool,
    /// Placed by its label (diamond covered by an objective icon or cut).
    pub from_label: bool,
}

pub struct MarkerReader {
    glyphs: LabelReader,
    /// V1/V2 sizes and offsets at this configuration's icon scale.
    full: FullGeometry,
}

/// The V1/V2 marker's sizes and offsets (frame px) at an icon scale `k`: the markers and their
/// labels are drawn at the HUD's scale, the map at the map frame's, so once a frame is mapped to
/// the reference geometry by the map frame, the icons are `k` = HUD scale / map frame scale times
/// their 150 % size (e.g. 0.885 at 125 % and 0.772 at 100 %). At `k` = 1 every value is
/// the constant it comes from. V3's minimap frame scales with the HUD: its markers keep their size.
#[derive(Clone, Copy, Debug)]
struct FullGeometry {
    k: f64,
    size: (u32, u32),
    nominal: (u32, u32),
    icon_offset: (f64, f64),
    label_dx: i32,
    label_dy: i32,
    label_w: usize,
    label_h: usize,
    /// Rows of the reference box the label box is resampled to: `LABEL_H`, and
    /// `LABEL_SCALED_MARGIN` more at another scale.
    ref_h: usize,
    ink_offset: (f64, f64),
    digit_left: [f64; 9],
    baseline: f64,
    letter_gap: f64,
    glyph_rows: (usize, usize),
    glyph_cols: (usize, usize),
    word_top: usize,
    word_gap: usize,
    label_match: f64,
    partial_match: f64,
    /// Margin (px) left of and above a label's ink to its box.
    ink_margin: i32,
    min_glyph_rows: usize,
    icon_reach: f64,
}

impl FullGeometry {
    fn at(k: f64) -> Self {
        let ref_h = if k == 1.0 { LABEL_H } else { LABEL_H + LABEL_SCALED_MARGIN };
        let i = |v: i32| (f64::from(v) * k).round() as i32;
        let u = |v: usize| ((v as f64) * k).round() as usize;
        let range = |(a, b): (u32, u32)| ((f64::from(a) * k).floor() as u32, (f64::from(b) * k).ceil() as u32);
        Self {
            k,
            size: range(FULL_SIZE),
            nominal: range(FULL_NOMINAL),
            icon_offset: (ICON_OFFSET_FULL.0 * k, ICON_OFFSET_FULL.1 * k),
            label_dx: i(LABEL_DX),
            label_dy: i(LABEL_DY),
            label_w: ((LABEL_W as f64) * k).ceil() as usize,
            label_h: ((ref_h as f64) * k).ceil() as usize,
            ref_h,
            ink_offset: (LABEL_INK_OFFSET.0 * k, LABEL_INK_OFFSET.1 * k),
            digit_left: DIGIT_LEFT.map(|d| d * k),
            baseline: LABEL_BASELINE * k,
            letter_gap: MAX_LETTER_GAP * k,
            glyph_rows: (u(GLYPH_ROWS.0), u(GLYPH_ROWS.1)),
            glyph_cols: (u(GLYPH_COLS.0).max(1), u(GLYPH_COLS.1)),
            word_top: u(WORD_TOP_TOLERANCE),
            word_gap: u(WORD_MAX_GAP),
            label_match: LABEL_MATCH_PX * k,
            partial_match: PARTIAL_MATCH_PX * k,
            ink_margin: i(4),
            min_glyph_rows: u(LABEL_MIN_GLYPH_ROWS),
            icon_reach: VEHICLE_ICON_REACH * k,
        }
    }
}

impl Default for MarkerReader {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimap spot check (`minimap_spot_clear`): square of this radius (frame px) around the point;
/// at most this many pixels may look like a marker or an icon over it: marker green, white
/// (lowest channel from `SPOT_WHITE`) or strongly coloured (channel spread from `SPOT_SATURATED`;
/// vehicle icons, the player arrow, objective icons). Terrain is dull.
const SPOT_RADIUS: i32 = 8;
const SPOT_MAX_ICON_PIXELS: usize = 2;
const SPOT_WHITE: u8 = 190;
const SPOT_SATURATED: u8 = 90;

/// Whether the minimap shows plain map around `map`: nothing there that could be a marker or hide
/// one. A target missing from a plain spot is gone; a covered spot says nothing.
pub fn minimap_spot_clear(view: &FrameView, t: &MapTransform, map: (f64, f64)) -> bool {
    let (fx, fy) = t.to_frame(map.0, map.1);
    let (cx, cy) = (fx.floor() as i32, fy.floor() as i32);
    let side = 2 * SPOT_RADIUS + 1;
    if !view.covers(cx - SPOT_RADIUS, cy - SPOT_RADIUS, side, side) {
        return false;
    }
    let icon_like = (cy - SPOT_RADIUS..=cy + SPOT_RADIUS)
        .flat_map(|y| (cx - SPOT_RADIUS..=cx + SPOT_RADIUS).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = view.px(x, y);
            let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
            is_marker_green(p) || lo >= SPOT_WHITE || hi - lo >= SPOT_SATURATED
        })
        .count();
    icon_like <= SPOT_MAX_ICON_PIXELS
}

/// Marker green test.
#[inline]
fn is_marker_green(p: [u8; 3]) -> bool {
    let [r, g, b] = p;
    if g < MIN_VALUE || g <= r || g <= b || b > r.saturating_add(MAX_BLUE_OVER_RED) {
        return false;
    }
    let min = r.min(b);
    f32::from(g - min) / f32::from(g) >= MIN_SATURATION
}

impl MarkerReader {
    pub fn new() -> Self {
        Self::with_icon_scale(1.0)
    }

    /// `icon_scale`: V1/V2 icons against their 150 % size in the reference geometry (see
    /// `FullGeometry`), measured per configuration in development.
    pub fn with_icon_scale(icon_scale: f64) -> Self {
        Self { glyphs: LabelReader::new(), full: FullGeometry::at(icon_scale) }
    }

    /// All markers inside the transform's container.
    pub fn read(&self, view: &FrameView, t: &MapTransform) -> Vec<MarkerSighting> {
        let mut out = self.read_diamonds(view, t);
        if t.view != MapView::Mini {
            self.place_by_labels(view, t, &mut out);
        }
        out
    }

    /// The markers whose diamond shows, without placing hidden ones by their labels: the cheaper
    /// pass for following a moving V1 (median 3.4 ms against 11.4 ms for `read` on a recorded V1
    /// session).
    pub fn read_diamonds(&self, view: &FrameView, t: &MapTransform) -> Vec<MarkerSighting> {
        let x0 = t.left.floor() as i32;
        let y0 = t.top.floor() as i32;
        let n = t.size.ceil() as i32;
        if !view.covers(x0, y0, n, n) {
            return Vec::new();
        }
        let (w, h) = (n as usize, n as usize);
        let mut green = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                green[y * w + x] = is_marker_green(view.px(x0 + x as i32, y0 + y as i32));
            }
        }
        let (size, nominal) = match t.view {
            MapView::Mini => (MINI_SIZE, MINI_NOMINAL),
            MapView::Full | MapView::Scope => (self.full.size, self.full.nominal),
        };
        let mut seen = vec![false; w * h];
        let mut stack = Vec::new();
        let mut out = Vec::new();
        for start in 0..w * h {
            if !green[start] || seen[start] {
                continue;
            }
            // 8-connected component.
            let (mut count, mut sx, mut sy) = (0u32, 0u64, 0u64);
            let mut pixels: Vec<(usize, usize)> = Vec::new();
            let (mut minx, mut miny, mut maxx, mut maxy) = (usize::MAX, usize::MAX, 0, 0);
            seen[start] = true;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let (x, y) = (i % w, i / w);
                pixels.push((x, y));
                count += 1;
                sx += x as u64;
                sy += y as u64;
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                            continue;
                        }
                        let j = ny as usize * w + nx as usize;
                        if green[j] && !seen[j] {
                            seen[j] = true;
                            stack.push(j);
                        }
                    }
                }
            }
            let (bw, bh) = ((maxx - minx + 1) as u32, (maxy - miny + 1) as u32);
            let in_range = |v: u32, r: (u32, u32)| v >= r.0 && v <= r.1;
            if !in_range(bw, size) || !in_range(bh, size) || count as f32 > MAX_FILL * (bw * bh) as f32 {
                continue;
            }
            let touches_edge = minx == 0 || miny == 0 || maxx == w - 1 || maxy == h - 1;
            let full_box = in_range(bw, nominal) && in_range(bh, nominal) && !touches_edge;
            let pixel_mean = (sx as f64 / f64::from(count), sy as f64 / f64::from(count));
            // An outline whose box is whole but one edge is short is partly covered (an icon over
            // one side): the pixel mean shifts away from the cover, the edge lines do not. Such a
            // sighting places the marker from its edges and only refreshes a target.
            let (mean, complete) = match t.view {
                MapView::Full | MapView::Scope if full_box => match edge_centre(&pixels, (minx, miny, maxx, maxy)) {
                    Some((centre, balance)) if balance < MIN_EDGE_BALANCE => (centre, false),
                    _ => (pixel_mean, true),
                },
                _ => (pixel_mean, full_box),
            };
            let centroid = (f64::from(x0) + mean.0 + 0.5, f64::from(y0) + mean.1 + 0.5);
            let (offset, label) = match t.view {
                MapView::Mini => (ICON_OFFSET_MINI, None),
                MapView::Full | MapView::Scope => {
                    let label = self.read_label(view, centroid, x0 + n).map(|l| {
                        let boxes: Vec<(f64, f64)> = self.label_glyph_boxes(view, centroid).iter().map(|b| (f64::from(b.0), f64::from(b.0 + b.2))).collect();
                        self.letter_checked(&l, &boxes)
                    });
                    (self.full.icon_offset, label)
                }
            };
            let frame = (centroid.0 - offset.0, centroid.1 - offset.1);
            out.push(MarkerSighting {
                frame,
                map: t.to_map(frame.0, frame.1),
                label,
                complete,
                from_label: false,
            });
        }
        out
    }

    /// Labels whose diamond is not seen whole: the label places the marker.
    fn place_by_labels(&self, view: &FrameView, t: &MapTransform, out: &mut Vec<MarkerSighting>) {
        let g = &self.full;
        let x0 = t.left.floor() as i32;
        let y0 = t.top.floor() as i32;
        let n = t.size.ceil() as usize;
        let ink: Vec<bool> = (0..n * n).map(|i| is_label_ink(view.px(x0 + (i % n) as i32, y0 + (i / n) as i32))).collect();
        // Glyph-sized ink blobs: (min x, min y, max x, max y) in container px.
        let mut seen = vec![false; n * n];
        let mut glyphs = Vec::new();
        let mut stack = Vec::new();
        for start in 0..n * n {
            if !ink[start] || seen[start] {
                continue;
            }
            seen[start] = true;
            stack.push(start);
            let (mut gx0, mut gy0, mut gx1, mut gy1) = (usize::MAX, usize::MAX, 0, 0);
            while let Some(i) = stack.pop() {
                let (x, y) = (i % n, i / n);
                (gx0, gy0, gx1, gy1) = (gx0.min(x), gy0.min(y), gx1.max(x), gy1.max(y));
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < n && (ny as usize) < n {
                            let j = ny as usize * n + nx as usize;
                            if ink[j] && !seen[j] {
                                seen[j] = true;
                                stack.push(j);
                            }
                        }
                    }
                }
            }
            let (rows, cols) = (gy1 - gy0 + 1, gx1 - gx0 + 1);
            if (g.glyph_rows.0..=g.glyph_rows.1).contains(&rows) && (g.glyph_cols.0..=g.glyph_cols.1).contains(&cols) {
                glyphs.push((gx0, gy0, gx1, gy1));
            }
        }
        // Words: glyphs left to right with aligned tops and small gaps.
        glyphs.sort_unstable();
        let mut used = vec![false; glyphs.len()];
        for i in 0..glyphs.len() {
            if used[i] {
                continue;
            }
            used[i] = true;
            let mut word = vec![glyphs[i]];
            loop {
                let last = *word.last().expect("word");
                let next = (0..glyphs.len()).find(|&j| {
                    !used[j] && glyphs[j].1.abs_diff(last.1) <= g.word_top + 1 && glyphs[j].0 > last.2 && glyphs[j].0 - last.2 <= g.word_gap + 1
                });
                let Some(j) = next else { break };
                used[j] = true;
                word.push(glyphs[j]);
            }
            let ink_left = f64::from(x0) + word[0].0 as f64;
            let ink_top = f64::from(y0) + word.iter().map(|g| g.1).min().expect("word") as f64;
            // Only the word's columns: a neighbouring diamond can reach into the label box.
            let ink_right = x0 + word.last().expect("word").2 as i32 + 1;
            // A word starting at the container's left edge may have lost its first characters.
            if word[0].0 == 0 {
                continue;
            }
            let Some(label) = self.read_label_at(view, (ink_left as i32 - g.ink_margin, ink_top as i32 - g.ink_margin), Some(ink_right), x0 + n as i32) else { continue };
            let digits = label.len() - 1;
            if word.len() < digits || label[1..].parse::<u32>().is_err() {
                continue;
            }
            let boxes: Vec<(f64, f64)> = word.iter().map(|g| (f64::from(x0) + g.0 as f64, f64::from(x0) + (g.2 + 1) as f64)).collect();
            let label = self.letter_checked(&label, &boxes);
            let digit = word[word.len() - digits];
            let first = label.as_bytes()[1] - b'0';
            let digit_left = f64::from(x0) + digit.0 as f64;
            let baseline = f64::from(y0) + (digit.3 + 1) as f64;
            let point = (digit_left - g.digit_left[usize::from(first.max(1)) - 1], baseline - g.baseline);
            // Already explained by a whole diamond.
            if out.iter().any(|m| m.complete && !m.from_label && (m.frame.0 - point.0).hypot(m.frame.1 - point.1) <= g.label_match) {
                continue;
            }
            let sighting = MarkerSighting {
                frame: point,
                map: t.to_map(point.0, point.1),
                label: Some(label),
                complete: true,
                from_label: true,
            };
            // A cut or partly covered diamond of the same marker gives way to the label position.
            match out.iter().position(|m| !m.complete && (m.frame.0 - point.0).hypot(m.frame.1 - point.1) <= g.partial_match) {
                Some(k) => out[k] = sighting,
                None => out.push(sighting),
            }
        }
    }

    /// The label above-right of a diamond centred at `centre`; `edge`: the container's right edge
    /// (frame x, exclusive).
    fn read_label(&self, view: &FrameView, centre: (f64, f64), edge: i32) -> Option<String> {
        self.read_label_at(view, (centre.0.floor() as i32 + self.full.label_dx, centre.1.floor() as i32 + self.full.label_dy), None, edge)
    }

    /// The label in the label box whose top-left corner is `(bx, by)`. Ink right of `word_right`
    /// (frame x, exclusive) is ignored; a label that reaches the container's right edge `edge` is
    /// cut there and not read.
    fn read_label_at(&self, view: &FrameView, at: (i32, i32), word_right: Option<i32>, edge: i32) -> Option<String> {
        let ink = self.label_ink(view, at, word_right, edge)?;
        let (letter, number) = self.glyphs.read_marker_text(&ink, LABEL_W, self.full.ref_h)?;
        Some(format!("{}{number}", letter.unwrap_or(CUT_LETTER)))
    }

    /// The glyphs (width, ink rows) of the label of a marker at `point` (the marked point, frame
    /// px): to harvest letter templates (`examples/harvest_marker_letters.rs`).
    pub fn label_glyphs(&self, view: &FrameView, point: (f64, f64)) -> Option<Vec<(usize, Vec<f32>)>> {
        let centre = (point.0 + self.full.icon_offset.0, point.1 + self.full.icon_offset.1);
        let ink = self.label_ink(view, (centre.0.floor() as i32 + self.full.label_dx, centre.1.floor() as i32 + self.full.label_dy), None, i32::MAX)?;
        self.glyphs.marker_glyphs(&ink, LABEL_W, self.full.ref_h)
    }

    /// The label with its letter replaced by `CUT_LETTER` when the glyph boxes (left, right;
    /// frame px, left to right) show the letter missing, narrower than its template, or not right
    /// before the first digit.
    fn letter_checked(&self, label: &str, boxes: &[(f64, f64)]) -> String {
        let digits = label.len() - 1;
        let width = label.chars().next().and_then(|c| self.glyphs.marker_letter_width(c));
        let whole = boxes.len() > digits
            && width.is_some_and(|w| {
                let (left, right) = boxes[boxes.len() - digits - 1];
                let gap = boxes[boxes.len() - digits].0 - right;
                (0.0..=self.full.letter_gap).contains(&gap) && right - left >= w as f64 * self.full.k - 1.0
            });
        if whole { label.to_owned() } else { format!("{CUT_LETTER}{}", &label[1..]) }
    }

    /// Label ink strength in the label box whose top-left corner is `(bx, by)`, resampled to the
    /// reference box (`LABEL_W` x `LABEL_H`) for the glyph reader when the icon scale is not 1;
    /// see `read_label_at`.
    fn label_ink(&self, view: &FrameView, (bx, by): (i32, i32), word_right: Option<i32>, edge: i32) -> Option<Vec<f32>> {
        let (lw, lh) = (self.full.label_w, self.full.label_h);
        if !view.covers(bx, by, lw as i32, lh as i32) {
            return None;
        }
        let mut ink = vec![0.0f32; lw * lh];
        let right = word_right.map_or(edge, |r| r.min(edge));
        let columns = usize::try_from(right - bx).unwrap_or(0).min(lw);
        for y in 0..lh {
            for x in 0..columns {
                ink[y * lw + x] = label_ink_strength(view.px(bx + x as i32, by + y as i32));
            }
        }
        if right == edge && columns < lw && columns > 0 && (0..lh).any(|y| ink[y * lw + columns - 1] > 0.0) {
            return None;
        }
        if self.full.k == 1.0 {
            return Some(ink);
        }
        // The box at the icon scale is rounded up from `k` times the reference box; its first `k`
        // x reference columns and rows map onto the reference box.
        let at = |x: i32, y: i32| if x >= 0 && y >= 0 && (x as usize) < lw && (y as usize) < lh { ink[y as usize * lw + x as usize] } else { 0.0 };
        let span = (LABEL_W as f64 * self.full.k, self.full.ref_h as f64 * self.full.k);
        Some(area_resample(at, (0.0, 0.0), span, (LABEL_W, self.full.ref_h)).data)
    }

    /// Whether `sighting`, placed by its label alone, has that label on the vehicle icon centred at
    /// `icon` (the player's, under the arrow's pivot): in a squad the own icon is green like the
    /// markers, the arrow over it splits it, and a glyph-sized piece read as a label "?1" that
    /// followed the arrow at every zoom (seen on Rohkstov). A diamond in view keeps its sighting.
    pub fn label_on_icon(&self, sighting: &MarkerSighting, icon: (f64, f64)) -> bool {
        if !sighting.from_label {
            return false;
        }
        let g = &self.full;
        let (px, py) = sighting.frame;
        // The label's ink box: from its left and top edges to the widest label ("C12") and the baseline.
        let (left, top) = (px + g.ink_offset.0, py + g.ink_offset.1);
        let (right, bottom) = (px + g.digit_left[0] + 2.0 * 7.0 * g.k, py + g.baseline);
        let dx = (left - icon.0).max(icon.0 - right).max(0.0);
        let dy = (top - icon.1).max(icon.1 - bottom).max(0.0);
        dx.hypot(dy) <= g.icon_reach
    }

    /// Glyph boxes (x, y, w, h, frame px), left to right, of the label of the marker at `centre`
    /// (centroid of a whole diamond): ink blobs at least `min_glyph_rows` tall, so slivers of the
    /// diamond outline are left out.
    pub fn label_glyph_boxes(&self, view: &FrameView, centre: (f64, f64)) -> Vec<(i32, i32, i32, i32)> {
        let g = &self.full;
        let (lw, lh) = (g.label_w, g.label_h);
        let bx = centre.0.floor() as i32 + g.label_dx;
        let by = centre.1.floor() as i32 + g.label_dy;
        if !view.covers(bx, by, lw as i32, lh as i32) {
            return Vec::new();
        }
        let ink: Vec<bool> = (0..lw * lh).map(|i| is_label_ink(view.px(bx + (i % lw) as i32, by + (i / lw) as i32))).collect();
        let mut seen = vec![false; ink.len()];
        let mut boxes = Vec::new();
        let mut stack = Vec::new();
        for start in 0..ink.len() {
            if !ink[start] || seen[start] {
                continue;
            }
            seen[start] = true;
            stack.push(start);
            let (mut cx0, mut cy0, mut cx1, mut cy1) = (usize::MAX, usize::MAX, 0, 0);
            while let Some(i) = stack.pop() {
                let (x, y) = (i % lw, i / lw);
                (cx0, cy0, cx1, cy1) = (cx0.min(x), cy0.min(y), cx1.max(x), cy1.max(y));
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < lw && (ny as usize) < lh {
                            let j = ny as usize * lw + nx as usize;
                            if ink[j] && !seen[j] {
                                seen[j] = true;
                                stack.push(j);
                            }
                        }
                    }
                }
            }
            if cy1 - cy0 + 1 >= g.min_glyph_rows {
                boxes.push((bx + cx0 as i32, by + cy0 as i32, (cx1 - cx0 + 1) as i32, (cy1 - cy0 + 1) as i32));
            }
        }
        boxes.sort_unstable();
        boxes
    }

    /// Bounding box (x, y, w, h, frame px) of all label glyphs of the marker at `centre`.
    pub fn label_ink_box(&self, view: &FrameView, centre: (f64, f64)) -> Option<(i32, i32, i32, i32)> {
        let boxes = self.label_glyph_boxes(view, centre);
        let x0 = boxes.iter().map(|b| b.0).min()?;
        let y0 = boxes.iter().map(|b| b.1).min()?;
        let x1 = boxes.iter().map(|b| b.0 + b.2).max()?;
        let y1 = boxes.iter().map(|b| b.1 + b.3).max()?;
        Some((x0, y0, x1 - x0, y1 - y0))
    }
}

/// Farthest a vehicle icon reaches from its centre (frame px, any heading): the icon is ~22 × 33 px
/// drawn at screen size.
pub const VEHICLE_ICON_REACH: f64 = 20.0;

/// Centre of a diamond outline (container px, pixel indices) from its edge lines, and the edges'
/// balance (fewest over most pixels): pixels on the upper-right and lower-left edges each share
/// one x − y, those on the upper-left and lower-right edges one x + y; the centre lies midway
/// between each pair, whatever part of an edge is missing. Pixels within `CORNER_PX` of the box's
/// middle lines (the corners) are left out. `None` when an edge has too few pixels.
fn edge_centre(pixels: &[(usize, usize)], (minx, miny, maxx, maxy): (usize, usize, usize, usize)) -> Option<((f64, f64), f64)> {
    const CORNER_PX: f64 = 3.0;
    const MIN_EDGE_PIXELS: usize = 4;
    let (cx, cy) = ((minx + maxx) as f64 / 2.0, (miny + maxy) as f64 / 2.0);
    // Upper-right, lower-left (x − y); upper-left, lower-right (x + y).
    let mut edges: [Vec<f64>; 4] = Default::default();
    for &(x, y) in pixels {
        let (x, y) = (x as f64, y as f64);
        let (dx, dy) = (x - cx, y - cy);
        if dx.abs() < CORNER_PX || dy.abs() < CORNER_PX {
            continue;
        }
        let k = match (dx > 0.0, dy > 0.0) {
            (true, false) => 0,
            (false, true) => 1,
            (false, false) => 2,
            (true, true) => 3,
        };
        edges[k].push(if k < 2 { x - y } else { x + y });
    }
    if edges.iter().any(|e| e.len() < MIN_EDGE_PIXELS) {
        return None;
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let d = (mean(&edges[0]) + mean(&edges[1])) / 2.0;
    let s = (mean(&edges[2]) + mean(&edges[3])) / 2.0;
    let counts = edges.iter().map(Vec::len);
    let balance = counts.clone().min().unwrap_or(0) as f64 / counts.max().unwrap_or(1) as f64;
    Some((((s + d) / 2.0, (s - d) / 2.0), balance))
}

/// Label ink strength: green above the floor on green-hued, saturated pixels, 0 elsewhere.
#[inline]
fn label_ink_strength(p: [u8; 3]) -> f32 {
    let [r, g, b] = p;
    if g > r && g > b && b <= r.saturating_add(MAX_BLUE_OVER_RED) && f32::from(g - r.min(b)) / f32::from(g) >= 0.4 {
        (f32::from(g) - LABEL_INK_FLOOR).max(0.0)
    } else {
        0.0
    }
}

/// Label ink test (green-hued, saturated, value above the floor).
#[inline]
fn is_label_ink(p: [u8; 3]) -> bool {
    label_ink_strength(p) > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_label(frame: (f64, f64)) -> MarkerSighting {
        MarkerSighting { frame, map: (0.0, 0.0), label: Some("?1".into()), complete: true, from_label: true }
    }

    #[test]
    fn a_label_on_the_players_icon_is_a_piece_of_it() {
        // A frame recorded on Rohkstov: the arrow's pivot at (1475, 867), the "?1" placed at
        // (1458, 901) from a piece of the green icon below-right of it.
        let reader = MarkerReader::new();
        assert!(reader.label_on_icon(&by_label((1458.0, 901.0)), (1475.0, 867.0)));
        // A label well away from the icon, or a marker whose diamond shows, stays.
        assert!(!reader.label_on_icon(&by_label((1300.0, 901.0)), (1475.0, 867.0)));
        assert!(!reader.label_on_icon(&MarkerSighting { from_label: false, ..by_label((1458.0, 901.0)) }, (1475.0, 867.0)));
    }
}
