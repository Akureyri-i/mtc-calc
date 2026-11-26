//! Which map is loaded: thumbnail correlation of an unzoomed view against every reference.
//!
//! The container is area-averaged to 256² and compared with each reference thumbnail by
//! normalised correlation of high-passed luma (the translucent map shows the 3D scene through,
//! which only adds low frequencies), luma gradient magnitude and the high-passed chroma channels
//! R−G and B−(R+G)/2 (field colours separate the low-contrast maps), masked to the reference's
//! opaque part. Only call it on a view the cues say is on screen: a hidden view still
//! correlates weakly with some map. Single frames of small, low-contrast maps on the minimap are
//! close calls, so [`MapVote`] averages scores over recent frames before deciding.
//!
//! In multiplayer the minimap is crowded with vehicle icons and pings and the thumbnail scores
//! drop below the thresholds (Muddy Fields: 0.10, lead 0.05). The objective icons stay legible and
//! their layout is fixed per map, so [`MapIdentifier::identify`] first matches the objectives seen
//! against each map's table ([`crate::map::objectives`]); thumbnails then only choose between maps
//! sharing a layout (Muddy Fields and Snowy Fields).

use std::collections::VecDeque;

use crate::map::geometry::{MapTransform, MapView};
use crate::map::objectives::{LAYOUT_MIN_MATCHES, Objective, ObjectiveReader, layout_fit};
use crate::map::raster::{Plane, area_resample, masked_ncc};
use crate::map::reference::MapReference;
use crate::view::FrameView;

const THUMB: usize = 256;
/// A thumbnail cell this much covered by icon pixels is left out (`keep_cells`).
const ICON_CELL_SHARE: f32 = 0.15;
const HIGH_PASS_SIGMA: f32 = 8.0;
/// Reference pixels at least this opaque count for the mask.
const MASK_MIN_ALPHA: f32 = 200.0 / 255.0;
/// A mask covering nearly everything is dropped (plain NCC over the whole thumbnail).
const MASK_MAX_COVERAGE: f32 = 0.98;
/// One frame decides on its own above these.
pub const SINGLE_MIN_SCORE: f32 = 0.15;
pub const SINGLE_MIN_MARGIN: f32 = 0.10;
/// V1 may be zoomed, and a zoomed map can look like another map unzoomed (Rohkstov at 1.6× is
/// Rohkshort), so V1 needs the scores only an unzoomed match reaches. The map is normally
/// decided on V3/V2 first and V1 never overrides it.
pub const FULL_MIN_SCORE: f32 = 0.25;
pub const FULL_MIN_MARGIN: f32 = 0.15;
/// Averaged over the vote window.
pub const VOTE_MIN_SCORE: f32 = 0.10;
pub const VOTE_MIN_MARGIN: f32 = 0.06;
pub const VOTE_MIN_FRAMES: usize = 3;
const VOTE_WINDOW: usize = 8;
/// Between maps sharing an objective layout, the thumbnail leader needs this lead.
pub const LAYOUT_TIE_MIN_MARGIN: f32 = 0.03;

/// Weights of luma detail, luma gradient, R−G detail and B−(R+G)/2 detail.
const WEIGHTS: [f32; 4] = [0.3, 0.3, 0.2, 0.2];

/// Luma detail, luma gradient, R−G detail and B−(R+G)/2 detail of a 256² thumbnail.
struct Features([Vec<f32>; 4]);

impl Features {
    fn from(gray: &Plane, rg: &Plane, by: &Plane) -> Self {
        Self([gray.high_pass(HIGH_PASS_SIGMA).data, gray.gradient().data, rg.high_pass(HIGH_PASS_SIGMA).data, by.high_pass(HIGH_PASS_SIGMA).data])
    }
}

/// A reference's features at one byte a value (256 KB a map instead of 1 MB): each plane mapped
/// onto 256 levels between its 0.5 % and 99.5 % values, clipped outside. Correlation ignores a
/// plane's scale and offset, so the levels are used as they are. Against f32 features on the
/// labelled frames: the same verdict on all 504, least lead of the right map 0.044 before and
/// 0.045 after, the debug map's best score 0.070 both.
///
/// Making one takes the reference decoded and its working planes (about 100 ms a map, 2.6 s for
/// 25), so the HUD keeps them on disk (`to_bytes`, `from_bytes`) and makes them again only for a
/// reference that changed.
#[derive(Clone, Debug, PartialEq)]
pub struct Thumbnail {
    planes: [Vec<u8>; 4],
    mask: Option<Vec<bool>>,
}

/// The saved thumbnails' format. Raise it whenever anything that makes a thumbnail changes (the
/// reference's working planes, `Features`, `quantize`, the mask): saved ones of another format are
/// made again. `tests::thumbnail_of_a_fixed_picture_is_unchanged` fails when the making changes.
pub const THUMBNAIL_FORMAT: u32 = 1;
const THUMBNAIL_MAGIC: &[u8; 4] = b"MTCT";
/// Score of a map whose thumbnail is not made yet: the lowest a score can be.
const NO_THUMBNAIL_SCORE: f32 = -1.0;

impl Thumbnail {
    /// The thumbnail of a reference.
    pub fn of(r: &MapReference) -> Self {
        let alpha = r.alpha.resample(THUMB, THUMB);
        let mask: Vec<bool> = alpha.data.iter().map(|&a| a > MASK_MIN_ALPHA).collect();
        let coverage = mask.iter().filter(|&&m| m).count() as f32 / mask.len() as f32;
        let features = Features::from(&r.gray.resample(THUMB, THUMB), &r.rg.resample(THUMB, THUMB), &r.by.resample(THUMB, THUMB));
        Self { planes: features.0.map(|p| quantize(&p)), mask: (coverage < MASK_MAX_COVERAGE).then_some(mask) }
    }

    /// Weighted correlation of the features with this thumbnail over the reference's opaque part,
    /// less the cells `keep` marks false (icons on the frame).
    fn score_keeping(&self, f: &Features, keep: &[bool]) -> f32 {
        let mask: Vec<bool> = match &self.mask {
            Some(m) => m.iter().zip(keep).map(|(&a, &b)| a && b).collect(),
            None => keep.to_vec(),
        };
        WEIGHTS.iter().zip(&f.0).zip(&self.planes).map(|((w, a), b)| w * masked_ncc(a, b, Some(&mask))).sum()
    }

    /// Saved form, tagged with `key` (what it was made from, e.g. a hash of the reference file):
    /// magic, format, key, whether a mask follows, the four planes, the mask (a byte a pixel).
    pub fn to_bytes(&self, key: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(17 + 5 * THUMB * THUMB);
        out.extend_from_slice(THUMBNAIL_MAGIC);
        out.extend_from_slice(&THUMBNAIL_FORMAT.to_le_bytes());
        out.extend_from_slice(&key.to_le_bytes());
        out.push(u8::from(self.mask.is_some()));
        for p in &self.planes {
            out.extend_from_slice(p);
        }
        if let Some(m) = &self.mask {
            out.extend(m.iter().map(|&b| u8::from(b)));
        }
        out
    }

    /// The thumbnail saved with `key` in this format; `None` for another key or format, or a
    /// damaged file.
    pub fn from_bytes(bytes: &[u8], key: u64) -> Option<Self> {
        let n = THUMB * THUMB;
        let header = bytes.get(..17)?;
        if &header[..4] != THUMBNAIL_MAGIC
            || u32::from_le_bytes(header[4..8].try_into().ok()?) != THUMBNAIL_FORMAT
            || u64::from_le_bytes(header[8..16].try_into().ok()?) != key
        {
            return None;
        }
        let masked = header[16] == 1;
        let body = &bytes[17..];
        if body.len() != n * if masked { 5 } else { 4 } {
            return None;
        }
        let plane = |i: usize| body[i * n..(i + 1) * n].to_vec();
        Some(Self { planes: [plane(0), plane(1), plane(2), plane(3)], mask: masked.then(|| body[4 * n..].iter().map(|&b| b == 1).collect()) })
    }
}

/// Best candidate and its lead over the runner-up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapMatch {
    /// Index into the references the identifier was built from.
    pub index: usize,
    pub score: f32,
    pub margin: f32,
    /// Decided by the objective layout (the thumbnail only separated maps sharing it).
    pub by_layout: bool,
}

impl MapMatch {
    /// The best-scoring map of a thumbnail score set and its lead.
    pub fn best(scores: &[f32]) -> Self {
        let mut order: Vec<usize> = (0..scores.len()).collect();
        order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]));
        let index = order[0];
        Self {
            index,
            score: scores[index],
            margin: scores[index] - order.get(1).map_or(0.0, |&i| scores[i]),
            by_layout: false,
        }
    }

    /// Decides on its own on V2/V3.
    pub fn confident(&self) -> bool {
        self.by_layout || (self.score >= SINGLE_MIN_SCORE && self.margin >= SINGLE_MIN_MARGIN)
    }

    /// Decides on its own on the given (unzoomed) view.
    pub fn confident_on(&self, view: MapView) -> bool {
        match view {
            MapView::Full => self.by_layout || (self.score >= FULL_MIN_SCORE && self.margin >= FULL_MIN_MARGIN),
            MapView::Scope | MapView::Mini => self.confident(),
        }
    }
}

/// A plane mapped onto 256 levels between its 0.5 % and 99.5 % values (clipped outside).
fn quantize(plane: &[f32]) -> Vec<u8> {
    let mut sorted = plane.to_vec();
    sorted.sort_by(f32::total_cmp);
    let (lo, hi) = (sorted[sorted.len() / 200], sorted[sorted.len() - 1 - sorted.len() / 200]);
    let scale = if hi > lo { 255.0 / (hi - lo) } else { 0.0 };
    plane.iter().map(|v| ((v - lo) * scale).round().clamp(0.0, 255.0) as u8).collect()
}

pub struct MapIdentifier {
    /// One per map; `None` while it is being made (the map is found by its objective layout only).
    thumbs: Vec<Option<Thumbnail>>,
    layouts: Vec<Vec<Objective>>,
    objectives: ObjectiveReader,
    /// Objective icons against their 150 % size per view (V1, V2, V3; `ObjectiveReader::read_at`,
    /// `UiLayout::map_icon_scale`): the containers follow their own rules, the icons the HUD.
    icon_scales: [f64; 3],
    /// The views come from another display configuration (`ObjectiveReader::read_elsewhere`).
    elsewhere: bool,
}

impl MapIdentifier {
    pub fn new(references: &[MapReference]) -> Self {
        let mut identifier = Self { thumbs: Vec::new(), layouts: Vec::new(), objectives: ObjectiveReader::new(), icon_scales: [1.0; 3], elsewhere: false };
        for r in references {
            identifier.push(r, &[]);
        }
        identifier
    }

    /// Adds one reference with its objective table (its index is the number pushed before it).
    /// Only the 256² thumbnail features are kept, so references can be loaded and dropped one at
    /// a time.
    pub fn push(&mut self, r: &MapReference, objectives: &[Objective]) {
        self.push_thumbnail(Some(Thumbnail::of(r)), objectives);
    }

    /// Adds one map by its thumbnail (`None`: made later, `set_thumbnail`) and objective table.
    pub fn push_thumbnail(&mut self, thumbnail: Option<Thumbnail>, objectives: &[Objective]) {
        self.layouts.push(objectives.to_vec());
        self.thumbs.push(thumbnail);
    }

    /// The configuration's V1/V2 icon scale (1 at 150 % on 2560 x 1334); the minimap's keeps its
    /// size.
    pub fn set_icon_scale(&mut self, icon_scale: f64) {
        self.icon_scales = [icon_scale, icon_scale, 1.0];
    }

    /// The configuration's icon scale on each view, in `MapView::ALL` order (V1, V2, V3), and
    /// whether it is another configuration than the reference (its letters read as such).
    pub fn set_icon_scales(&mut self, icon_scales: [f64; 3], elsewhere: bool) {
        self.icon_scales = icon_scales;
        self.elsewhere = elsewhere;
    }

    /// The thumbnail of map `index`, made after it was pushed.
    pub fn set_thumbnail(&mut self, index: usize, thumbnail: Thumbnail) {
        self.thumbs[index] = Some(thumbnail);
    }

    /// Drops every thumbnail, keeping the objective layouts (the caller keeps the thumbnails on
    /// disk and sets them again before `scores` or `identify` need them).
    pub fn drop_thumbnails(&mut self) {
        self.thumbs.iter_mut().for_each(|t| *t = None);
    }

    pub fn len(&self) -> usize {
        self.thumbs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.thumbs.is_empty()
    }

    fn features(view: &FrameView, t: &MapTransform) -> Option<Features> {
        // The pixels the area resampling touches: exactly the container's integer frame rectangle
        // (`MapView::frame_rect`), which is what the live HUD reads back.
        let (x, y) = (t.left.floor() as i32, t.top.floor() as i32);
        let (x1, y1) = ((t.left + t.size).ceil() as i32, (t.top + t.size).ceil() as i32);
        if !view.covers(x, y, x1 - x, y1 - y) {
            return None;
        }
        let channel = |f: fn([u8; 3]) -> f32| {
            area_resample(|x, y| f(view.px(x, y)), (t.left, t.top), (t.size, t.size), (THUMB, THUMB))
        };
        Some(Features::from(
            &channel(|[r, g, b]| 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)),
            &channel(|[r, g, _]| f32::from(r) - f32::from(g)),
            &channel(|[r, g, b]| f32::from(b) - (f32::from(r) + f32::from(g)) / 2.0),
        ))
    }

    fn score(&self, features: &Features, keep: &[bool], index: usize) -> f32 {
        self.thumbs[index].as_ref().map_or(NO_THUMBNAIL_SCORE, |t| t.score_keeping(features, keep))
    }

    /// The thumbnail cells to compare: all but those under vehicle icons, pings and markers
    /// (cyan, white and green pixels, `ICON_CELL_SHARE` of the cell or more, grown by a cell). In
    /// multiplayer teammates' icons cover much of the minimap; left out, the right map's score on
    /// public-server recordings rose from 0.201 to 0.395 (minimap, median) and 0.456 to 0.695
    /// (V1), its least lead from 0.045 to 0.147, and no frame's leader changed
    /// (`examples/ident_public.rs`).
    fn keep_cells(view: &FrameView, t: &MapTransform) -> Vec<bool> {
        let icon = |[r, g, b]: [u8; 3]| -> f32 {
            let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
            let cyan = b > 170 && g > 150 && r < 130;
            let white = r > 215 && g > 215 && b > 215;
            let green = g > 170 && r < 120 && b < 120;
            if cyan || white || green { 1.0 } else { 0.0 }
        };
        let share = area_resample(|x, y| icon(view.px(x, y)), (t.left, t.top), (t.size, t.size), (THUMB, THUMB));
        let covered: Vec<bool> = share.data.iter().map(|&v| v >= ICON_CELL_SHARE).collect();
        (0..THUMB * THUMB)
            .map(|i| {
                let (x, y) = ((i % THUMB) as i32, (i / THUMB) as i32);
                !(-1..=1).any(|dy| {
                    (-1..=1).any(|dx| {
                        let (nx, ny) = (x + dx, y + dy);
                        nx >= 0 && ny >= 0 && (nx as usize) < THUMB && (ny as usize) < THUMB && covered[ny as usize * THUMB + nx as usize]
                    })
                })
            })
            .collect()
    }

    /// Correlation with every reference for an unzoomed view, `None` when the container is not
    /// inside `view`.
    pub fn scores(&self, view: &FrameView, t: &MapTransform) -> Option<Vec<f32>> {
        let features = Self::features(view, t)?;
        let keep = Self::keep_cells(view, t);
        Some((0..self.thumbs.len()).map(|i| self.score(&features, &keep, i)).collect())
    }

    /// The map whose objective table fits the objectives seen on an unzoomed view: at least
    /// [`LAYOUT_MIN_MATCHES`] letters in place and none out of place. Maps sharing the layout are
    /// separated by thumbnail score (lead of [`LAYOUT_TIE_MIN_MARGIN`]).
    pub fn identify_by_layout(&self, view: &FrameView, t: &MapTransform) -> Option<MapMatch> {
        let index = MapView::ALL.iter().position(|&v| v == t.view).unwrap_or(0);
        let seen = if self.elsewhere { self.objectives.read_elsewhere(view, t, self.icon_scales[index]) } else { self.objectives.read_at(view, t, self.icon_scales[index]) };
        if seen.len() < LAYOUT_MIN_MATCHES {
            return None;
        }
        let candidates: Vec<usize> = (0..self.layouts.len())
            .filter(|&i| {
                let (matched, contradicted) = layout_fit(&seen, &self.layouts[i]);
                matched >= LAYOUT_MIN_MATCHES && contradicted == 0
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let features = Self::features(view, t)?;
        let keep = Self::keep_cells(view, t);
        let mut scored: Vec<(usize, f32)> = candidates.iter().map(|&i| (i, self.score(&features, &keep, i))).collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        // `margin` is the lead over the next map sharing the layout (over 0 when the layout is unique).
        let (tied, margin) = match scored.get(1) {
            Some(next) => (scored[0].1 - next.1 < LAYOUT_TIE_MIN_MARGIN, scored[0].1 - next.1),
            None => (false, scored[0].1),
        };
        (!tied).then(|| MapMatch { index: scored[0].0, score: scored[0].1, margin, by_layout: true })
    }

    /// Best single-frame match for an unzoomed view: by objective layout when it decides,
    /// otherwise the thumbnail leader.
    pub fn identify(&self, view: &FrameView, t: &MapTransform) -> Option<MapMatch> {
        self.identify_by_layout(view, t).or_else(|| self.scores(view, t).map(|s| MapMatch::best(&s)))
    }
}

/// Averages recent identification scores; decides once a map leads clearly.
#[derive(Default)]
pub struct MapVote {
    recent: VecDeque<Vec<f32>>,
}

impl MapVote {
    pub fn clear(&mut self) {
        self.recent.clear();
    }

    /// Adds one frame's scores and returns the decided map, if any.
    pub fn push(&mut self, scores: Vec<f32>) -> Option<MapMatch> {
        let single = MapMatch::best(&scores);
        if self.recent.len() == VOTE_WINDOW {
            self.recent.pop_front();
        }
        self.recent.push_back(scores);
        if single.confident() {
            return Some(single);
        }
        if self.recent.len() < VOTE_MIN_FRAMES {
            return None;
        }
        let n = self.recent.len() as f32;
        let mean: Vec<f32> = (0..single_len(&self.recent))
            .map(|i| self.recent.iter().map(|s| s[i]).sum::<f32>() / n)
            .collect();
        let avg = MapMatch::best(&mean);
        (avg.score >= VOTE_MIN_SCORE && avg.margin >= VOTE_MIN_MARGIN).then_some(avg)
    }
}

fn single_len(recent: &VecDeque<Vec<f32>>) -> usize {
    recent.front().map_or(0, Vec::len)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 512² picture with detail at several scales and a transparent border, the same every run.
    fn picture() -> MapReference {
        let n = 512u32;
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rgba = Vec::with_capacity((n * n * 4) as usize);
        for y in 0..n {
            for x in 0..n {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let noise = (seed >> 56) as u8 / 4;
                let r = ((x * 255 / n) as u8).wrapping_add(noise);
                let g = ((y * 255 / n) as u8).wrapping_add(noise / 2);
                let b = (((x / 32 + y / 32) % 2) * 160) as u8 + noise;
                let a = if x < 16 || y < 16 || x >= n - 16 || y >= n - 16 { 0 } else { 255 };
                rgba.extend_from_slice(&[r, g, b, a]);
            }
        }
        MapReference::from_rgba("test", 1000.0, n, n, &rgba)
    }

    fn fnv(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
    }

    #[test]
    fn thumbnail_of_a_fixed_picture_is_unchanged() {
        // When this fails the way thumbnails are made changed: raise `THUMBNAIL_FORMAT` (saved
        // thumbnails are made again) and put the new value here.
        let sum = fnv(&Thumbnail::of(&picture()).to_bytes(0));
        assert_eq!(sum, 0x359e_dc69_de1e_9d72, "thumbnail checksum {sum:#018x}");
    }

    #[test]
    fn a_saved_thumbnail_reads_back_only_with_its_key() {
        let t = Thumbnail::of(&picture());
        let bytes = t.to_bytes(42);
        assert_eq!(Thumbnail::from_bytes(&bytes, 42), Some(t));
        assert_eq!(Thumbnail::from_bytes(&bytes, 43), None, "another reference");
        assert_eq!(Thumbnail::from_bytes(&bytes[..bytes.len() - 1], 42), None, "cut short");
    }
}
