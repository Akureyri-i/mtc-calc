//! Which map views are on screen, from fixed UI cues next to them.
//!
//! - V1: the "Ping" mouse-hint icon under the full map (only drawn while V1 is open). Never while
//!   scoped: in the sight M opens V2, and the icon's place is the sight picture.
//! - V3: the grey "M" key box under the minimap (only drawn while V3 is shown, scoped or not).
//! - V2: scoped (the sight's black surround) while the minimap's "M" box is gone — opening the
//!   map in the sight replaces V3 with V2 — and a map in the V2 container: a black screen (death,
//!   loading) has the black surround and no "M" box too, but nothing lit where the map would be.
//!
//! In V1 the right-mouse hint reads "Remove marker" instead of "Add marker" while the cursor is
//! on a marker, which target bookkeeping uses to attribute deletions. Its text turns translucent
//! while a right click cannot add a marker: a marker is being dragged, or the cursor is off the map
//! (not while the map is panned), which tells whether a press picked a marker up.
//!
//! The icons are translucent light boxes with a dark glyph; masked normalised correlation over
//! the box keeps them recognisable over bright desert as well as dark grass.
//!
//! At another display configuration (`CueReader::with_layout`) each cue sits where the measured
//! layout rules put it on that client, with the icons resampled to their size there and searched
//! for over a slightly wider area.

use serde::Deserialize;

use crate::map::geometry::MapView;
use crate::view::FrameView;

const SEARCH: i32 = 2;
/// A window has to vary at least this share of the template's own spread (standard deviation)
/// to be scored. On a flat bright patch (cloud, a white hull) the one-pass variance cancels to
/// rounding noise and the correlation is meaningless: windows varying under half a luma level
/// scored up to 1.92, while from half a level up no window without the icon scored above 0.30.
/// Real icons vary at least 0.86 (Ping) and 0.34 (M box) of their template's spread.
const MIN_SPREAD_SHARE: f32 = 0.1;
/// Off the reference the cues' places come from rules: the V1 hint row is right-aligned after
/// text drawn at stepped font sizes (the Ping icon was found up to 2.9 px from its rule), and the
/// minimap's M box sits 1-2 px off its container's scale where that is held to 0.32 H.
const SEARCH_ELSEWHERE: i32 = 4;
/// Sideways search for the Ping icon off the reference: above 150 % scaling the hint row's text
/// steps put it further left than its rule, 3, 7 and 13 px at 175, 200 and 225 % on a 2560 x 1440
/// screen.
const PING_SEARCH_X_ELSEWHERE: i32 = 16;
const PING_MIN_SCORE: f32 = 0.5;
const M_KEY_MIN_SCORE: f32 = 0.5;
/// Off the reference the M is text drawn at the configuration's stepped size: on recorded gameplay
/// the box scored 0.547-0.99 on 1686 minimap frames and at most 0.34 elsewhere; midway.
const M_KEY_MIN_SCORE_ELSEWHERE: f32 = 0.45;
/// Scope surround probes: one column at client x 300, y 266..=916 every 25 px (left of the
/// sight, clear of the HUD).
const SCOPE_PROBE_X: i32 = 300;
const SCOPE_PROBE_Y: (i32, i32) = (266, 916);
const SCOPE_PROBE_STEP: i32 = 25;
const SCOPE_BLACK_MAX: u8 = 12;
const SCOPE_MIN_BLACK_FRACTION: f32 = 0.9;
/// The row through the middle of the V2 container (8 px in from each side): at least this share
/// lit for V2. On recorded frames: labelled V2 at least 0.698 (maps with transparent edges show
/// the black surround at the sides; all 405 frames the cues take for V2 scored 0.698 or more), the
/// 13 black and loading screens the cues once took for V2 0, and the end-of-match screen's
/// checkerboard transition 0.388.
const SCOPE_MAP_INSET: f64 = 8.0;
const SCOPE_MIN_LIT: f32 = 0.55;
/// V1 "Clear all" button (client coordinates; drawn while markers exist).
pub const CLEAR_ALL_BUTTON: (i32, i32, i32, i32) = (1632, 238, 78, 34);
/// A measured text factor in this range is Roblox's text size one step up (the readout fits about
/// 1.25 there, two steps about 1.75).
pub const TEXT_ONE_STEP: (f32, f32) = (1.15, 1.45);
/// One text size step up, "Remove marker" no longer fits its box under the full map: the box grows
/// leftwards and pushes the Ping icon this far left (px at the reference scale, measured in a
/// recording); "Add marker" still fits and leaves the icon where it was.
const REMOVE_OVERFLOW_PX: f64 = 50.0;
/// Sideways search (px) around that pushed place.
const OVERFLOW_SEARCH_X: i32 = 8;
/// The hint text's brightness above the ground beside it, as a share of the "Ping" word's (drawn
/// opaque) above the same ground: the translucent text measured 0.55-0.75 on 1430 recorded V1
/// frames, the opaque one 0.90-1.05 on 4950, with 34 frames between (fades).
const HINT_DIMMED_MAX: f32 = 0.8;
const HINT_OPAQUE_MIN: f32 = 0.88;
/// Undecided when the ground beside the row comes within this share of the Ping word's
/// brightness: over bright ground the translucent text is as bright as the opaque one.
const HINT_GROUND_MARGIN: f32 = 0.3;

#[derive(Deserialize)]
struct CueFile {
    ping: CueAsset,
    m_key: CueAsset,
    hint_add: CueAsset,
    hint_remove: CueAsset,
}

#[derive(Clone, Deserialize)]
struct CueAsset {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    gray: Vec<u8>,
    mask: Vec<u8>,
}

impl CueAsset {
    /// The key box with its letter drawn `factor` times larger about the letter's centre (Roblox's
    /// text size raised: the letter grows, the box does not). Pixels 3 px or more inside the box
    /// take the letter's values sampled back toward its centre; the box's edge stays.
    fn letter_scaled(&self, factor: f64) -> Self {
        let (w, h) = (self.width as usize, self.height as usize);
        let at = |x: usize, y: usize| f64::from(self.gray[y * w + x]);
        let inner = |x: usize, y: usize| x >= 3 && y >= 3 && x + 3 < w && y + 3 < h;
        let mut box_values: Vec<f64> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| inner(x, y)).map(|(x, y)| at(x, y)).collect();
        box_values.sort_by(f64::total_cmp);
        let (darkest, middle) = (box_values[0], box_values[box_values.len() / 2]);
        let ink = (darkest + middle) / 2.0;
        let letter: Vec<(f64, f64)> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| inner(x, y) && at(x, y) < ink).map(|(x, y)| (x as f64, y as f64)).collect();
        if letter.is_empty() {
            return self.clone();
        }
        let n = letter.len() as f64;
        let (cx, cy) = (letter.iter().map(|p| p.0).sum::<f64>() / n, letter.iter().map(|p| p.1).sum::<f64>() / n);
        let sample = |x: f64, y: f64| {
            let (x0, y0) = (x.floor().clamp(0.0, (w - 1) as f64), y.floor().clamp(0.0, (h - 1) as f64));
            let (x1, y1) = ((x0 + 1.0).min((w - 1) as f64), (y0 + 1.0).min((h - 1) as f64));
            let (fx, fy) = ((x - x0).clamp(0.0, 1.0), (y - y0).clamp(0.0, 1.0));
            let g = |x: f64, y: f64| at(x as usize, y as usize);
            (g(x0, y0) * (1.0 - fx) + g(x1, y0) * fx) * (1.0 - fy) + (g(x0, y1) * (1.0 - fx) + g(x1, y1) * fx) * fy
        };
        let mut gray = self.gray.clone();
        for y in 0..h {
            for x in 0..w {
                if inner(x, y) {
                    gray[y * w + x] = sample(cx + (x as f64 - cx) / factor, cy + (y as f64 - cy) / factor).round() as u8;
                }
            }
        }
        Self { gray, ..self.clone() }
    }
}

#[derive(Clone)]
struct CueTemplate {
    x: i32,
    y: i32,
    search: i32,
    /// Sideways search (px), `search` unless widened.
    search_x: i32,
    width: i32,
    height: i32,
    /// Mean-removed template values at masked pixels, with their offsets.
    points: Vec<(i32, i32, f32)>,
    norm: f32,
}

impl CueTemplate {
    fn from(asset: CueAsset) -> Self {
        let (w, h) = (asset.width as usize, asset.height as usize);
        let gray: Vec<f32> = asset.gray.iter().map(|&v| f32::from(v)).collect();
        let mask: Vec<f32> = asset.mask.iter().map(|&v| if v != 0 { 1.0 } else { 0.0 }).collect();
        Self::of((asset.x, asset.y), (w, h), &gray, &mask, SEARCH)
    }

    /// The asset's template at another display configuration: drawn `k` times its size with its
    /// top-left at `at` (client px), each pixel the area mean of the asset pixels it covers.
    fn scaled(asset: &CueAsset, at: (f64, f64), k: f64) -> Self {
        let (w, h) = (asset.width as usize, asset.height as usize);
        let (nw, nh) = (((w as f64 * k).round() as usize).max(3), ((h as f64 * k).round() as usize).max(3));
        let (kx, ky) = (nw as f64 / w as f64, nh as f64 / h as f64);
        let (mut gray, mut mask) = (vec![0.0f32; nw * nh], vec![0.0f32; nw * nh]);
        for j in 0..nh {
            let (y0, y1) = (j as f64 / ky, (j + 1) as f64 / ky);
            for i in 0..nw {
                let (x0, x1) = (i as f64 / kx, (i + 1) as f64 / kx);
                let (mut sum, mut weight, mut masked) = (0.0, 0.0, 0.0);
                for sy in y0.floor() as usize..(y1.ceil() as usize).min(h) {
                    let wy = (y1.min(sy as f64 + 1.0) - y0.max(sy as f64)).max(0.0);
                    for sx in x0.floor() as usize..(x1.ceil() as usize).min(w) {
                        let a = wy * (x1.min(sx as f64 + 1.0) - x0.max(sx as f64)).max(0.0);
                        let m = if asset.mask[sy * w + sx] != 0 { 1.0 } else { 0.0 };
                        sum += a * m * f64::from(asset.gray[sy * w + sx]);
                        masked += a * m;
                        weight += a;
                    }
                }
                let k = j * nw + i;
                mask[k] = if weight > 0.0 && masked / weight >= 0.5 { 1.0 } else { 0.0 };
                gray[k] = if masked > 0.0 { (sum / masked) as f32 } else { 0.0 };
            }
        }
        Self::of((at.0.round() as i32, at.1.round() as i32), (nw, nh), &gray, &mask, SEARCH_ELSEWHERE)
    }

    fn of(at: (i32, i32), (w, h): (usize, usize), gray: &[f32], mask: &[f32], search: i32) -> Self {
        let mut raw = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if mask[i] != 0.0 {
                    raw.push((x as i32, y as i32, gray[i]));
                }
            }
        }
        let mean = raw.iter().map(|p| p.2).sum::<f32>() / raw.len() as f32;
        let points: Vec<_> = raw.into_iter().map(|(x, y, v)| (x, y, v - mean)).collect();
        let norm = points.iter().map(|p| p.2 * p.2).sum::<f32>().sqrt();
        Self { x: at.0, y: at.1, search, search_x: search, width: w as i32, height: h as i32, points, norm }
    }

    fn region(&self, client_origin: (i32, i32)) -> (i32, i32, i32, i32) {
        (
            self.x + client_origin.0 - self.search_x,
            self.y + client_origin.1 - self.search,
            self.width + 2 * self.search_x,
            self.height + 2 * self.search,
        )
    }

    /// Best masked NCC within ±`search` px of the nominal position, in whichever view covers it.
    fn score(&self, views: &[FrameView], client_origin: (i32, i32)) -> f32 {
        self.score_detail(views, client_origin, MIN_SPREAD_SHARE).0
    }

    /// The template's spread: standard deviation of its masked values (luma levels).
    fn spread(&self) -> f32 {
        self.norm / (self.points.len() as f32).sqrt()
    }

    /// Best masked NCC and the spread (standard deviation, luma levels) of the window it was
    /// found in, over the windows that vary at least `min_share` of the template's spread.
    fn score_detail(&self, views: &[FrameView], client_origin: (i32, i32), min_share: f32) -> (f32, f32) {
        let (rx, ry, rw, rh) = self.region(client_origin);
        let Some(view) = views.iter().find(|v| v.covers(rx, ry, rw, rh)) else {
            return (0.0, 0.0);
        };
        let n = self.points.len() as f32;
        // As variance summed over the points (`var` below); at least 1 for a defined score.
        let min_var = ((min_share * self.spread()).powi(2) * n).max(1.0);
        let (mut best, mut best_spread) = (f32::MIN, 0.0f32);
        for dy in -self.search..=self.search {
            for dx in -self.search_x..=self.search_x {
                let (ox, oy) = (self.x + client_origin.0 + dx, self.y + client_origin.1 + dy);
                let (mut sum, mut sum_sq, mut cross) = (0.0f32, 0.0f32, 0.0f32);
                for &(px, py, t) in &self.points {
                    let v = view.luma(ox + px, oy + py);
                    sum += v;
                    sum_sq += v * v;
                    cross += v * t;
                }
                let var = sum_sq - sum * sum / n;
                if var > min_var {
                    let score = cross / (var.sqrt() * self.norm);
                    if score > best {
                        (best, best_spread) = (score, (var / n).sqrt());
                    }
                }
            }
        }
        (best, best_spread)
    }

    /// Development: the best masked NCC within ±`radius` px of the nominal position and its
    /// offset from it, over windows with the usual spread floor.
    fn best_within(&self, views: &[FrameView], client_origin: (i32, i32), radius: i32) -> (f32, i32, i32) {
        let (x, y) = (self.x + client_origin.0 - radius, self.y + client_origin.1 - radius);
        let Some(view) = views.iter().find(|v| v.covers(x, y, self.width + 2 * radius, self.height + 2 * radius)) else {
            return (0.0, 0, 0);
        };
        let n = self.points.len() as f32;
        let min_var = ((MIN_SPREAD_SHARE * self.spread()).powi(2) * n).max(1.0);
        let mut best = (f32::MIN, 0, 0);
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let (ox, oy) = (self.x + client_origin.0 + dx, self.y + client_origin.1 + dy);
                let (mut sum, mut sum_sq, mut cross) = (0.0f32, 0.0f32, 0.0f32);
                for &(px, py, t) in &self.points {
                    let v = view.luma(ox + px, oy + py);
                    sum += v;
                    sum_sq += v * v;
                    cross += v * t;
                }
                let var = sum_sq - sum * sum / n;
                if var > min_var {
                    let score = cross / (var.sqrt() * self.norm);
                    if score > best.0 {
                        best = (score, dx, dy);
                    }
                }
            }
        }
        best
    }
}

/// Cue readings for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewCues {
    pub ping_score: f32,
    pub m_key_score: f32,
    /// Fraction of scope-surround probes that are black.
    pub scope_black: f32,
    /// Fraction of the row through the V2 container that is not black.
    pub scope_lit: f32,
    /// V1 hint text correlation with "Remove marker" minus with "Add marker".
    pub remove_hint_lead: f32,
    /// V1's "Add marker" drawn translucent (a right click cannot add a marker: one is being dragged
    /// or the cursor is off the map) or opaque; `None` when not read or not decidable (V1 not
    /// shown, "Remove marker", bright ground behind the row, Roblox's text size raised).
    pub hint_dimmed: Option<bool>,
    /// The M box score from which V3 shows (the reader's: lower off the reference).
    pub m_key_min: f32,
}

impl ViewCues {
    /// No view shown: the cues not read (a display configuration they are not mapped to yet).
    pub const NONE: Self = Self { ping_score: -1.0, m_key_score: -1.0, scope_black: 0.0, scope_lit: 0.0, remove_hint_lead: 0.0, hint_dimmed: None, m_key_min: M_KEY_MIN_SCORE };

    pub fn scoped(&self) -> bool {
        self.scope_black >= SCOPE_MIN_BLACK_FRACTION
    }

    /// Whether `view` is on screen. In the sight M opens the scope map, never the full map, so a
    /// Ping score while scoped is the scene under the hint row's place.
    pub fn shows(&self, view: MapView) -> bool {
        match view {
            MapView::Full => self.ping_score >= PING_MIN_SCORE && !self.scoped(),
            MapView::Mini => self.m_key_score >= self.m_key_min,
            MapView::Scope => self.scoped() && self.m_key_score < self.m_key_min && self.scope_lit >= SCOPE_MIN_LIT,
        }
    }

    /// V1 shows "Remove marker": the cursor is on a marker.
    pub fn remove_hint(&self) -> bool {
        self.shows(MapView::Full) && self.remove_hint_lead > 0.0
    }

    /// The largest view on screen (V1 > V2 > V3).
    pub fn primary(&self) -> Option<MapView> {
        MapView::ALL.into_iter().find(|&v| self.shows(v))
    }
}

/// Where the hint row's brightness is measured (client px: x, y, width, height), from the Ping
/// icon's and the marker hint's places at HUD scale `s`: the "Ping" word right of its icon, the
/// "Add marker" glyphs, and the ground left of the right-mouse icon and right of the text.
#[derive(Clone, Copy, Debug)]
struct HintBoxes {
    ping_word: (i32, i32, i32, i32),
    text: (i32, i32, i32, i32),
    ground: [(i32, i32, i32, i32); 2],
    /// How many of the brightest pixels stand for a glyph's core (the word, the text).
    cores: (usize, usize),
}

impl HintBoxes {
    fn at(ping: (i32, i32), hint: (i32, i32), s: f64) -> Self {
        let r = |x: f64, y: f64, w: f64, h: f64| (x.round() as i32, y.round() as i32, (w * s).round().max(1.0) as i32, (h * s).round().max(1.0) as i32);
        let (px, py, hx, hy) = (f64::from(ping.0), f64::from(ping.1), f64::from(hint.0), f64::from(hint.1));
        Self {
            ping_word: r(px + 42.0 * s, py + 8.0 * s, 48.0, 26.0),
            text: r(hx + 2.0 * s, hy + 9.0 * s, 102.0, 14.0),
            ground: [r(hx - 70.0 * s, hy + 4.0 * s, 22.0, 20.0), r(hx + 108.0 * s, hy + 4.0 * s, 30.0, 20.0)],
            cores: (((30.0 * s * s).round() as usize).max(4), ((60.0 * s * s).round() as usize).max(6)),
        }
    }

    /// Whether the hint text is translucent, `None` when the frame cannot tell.
    fn dimmed(&self, views: &[FrameView], client_origin: (i32, i32)) -> Option<bool> {
        let lumas = |(x, y, w, h): (i32, i32, i32, i32)| -> Option<Vec<f32>> {
            let (fx, fy) = (x + client_origin.0, y + client_origin.1);
            let view = views.iter().find(|v| v.covers(fx, fy, w, h))?;
            Some((fy..fy + h).flat_map(|y| (fx..fx + w).map(move |x| (x, y))).map(|(x, y)| view.luma(x, y)).collect())
        };
        // The mean of the `n` brightest values, and the median.
        let core = |mut v: Vec<f32>, n: usize| {
            let from = v.len().saturating_sub(n);
            v.select_nth_unstable_by(from, f32::total_cmp);
            v[from..].iter().sum::<f32>() / (v.len() - from) as f32
        };
        let median = |mut v: Vec<f32>| {
            let mid = v.len() / 2;
            *v.select_nth_unstable_by(mid, f32::total_cmp).1
        };
        let ping = core(lumas(self.ping_word)?, self.cores.0);
        let text = core(lumas(self.text)?, self.cores.1);
        let ground = self.ground.iter().map(|&b| lumas(b).map(median)).collect::<Option<Vec<f32>>>()?.into_iter().fold(0.0f32, f32::max);
        if ping - ground < HINT_GROUND_MARGIN * ping {
            return None;
        }
        let share = (text - ground) / (ping - ground);
        if share <= HINT_DIMMED_MAX {
            Some(true)
        } else if share >= HINT_OPAQUE_MIN {
            Some(false)
        } else {
            None
        }
    }
}

/// The row through the middle of the V2 container (client px): left, right (exclusive), y.
fn scope_map_row((x, y, side): (f64, f64, f64)) -> (i32, i32, i32) {
    let inset = SCOPE_MAP_INSET * side / MapView::Scope.container().size;
    ((x + inset).round() as i32, (x + side - inset).round() as i32, (y + side / 2.0).round() as i32)
}

pub struct CueReader {
    ping: CueTemplate,
    /// The Ping icon where "Remove marker" pushes it with the text size one step up
    /// (`set_text_factor`); there it also says the hint is "Remove marker".
    ping_overflow: Option<CueTemplate>,
    m_key: CueTemplate,
    /// Where the M box's template goes and its scale (`None`: the reference's own template).
    m_key_place: Option<((f64, f64), f64)>,
    /// The configuration's HUD scale (1 on the reference).
    scale: f64,
    hint_add: CueTemplate,
    hint_remove: CueTemplate,
    hint_boxes: HintBoxes,
    /// Scope surround probes (client px): the column, its first and last row, the step.
    probe_x: i32,
    probe_rows: (i32, i32, i32),
    /// The row through the V2 container: left, right (exclusive), y.
    scope_row: (i32, i32, i32),
    m_key_min: f32,
}

impl Default for CueReader {
    fn default() -> Self {
        Self::new()
    }
}

impl CueReader {
    pub fn new() -> Self {
        let file = Self::asset();
        let c = MapView::Scope.container();
        let hint_boxes = HintBoxes::at((file.ping.x, file.ping.y), (file.hint_add.x, file.hint_add.y), 1.0);
        Self {
            ping: CueTemplate::from(file.ping),
            ping_overflow: None,
            m_key: CueTemplate::from(file.m_key),
            m_key_place: None,
            scale: 1.0,
            hint_add: CueTemplate::from(file.hint_add),
            hint_remove: CueTemplate::from(file.hint_remove),
            hint_boxes,
            probe_x: SCOPE_PROBE_X,
            probe_rows: (SCOPE_PROBE_Y.0, SCOPE_PROBE_Y.1, SCOPE_PROBE_STEP),
            scope_row: scope_map_row((c.x, c.y, c.size)),
            m_key_min: M_KEY_MIN_SCORE,
        }
    }

    fn asset() -> CueFile {
        serde_json::from_str(include_str!("../../assets/map_cues.json")).expect("cue asset")
    }

    /// The cues at another display configuration (rules measured on recorded gameplay):
    /// - the V1 hint row under the full map, right-aligned: the Ping icon (331.5 - 11 d) s left of
    ///   the full map's right edge and 11 s below it, the marker hint ending 9 s in from that edge,
    ///   icons s times their size;
    /// - the M box where the minimap's container puts it, at its scale;
    /// - the scope probes in the same share (0.49) of the way from the client's left to the
    ///   sight's leftmost point (the sight is a circle of radius H / 2 on the client's centre), at
    ///   the same shares of the height;
    /// - the row through the scope map's container.
    pub fn with_layout(layout: &crate::layout::UiLayout) -> Self {
        Self::for_placement(&crate::layout::Placement::new((0, 0), *layout))
    }

    /// As `with_layout`, from the map containers a placement has (found on screen, else by rule).
    pub fn for_placement(placement: &crate::layout::Placement) -> Self {
        let layout = &placement.layout;
        if layout.is_reference() {
            return Self::new();
        }
        let file = Self::asset();
        let (s, d) = (layout.hud_scale(), layout.d());
        let (w, h) = (f64::from(layout.width), f64::from(layout.height));
        let (fl, ft, fs) = placement.map_container(MapView::Full);
        let (right, bottom) = (fl + fs, ft + fs);
        let mut ping = CueTemplate::scaled(&file.ping, (right - (331.5 - 11.0 * d) * s, bottom + 11.0 * s), s);
        ping.search_x = PING_SEARCH_X_ELSEWHERE;
        let hint_at = |a: &CueAsset| (right - 9.0 * s - f64::from(a.width) * s, bottom + 15.0 * s);
        let hint_add = CueTemplate::scaled(&file.hint_add, hint_at(&file.hint_add), s);
        let hint_remove = CueTemplate::scaled(&file.hint_remove, hint_at(&file.hint_remove), s);
        let mini = placement.map_similarity(MapView::Mini);
        let m_key_at = mini.to_client((f64::from(file.m_key.x), f64::from(file.m_key.y)));
        let m_key = CueTemplate::scaled(&file.m_key, m_key_at, mini.s);
        let reference = f64::from(crate::layout::UiLayout::REFERENCE.height);
        let row = |y: i32| (f64::from(y) * h / reference).round() as i32;
        let (first, last) = (row(SCOPE_PROBE_Y.0), row(SCOPE_PROBE_Y.1));
        let samples = (SCOPE_PROBE_Y.1 - SCOPE_PROBE_Y.0) / SCOPE_PROBE_STEP;
        let share = f64::from(SCOPE_PROBE_X) / (1280.0 - 667.0);
        let hint_boxes = HintBoxes::at((ping.x, ping.y), (hint_add.x, hint_add.y), s);
        Self {
            ping,
            ping_overflow: None,
            m_key,
            m_key_place: Some((m_key_at, mini.s)),
            scale: s,
            hint_add,
            hint_remove,
            hint_boxes,
            probe_x: (share * (w / 2.0 - h / 2.0)).round() as i32,
            probe_rows: (first, last, ((last - first) / samples).max(1)),
            scope_row: scope_map_row(placement.map_container(MapView::Scope)),
            m_key_min: M_KEY_MIN_SCORE_ELSEWHERE,
        }
    }

    /// The text factor measured on screen (`1` as the rules have it): with Roblox's text size one
    /// step up (`TEXT_ONE_STEP`) the Ping icon is looked for where "Remove marker" pushes it too and
    /// the M box's letter is drawn as much larger; otherwise the cues are read as the rules put
    /// them.
    pub fn set_text_factor(&mut self, factor: f32) {
        let one_step = (TEXT_ONE_STEP.0..TEXT_ONE_STEP.1).contains(&factor);
        let file = Self::asset();
        let asset = if one_step { file.m_key.letter_scaled(f64::from(factor)) } else { file.m_key };
        self.m_key = match self.m_key_place {
            None => CueTemplate::from(asset),
            Some((at, s)) => CueTemplate::scaled(&asset, at, s),
        };
        self.ping_overflow = one_step.then(|| {
            let mut pushed = CueTemplate { x: self.ping.x - (REMOVE_OVERFLOW_PX * self.scale).round() as i32, ..self.ping.clone() };
            pushed.search_x = OVERFLOW_SEARCH_X;
            pushed
        });
    }

    /// Frame rectangles the cues read, for region read-back: the V1 hint row (Ping icon and
    /// marker hint), the minimap's M key, the scope probe column and the row through the V2
    /// container.
    pub fn regions(&self, client_origin: (i32, i32)) -> [(i32, i32, i32, i32); 4] {
        let union = |a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)| {
            let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
            let (x1, y1) = ((a.0 + a.2).max(b.0 + b.2), (a.1 + a.3).max(b.1 + b.3));
            (x0, y0, x1 - x0, y1 - y0)
        };
        let mut hints = union(
            self.ping.region(client_origin),
            union(self.hint_add.region(client_origin), self.hint_remove.region(client_origin)),
        );
        if let Some(pushed) = &self.ping_overflow {
            hints = union(hints, pushed.region(client_origin));
        }
        [
            hints,
            self.m_key.region(client_origin),
            (self.probe_x + client_origin.0, self.probe_rows.0 + client_origin.1, 1, self.probe_rows.1 - self.probe_rows.0 + 1),
            {
                let (x0, x1, y) = self.scope_row;
                (x0 + client_origin.0, y + client_origin.1, x1 - x0, 1)
            },
        ]
    }

    /// Development: the Ping icon's best score over every window (no spread floor), the spread of
    /// the window it was found in and the template's own spread (luma levels).
    pub fn ping_detail(&self, views: &[FrameView], client_origin: (i32, i32)) -> (f32, f32, f32) {
        let (score, spread) = self.ping.score_detail(views, client_origin, 0.0);
        (score, spread, self.ping.spread())
    }

    /// Development: the Ping icon's and the M box's best score within ±`radius` px of where the
    /// layout puts them, each with its offset (x, y) from there.
    pub fn cue_offsets(&self, views: &[FrameView], client_origin: (i32, i32), radius: i32) -> [(f32, i32, i32); 2] {
        [self.ping.best_within(views, client_origin, radius), self.m_key.best_within(views, client_origin, radius)]
    }

    /// Development: the marker hint's translucency alone (`ViewCues::hint_dimmed` without the
    /// checks of whether V1 shows it).
    pub fn hint_translucency(&self, views: &[FrameView], client_origin: (i32, i32)) -> Option<bool> {
        self.hint_boxes.dimmed(views, client_origin)
    }

    /// Development: as `ping_detail`, for the minimap's M box.
    pub fn m_key_detail(&self, views: &[FrameView], client_origin: (i32, i32)) -> (f32, f32, f32) {
        let (score, spread) = self.m_key.score_detail(views, client_origin, 0.0);
        (score, spread, self.m_key.spread())
    }

    pub fn read(&self, view: &FrameView, client_origin: (i32, i32)) -> ViewCues {
        self.read_views(std::slice::from_ref(view), client_origin)
    }

    /// Reads the cues from several views (each piece from whichever view covers it), e.g. the
    /// read-back regions of [`CueReader::regions`].
    pub fn read_views(&self, views: &[FrameView], client_origin: (i32, i32)) -> ViewCues {
        let (mut black, mut total) = (0u32, 0u32);
        let (first, last, step) = self.probe_rows;
        for y in (first..=last).step_by(step as usize) {
            let (fx, fy) = (self.probe_x + client_origin.0, y + client_origin.1);
            if let Some(view) = views.iter().find(|v| v.covers(fx, fy, 1, 1)) {
                total += 1;
                let p = view.px(fx, fy);
                if p[0].max(p[1]).max(p[2]) <= SCOPE_BLACK_MAX {
                    black += 1;
                }
            }
        }
        let (x0, x1, y) = self.scope_row;
        let (y, mut lit, mut seen) = (y + client_origin.1, 0u32, 0u32);
        for x in x0 + client_origin.0..x1 + client_origin.0 {
            if let Some(view) = views.iter().find(|v| v.covers(x, y, 1, 1)) {
                seen += 1;
                let p = view.px(x, y);
                if p[0].max(p[1]).max(p[2]) > SCOPE_BLACK_MAX {
                    lit += 1;
                }
            }
        }
        let (ping_score, remove_hint_lead): (f32, f32) = match &self.ping_overflow {
            // One text size step up: the icon where it was says "Add marker", pushed left says
            // "Remove marker" (the hint's own text is drawn larger than its templates).
            Some(pushed) => {
                let (here, there) = (self.ping.score(views, client_origin), pushed.score(views, client_origin));
                (here.max(there), there - here)
            }
            None => {
                let score = self.ping.score(views, client_origin);
                let lead = if score >= PING_MIN_SCORE {
                    self.hint_remove.score(views, client_origin) - self.hint_add.score(views, client_origin)
                } else {
                    0.0
                };
                (score, lead)
            }
        };
        // With the text size raised the hint's text is drawn larger than the boxes assume.
        let hint_dimmed = if ping_score >= PING_MIN_SCORE && remove_hint_lead <= 0.0 && self.ping_overflow.is_none() {
            self.hint_boxes.dimmed(views, client_origin)
        } else {
            None
        };
        ViewCues {
            ping_score,
            m_key_score: self.m_key.score(views, client_origin),
            scope_black: if total == 0 { 0.0 } else { black as f32 / total as f32 },
            scope_lit: if seen == 0 { 0.0 } else { lit as f32 / seen as f32 },
            remove_hint_lead,
            hint_dimmed,
            m_key_min: self.m_key_min,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat bright patch where an icon would be (a white hull, a cloud) scores nothing, however
    /// the rounding of its variance falls.
    #[test]
    fn a_flat_patch_is_not_an_icon() {
        let (w, h) = (2560u32, 1368u32);
        let data: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let v = 205 + ((i % 7 + i / w % 3) % 2) as u8;
                [v, v, v]
            })
            .collect();
        let cues = CueReader::new().read(&FrameView::rgb(&data, w, h), (0, 34));
        assert!(cues.ping_score < PING_MIN_SCORE && cues.m_key_score < M_KEY_MIN_SCORE, "{cues:?}");
    }

    /// The M box's letter drawn larger keeps the box: same size and mask, more dark pixels, the
    /// box's edge unchanged.
    #[test]
    fn a_larger_letter_keeps_its_box() {
        let asset = CueReader::asset().m_key;
        let larger = asset.letter_scaled(1.25);
        let dark = |a: &CueAsset| a.gray.iter().filter(|&&v| v < 110).count();
        assert_eq!((larger.width, larger.height, &larger.mask), (asset.width, asset.height, &asset.mask));
        assert!(dark(&larger) > dark(&asset) * 5 / 4, "{} vs {}", dark(&larger), dark(&asset));
        let w = asset.width as usize;
        assert!((0..w).all(|x| larger.gray[x] == asset.gray[x] && larger.gray[w * (w - 1) + x] == asset.gray[w * (w - 1) + x]));
    }

    /// In the sight M opens the scope map: a Ping score there neither shows V1 nor hides V2.
    #[test]
    fn no_full_map_while_scoped() {
        let scoped = ViewCues { ping_score: 0.9, m_key_score: 0.0, scope_black: 1.0, scope_lit: 0.8, remove_hint_lead: 0.0, hint_dimmed: None, m_key_min: M_KEY_MIN_SCORE };
        assert_eq!(scoped.primary(), Some(MapView::Scope));
        assert_eq!(ViewCues { scope_black: 0.0, ..scoped }.primary(), Some(MapView::Full));
    }
}
