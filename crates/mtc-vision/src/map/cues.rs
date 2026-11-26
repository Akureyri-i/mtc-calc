//! Which map views are on screen, from fixed UI cues next to them.
//!
//! - V1: the "Ping" mouse-hint icon under the full map (only drawn while V1 is open).
//! - V3: the grey "M" key box under the minimap (only drawn while V3 is shown, scoped or not).
//! - V2: scoped (the sight's black surround) while the minimap's "M" box is gone — opening the
//!   map in the sight replaces V3 with V2 — and a map in the V2 container: a black screen (death,
//!   loading) has the black surround and no "M" box too, but nothing lit where the map would be.
//!
//! In V1 the right-mouse hint reads "Remove marker" instead of "Add marker" while the cursor is
//! on a marker, which target bookkeeping uses to attribute deletions.
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
/// Off the reference the cues' places come from rules: the V1 hint row is right-aligned after
/// text drawn at stepped font sizes (the Ping icon was found up to 2.9 px from its rule), and the
/// minimap's M box sits 1-2 px off its container's scale where that is held to 0.32 H.
const SEARCH_ELSEWHERE: i32 = 4;
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

#[derive(Deserialize)]
struct CueFile {
    ping: CueAsset,
    m_key: CueAsset,
    hint_add: CueAsset,
    hint_remove: CueAsset,
}

#[derive(Deserialize)]
struct CueAsset {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    gray: Vec<u8>,
    mask: Vec<u8>,
}

struct CueTemplate {
    x: i32,
    y: i32,
    search: i32,
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
        Self { x: at.0, y: at.1, search, width: w as i32, height: h as i32, points, norm }
    }

    fn region(&self, client_origin: (i32, i32)) -> (i32, i32, i32, i32) {
        (
            self.x + client_origin.0 - self.search,
            self.y + client_origin.1 - self.search,
            self.width + 2 * self.search,
            self.height + 2 * self.search,
        )
    }

    /// Best masked NCC within ±`search` px of the nominal position, in whichever view covers it.
    fn score(&self, views: &[FrameView], client_origin: (i32, i32)) -> f32 {
        let (rx, ry, rw, rh) = self.region(client_origin);
        let Some(view) = views.iter().find(|v| v.covers(rx, ry, rw, rh)) else {
            return 0.0;
        };
        let n = self.points.len() as f32;
        let mut best = f32::MIN;
        for dy in -self.search..=self.search {
            for dx in -self.search..=self.search {
                let (ox, oy) = (self.x + client_origin.0 + dx, self.y + client_origin.1 + dy);
                let (mut sum, mut sum_sq, mut cross) = (0.0f32, 0.0f32, 0.0f32);
                for &(px, py, t) in &self.points {
                    let v = view.luma(ox + px, oy + py);
                    sum += v;
                    sum_sq += v * v;
                    cross += v * t;
                }
                let var = sum_sq - sum * sum / n;
                if var > 1.0 {
                    best = best.max(cross / (var.sqrt() * self.norm));
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
    /// The M box score from which V3 shows (the reader's: lower off the reference).
    pub m_key_min: f32,
}

impl ViewCues {
    /// No view shown: the cues not read (a display configuration they are not mapped to yet).
    pub const NONE: Self = Self { ping_score: -1.0, m_key_score: -1.0, scope_black: 0.0, scope_lit: 0.0, remove_hint_lead: 0.0, m_key_min: M_KEY_MIN_SCORE };

    pub fn scoped(&self) -> bool {
        self.scope_black >= SCOPE_MIN_BLACK_FRACTION
    }

    pub fn shows(&self, view: MapView) -> bool {
        match view {
            MapView::Full => self.ping_score >= PING_MIN_SCORE,
            MapView::Mini => self.m_key_score >= self.m_key_min,
            MapView::Scope => {
                self.scoped() && self.m_key_score < self.m_key_min && self.ping_score < PING_MIN_SCORE && self.scope_lit >= SCOPE_MIN_LIT
            }
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

/// The row through the middle of the V2 container (client px): left, right (exclusive), y.
fn scope_map_row((x, y, side): (f64, f64, f64)) -> (i32, i32, i32) {
    let inset = SCOPE_MAP_INSET * side / MapView::Scope.container().size;
    ((x + inset).round() as i32, (x + side - inset).round() as i32, (y + side / 2.0).round() as i32)
}

pub struct CueReader {
    ping: CueTemplate,
    m_key: CueTemplate,
    hint_add: CueTemplate,
    hint_remove: CueTemplate,
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
        Self {
            ping: CueTemplate::from(file.ping),
            m_key: CueTemplate::from(file.m_key),
            hint_add: CueTemplate::from(file.hint_add),
            hint_remove: CueTemplate::from(file.hint_remove),
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
        let ping = CueTemplate::scaled(&file.ping, (right - (331.5 - 11.0 * d) * s, bottom + 11.0 * s), s);
        let hint_at = |a: &CueAsset| (right - 9.0 * s - f64::from(a.width) * s, bottom + 15.0 * s);
        let hint_add = CueTemplate::scaled(&file.hint_add, hint_at(&file.hint_add), s);
        let hint_remove = CueTemplate::scaled(&file.hint_remove, hint_at(&file.hint_remove), s);
        let mini = placement.map_similarity(MapView::Mini);
        let m_key = CueTemplate::scaled(&file.m_key, mini.to_client((f64::from(file.m_key.x), f64::from(file.m_key.y))), mini.s);
        let reference = f64::from(crate::layout::UiLayout::REFERENCE.height);
        let row = |y: i32| (f64::from(y) * h / reference).round() as i32;
        let (first, last) = (row(SCOPE_PROBE_Y.0), row(SCOPE_PROBE_Y.1));
        let samples = (SCOPE_PROBE_Y.1 - SCOPE_PROBE_Y.0) / SCOPE_PROBE_STEP;
        let share = f64::from(SCOPE_PROBE_X) / (1280.0 - 667.0);
        Self {
            ping,
            m_key,
            hint_add,
            hint_remove,
            probe_x: (share * (w / 2.0 - h / 2.0)).round() as i32,
            probe_rows: (first, last, ((last - first) / samples).max(1)),
            scope_row: scope_map_row(placement.map_container(MapView::Scope)),
            m_key_min: M_KEY_MIN_SCORE_ELSEWHERE,
        }
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
        let hints = union(
            self.ping.region(client_origin),
            union(self.hint_add.region(client_origin), self.hint_remove.region(client_origin)),
        );
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
        let ping_score = self.ping.score(views, client_origin);
        let remove_hint_lead = if ping_score >= PING_MIN_SCORE {
            self.hint_remove.score(views, client_origin) - self.hint_add.score(views, client_origin)
        } else {
            0.0
        };
        ViewCues {
            ping_score,
            m_key_score: self.m_key.score(views, client_origin),
            scope_black: if total == 0 { 0.0 } else { black as f32 / total as f32 },
            scope_lit: if seen == 0 { 0.0 } else { lit as f32 / seen as f32 },
            remove_hint_lead,
            m_key_min: self.m_key_min,
        }
    }
}
