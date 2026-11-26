//! Objective (capture point) icons on an unzoomed map view, and which map their layout belongs to.
//!
//! Objective icons are 41 px solid diamonds with a centred white letter, the same size in V1, V2
//! and V3. An icon is found from its letter: a white blob of letter size
//! whose surroundings are one uniform fill colour; the letter is classified against templates
//! (`assets/objective_glyphs.json`, cut from recordings) and its bounding-box
//! centre is the icon centre. Objective positions are fixed per map (`data/objectives.json`), so
//! the letters seen and their map positions identify the map even when vehicle icons, pings and
//! the 3D scene behind the translucent minimap drown the thumbnail correlation.

use serde::Deserialize;

use crate::map::geometry::MapTransform;
use crate::map::raster::area_resample;
use crate::view::FrameView;

const WHITE_MIN: u8 = 185;
const WHITE_SPREAD: u8 = 45;
const LETTER_H: (usize, usize) = (14, 22);
const LETTER_W: (usize, usize) = (8, 20);
/// Fill ring around the letter centre (L1 distance in px) and how uniform it must be.
const RING: (f64, f64) = (13.0, 17.0);
const RING_MIN_PIXELS: usize = 20;
const FILL_DISTANCE: f32 = 38.0;
const FILL_MIN_SHARE: f32 = 0.8;
const MIN_GLYPH_IOU: f32 = 0.6;
/// Off the reference configuration the letters are drawn at the configuration's own font size
/// and come resampled: the right letter still ranks first but often scores under 0.6 (median
/// 0.54-0.70, 5th percentile 0.38-0.48), while B and E were confused with a 0.04 lead. A letter
/// there is taken from this overlap when it leads the next by `MIN_GLYPH_LEAD_ELSEWHERE` (at 100 %
/// display scaling, layout checks then passed on 968 of 1049 minimap frames instead of 258, with
/// none newly wrong).
const MIN_GLYPH_IOU_ELSEWHERE: f32 = 0.45;
const MIN_GLYPH_LEAD_ELSEWHERE: f32 = 0.05;
/// A sighting matches a table entry of the same letter within this distance (map units; 4 px on
/// V3, 11 px on unzoomed V1).
pub const LAYOUT_MATCH: f64 = 0.012;
/// A layout decides only with at least this many matching letters and no contradicting one.
pub const LAYOUT_MIN_MATCHES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectiveSighting {
    pub letter: char,
    /// Continuous frame coordinates of the icon centre.
    pub frame: (f64, f64),
    pub map: (f64, f64),
}

/// Development: a letter's white blob that passed the size and fill checks, with its best and
/// second-best template and their overlaps (`ObjectiveReader::scored_at`).
#[derive(Clone, Copy, Debug)]
pub struct LetterScores {
    pub frame: (f64, f64),
    pub best: (char, f32),
    pub second: (char, f32),
}

/// An objective of a map: letter and normalised map position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Objective {
    pub letter: char,
    pub u: f64,
    pub v: f64,
}

#[derive(Deserialize)]
struct GlyphAsset {
    size: usize,
    letters: std::collections::BTreeMap<char, Vec<String>>,
}

pub struct ObjectiveReader {
    size: usize,
    templates: Vec<(char, Vec<bool>)>,
}

impl Default for ObjectiveReader {
    fn default() -> Self {
        Self::new()
    }
}

impl ObjectiveReader {
    pub fn new() -> Self {
        let asset: GlyphAsset = serde_json::from_str(include_str!("../../assets/objective_glyphs.json")).expect("objective glyph asset");
        let templates = asset
            .letters
            .into_iter()
            .map(|(k, rows)| (k, rows.iter().flat_map(|r| r.bytes().map(|b| b == b'1')).collect()))
            .collect();
        Self { size: asset.size, templates }
    }

    /// Objective icons inside the transform's container.
    pub fn read(&self, view: &FrameView, t: &MapTransform) -> Vec<ObjectiveSighting> {
        self.read_at(view, t, 1.0)
    }

    /// As `read` with the icons drawn `icon_scale` times their 150 % size in the transform's frame:
    /// on V1 and V2 of another configuration mapped to the reference geometry by the map frame, the
    /// HUD's scale over the map frame's. The letter's size range, the fill ring and
    /// the glyph window scale with it; the glyph is resampled to the templates' size.
    pub fn read_at(&self, view: &FrameView, t: &MapTransform, icon_scale: f64) -> Vec<ObjectiveSighting> {
        self.scored_at(view, t, icon_scale)
            .into_iter()
            .filter(|s| s.best.1 >= MIN_GLYPH_IOU)
            .map(|s| ObjectiveSighting { letter: s.best.0, frame: s.frame, map: t.to_map(s.frame.0, s.frame.1) })
            .collect()
    }

    /// As `read_at` on a view of another display configuration mapped to the reference container:
    /// a letter taken from `MIN_GLYPH_IOU_ELSEWHERE` when it leads the next template by
    /// `MIN_GLYPH_LEAD_ELSEWHERE`.
    pub fn read_elsewhere(&self, view: &FrameView, t: &MapTransform, icon_scale: f64) -> Vec<ObjectiveSighting> {
        self.scored_at(view, t, icon_scale)
            .into_iter()
            .filter(|s| s.best.1 >= MIN_GLYPH_IOU_ELSEWHERE && s.best.1 - s.second.1 >= MIN_GLYPH_LEAD_ELSEWHERE)
            .map(|s| ObjectiveSighting { letter: s.best.0, frame: s.frame, map: t.to_map(s.frame.0, s.frame.1) })
            .collect()
    }

    /// Development: every letter blob `read_at` looks at, with its template overlaps.
    #[doc(hidden)]
    pub fn scored_at(&self, view: &FrameView, t: &MapTransform, icon_scale: f64) -> Vec<LetterScores> {
        let k = icon_scale;
        let range = |(a, b): (usize, usize)| (((a as f64) * k).floor() as usize, ((b as f64) * k).ceil() as usize);
        let (letter_h, letter_w) = (range(LETTER_H), range(LETTER_W));
        let x0 = t.left.floor() as i32;
        let y0 = t.top.floor() as i32;
        let n = t.size.ceil() as usize;
        if !view.covers(x0, y0, n as i32, n as i32) {
            return Vec::new();
        }
        let px = |x: usize, y: usize| view.px(x0 + x as i32, y0 + y as i32);
        let white: Vec<bool> = (0..n * n)
            .map(|i| {
                let p = px(i % n, i / n);
                let (mx, mn) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
                mn >= WHITE_MIN && mx - mn <= WHITE_SPREAD
            })
            .collect();
        let mut label = vec![0u32; n * n];
        let mut found = Vec::new();
        let mut stack = Vec::new();
        let mut next = 0u32;
        for start in 0..n * n {
            if !white[start] || label[start] != 0 {
                continue;
            }
            next += 1;
            let (mut minx, mut maxx, mut miny, mut maxy) = (usize::MAX, 0, usize::MAX, 0);
            label[start] = next;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let (x, y) = (i % n, i / n);
                (minx, maxx, miny, maxy) = (minx.min(x), maxx.max(x), miny.min(y), maxy.max(y));
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < n && (ny as usize) < n {
                            let j = ny as usize * n + nx as usize;
                            if white[j] && label[j] == 0 {
                                label[j] = next;
                                stack.push(j);
                            }
                        }
                    }
                }
            }
            let (w, h) = (maxx - minx + 1, maxy - miny + 1);
            if !(letter_h.0..=letter_h.1).contains(&h) || !(letter_w.0..=letter_w.1).contains(&w) {
                continue;
            }
            let (cx, cy) = ((minx + maxx + 1) as f64 / 2.0, (miny + maxy + 1) as f64 / 2.0);
            if !self.uniform_fill(&px, &white, n, cx, cy, k) {
                continue;
            }
            let Some((best, second)) = self.classify(&label, next, n, cx, cy, k) else { continue };
            found.push(LetterScores { frame: (f64::from(x0) + cx, f64::from(y0) + cy), best, second });
        }
        found
    }

    /// The ring around the letter is one colour (the icon's fill).
    fn uniform_fill(&self, px: &impl Fn(usize, usize) -> [u8; 3], white: &[bool], n: usize, cx: f64, cy: f64, k: f64) -> bool {
        let ring_span = (RING.0 * k, RING.1 * k);
        let r = ring_span.1.ceil() as i64;
        let mut ring = Vec::new();
        for y in (cy as i64 - r).max(0)..=(cy as i64 + r).min(n as i64 - 1) {
            for x in (cx as i64 - r).max(0)..=(cx as i64 + r).min(n as i64 - 1) {
                let l1 = (x as f64 - cx).abs() + (y as f64 - cy).abs();
                if (ring_span.0..=ring_span.1).contains(&l1) && !white[y as usize * n + x as usize] {
                    ring.push(px(x as usize, y as usize));
                }
            }
        }
        if (ring.len() as f64) < RING_MIN_PIXELS as f64 * k * k {
            return false;
        }
        let median = |c: usize| {
            let mut v: Vec<u8> = ring.iter().map(|p| p[c]).collect();
            v.sort_unstable();
            f32::from(v[v.len() / 2])
        };
        let fill = [median(0), median(1), median(2)];
        let near = ring
            .iter()
            .filter(|p| (0..3).map(|c| (f32::from(p[c]) - fill[c]).powi(2)).sum::<f32>().sqrt() <= FILL_DISTANCE)
            .count();
        near as f32 >= FILL_MIN_SHARE * ring.len() as f32
    }

    /// The best and second-best templates for the blob's mask in a window centred on its bounding
    /// box, with their overlaps.
    fn classify(&self, label: &[u32], id: u32, n: usize, cx: f64, cy: f64, k: f64) -> Option<((char, f32), (char, f32))> {
        let s = self.size as i64;
        // The glyph window at the icon scale, resampled to the templates' size (half covered: ink).
        let ws = ((self.size as f64) * k).round() as i64;
        let (gx0, gy0) = (cx.round_ties_even() as i64 - ws / 2, cy.round_ties_even() as i64 - ws / 2);
        let inked = |x: i64, y: i64| {
            let (x, y) = (gx0 + x, gy0 + y);
            x >= 0 && y >= 0 && (x as usize) < n && (y as usize) < n && label[y as usize * n + x as usize] == id
        };
        let glyph: Vec<bool> = if ws == s {
            (0..s * s).map(|i| inked(i % s, i / s)).collect()
        } else {
            let mask = area_resample(|x, y| if inked(i64::from(x), i64::from(y)) { 1.0 } else { 0.0 }, (0.0, 0.0), (ws as f64, ws as f64), (self.size, self.size));
            mask.data.iter().map(|&v| v >= 0.5).collect()
        };
        let mut scored: Vec<(char, f32)> = self.templates.iter().map(|(k, t)| (*k, shifted_iou(&glyph, t, self.size))).collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        Some((*scored.first()?, scored.get(1).copied().unwrap_or((' ', 0.0))))
    }
}

/// Intersection over union of two square masks, best over ±1 px shifts of `b`.
fn shifted_iou(a: &[bool], b: &[bool], size: usize) -> f32 {
    let s = size as i64;
    let mut best = 0.0f32;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (mut inter, mut union) = (0u32, 0u32);
            for y in 0..s {
                for x in 0..s {
                    let av = a[(y * s + x) as usize];
                    let (bx, by) = (x - dx, y - dy);
                    let bv = bx >= 0 && by >= 0 && bx < s && by < s && b[(by * s + bx) as usize];
                    inter += u32::from(av && bv);
                    union += u32::from(av || bv);
                }
            }
            if union > 0 {
                best = best.max(inter as f32 / union as f32);
            }
        }
    }
    best
}

/// How a set of sightings fits a map's objective table: letters within [`LAYOUT_MATCH`] of the
/// same letter, and letters that contradict the table (absent, or elsewhere).
pub fn layout_fit(sightings: &[ObjectiveSighting], table: &[Objective]) -> (usize, usize) {
    let (mut matched, mut contradicted) = (0, 0);
    for s in sightings {
        let hit = table
            .iter()
            .any(|o| o.letter == s.letter && (o.u - s.map.0).hypot(o.v - s.map.1) <= LAYOUT_MATCH);
        if hit {
            matched += 1;
        } else {
            contradicted += 1;
        }
    }
    (matched, contradicted)
}
