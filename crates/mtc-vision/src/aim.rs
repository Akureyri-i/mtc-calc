//! Gunner angle readout ("Elevation +12.34°", "Azimuth 056.78°"), read by glyph shape so that it
//! reads at any display scaling and window size once the frame is mapped to the reference geometry.
//!
//! - How large the label and the digits are drawn at the current configuration is given
//!   ([`TextScale`], measured for each configuration); nothing is searched over scales at run
//!   time.
//! - Ink is grey coverage over the ground, (lowest channel − ground) / (255 − ground), each value
//!   line stretched so its brightest strokes count as full ink.
//! - The "Elevation" label is found along the row by normalised correlation of a grey template
//!   (blurred, broad light ramps taken out) at the given scale, light on dark and dark on bright
//!   (snow): half resolution first, the best places refined at full resolution. The best few are
//!   tried in turn; the "Azimuth" label must sit under the one taken.
//! - Each value line is resampled so its digits match the templates (their bottom on a fixed row),
//!   and every run of ink is compared with each symbol's mean glyph slid along it (one row up or
//!   down allowed). A dynamic program fits the runs to the readout's format.
//! - A line is skipped rather than read wrong: its total cost, each glyph's cost and lead over the
//!   next candidate (more for a glyph that is the runner-up plus strokes), and the two lines'
//!   alignment under the label are all checked.
//!
//! The place that read last is tried first on the next frame, so the label search only runs when
//! the readout moves or first appears.

use serde::Deserialize;

use crate::map::raster::{Plane, resize};
use crate::view::FrameView;

/// Rows in client coordinates at the reference geometry (2560×1334 client, 150 %).
const LABEL_TOP: i32 = 1221;
const ELEVATION_TOP: i32 = 1242;
const AZIMUTH_TOP: i32 = 1298;
/// The digits' bottom row below a value line's top (the same at every configuration measured:
/// larger or smaller digits grow upwards).
const DIGIT_BOTTOM: f32 = 15.0;
/// Value line rows, the margin around them in a line window, and the window's half width.
const ROWS: usize = 18;
const MARGIN: usize = 3;
const HALF: i32 = 64;
/// Widest single glyph (px); runs up to `GW + 2` are one glyph, wider ones two.
const GW: usize = 13;
/// Coverage above which a pixel is ink when cutting a line into runs.
const SEG: f32 = 0.55;
/// Runs at most `CHAIN_GAP_ANY` columns apart are one line of text, and text-shaped runs (within
/// the digit band's rows) up to `CHAIN_GAP`: a "1" before the dot stands 3 apart at the reference
/// and 4-5 at 100 % (1920x760), where 4 dropped the "+11"; a grid line 5 columns past the degree
/// sign is no text.
const CHAIN_GAP_ANY: usize = 4;
const CHAIN_GAP: usize = 6;
/// A gap between text-shaped runs up to this wide is bridged when it holds coverage above
/// `DOT_FAINT` on the dot's rows.
const DOT_BRIDGE: usize = 12;
const DOT_FAINT: f32 = 0.25;
/// Runs this wide and up to `GW + 2` are also read as two glyphs, one of them a "1".
const NARROW_PAIR: usize = 11;
/// A run's 8-connected pieces sharing more columns than this are one glyph (`split_run`).
const SPLIT_OVERLAP: usize = 2;
/// A run's pieces with fewer px than this are specks that belong to their neighbour.
const SPLIT_MIN_INK: usize = 6;
/// Value lines: a pixel's lowest channel fades out between these saturations (the readout is white
/// and grey; bright grass blades or sand across a glyph are coloured). `None` reads the lowest
/// channel as it is. The label search always uses the lowest channel as it is.
const LINE_SAT_FADE: Option<(f32, f32)> = match option_env!("MTC_AIM_SAT_FADE") {
    Some(_) => Some((0.3, 0.55)),
    None => None,
};
/// The frame area the reader needs (client x, y, width, height).
const AREA: (i32, i32, i32, i32) = (330, 1205, 1340, 129);
/// Spare columns each side of what reading at a known place looks at (`AimReader::near`).
const NEAR_MARGIN: i32 = 4;
/// Label search: client columns, strip rows around `LABEL_TOP`, blur, high-pass.
const LABEL_SEARCH: (i32, i32) = (396, 1604);
const LABEL_STRIP: (i32, i32) = (-6, 20);
const LABEL_BLUR: f32 = 1.2;
const LABEL_HIGHPASS: f32 = 4.0;
const LABEL_MIN: f32 = 0.45;
/// Windows flatter than this (standard deviation after the high-pass) are not scored: the sight's
/// black surround gives meaningless correlations.
const LABEL_MIN_STD: f32 = 4.0;
const LABEL_CANDIDATES: usize = 6;
const LABEL_APART: i32 = 30;
/// Half-resolution search: lowest score kept, flatness bar relative to `LABEL_MIN_STD`, how far
/// apart (half px) and how many places go on to the full-resolution refinement; the margin of the
/// local strip prepared around each.
const COARSE_MIN: f32 = 0.3;
const COARSE_STD: f32 = 0.5;
const COARSE_APART: i32 = 8;
const COARSE_SEEDS: usize = 8;
const REFINE_MARGIN: i32 = 24;
/// The "Azimuth" label's score a candidate needs before its lines are decoded (the decode's own
/// checks keep wrong readings out). At 0.5 faint snow readouts and labels partly behind a
/// building were passed over; 0.3 read 43 more frames of 19091, all right, none changed.
const AZ_MIN: f32 = 0.3;
const GLYPH_BLUR: f32 = 0.6;
const W_SHAPE: f32 = 30.0;
const W_EXTRA: f32 = 2.0;
const NOISE_PER_PX: f32 = 0.4;
/// The most of the digit height a sign's ink may span ("+" spans about 0.57 of it).
const SIGN_MAX_HEIGHT: f32 = 0.8;
/// Columns a run may be wider than the glyph it is read as.
const RUN_SPARE: usize = 3;
const TALL_NOISE: f32 = 3.0;
/// A missing dot costs more than dropping a dot-shaped run as background and reading the rest
/// shifted (a line merged into the last digits read "+00.07" for "+0.00").
const MISSING_DOT: f32 = 12.0;
const MISSING_DEGREE: f32 = 6.0;
const MISSING_OTHER: f32 = 60.0;
const MAX_COST: f32 = 60.0;
const G_MAX_DIGIT: f32 = 12.0;
const G_MAX_OTHER: f32 = 15.0;
const M_MIN: f32 = 3.0;
/// A sign with more ink than the runner-up ("+" over "-") must lead by this much: specks add
/// strokes, they never take them away, and a flipped sign is the worst misreading.
const M_SUPERSET: f32 = 8.0;
/// Ink ratio above which the chosen glyph counts as the runner-up plus strokes.
const SUPERSET_INK: f32 = 1.1;
/// A glyph matching its template this closely needs only the usual lead.
const SUPERSET_CLEAN: f32 = 2.0;
/// Shapes by where their ink is (heights as fractions of the digits' height): a digit's ink spans
/// at least `DIGIT_MIN_HEIGHT`; at most `STRAY_INK` of a dot's ink lies above the bottom
/// `DOT_ZONE`, of a degree ring's below the top `DEGREE_ZONE` (a piece cut from a run may carry a
/// little of its neighbour). A piece with more than `SPILL_INK` of its ink over `BAND_SPILL` rows
/// above or below the digits (a line, a grass blade across the text) is no digit or sign.
const DIGIT_MIN_HEIGHT: f32 = 0.75;
const DOT_ZONE: f32 = 0.4;
const DEGREE_ZONE: f32 = 0.6;
const STRAY_INK: f32 = 0.25;
const BAND_SPILL: usize = 2;
const SPILL_INK: f32 = 0.15;
/// Rows off the digits with ink on them that make a piece a line rather than a digit or sign.
const STRAY_ROWS: usize = 3;
/// Empty rows that part a piece's ink into separate things. The readout is drawn over the scene,
/// so a glyph is never broken up: of a piece's row segments only the one with the
/// most ink on the digits is the text, the others are background (a flower under a "-").
const STRAY_GAP: usize = 2;

/// Horizontal sub-pixel offsets a value line is resampled at when it does not decode level: the
/// digits' sub-pixel position is not known (the string's width moves them), and cutting a line
/// into runs is sensitive to it.
const PHASES: [f32; 2] = [-1.0 / 3.0, 1.0 / 3.0];
/// A readout that does not decode is reported as there (`Unreadable`) only with labels this sure.
const UNREADABLE_LABEL: f32 = 0.7;
const UNREADABLE_AZ: f32 = 0.55;
/// The glyph chain's centre relative to the label centre, and between the two lines.
const POS_X_RANGE: (f32, f32) = (-20.0, 10.0);
const PAIR_TOL_X: f32 = 8.0;
/// The readout's drop shadow: the same glyphs 2 reference px right and 2 down,
/// 35 % black, unblurred.
const SHADOW_OFFSET: f32 = 2.0;
const SHADOW_ALPHA: f32 = 0.35;
/// Half-width of the box the local ground brightness is averaged over (reference px): wider than a
/// stroke, narrower than the ground's own changes behind a line of text.
const GROUND_RADIUS: f32 = 8.0;

/// Development: `MTC_AIM_DEBUG` set prints why lines do not decode (read once).
fn debug_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MTC_AIM_DEBUG").is_some())
}

const SYMBOLS: [char; 14] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '+', '-', '.', '°',
];
const INF: f32 = 1e30;

/// How large the readout's text is drawn at the current configuration, relative to the reference
/// (150 %, 2560×1334), once the frame is mapped to the reference geometry: (x, y) scale of the
/// labels and of the value lines' digits. Measured per configuration, never searched at run time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextScale {
    pub label: (f32, f32),
    pub digits: (f32, f32),
}

impl TextScale {
    pub const REFERENCE: Self = Self {
        label: (1.0, 1.0),
        digits: (1.0, 1.0),
    };

    /// Labels and digits both `factor` times as large.
    pub fn times(self, factor: f32) -> Self {
        Self { label: (self.label.0 * factor, self.label.1 * factor), digits: (self.digits.0 * factor, self.digits.1 * factor) }
    }
}

/// "label x, label y, digits x, digits y" (the development tools' `MTC_AIM_SCALE`).
impl std::str::FromStr for TextScale {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let f: Vec<f32> = s.split(',').map(|x| x.trim().parse::<f32>().map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
        match f[..] {
            [lx, ly, dx, dy] => Ok(Self { label: (lx, ly), digits: (dx, dy) }),
            _ => Err(format!("four numbers, got {}", f.len())),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AimReading {
    /// Displayed text without the degree sign ("+12.34", "056.78").
    pub elevation_text: String,
    pub azimuth_text: String,
    pub elevation: f64,
    pub azimuth: f64,
    /// Frame x of the label centre (the readout's horizontal anchor).
    pub anchor_x: i32,
    /// Frame x of the "Elevation" label's left edge (locates the ammo bar).
    pub label_x: i32,
    /// Decoding cost of both lines, rounded; 0 is a perfect match.
    pub cost: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AimResult {
    /// No angle readout on screen (not in a gunner seat, a menu covers it, ...).
    Absent,
    /// The labels are there but a line did not decode; the frame is skipped.
    Unreadable {
        anchor_x: i32,
        label_x: i32,
    },
    Read(AimReading),
}

#[derive(Deserialize)]
struct ShapeFile {
    label: Vec<Vec<f32>>,
    az_label: Vec<Vec<f32>>,
    glyphs: Vec<GlyphRows>,
}

#[derive(Deserialize)]
struct GlyphRows {
    char: char,
    rows: Vec<Vec<f32>>,
}

/// A symbol's glyph over the digit band rows, drawn at its own width and one pixel narrower and
/// wider (the same glyph comes out a pixel narrower or wider at different sub-pixel positions):
/// blurred, zero mean, unit norm.
struct GlyphPatch {
    widths: Vec<Plane>,
}

/// A place the "Elevation" label may be.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Candidate {
    score: f32,
    /// Frame x of the label's left edge and frame y of its top.
    x: i32,
    y: i32,
}

pub struct AimReader {
    scale: TextScale,
    /// The labels as harvested (lowest channel), for rebuilding at another scale.
    label_src: Plane,
    az_src: Plane,
    /// The "Elevation" label at the current scale, blurred and high-passed, zero mean (with its sum
    /// of squares), and the same at half resolution; the "Azimuth" label at the current scale,
    /// blurred.
    label: (Plane, f32),
    label_half: (Plane, f32),
    az: Plane,
    glyphs: Vec<GlyphPatch>,
    /// Each symbol's template ink (sum of its mean coverage).
    ink: [f32; 14],
    /// Template digit height and the canvas row of the digits' bottom.
    digit_h: usize,
    base: usize,
    /// The candidate that read last, tried first on the next frame, and whether it read from the
    /// shadow-corrected plane (`Area::shadow_corrected`).
    last: Option<Candidate>,
    last_corrected: bool,
    /// Rows the value lines sit below where the reference geometry puts them (`fit`): the rules
    /// place them a pixel or two off on configurations they were not measured on, and raising
    /// Roblox's text size moves them.
    line_dy: i32,
}

/// A text scale and value-line offset that read the readout on one frame (`AimReader::fit`):
/// the multiple of the configuration's rule scale, the offset (rows) and the decode's cost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadoutFit {
    pub factor: f32,
    pub dy: i32,
    pub cost: u32,
}

impl Default for AimReader {
    fn default() -> Self {
        Self::new()
    }
}

/// Lowest channel over a frame rectangle, 0 outside the view; and the same faded by saturation
/// for the value lines (`LINE_SAT_FADE`).
struct Area {
    x0: i32,
    y0: i32,
    w: usize,
    h: usize,
    data: Vec<f32>,
    lines: Vec<f32>,
}

impl Area {
    fn from_view(view: &FrameView, x0: i32, y0: i32, w: usize, h: usize) -> Self {
        let mut data = vec![0.0f32; w * h];
        let mut lines = vec![0.0f32; if LINE_SAT_FADE.is_some() { w * h } else { 0 }];
        let (vx0, vy0) = view.origin;
        let (vx1, vy1) = (vx0 + view.width as i32, vy0 + view.height as i32);
        for y in y0.max(vy0)..(y0 + h as i32).min(vy1) {
            for x in x0.max(vx0)..(x0 + w as i32).min(vx1) {
                let p = view.px(x, y);
                let (mn, mx) = (
                    f32::from(p[0].min(p[1]).min(p[2])),
                    f32::from(p[0].max(p[1]).max(p[2])),
                );
                let i = (y - y0) as usize * w + (x - x0) as usize;
                data[i] = mn;
                if let Some((lo, hi)) = LINE_SAT_FADE {
                    let sat = (mx - mn) / mx.max(1.0);
                    lines[i] = mn * ((hi - sat) / (hi - lo)).clamp(0.0, 1.0);
                }
            }
        }
        Self {
            x0,
            y0,
            w,
            h,
            data,
            lines,
        }
    }

    #[inline]
    fn at(&self, x: i32, y: i32) -> f32 {
        Self::get(&self.data, self.x0, self.y0, self.w, self.h, x, y)
    }

    #[inline]
    fn get(data: &[f32], x0: i32, y0: i32, w: usize, h: usize, x: i32, y: i32) -> f32 {
        let (bx, by) = (x - x0, y - y0);
        if bx < 0 || by < 0 || bx >= w as i32 || by >= h as i32 {
            return 0.0;
        }
        data[by as usize * w + bx as usize]
    }

    /// Bilinear sample of the value-line plane at frame coordinates (pixel centres on integers),
    /// 0 outside.
    #[inline]
    fn bilinear(&self, x: f32, y: f32) -> f32 {
        let data = if LINE_SAT_FADE.is_some() {
            &self.lines
        } else {
            &self.data
        };
        let at = |x: i32, y: i32| Self::get(data, self.x0, self.y0, self.w, self.h, x, y);
        let (fx, fy) = (x.floor(), y.floor());
        let (ax, ay) = (x - fx, y - fy);
        let (ix, iy) = (fx as i32, fy as i32);
        let top = at(ix, iy) * (1.0 - ax) + at(ix + 1, iy) * ax;
        let bottom = at(ix, iy + 1) * (1.0 - ax) + at(ix + 1, iy + 1) * ax;
        top * (1.0 - ay) + bottom * ay
    }

    fn crop(&self, x: i32, y: i32, w: usize, h: usize) -> Plane {
        let mut p = Plane::new(w, h);
        for r in 0..h {
            for c in 0..w {
                p.data[r * w + c] = self.at(x + c as i32, y + r as i32);
            }
        }
        p
    }

    /// The frame columns `x0..x1` of this area (within it).
    fn columns(&self, x0: i32, x1: i32) -> Self {
        let (a, b) = ((x0 - self.x0).max(0) as usize, ((x1 - self.x0).max(0) as usize).min(self.w));
        let w = b.saturating_sub(a).max(1).min(self.w);
        let cut = |plane: &[f32]| -> Vec<f32> {
            if plane.is_empty() {
                return Vec::new();
            }
            (0..self.h).flat_map(|y| plane[y * self.w + a..y * self.w + a + w].iter().copied()).collect()
        };
        Self {
            x0: self.x0 + a as i32,
            y0: self.y0,
            w,
            h: self.h,
            data: cut(&self.data),
            lines: cut(&self.lines),
        }
    }

    /// The glyphs' coverage estimated from both things they leave on the ground, scaled to 0–255
    /// like the value plane: their own brightness over the local ground (box mean of half-width
    /// `radius`), and the darkening of their drop shadow `offset` (frame px) away. Each estimate is
    /// weighted by the contrast it can carry, the glyph's 255 − B against the shadow's 0.35·B, so
    /// on dark ground this is the glyphs' own estimate and on ground about as bright as they are
    /// (snow), where the shadow is the only contrast left, it is the shadow moved back under them.
    fn shadow_corrected(&self, offset: (f32, f32), radius: usize) -> Self {
        let (w, h) = (self.w, self.h);
        let mut sums = vec![0.0f64; (w + 1) * (h + 1)];
        for y in 0..h {
            let mut row = 0.0f64;
            for x in 0..w {
                row += f64::from(self.data[y * w + x]);
                sums[(y + 1) * (w + 1) + x + 1] = sums[y * (w + 1) + x + 1] + row;
            }
        }
        let mut ground = vec![0.0f32; w * h];
        for y in 0..h {
            let (y0, y1) = (y.saturating_sub(radius), (y + radius + 1).min(h));
            for x in 0..w {
                let (x0, x1) = (x.saturating_sub(radius), (x + radius + 1).min(w));
                let s = sums[y1 * (w + 1) + x1] - sums[y0 * (w + 1) + x1] - sums[y1 * (w + 1) + x0] + sums[y0 * (w + 1) + x0];
                ground[y * w + x] = (s / ((y1 - y0) * (x1 - x0)) as f64) as f32;
            }
        }
        let sample = |plane: &[f32], x: f32, y: f32| -> f32 {
            let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
            let (ix, iy) = (x.floor() as usize, y.floor() as usize);
            let (jx, jy) = ((ix + 1).min(w - 1), (iy + 1).min(h - 1));
            let (ax, ay) = (x - ix as f32, y - iy as f32);
            let top = plane[iy * w + ix] * (1.0 - ax) + plane[iy * w + jx] * ax;
            let bottom = plane[jy * w + ix] * (1.0 - ax) + plane[jy * w + jx] * ax;
            top * (1.0 - ay) + bottom * ay
        };
        // A whole-pixel offset (the reference size) is read straight from the planes.
        let whole = (offset.0.fract() == 0.0 && offset.1.fract() == 0.0).then(|| (offset.0 as usize, offset.1 as usize));
        let mut data = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (v, b) = (self.data[y * w + x], ground[y * w + x]);
                let glyph = ((v - b) / (255.0 - b).max(8.0)).clamp(0.0, 1.0);
                let glyph_weight = ((255.0 - b) / (SHADOW_ALPHA * b).max(1.0)).min(1.0);
                let (vs, bs) = match whole {
                    Some((dx, dy)) => {
                        let i = (y + dy).min(h - 1) * w + (x + dx).min(w - 1);
                        (self.data[i], ground[i])
                    }
                    None => {
                        let (sx, sy) = (x as f32 + offset.0, y as f32 + offset.1);
                        (sample(&self.data, sx, sy), sample(&ground, sx, sy))
                    }
                };
                let shadow = ((1.0 - vs / bs.max(1.0)) / SHADOW_ALPHA).clamp(0.0, 1.0);
                let shadow_weight = (SHADOW_ALPHA * bs / (255.0 - bs).max(1.0)).min(1.0);
                data[y * w + x] = 255.0 * (glyph_weight * glyph).max(shadow_weight * shadow);
            }
        }
        Self {
            x0: self.x0,
            y0: self.y0,
            w,
            h,
            lines: if LINE_SAT_FADE.is_some() { data.clone() } else { Vec::new() },
            data,
        }
    }
}

/// OpenCV's reflect-101 border index.
#[inline]
fn reflect101(mut i: isize, n: isize) -> usize {
    if n == 1 {
        return 0;
    }
    while i < 0 || i >= n {
        i = if i < 0 { -i } else { 2 * (n - 1) - i };
    }
    i as usize
}

/// Gaussian blur as OpenCV's `GaussianBlur(src, (0, 0), sigma)` on float images: kernel size
/// `round(sigma·8 + 1) | 1`, reflect-101 borders.
fn blur(p: &Plane, sigma: f32) -> Plane {
    let ksize = ((sigma * 8.0 + 1.0).round() as isize) | 1;
    let r = ksize / 2;
    let mut k: Vec<f32> = (-r..=r)
        .map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= total);
    let (w, h) = (p.width as isize, p.height as isize);
    let mut tmp = Plane::new(p.width, p.height);
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() {
                s += kv * p.data[y as usize * p.width + reflect101(x + j as isize - r, w)];
            }
            tmp.data[y as usize * p.width + x as usize] = s;
        }
    }
    let mut out = Plane::new(p.width, p.height);
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() {
                s += kv * tmp.data[reflect101(y + j as isize - r, h) * p.width + x as usize];
            }
            out.data[y as usize * p.width + x as usize] = s;
        }
    }
    out
}

fn plane_of(rows: &[Vec<f32>]) -> Plane {
    let (h, w) = (rows.len(), rows[0].len());
    Plane {
        width: w,
        height: h,
        data: rows.iter().flatten().copied().collect(),
    }
}

fn sub_plane(p: &Plane, x: usize, y: usize, w: usize, h: usize) -> Plane {
    let mut out = Plane::new(w, h);
    for r in 0..h {
        out.data[r * w..(r + 1) * w]
            .copy_from_slice(&p.data[(y + r) * p.width + x..(y + r) * p.width + x + w]);
    }
    out
}

/// Zero mean and its sum of squares.
fn centred(mut p: Plane) -> (Plane, f32) {
    let mean = p.data.iter().sum::<f32>() / p.data.len() as f32;
    p.data.iter_mut().for_each(|v| *v -= mean);
    let ss = p.data.iter().map(|v| v * v).sum();
    (p, ss)
}

/// Normalised cross-correlation (OpenCV TM_CCOEFF_NORMED) of the zero-mean template `t` (sum of
/// squares `tt`) at every position of `img`; also each window's standard deviation.
fn match_template(img: &Plane, t: &Plane, tt: f32) -> (Plane, Plane) {
    let (rw, rh) = (img.width + 1 - t.width, img.height + 1 - t.height);
    let mut res = Plane::new(rw, rh);
    let mut sd = Plane::new(rw, rh);
    let n = (t.width * t.height) as f64;
    // integral images of the image and its square
    let iw = img.width + 1;
    let mut s1 = vec![0.0f64; iw * (img.height + 1)];
    let mut s2 = vec![0.0f64; iw * (img.height + 1)];
    for y in 0..img.height {
        let (mut r1, mut r2) = (0.0f64, 0.0f64);
        for x in 0..img.width {
            let v = f64::from(img.data[y * img.width + x]);
            r1 += v;
            r2 += v * v;
            s1[(y + 1) * iw + x + 1] = s1[y * iw + x + 1] + r1;
            s2[(y + 1) * iw + x + 1] = s2[y * iw + x + 1] + r2;
        }
    }
    let rect = |s: &[f64], x: usize, y: usize| {
        s[(y + t.height) * iw + x + t.width] - s[y * iw + x + t.width] - s[(y + t.height) * iw + x]
            + s[y * iw + x]
    };
    for y in 0..rh {
        for x in 0..rw {
            let mut num = 0.0f32;
            for ty in 0..t.height {
                let row = &img.data[(y + ty) * img.width + x..][..t.width];
                let trow = &t.data[ty * t.width..][..t.width];
                num += row.iter().zip(trow).map(|(a, b)| a * b).sum::<f32>();
            }
            let (a, b) = (rect(&s1, x, y), rect(&s2, x, y));
            let var = (b - a * a / n).max(0.0);
            sd.data[y * rw + x] = (var / n).sqrt() as f32;
            let den = (f64::from(tt) * var).sqrt();
            res.data[y * rw + x] = if den > 1e-9 {
                (f64::from(num) / den) as f32
            } else {
                0.0
            };
        }
    }
    (res, sd)
}

/// Normalised cross-correlation of the zero-mean template `t` at one position of `img`, and the
/// window's standard deviation.
fn ncc_at(img: &Plane, x: usize, y: usize, t: &Plane, tt: f32) -> (f32, f32) {
    let n = (t.width * t.height) as f32;
    let (mut sum, mut sq, mut dot) = (0.0f32, 0.0f32, 0.0f32);
    for ty in 0..t.height {
        let row = &img.data[(y + ty) * img.width + x..][..t.width];
        let trow = &t.data[ty * t.width..][..t.width];
        for (a, b) in row.iter().zip(trow) {
            sum += a;
            sq += a * a;
            dot += a * b;
        }
    }
    let var = (sq - sum * sum / n).max(0.0);
    let den = (tt * var).sqrt();
    (if den > 1e-9 { dot / den } else { 0.0 }, (var / n).sqrt())
}

/// np.median: the middle value, or the mean of the two middle values.
fn median(mut v: Vec<f32>) -> f32 {
    let n = v.len();
    let (_, hi, _) = v.select_nth_unstable_by(n / 2, f32::total_cmp);
    let hi = *hi;
    if n % 2 == 1 {
        return hi;
    }
    let lo = v[..n / 2].iter().copied().fold(f32::MIN, f32::max);
    (lo + hi) / 2.0
}

/// np.percentile with linear interpolation.
fn percentile(mut v: Vec<f32>, q: f32) -> f32 {
    let pos = q / 100.0 * (v.len() - 1) as f32;
    let (i, a) = (pos.floor() as usize, pos - pos.floor());
    let (_, lo, rest) = v.select_nth_unstable_by(i, f32::total_cmp);
    let lo = *lo;
    match rest.iter().copied().reduce(f32::min) {
        Some(hi) if a > 0.0 => lo * (1.0 - a) + hi * a,
        _ => lo,
    }
}

/// The digit band of a line window with one spare row above and below: raw and blurred, with
/// column prefix sums for each of the three row offsets (the template one row up, level, down).
struct Band {
    width: usize,
    /// Rows compared with a template (the band without its spare rows).
    rows: usize,
    bb: Plane,
    raw_cols: Vec<f32>,
    b_cols: [Vec<f32>; 3],
    bb_cols: [Vec<f32>; 3],
}

impl Band {
    fn new(raw: &Plane) -> Self {
        let bb = blur(raw, GLYPH_BLUR);
        let rows = raw.height - 2;
        let prefix = |p: &Plane, from: usize, f: &dyn Fn(f32) -> f32| -> Vec<f32> {
            let mut out = vec![0.0f32; p.width + 1];
            for x in 0..p.width {
                out[x + 1] = out[x]
                    + (from..from + rows)
                        .map(|y| f(p.data[y * p.width + x]))
                        .sum::<f32>();
            }
            out
        };
        Self {
            width: raw.width,
            rows,
            raw_cols: prefix(raw, 1, &|v| v),
            b_cols: [0, 1, 2].map(|d| prefix(&bb, d, &|v| v)),
            bb_cols: [0, 1, 2].map(|d| prefix(&bb, d, &|v| v * v)),
            bb,
        }
    }

    /// Raw ink over columns a..b (level rows).
    fn raw(&self, a: usize, b: usize) -> f32 {
        self.raw_cols[b] - self.raw_cols[a]
    }

    /// Sum and sum of squares of the blurred band over columns a..b at row offset `d`.
    fn blurred(&self, d: usize, a: usize, b: usize) -> (f32, f32) {
        (
            self.b_cols[d][b] - self.b_cols[d][a],
            self.bb_cols[d][b] - self.bb_cols[d][a],
        )
    }
}

/// A run of ink columns in a line window.
#[derive(Clone, Copy, Debug)]
struct Run {
    x: usize,
    w: usize,
    top: usize,
    bottom: usize,
}

/// Column runs of `cov > SEG` cut between glyphs (`split_run`, at most 31 wide), the inkiest chain
/// of runs at most `CHAIN_GAP_ANY` apart (`CHAIN_GAP`, or `DOT_BRIDGE` over a faint dot on
/// `dot_rows`, between runs within `text_rows`), and the degree ring's broken columns joined.
fn runs_of(cov: &Plane, dot_rows: (usize, usize), text_rows: (usize, usize)) -> Vec<Run> {
    let (w, h) = (cov.width, cov.height);
    let ink = |x: usize, y: usize| cov.data[y * w + x] > SEG;
    let col = |x: usize| (0..h).any(|y| ink(x, y));
    let mut runs = Vec::new();
    let mut x = 0;
    while x < w {
        if !col(x) {
            x += 1;
            continue;
        }
        let s = x;
        while x < w && col(x) {
            x += 1;
        }
        for (a, b) in split_run(cov, s, x) {
            for a in (a..b).step_by(31) {
                let b = (a + 31).min(b);
                let rows: Vec<usize> = (0..h).filter(|&y| (a..b).any(|c| ink(c, y))).collect();
                runs.push(Run {
                    x: a,
                    w: b - a,
                    top: rows[0],
                    bottom: *rows.last().expect("ink"),
                });
            }
        }
    }
    // A dot too faint to be ink (12-13 px digits at 100 %) leaves a wider gap: bridged when the
    // gap has faint ink on the dot's rows ("+51.23" lost its "+51" at 1280x720).
    let faint_dot = |a: usize, b: usize| (a..b).any(|c| (dot_rows.0..=dot_rows.1.min(h - 1)).any(|y| cov.data[y * w + c] > DOT_FAINT));
    let text_like = |r: &Run| r.top >= text_rows.0 && r.bottom <= text_rows.1;
    let mut groups: Vec<Vec<Run>> = Vec::new();
    for r in runs {
        match groups.last_mut() {
            Some(g) if {
                let last = g.last().expect("group");
                let (end, gap) = (last.x + last.w, r.x - (last.x + last.w));
                gap <= CHAIN_GAP_ANY || (text_like(last) && text_like(&r) && (gap <= CHAIN_GAP || (gap <= DOT_BRIDGE && faint_dot(end, r.x))))
            } => g.push(r),
            _ => groups.push(vec![r]),
        }
    }
    let count = |g: &Vec<Run>| -> usize {
        g.iter()
            .map(|r| {
                (r.x..r.x + r.w)
                    .map(|c| (0..h).filter(|&y| ink(c, y)).count())
                    .sum::<usize>()
            })
            .sum()
    };
    let mut best: Option<(usize, Vec<Run>)> = None;
    for g in groups {
        let c = count(&g);
        if best.as_ref().is_none_or(|(bc, _)| c > *bc) {
            best = Some((c, g));
        }
    }
    let Some((_, chain)) = best else {
        return Vec::new();
    };
    let mut out: Vec<Run> = Vec::new();
    for r in chain {
        if let Some(p) = out.last_mut()
            && r.bottom <= 9 + MARGIN
            && p.bottom <= 9 + MARGIN
            && r.x - (p.x + p.w) <= 2
            && r.x + r.w - p.x <= GW
        {
            p.w = r.x + r.w - p.x;
            p.top = p.top.min(r.top);
            p.bottom = p.bottom.max(r.bottom);
            continue;
        }
        out.push(r);
    }
    out
}

/// Columns `s..e` (all with ink) cut where the ink's 8-connected pieces part: at small sizes
/// neighbouring glyphs follow column on column without touching ("247" at 100 %, 1920x920: the
/// "4"'s bar starts in the column after the "2"'s base; "+1" at 1920x1000), so column gaps alone
/// leave them in one run. Pieces sharing more than `SPLIT_OVERLAP` columns are one glyph (strokes
/// that broke apart), specks under `SPLIT_MIN_INK` px go to their nearest piece; pieces that
/// share a column or two are cut in the middle of what they share.
fn split_run(cov: &Plane, s: usize, e: usize) -> Vec<(usize, usize)> {
    let (w, h) = (cov.width, cov.height);
    let ink = |x: usize, y: usize| cov.data[y * w + x] > SEG;
    let mut seen = vec![false; (e - s) * h];
    // (first column, last column, px)
    let mut pieces: Vec<(usize, usize, usize)> = Vec::new();
    for x0 in s..e {
        for y0 in 0..h {
            if !ink(x0, y0) || seen[(x0 - s) * h + y0] {
                continue;
            }
            seen[(x0 - s) * h + y0] = true;
            let mut stack = vec![(x0, y0)];
            let mut p = (x0, x0, 0usize);
            while let Some((x, y)) = stack.pop() {
                p = (p.0.min(x), p.1.max(x), p.2 + 1);
                for nx in x.saturating_sub(1).max(s)..(x + 2).min(e) {
                    for ny in y.saturating_sub(1)..(y + 2).min(h) {
                        if ink(nx, ny) && !seen[(nx - s) * h + ny] {
                            seen[(nx - s) * h + ny] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            pieces.push(p);
        }
    }
    let shared = |p: (usize, usize, usize), q: (usize, usize, usize)| (p.1.min(q.1) + 1).saturating_sub(p.0.max(q.0));
    let gap = |p: (usize, usize, usize), q: (usize, usize, usize)| p.0.max(q.0).saturating_sub(p.1.min(q.1));
    let join = |pieces: &mut Vec<(usize, usize, usize)>, i: usize, j: usize| {
        let q = pieces.remove(j.max(i));
        let p = &mut pieces[i.min(j)];
        *p = (p.0.min(q.0), p.1.max(q.1), p.2 + q.2);
    };
    while let Some((i, j)) = (0..pieces.len()).flat_map(|i| (i + 1..pieces.len()).map(move |j| (i, j))).find(|&(i, j)| shared(pieces[i], pieces[j]) > SPLIT_OVERLAP) {
        join(&mut pieces, i, j);
    }
    while pieces.len() > 1
        && let Some(i) = pieces.iter().position(|p| p.2 < SPLIT_MIN_INK)
    {
        let j = (0..pieces.len()).filter(|&j| j != i).min_by_key(|&j| gap(pieces[i], pieces[j])).expect("another piece");
        join(&mut pieces, i, j);
    }
    pieces.sort_by_key(|p| p.0);
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (k, p) in pieces.iter().enumerate() {
        let end = match pieces.get(k + 1) {
            Some(q) if q.0 > p.1 => q.0,
            Some(q) => (q.0 + p.1 + 1).div_ceil(2),
            None => e,
        };
        let start = out.last().map_or(s, |l| l.1);
        if end <= start {
            continue;
        }
        out.push((start, end));
    }
    if let Some(l) = out.last_mut() {
        l.1 = e;
    }
    out
}

/// Zeroes a piece's row ink outside its text segment: segments are parted by at least `STRAY_GAP`
/// empty rows, the text one has the most ink on rows `lo..=hi`.
fn keep_text_segment(row_ink: &mut [f32], lo: usize, hi: usize) {
    let mut segments: Vec<(usize, usize)> = Vec::new();
    let mut last: Option<usize> = None;
    for (y, v) in row_ink.iter().enumerate() {
        if *v <= 0.0 {
            continue;
        }
        match (last, segments.last_mut()) {
            (Some(l), Some(s)) if y - l <= STRAY_GAP => s.1 = y,
            _ => segments.push((y, y)),
        }
        last = Some(y);
    }
    if segments.len() < 2 {
        return;
    }
    let on_digits = |&(a, b): &(usize, usize)| -> f32 { (a.max(lo)..=b.min(hi)).map(|y| row_ink[y]).sum() };
    let keep = segments
        .iter()
        .copied()
        .max_by(|p, q| on_digits(p).total_cmp(&on_digits(q)))
        .expect("segments");
    for (y, v) in row_ink.iter_mut().enumerate() {
        if y < keep.0 || y > keep.1 {
            *v = 0.0;
        }
    }
}

/// What a run can be read as: one glyph; two (wider than one glyph); a digit, a dot and a digit (the
/// dot touching both).
struct RunCosts {
    one: Option<[f32; 14]>,
    two: Option<Box<[[f32; 14]; 14]>>,
    three: Option<Box<[[f32; 10]; 10]>>,
    /// A run no wider than one glyph (`NARROW_PAIR`): read as two glyphs only when one is a "1".
    /// `two` still holds every pair, so that a pair without a "1" that fits better (a "0°" joined
    /// by a grid line) is the runner-up and the reading is dropped.
    narrow: bool,
}

impl RunCosts {
    /// The cost of reading the run as `symbols` (one, two or digit-dot-digit).
    fn of(&self, symbols: &[usize]) -> f32 {
        match symbols {
            [a] => self.one.map_or(INF, |c| c[*a]),
            [a, b] => self.two.as_ref().map_or(INF, |c| c[*a][*b]),
            [a, 12, b] => self.three.as_ref().map_or(INF, |c| c[*a][*b]),
            _ => INF,
        }
    }

    /// Every reading the slots allow, as (symbols, cost).
    fn readings(&self, slots: &[char]) -> Vec<(Vec<usize>, f32)> {
        match slots {
            [s] => allowed(*s).iter().map(|&a| (vec![a], self.of(&[a]))).collect(),
            [s, t] => allowed(*s).iter().flat_map(|&a| allowed(*t).iter().map(move |&b| (vec![a, b], self.of(&[a, b])))).collect(),
            _ => (0..10).flat_map(|a| (0..10).map(move |b| (vec![a, 12, b], self.of(&[a, 12, b])))).collect(),
        }
    }
}

fn allowed(slot: char) -> &'static [usize] {
    match slot {
        's' => &[10, 11],
        'd' => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        '.' => &[12],
        _ => &[13],
    }
}

fn missing(slot: char) -> f32 {
    match slot {
        '.' => MISSING_DOT,
        '°' => MISSING_DEGREE,
        _ => MISSING_OTHER,
    }
}

#[derive(Clone, Copy)]
enum Step {
    Start,
    Missing,
    Noise,
    One(usize),
    Two(usize, usize),
    Three(usize, usize),
}

/// One assignment of a run in the cheapest decode: run index, its slots, its symbols.
struct Pick {
    run: usize,
    slots: Vec<char>,
    symbols: Vec<usize>,
}

/// The cheapest fit of runs to `format` (each run one slot, two slots, a digit-dot-digit, or
/// background); the text, its cost and the picks. A missing digit or sign is not a reading.
/// `digit_like`: runs shaped as a whole digit (in the digit band, full height). Such a run is not
/// noise once the text has begun and before it ends: dropping a thin "1" whose ink is little read
/// "+11.32" as "+1.32" at 100 % (1920x720 with the digits drawn at 12 px).
fn fit(costs: &[RunCosts], noise: &[f32], digit_like: &[bool], format: &[char]) -> Option<(String, f32, Vec<Pick>)> {
    let (n, m) = (costs.len(), format.len());
    let mut d = vec![vec![(INF, Step::Start); m + 1]; n + 1];
    d[0][0].0 = 0.0;
    for i in 0..=n {
        for j in 0..=m {
            let here = d[i][j].0;
            if here >= INF {
                continue;
            }
            if j < m {
                let c = here + missing(format[j]);
                if c < d[i][j + 1].0 {
                    d[i][j + 1] = (c, Step::Missing);
                }
            }
            if i == n {
                continue;
            }
            let c = here + noise[i];
            if !(digit_like[i] && j > 0 && j < m) && c < d[i + 1][j].0 {
                d[i + 1][j] = (c, Step::Noise);
            }
            let rc = &costs[i];
            if let Some(cc) = &rc.one
                && j < m
            {
                for &s in allowed(format[j]) {
                    let c = here + cc[s];
                    if c < d[i + 1][j + 1].0 {
                        d[i + 1][j + 1] = (c, Step::One(s));
                    }
                }
            }
            if let Some(cc) = &rc.two
                && j + 1 < m
            {
                for &a in allowed(format[j]) {
                    for &b in allowed(format[j + 1]) {
                        if rc.narrow && a != 1 && b != 1 {
                            continue;
                        }
                        let c = here + cc[a][b];
                        if c < d[i + 1][j + 2].0 {
                            d[i + 1][j + 2] = (c, Step::Two(a, b));
                        }
                    }
                }
            }
            if let Some(cc) = &rc.three
                && j + 2 < m
                && format[j..j + 3] == ['d', '.', 'd']
            {
                for (a, row) in cc.iter().enumerate() {
                    for (b, v) in row.iter().enumerate() {
                        let c = here + v;
                        if c < d[i + 1][j + 3].0 {
                            d[i + 1][j + 3] = (c, Step::Three(a, b));
                        }
                    }
                }
            }
        }
    }
    let total = d[n][m].0;
    if total >= INF {
        return None;
    }
    let (mut text, mut picks) = (Vec::new(), Vec::new());
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        match d[i][j].1 {
            Step::Noise => i -= 1,
            Step::Missing => {
                j -= 1;
                if !matches!(format[j], '.' | '°') {
                    return None;
                }
                text.push(format[j]);
            }
            Step::One(s) => {
                text.push(SYMBOLS[s]);
                i -= 1;
                j -= 1;
                picks.push(Pick { run: i, slots: vec![format[j]], symbols: vec![s] });
            }
            Step::Two(a, b) => {
                text.push(SYMBOLS[b]);
                text.push(SYMBOLS[a]);
                i -= 1;
                j -= 2;
                picks.push(Pick { run: i, slots: vec![format[j], format[j + 1]], symbols: vec![a, b] });
            }
            Step::Three(a, b) => {
                text.push(SYMBOLS[b]);
                text.push('.');
                text.push(SYMBOLS[a]);
                i -= 1;
                j -= 3;
                picks.push(Pick { run: i, slots: vec!['d', '.', 'd'], symbols: vec![a, 12, b] });
            }
            Step::Start => return None,
        }
    }
    text.reverse();
    Some((text.into_iter().collect(), total, picks))
}

impl AimReader {
    pub fn new() -> Self {
        Self::with_scale(TextScale::REFERENCE)
    }

    pub fn with_scale(scale: TextScale) -> Self {
        let file: ShapeFile = serde_json::from_str(include_str!("../assets/aim_shape.json"))
            .expect("aim shape asset");
        // Glyph templates: mean coverage canvases (rows = ROWS + 2·MARGIN, GW wide).
        let mut canvases: Vec<Plane> = vec![Plane::new(GW, ROWS + 2 * MARGIN); SYMBOLS.len()];
        for g in &file.glyphs {
            let i = SYMBOLS
                .iter()
                .position(|&s| s == g.char)
                .expect("known symbol");
            canvases[i] = plane_of(&g.rows);
        }
        let zero = &canvases[0];
        let ink_rows: Vec<usize> = (0..zero.height)
            .filter(|&y| (0..zero.width).any(|x| zero.at(x, y) > 0.5))
            .collect();
        let (digit_h, base) = (
            ink_rows.last().expect("digit ink") - ink_rows[0] + 1,
            *ink_rows.last().expect("digit ink"),
        );
        let (lo, hi) = (base + 1 - digit_h - 4, base + 2);
        let glyphs = canvases
            .iter()
            .map(|c| {
                let max = c.data.iter().copied().fold(0.0f32, f32::max).max(1e-3);
                let norm = Plane {
                    data: c.data.iter().map(|v| (v / max).clamp(0.0, 1.0)).collect(),
                    ..c.clone()
                };
                let cols: Vec<usize> = (0..GW)
                    .filter(|&x| (0..norm.height).any(|y| norm.at(x, y) > 0.3))
                    .collect();
                let (c0, c1) = (cols[0], *cols.last().expect("glyph columns"));
                let tw = c1 - c0 + 1;
                let patch = sub_plane(&norm, c0, lo, tw, hi - lo);
                let widths = [tw.saturating_sub(1).max(2), tw, tw + 1]
                    .into_iter()
                    .fold(Vec::new(), |mut v: Vec<usize>, w| {
                        if !v.contains(&w) {
                            v.push(w);
                        }
                        v
                    })
                    .into_iter()
                    .map(|w| {
                        let (mut t, ss) =
                            centred(blur(&resize(&patch, w, patch.height), GLYPH_BLUR));
                        let n = ss.sqrt().max(1e-6);
                        t.data.iter_mut().for_each(|v| *v /= n);
                        t
                    })
                    .collect();
                GlyphPatch { widths }
            })
            .collect();
        let mut ink = [0.0f32; 14];
        for (i, c) in canvases.iter().enumerate() {
            ink[i] = c.data.iter().sum();
        }
        let empty = (Plane::new(1, 1), 0.0);
        let mut reader = Self {
            scale,
            label_src: plane_of(&file.label),
            az_src: plane_of(&file.az_label),
            label: empty.clone(),
            label_half: empty,
            az: Plane::new(1, 1),
            glyphs,
            ink,
            digit_h,
            base,
            last: None,
            last_corrected: false,
            line_dy: 0,
        };
        reader.set_scale(scale);
        reader
    }

    /// The configuration's text scale; the label templates are redrawn for it.
    pub fn set_scale(&mut self, scale: TextScale) {
        self.scale = scale;
        let (sx, sy) = scale.label;
        let b = blur(&self.label_src, LABEL_BLUR);
        let ramp = blur(&b, LABEL_HIGHPASS);
        let prepared = Plane {
            data: b.data.iter().zip(&ramp.data).map(|(a, r)| a - r).collect(),
            ..b
        };
        let t = resize(
            &prepared,
            (prepared.width as f32 * sx).round() as usize,
            (prepared.height as f32 * sy).round() as usize,
        );
        self.label_half = centred(resize(&t, t.width.div_ceil(2), t.height.div_ceil(2)));
        self.label = centred(t);
        let az = blur(&self.az_src, LABEL_BLUR);
        self.az = resize(
            &az,
            ((az.width as f32 * sx).round() as usize).max(8),
            ((az.height as f32 * sy).round() as usize).max(6),
        );
        self.last = None;
    }

    pub fn scale(&self) -> TextScale {
        self.scale
    }

    /// The value lines' offset below the reference rows (`fit`).
    pub fn set_line_offset(&mut self, dy: i32) {
        self.line_dy = dy;
        self.last = None;
    }

    pub fn line_offset(&self) -> i32 {
        self.line_dy
    }

    /// The best of `factors` (multiples of `base`) and `dys` (value-line offsets) that decodes both
    /// lines on this frame, by the decode's cost; `None` when none decodes. The label is searched at
    /// each factor, the lines at each offset under it. Costs about 20 ms per factor: run off the
    /// frame loop.
    pub fn fit(base: TextScale, view: &FrameView, client_origin: (i32, i32), factors: &[f32], dys: std::ops::RangeInclusive<i32>) -> Option<ReadoutFit> {
        let (ox, oy) = client_origin;
        let mut best: Option<ReadoutFit> = None;
        for &factor in factors {
            let mut reader = Self::with_scale(base.times(factor));
            let area = Area::from_view(view, ox + AREA.0, oy + AREA.1, AREA.2 as usize, AREA.3 as usize);
            let Some(&c) = reader.candidates(&area, ox, oy).first() else { continue };
            for dy in dys.clone() {
                reader.line_dy = dy;
                if let Some(r) = reader.decode_at(&area, None, oy, c)
                    && best.is_none_or(|b| r.cost < b.cost)
                {
                    best = Some(ReadoutFit { factor, dy, cost: r.cost });
                }
            }
        }
        best
    }

    /// The frame rectangle (x, y, width, height) of the whole area the readout is searched in.
    pub fn whole_region(client_origin: (i32, i32)) -> (i32, i32, i32, i32) {
        (client_origin.0 + AREA.0, client_origin.1 + AREA.1, AREA.2, AREA.3)
    }

    /// The "Elevation" label's best score on this frame at `scale` (a quick look for a readout the
    /// current scale misses; `None` when no place scores).
    pub fn label_score_at(scale: TextScale, view: &FrameView, client_origin: (i32, i32)) -> Option<f32> {
        let (ox, oy) = client_origin;
        let reader = Self::with_scale(scale);
        let area = Area::from_view(view, ox + AREA.0, oy + AREA.1, AREA.2 as usize, AREA.3 as usize);
        reader.candidates(&area, ox, oy).first().map(|c| c.score)
    }

    /// Whether the last reading came from the shadow-corrected plane (ground about as bright as the
    /// glyphs).
    pub fn last_from_shadow(&self) -> bool {
        self.last.is_some() && self.last_corrected
    }

    /// Frame y of the (label, value) line centres for elevation and azimuth.
    pub fn line_centers(client_origin: (i32, i32)) -> [(i32, i32); 2] {
        let oy = client_origin.1;
        let label_mid = 7;
        let value_mid = ROWS as i32 / 2;
        [
            (oy + LABEL_TOP + label_mid, oy + ELEVATION_TOP + value_mid),
            (
                oy + LABEL_TOP + (AZIMUTH_TOP - ELEVATION_TOP) + label_mid,
                oy + AZIMUTH_TOP + value_mid,
            ),
        ]
    }

    /// The frame rectangle (x, y, width, height) to read back: the columns around the last place
    /// once the readout is found (`read_near`), the whole search area otherwise (`read`). The HUD
    /// reads back only this (the whole area every frame was 737 KB against about 150 KB, and the
    /// graphics driver's threads took 0.06–0.10 core instead of 0.04, measured in play).
    pub fn wanted_region(&self, client_origin: (i32, i32)) -> (i32, i32, i32, i32) {
        let (ox, oy) = client_origin;
        let (ax, ay, aw, ah) = (ox + AREA.0, oy + AREA.1, AREA.2, AREA.3);
        match self.last {
            Some(c) => {
                let (x0, x1) = self.near(c, ax, aw);
                (x0, ay, x1 - x0, ah)
            }
            None => (ax, ay, aw, ah),
        }
    }

    /// Frame columns x0..x1 that reading at candidate `c` looks at, within the search area: both
    /// value lines' windows (`HALF` digit widths each side of the label's centre, the bilinear
    /// sample and the third-pixel phases) and the "Azimuth" label's window with its blur, plus
    /// `NEAR_MARGIN`. Two windows each side of the label would read back 322 px.
    fn near(&self, c: Candidate, ax: i32, aw: i32) -> (i32, i32) {
        let cx = self.centre(c);
        let line = HALF as f32 * self.scale.digits.0 + 2.0;
        let az = self.az.width as f32 / 2.0 + 6.0 + (LABEL_BLUR * 4.0).ceil() + 1.0;
        let half = line.max(az) + NEAR_MARGIN as f32;
        let x0 = ((cx - half).floor() as i32).max(ax);
        let x1 = ((cx + half).ceil() as i32).min(ax + aw);
        (x0, x1.max(x0 + 1))
    }

    /// Whether the readout's place is known: `wanted_region` is then the columns around it.
    pub fn knows_place(&self) -> bool {
        self.last.is_some()
    }

    /// The readout at the last place only, from a view holding just `wanted_region` (what the HUD
    /// reads back while the place is known). `None` when there is no last place or it does not
    /// read there: the place is forgotten, `wanted_region` becomes the whole area, and `read` on
    /// that (read back from the same frame) searches it.
    pub fn read_near(&mut self, view: &FrameView, client_origin: (i32, i32)) -> Option<AimResult> {
        let c = self.last?;
        let (ax, ay, aw, ah) = (client_origin.0 + AREA.0, client_origin.1 + AREA.1, AREA.2, AREA.3);
        let (x0, x1) = self.near(c, ax, aw);
        let area = Area::from_view(view, x0, ay, (x1 - x0) as usize, ah as usize);
        let az = self.az_score(&area, self.centre(c), c.y as f32);
        let shadow = self.last_corrected.then(|| self.corrected(&area));
        if az >= AZ_MIN
            && let Some(r) = self.decode_at(&area, shadow.as_ref(), client_origin.1, c)
        {
            return Some(AimResult::Read(r));
        }
        if debug_on() {
            eprintln!("  cached place {c:?} failed (az {az:.2})");
        }
        self.last = None;
        None
    }

    /// The readout from a view holding the whole search area (`wanted_region` with no last place):
    /// the last place first, then every candidate.
    pub fn read(&mut self, view: &FrameView, client_origin: (i32, i32)) -> AimResult {
        let (ox, oy) = client_origin;
        let (ax, ay, aw, ah) = (ox + AREA.0, oy + AREA.1, AREA.2, AREA.3);
        // The last place, already read again by `read_near` (with the shadow when it read from it).
        let tried = self.last.map(|c| (c, self.last_corrected));
        if let Some(r) = self.read_near(view, client_origin) {
            return r;
        }
        let area = Area::from_view(view, ax, ay, aw as usize, ah as usize);
        let mut sure = None;
        for c in self.candidates(&area, ox, oy) {
            let tried_here = tried.filter(|(l, _)| l.x == c.x && l.y == c.y);
            if tried_here.is_some_and(|(_, with_shadow)| with_shadow) {
                continue;
            }
            let cx = self.centre(c);
            let az = self.az_score(&area, cx, c.y as f32);
            if az < AZ_MIN {
                continue;
            }
            let is_sure = c.score >= UNREADABLE_LABEL && az >= UNREADABLE_AZ;
            if sure.is_none() && is_sure {
                sure = Some(c);
            }
            if tried_here.is_none()
                && let Some(r) = self.decode_at(&area, None, oy, c)
            {
                self.last = Some(c);
                self.last_corrected = false;
                return AimResult::Read(r);
            }
            // The label is surely there but the digits do not decode: on ground about as bright as
            // the glyphs (snow) their drop shadow carries them; read again with it, around the
            // label only (rare, so the extra plane costs little).
            if is_sure {
                if debug_on() {
                    eprintln!("  shadow-corrected digits under the label at {}", c.x);
                }
                let shadow = self.corrected_near(&area, cx);
                if let Some(r) = self.decode_at(&area, Some(&shadow), oy, c) {
                    self.last = Some(c);
                    self.last_corrected = true;
                    return AimResult::Read(r);
                }
            }
        }
        self.last = None;
        match sure {
            Some(c) => AimResult::Unreadable {
                anchor_x: self.centre(c).round() as i32,
                label_x: c.x,
            },
            None => AimResult::Absent,
        }
    }

    /// The columns of `area` the value lines under a label centred at frame x `cx` are read from,
    /// with the glyphs rebuilt from their drop shadow (`Area::shadow_corrected`).
    fn corrected_near(&self, area: &Area, cx: f32) -> Area {
        let (sx, _) = self.scale.digits;
        let reach = (HALF as f32 + GROUND_RADIUS + SHADOW_OFFSET + 4.0) * sx;
        let x0 = ((cx - reach).floor() as i32).max(area.x0);
        let x1 = ((cx + reach).ceil() as i32).min(area.x0 + area.w as i32);
        self.corrected(&area.columns(x0, x1))
    }

    /// `area` with the glyphs rebuilt from their drop shadow, at the configuration's scale.
    fn corrected(&self, area: &Area) -> Area {
        let (sx, sy) = self.scale.digits;
        area.shadow_corrected((SHADOW_OFFSET * sx, SHADOW_OFFSET * sy), (GROUND_RADIUS * sx).round() as usize)
    }

    /// Frame x of a candidate label's centre.
    fn centre(&self, c: Candidate) -> f32 {
        c.x as f32 + self.label.0.width as f32 / 2.0
    }

    /// The label strip from frame (x0, y0), `w` wide: lowest channel, blurred, broad ramps removed.
    fn prepared_strip(&self, area: &Area, x0: i32, y0: i32, w: usize, half: bool) -> Plane {
        let raw = area.crop(x0, y0, w, (LABEL_STRIP.1 - LABEL_STRIP.0) as usize);
        let (raw, k) = if half {
            (
                resize(&raw, raw.width.div_ceil(2), raw.height.div_ceil(2)),
                0.5,
            )
        } else {
            (raw, 1.0)
        };
        let b = blur(&raw, LABEL_BLUR * k);
        let ramp = blur(&b, LABEL_HIGHPASS * k);
        Plane {
            data: b.data.iter().zip(&ramp.data).map(|(a, r)| a - r).collect(),
            ..b
        }
    }

    /// Places the label may be, best first, at least `LABEL_APART` apart: the row searched at half
    /// resolution, the best places refined at full resolution.
    fn candidates(&self, area: &Area, ox: i32, oy: i32) -> Vec<Candidate> {
        let (x0, y0) = (ox + LABEL_SEARCH.0, oy + LABEL_TOP + LABEL_STRIP.0);
        let half = self.prepared_strip(
            area,
            x0,
            y0,
            (LABEL_SEARCH.1 - LABEL_SEARCH.0) as usize,
            true,
        );
        let (ht, htt) = &self.label_half;
        if ht.height > half.height || ht.width > half.width {
            return Vec::new();
        }
        let (res, sd) = match_template(&half, ht, *htt);
        // coarse peaks: (score, half x, half y)
        let mut coarse: Vec<(f32, usize, usize, f32)> = Vec::new();
        for sign in [1.0f32, -1.0] {
            for x in 0..res.width {
                let (mut best, mut row) = (-1.0f32, 0usize);
                for y in 0..res.height {
                    let v = if sd.at(x, y) < LABEL_MIN_STD * COARSE_STD {
                        -1.0
                    } else {
                        sign * res.at(x, y)
                    };
                    if v > best {
                        best = v;
                        row = y;
                    }
                }
                if best >= COARSE_MIN {
                    coarse.push((best, x, row, sign));
                }
            }
        }
        coarse.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
        let mut seeds: Vec<(f32, usize, usize, f32)> = Vec::new();
        for c in coarse {
            if seeds
                .iter()
                .all(|s| (c.1 as i32 - s.1 as i32).abs() >= COARSE_APART || s.3 != c.3)
            {
                seeds.push(c);
                if seeds.len() == COARSE_SEEDS {
                    break;
                }
            }
        }
        let (t, tt) = &self.label;
        let mut peaks: Vec<Candidate> = Vec::new();
        for (_, hx, hy, sign) in seeds {
            let lx0 = x0 + 2 * hx as i32 - REFINE_MARGIN;
            let local =
                self.prepared_strip(area, lx0, y0, t.width + 2 * REFINE_MARGIN as usize, false);
            let mut best: Option<Candidate> = None;
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let (x, y) = (REFINE_MARGIN + dx, 2 * hy as i32 + dy);
                    if x < 0
                        || y < 0
                        || x as usize + t.width > local.width
                        || y as usize + t.height > local.height
                    {
                        continue;
                    }
                    let (v, s) = ncc_at(&local, x as usize, y as usize, t, *tt);
                    let v = sign * v;
                    if s < LABEL_MIN_STD || v < LABEL_MIN {
                        continue;
                    }
                    if best.is_none_or(|b| v > b.score) {
                        best = Some(Candidate {
                            score: v,
                            x: lx0 + x,
                            y: y0 + y,
                        });
                    }
                }
            }
            if let Some(b) = best {
                peaks.push(b);
            }
        }
        peaks.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(b.x.cmp(&a.x))
                .then(b.y.cmp(&a.y))
        });
        let mut out: Vec<Candidate> = Vec::new();
        for p in peaks {
            if out.iter().all(|q| (p.x - q.x).abs() >= LABEL_APART) {
                out.push(p);
                if out.len() == LABEL_CANDIDATES {
                    break;
                }
            }
        }
        out
    }

    /// The "Azimuth" label where it belongs under an "Elevation" label whose top is at `ly`.
    fn az_score(&self, area: &Area, cx: f32, ly: f32) -> f32 {
        let t = &self.az;
        // the labels' spacing is layout, already at the reference size; only the glyphs scale
        let top = ly + (AZIMUTH_TOP - ELEVATION_TOP) as f32;
        let (y0, x0) = (
            top.round() as i32 - 5,
            (cx - t.width as f32 / 2.0).round() as i32 - 6,
        );
        let win = blur(&area.crop(x0, y0, t.width + 12, t.height + 10), LABEL_BLUR);
        let (t, tt) = centred(t.clone());
        let (res, _) = match_template(&win, &t, tt);
        res.data.iter().fold(-1.0f32, |m, v| m.max(v.abs()))
    }

    /// Both lines decoded under candidate `c`, aligned under one label. With the shadow-corrected
    /// plane `shadow`, a line that does not decode on `area` is read on it; and where neither reads
    /// it with the margins a decode needs, the same text from both (two looks at different pixels:
    /// the glyphs and their shadow) is taken.
    fn decode_at(&self, area: &Area, shadow: Option<&Area>, oy: i32, c: Candidate) -> Option<AimReading> {
        let cx = self.centre(c);
        let line = |base: f32, signed: bool| -> Option<(String, f32, f32)> {
            self.read_line(area, cx, base, signed, true).or_else(|| {
                let shadow = shadow?;
                self.read_line(shadow, cx, base, signed, true).or_else(|| {
                    let plain = self.read_line(area, cx, base, signed, false)?;
                    let corrected = self.read_line(shadow, cx, base, signed, false)?;
                    if debug_on() {
                        eprintln!("  loose reads: {} and {} from the shadow", plain.0, corrected.0);
                    }
                    (plain.0 == corrected.0).then_some((plain.0, plain.1.max(corrected.1), plain.2))
                })
            })
        };
        let (e, ec, ex) = line((oy + ELEVATION_TOP + self.line_dy) as f32 + DIGIT_BOTTOM, true)?;
        let (a, ac, ax) = line((oy + AZIMUTH_TOP + self.line_dy) as f32 + DIGIT_BOTTOM, false)?;
        if (ex - ax).abs() > PAIR_TOL_X {
            return None;
        }
        let (elevation, azimuth): (f64, f64) = (e.parse().ok()?, a.parse().ok()?);
        if elevation.abs() > 90.0 || azimuth >= 360.0 {
            return None;
        }
        Some(AimReading {
            elevation_text: e,
            azimuth_text: a,
            elevation,
            azimuth,
            anchor_x: cx.round() as i32,
            label_x: c.x,
            cost: (ec + ac).round() as u32,
        })
    }

    /// One value line whose digits' bottom is frame row `base_in`, decoded: text without the degree
    /// sign, cost, and the glyph chain's centre relative to `cx`. Read level first; when that does
    /// not decode, a third of a pixel to either side (the cheaper reading that decodes).
    fn read_line(
        &self,
        area: &Area,
        cx: f32,
        base_in: f32,
        signed: bool,
        strict: bool,
    ) -> Option<(String, f32, f32)> {
        if let Some(r) = self.read_line_at(area, cx, 0.0, base_in, signed, strict) {
            return Some(r);
        }
        PHASES
            .iter()
            .filter_map(|&dx| self.read_line_at(area, cx, dx, base_in, signed, strict))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The line sampled with its columns shifted by `dx` px.
    fn read_line_at(
        &self,
        area: &Area,
        cx: f32,
        dx: f32,
        base_in: f32,
        signed: bool,
        strict: bool,
    ) -> Option<(String, f32, f32)> {
        let (sx, sy) = self.scale.digits;
        let w = 2 * HALF as usize;
        let rows = ROWS + 2 * MARGIN;
        // window (r, c) samples frame (cx + dx + sx·(c − HALF), base_in + sy·(r − base))
        let mut win = Plane::new(w, rows);
        for r in 0..rows {
            for col in 0..w {
                win.data[r * w + col] = area.bilinear(
                    cx + dx + sx * (col as f32 - HALF as f32),
                    base_in + sy * (r as f32 - self.base as f32),
                );
            }
        }
        let sample: Vec<f32> = (0..rows)
            .step_by(2)
            .flat_map(|r| (0..w).step_by(2).map(move |c| (r, c)))
            .map(|(r, c)| win.data[r * w + c])
            .collect();
        let g = median(sample);
        let den = (255.0 - g).max(8.0);
        win.data
            .iter_mut()
            .for_each(|v| *v = ((*v - g) / den).clamp(0.0, 1.0));
        let hi = percentile(win.data.clone(), 99.0);
        if hi >= 0.3 {
            win.data
                .iter_mut()
                .for_each(|v| *v = (*v / hi).clamp(0.0, 1.0));
        }

        let (text, cost, (x0, x1)) = self.decode(&win, signed, strict)?;
        let centre = cx + dx - HALF as f32 * sx + sx * (x0 + x1) as f32 / 2.0;
        if debug_on() {
            eprintln!("  line {text} cost {cost:.1} centre {:.1}", centre - cx);
        }
        (POS_X_RANGE.0..=POS_X_RANGE.1)
            .contains(&(centre - cx))
            .then_some((text, cost, centre - cx))
    }

    /// Each symbol's template slid along the run (and one row up or down): the best window counts,
    /// ink of the run outside it costs a little. Only the symbols `allowed` are slid (the shape
    /// rules exclude the rest: sliding all 14 was most of a read's time).
    fn slide_costs(&self, band: &Band, x: usize, w: usize, allowed: [bool; 14]) -> [f32; 14] {
        let run_ink = band.raw(x, x + w);
        let mut out = [INF; 14];
        for (i, g) in self.glyphs.iter().enumerate() {
            if !allowed[i] {
                continue;
            }
            for t in &g.widths {
                let width = t.width;
                // A run this much wider than the glyph holds more than it: a "+" with the "1" after
                // it merged into one run read "+11.32" as "+1.32" (1920x720 at 100 %).
                if w > width + RUN_SPARE {
                    continue;
                }
                let hi = x as isize + (w as isize - width as isize).max(0) + 2;
                let n = (band.rows * width) as f32;
                for o in (x as isize - 1)..hi {
                    if o < 0 || o as usize + width > band.width {
                        continue;
                    }
                    let o = o as usize;
                    let (a, b) = (o.max(x), (o + width).min(x + w));
                    let outside = run_ink - if b > a { band.raw(a, b) } else { 0.0 };
                    let extra = W_EXTRA * outside / self.digit_h as f32;
                    for d in 0..3 {
                        let (sum, sq) = band.blurred(d, o, o + width);
                        let var = sq - sum * sum / n;
                        if var < 1e-9 {
                            continue;
                        }
                        let mut dot = 0.0f32;
                        for r in 0..band.rows {
                            let row = &band.bb.data[(r + d) * band.width + o..][..width];
                            let trow = &t.data[r * width..][..width];
                            dot += row.iter().zip(trow).map(|(p, q)| p * q).sum::<f32>();
                        }
                        let c = W_SHAPE * (1.0 - dot / var.sqrt()) + extra;
                        if c < out[i] {
                            out[i] = c;
                        }
                    }
                }
            }
        }
        out
    }

    /// Text (without the degree sign), cost and the window columns the glyphs span of a line window,
    /// or `None` when it does not pass every check; not `strict`, without the margin each glyph must
    /// win by over its runner-up. The span leaves noise runs out: a speck in the chain moved a line's
    /// centre by 8 px and the two lines no longer paired.
    fn decode(&self, cov: &Plane, signed: bool, strict: bool) -> Option<(String, f32, (usize, usize))> {
        let dot_row = (self.base as f32 + 1.0 - DOT_ZONE * self.digit_h as f32).round() as usize;
        let band_rows = ((self.base + 1 - self.digit_h).saturating_sub(BAND_SPILL), self.base + BAND_SPILL);
        let runs = runs_of(cov, (dot_row, self.base + 1), band_rows);
        let lo = self.base + 1 - self.digit_h - 4;
        // the template rows with a spare row above and below
        let band = Band::new(&sub_plane(cov, 0, lo - 1, cov.width, self.base + 2 - lo + 2));
        let (band_top, dh) = (self.base + 1 - self.digit_h, self.digit_h as f32);
        // which shapes the ink of columns x..x+w allows: (digit, sign, dot, degree)
        let shape = |x: usize, w: usize| -> (bool, bool, bool, bool) {
            let mut row_ink: Vec<f32> = (0..cov.height).map(|y| (x..x + w).map(|c| cov.at(c, y)).filter(|v| *v > SEG).sum()).collect();
            keep_text_segment(&mut row_ink, band_top.saturating_sub(BAND_SPILL), self.base + BAND_SPILL);
            let (Some(top), Some(bottom)) = (row_ink.iter().position(|v| *v > 0.0), row_ink.iter().rposition(|v| *v > 0.0)) else {
                return (false, false, false, false);
            };
            let total: f32 = row_ink.iter().sum::<f32>().max(1e-6);
            let share = |keep: &dyn Fn(usize) -> bool| row_ink.iter().enumerate().filter(|(y, _)| keep(*y)).map(|(_, v)| v).sum::<f32>() / total;
            let out_of_band = |y: usize| y + BAND_SPILL < band_top || y > self.base + BAND_SPILL;
            // a line or a blade across the text: much of its ink, or ink on many rows, off the digits
            let stray_rows = row_ink.iter().enumerate().filter(|(y, v)| out_of_band(*y) && **v >= 1.0).count();
            let within = share(&out_of_band) <= SPILL_INK && stray_rows < STRAY_ROWS;
            let dot_top = self.base as f32 + 1.0 - DOT_ZONE * dh;
            let degree_bottom = band_top as f32 + DEGREE_ZONE * dh;
            (
                within && (bottom + 1 - top) as f32 >= DIGIT_MIN_HEIGHT * dh,
                // A sign spans the middle of the digit band only; a "+" run with a digit's full height
                // has a digit merged into it (1920x720 at 100 %: "+1" read as "+").
                within && ((bottom + 1 - top) as f32) <= SIGN_MAX_HEIGHT * dh,
                share(&|y| (y as f32) < dot_top) <= STRAY_INK && bottom + BAND_SPILL >= self.base,
                share(&|y| (y as f32) > degree_bottom) <= STRAY_INK,
            )
        };
        // each symbol's cost for columns x..x+w, ruling out the shapes their ink excludes (the same
        // piece comes up again and again among a wide run's splits)
        let memo: std::cell::RefCell<std::collections::HashMap<(usize, usize), [f32; 14]>> = Default::default();
        let glyph = |x: usize, w: usize| -> [f32; 14] {
            if let Some(c) = memo.borrow().get(&(x, w)) {
                return *c;
            }
            let (digit, sign, dot, degree) = shape(x, w);
            let allowed: [bool; 14] = std::array::from_fn(|i| match i {
                0..10 => digit,
                10 | 11 => sign,
                12 => dot,
                _ => degree,
            });
            let c = self.slide_costs(&band, x, w, allowed);
            memo.borrow_mut().insert((x, w), c);
            c
        };
        // the dot alone, for the middle piece of a digit-dot-digit split
        let dot_only = |x: usize, w: usize| -> f32 {
            if !shape(x, w).2 {
                return INF;
            }
            self.slide_costs(&band, x, w, std::array::from_fn(|i| i == 12))[12]
        };
        let costs: Vec<RunCosts> = runs
            .iter()
            .map(|r| {
                // the run cut into two glyphs, both at most GW + 2 wide
                let pairs = || {
                    let mut pair = Box::new([[INF; 14]; 14]);
                    for cut in 3..r.w.saturating_sub(2) {
                        if cut > GW + 2 || r.w - cut > GW + 2 {
                            continue;
                        }
                        let (a, b) = (glyph(r.x, cut), glyph(r.x + cut, r.w - cut));
                        for (i, ai) in a.iter().enumerate() {
                            for (j, bj) in b.iter().enumerate() {
                                pair[i][j] = pair[i][j].min(ai + bj);
                            }
                        }
                    }
                    pair
                };
                if r.w <= GW + 2 {
                    // Only a "1" is narrow enough to share one glyph's width with another: at 100 %
                    // "51" and "71" overlap in columns and come as one 14-15 px run (1600x900 and
                    // 2560x1080).
                    let two = (r.w >= NARROW_PAIR).then(pairs);
                    return RunCosts { one: Some(glyph(r.x, r.w)), two, three: None, narrow: true };
                }
                let pair = pairs();
                // a digit, a dot and a digit: the middle piece narrow
                let mut triple = Box::new([[INF; 10]; 10]);
                for c1 in 3..r.w.saturating_sub(4) {
                    for c2 in c1 + 1..(c1 + 6).min(r.w.saturating_sub(2)) {
                        if c1 > GW + 2 || r.w - c2 > GW + 2 {
                            continue;
                        }
                        let dot = dot_only(r.x + c1, c2 - c1);
                        if dot >= INF {
                            continue;
                        }
                        let (a, b) = (glyph(r.x, c1), glyph(r.x + c2, r.w - c2));
                        for (i, ai) in a[..10].iter().enumerate() {
                            for (j, bj) in b[..10].iter().enumerate() {
                                triple[i][j] = triple[i][j].min(ai + dot + bj);
                            }
                        }
                    }
                }
                RunCosts { one: None, two: Some(pair), three: Some(triple), narrow: false }
            })
            .collect();
        let noise: Vec<f32> = runs
            .iter()
            .map(|r| {
                let ink: f32 = (0..cov.height).map(|y| cov.data[y * cov.width + r.x..y * cov.width + r.x + r.w].iter().sum::<f32>()).sum();
                let tall = (r.bottom - r.top + 1) as f32 >= 0.7 * dh;
                NOISE_PER_PX * ink * if tall { TALL_NOISE } else { 1.0 }
            })
            .collect();
        let digit_like: Vec<bool> = runs.iter().map(|r| shape(r.x, r.w).0).collect();
        let formats: &[&[char]] = if signed {
            &[&['s', 'd', '.', 'd', 'd', '°'], &['s', 'd', 'd', '.', 'd', 'd', '°']]
        } else {
            &[&['d', 'd', 'd', '.', 'd', 'd', '°']]
        };
        let mut best: Option<(String, f32, Vec<Pick>)> = None;
        for f in formats {
            if let Some(r) = fit(&costs, &noise, &digit_like, f)
                && best.as_ref().is_none_or(|b| r.1 < b.1)
            {
                best = Some(r);
            }
        }
        let debug = debug_on();
        let show = |picks: &[Pick]| {
            for p in picks {
                let r = runs[p.run];
                let text: String = p.symbols.iter().map(|&s| SYMBOLS[s]).collect();
                eprintln!("    run x {} w {} rows {}..{} -> {text:?} cost {:.1}", r.x, r.w, r.top, r.bottom, costs[p.run].of(&p.symbols));
            }
            let used: Vec<usize> = picks.iter().map(|p| p.run).collect();
            for (i, r) in runs.iter().enumerate().filter(|(i, _)| !used.contains(i)) {
                eprintln!("    run x {} w {} rows {}..{} -> noise {:.1}", r.x, r.w, r.top, r.bottom, noise[i]);
            }
        };
        let Some((text, total, picks)) = best else {
            if debug {
                eprintln!("  decode: no format, runs {:?}", runs.iter().map(|r| (r.x, r.w, r.top, r.bottom)).collect::<Vec<_>>());
            }
            return None;
        };
        if total > MAX_COST {
            if debug {
                eprintln!("  decode: {text} total {total:.1}");
                show(&picks);
            }
            return None;
        }
        for p in &picks {
            let mut cand = costs[p.run].readings(&p.slots);
            cand.sort_by(|a, b| a.1.total_cmp(&b.1));
            let mine = costs[p.run].of(&p.symbols);
            let digits = p.symbols.iter().filter(|&&s| s < 10).count() as f32;
            let cap = G_MAX_DIGIT * digits + G_MAX_OTHER * (p.symbols.len() as f32 - digits);
            if mine > cap {
                if debug {
                    eprintln!("  decode: {text} glyph {:?} cost {mine:.1} > {cap}", p.symbols);
                }
                return None;
            }
            // A narrow run read as one glyph or as two (`NARROW_PAIR`): the other way must cost clearly
            // more, or a wide glyph may be read as a "1" and a piece.
            if let (Some(one), Some(two)) = (&costs[p.run].one, &costs[p.run].two) {
                let best_one = one.iter().copied().fold(INF, f32::min);
                let best_two = (0..14).flat_map(|a| (0..14).map(move |b| (a, b))).filter(|&(a, b)| a == 1 || b == 1).map(|(a, b)| two[a][b]).fold(INF, f32::min);
                // read as a "1" and another glyph: any pair without a "1" must fit clearly worse too
                let without_one = (0..14).flat_map(|a| (0..14).map(move |b| (a, b))).filter(|&(a, b)| a != 1 && b != 1).map(|(a, b)| two[a][b]).fold(INF, f32::min);
                let other = if p.symbols.len() == 1 { best_two } else { best_one.min(without_one) };
                if other - mine < M_MIN {
                    if debug {
                        eprintln!("  decode: {text} glyph {:?} {mine:.1} vs one/two {other:.1}", p.symbols);
                    }
                    return None;
                }
            }
            if let Some((runner, second)) = cand.iter().find(|(s, _)| *s != p.symbols).filter(|_| strict) {
                let ink = |s: &[usize]| s.iter().map(|&i| self.ink[i]).sum::<f32>();
                // only for the sign: "+" read for a "-" crossed by a speck flips the elevation
                let sign = p.slots.contains(&'s');
                let need = if sign && mine > SUPERSET_CLEAN && ink(&p.symbols) > SUPERSET_INK * ink(runner) { M_SUPERSET } else { M_MIN };
                if second - mine < need {
                    if debug {
                        eprintln!("  decode: {text} glyph {:?} {mine:.1} vs {runner:?} {second:.1}", p.symbols);
                    }
                    return None;
                }
            }
        }
        if debug {
            show(&picks);
        }
        let x0 = picks.iter().map(|p| runs[p.run].x).min().expect("picks");
        let x1 = picks.iter().map(|p| runs[p.run].x + runs[p.run].w).max().expect("picks");
        Some((text.trim_end_matches('°').to_owned(), total, (x0, x1)))
    }

    /// Development: the best label candidate (score, frame x, frame y) at this reader's scale, and
    /// the "Azimuth" label's score under it; for measuring a configuration's text scale.
    #[doc(hidden)]
    /// Development: every label candidate (score, x, y, "Azimuth" score) on the plain plane or the
    /// shadow-corrected one.
    pub fn probe_candidates(&self, view: &FrameView, client_origin: (i32, i32), corrected: bool) -> Vec<(f32, i32, i32, f32)> {
        let (ox, oy) = client_origin;
        let mut area = Area::from_view(view, ox + AREA.0, oy + AREA.1, AREA.2 as usize, AREA.3 as usize);
        if corrected {
            area = self.corrected(&area);
        }
        self.candidates(&area, ox, oy).into_iter().map(|c| (c.score, c.x, c.y, self.az_score(&area, self.centre(c), c.y as f32))).collect()
    }

    pub fn probe_label(&self, view: &FrameView, client_origin: (i32, i32)) -> Option<(f32, i32, i32, f32)> {
        let (ox, oy) = client_origin;
        let area = Area::from_view(view, ox + AREA.0, oy + AREA.1, AREA.2 as usize, AREA.3 as usize);
        let c = *self.candidates(&area, ox, oy).first()?;
        Some((c.score, c.x, c.y, self.az_score(&area, self.centre(c), c.y as f32)))
    }

    /// Development: both lines decoded under a label at frame (x, y), at this reader's scale.
    #[doc(hidden)]
    pub fn probe_lines(&self, view: &FrameView, client_origin: (i32, i32), x: i32, y: i32) -> Option<(String, String, u32)> {
        let (ox, oy) = client_origin;
        let area = Area::from_view(view, ox + AREA.0, oy + AREA.1, AREA.2 as usize, AREA.3 as usize);
        self.decode_at(&area, None, oy, Candidate { score: 1.0, x, y }).map(|r| (r.elevation_text, r.azimuth_text, r.cost))
    }

    /// Both lines' texts when the readout reads, for diagnostics.
    #[doc(hidden)]
    pub fn raw_lines(
        &mut self,
        view: &FrameView,
        client_origin: (i32, i32),
    ) -> Option<(String, String)> {
        match self.read(view, client_origin) {
            AimResult::Read(r) => Some((r.elevation_text, r.azimuth_text)),
            _ => None,
        }
    }
}
