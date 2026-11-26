//! Gunner ammo bar layout.
//!
//! The bar is centred on screen: shell slots, an optional `G` cell (smoke grenade launcher)
//! and the `V` rangefinder cell, 110 px apart. The angle readout is attached to its left edge,
//! so the readout's label position already gives the cell count; the `G`/`V` header letters
//! (white on an opaque dark box, unlike the translucent slot headers) confirm it. The cells are
//! placed from the screen's centre line, not from the label: the label is text, which the game
//! lays out again at another display scaling (4 px off in testing), the bar is not.

use serde::Deserialize;

use crate::map::raster::{Plane, resize};
use crate::view::FrameView;

/// The bar's centre line (client x): the client's centre. Measured on the V header of 2- to
/// 6-cell bars (7 vehicles): the first cell's left edge at 1280 − span / 2 in all of them.
const BAR_CENTRE_X: i32 = 1280;
/// Cell spacing.
const CELL_PITCH: i32 = 110;
const CELL_WIDTH: i32 = 106;
/// Label x for a five-cell bar; each extra cell moves the label half a pitch left.
const FIVE_CELL_LABEL_X: f32 = 903.0;
const HALF_PITCH: f32 = 55.25;
/// Header letter box (client y and height); its letter is centred on the cell and the box.
const HEADER_TOP: i32 = 1192;
const HEADER_HEIGHT: i32 = 24;
/// The letter's centre is looked for this far (px) round the box's middle.
const SEARCH: i32 = 3;
/// A header letter is white: at least this many of its window's pixels this bright (luma, on the
/// source pixels). The grey slot digits peak at 148; a small V peaks at 194 with one pixel above
/// 170 (125 %, 1920x1120), 13-14 at 150 % on 2560x1334.
const HEADER_MIN_VALUE: f32 = 170.0;
const MIN_BRIGHT_PIXELS: usize = 2;
const LETTER_MIN_SCORE: f32 = 0.6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarLayout {
    /// All cells including `G` and `V`.
    pub cells: usize,
    pub has_grenade: bool,
    /// Shell slots (cells minus `G` and `V`).
    pub shell_slots: usize,
    /// Frame x of the first cell's left edge.
    pub first_cell_x: i32,
}

impl BarLayout {
    pub const PITCH: i32 = CELL_PITCH;
    pub const CELL_WIDTH: i32 = CELL_WIDTH;

    /// Frame x of cell `i`'s left edge.
    pub fn cell_x(&self, i: usize) -> i32 {
        self.first_cell_x + CELL_PITCH * i as i32
    }
}

#[derive(Deserialize)]
struct HeaderFile {
    #[serde(rename = "G")]
    g: GreyFile,
    #[serde(rename = "V")]
    v: GreyFile,
}

/// A header letter's grey levels round its ink, centred on it, cut from recorded frames.
#[derive(Deserialize)]
struct GreyFile {
    width: usize,
    height: usize,
    /// Two hex digits a pixel, row by row.
    grey: String,
}

/// A header letter resampled to the configuration's text scale, mean removed.
struct LetterTemplate {
    width: i32,
    height: i32,
    values: Vec<f32>,
    norm: f32,
}

impl LetterTemplate {
    fn from(file: &GreyFile, (sx, sy): (f32, f32)) -> Self {
        let grey: Vec<f32> = (0..file.width * file.height)
            .map(|i| f32::from(u8::from_str_radix(&file.grey[2 * i..2 * i + 2], 16).expect("hex grey")))
            .collect();
        let plane = Plane { width: file.width, height: file.height, data: grey };
        let (w, h) = (((file.width as f32 * sx).round() as usize).max(3), ((file.height as f32 * sy).round() as usize).max(3));
        let scaled = resize(&plane, w, h);
        let mean = scaled.data.iter().sum::<f32>() / scaled.data.len() as f32;
        let values: Vec<f32> = scaled.data.iter().map(|v| v - mean).collect();
        let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
        Self { width: w as i32, height: h as i32, values, norm }
    }
}

pub struct BarReader {
    g: LetterTemplate,
    v: LetterTemplate,
}

impl Default for BarReader {
    fn default() -> Self {
        Self::new()
    }
}

impl BarReader {
    pub fn new() -> Self {
        Self::with_text_scale((1.0, 1.0))
    }

    /// `text_scale`: the header letters' horizontal and vertical scale in this configuration,
    /// against 150 % on 2560 x 1334, after the bar is mapped to that geometry (measured: 125 %
    /// (1.14, 0.96), 100 % (1.08, 0.90)).
    pub fn with_text_scale(text_scale: (f32, f32)) -> Self {
        let file: HeaderFile = serde_json::from_str(include_str!("../assets/bar_headers.json")).expect("header asset");
        Self { g: LetterTemplate::from(&file.g, text_scale), v: LetterTemplate::from(&file.v, text_scale) }
    }

    /// Layout of the bar whose angle readout label starts at `label_x` (frame coordinates).
    pub fn layout(&self, view: &FrameView, client_origin: (i32, i32), label_x: i32) -> Option<BarLayout> {
        let steps = (FIVE_CELL_LABEL_X + client_origin.0 as f32 - label_x as f32) / HALF_PITCH;
        let rounded = steps.round();
        if (steps - rounded).abs() > 0.1 {
            return None;
        }
        let cells = usize::try_from(5 + rounded as i32).ok().filter(|&c| c >= 2)?;
        let first_cell_x = client_origin.0 + BAR_CENTRE_X - (CELL_PITCH * (cells as i32 - 1) + CELL_WIDTH) / 2;
        let header_top = client_origin.1 + HEADER_TOP;
        let center = |i: usize| first_cell_x + CELL_PITCH * i as i32 + CELL_WIDTH / 2;
        if self.score(view, &self.v, center(cells - 1), header_top) < LETTER_MIN_SCORE {
            return None;
        }
        let has_grenade = cells >= 3 && self.score(view, &self.g, center(cells - 2), header_top) >= LETTER_MIN_SCORE;
        let shell_slots = cells - 1 - usize::from(has_grenade);
        (shell_slots >= 1).then_some(BarLayout {
            cells,
            has_grenade,
            shell_slots,
            first_cell_x,
        })
    }

    /// Best normalised correlation of the letter's grey template with the luma, centred within
    /// `SEARCH` px of the header box's middle at `center_x`. Grey levels, not a threshold: at another
    /// display scaling the letter is drawn again and resampled to the reference geometry, most of
    /// its strokes fall between 110 and 170 (at 125 % and 100 %).
    fn score(&self, view: &FrameView, t: &LetterTemplate, center_x: i32, header_top: i32) -> f32 {
        let (cx, cy) = (center_x, header_top + HEADER_HEIGHT / 2);
        let n = t.values.len() as f32;
        let mut best = (0f32, 0, 0);
        for dy in -SEARCH..=SEARCH {
            for dx in -SEARCH..=SEARCH {
                let (x0, y0) = (cx - t.width / 2 + dx, cy - t.height / 2 + dy);
                if !view.covers(x0, y0, t.width, t.height) {
                    continue;
                }
                let (mut sum, mut sum2, mut dot) = (0f32, 0f32, 0f32);
                for r in 0..t.height {
                    for c in 0..t.width {
                        let v = view.luma(x0 + c, y0 + r);
                        sum += v;
                        sum2 += v * v;
                        dot += v * t.values[(r * t.width + c) as usize];
                    }
                }
                let var = sum2 - sum * sum / n;
                if var > 0.0 && t.norm > 0.0 && dot / (var.sqrt() * t.norm) > best.0 {
                    best = (dot / (var.sqrt() * t.norm), x0, y0);
                }
            }
        }
        // White letters, unlike the grey digits of the translucent slot headers ("3" of the 60 mm
        // mortar is G-shaped enough, its brightest pixel 148): 13–14 such pixels in the V at 150 %,
        // 125 % and 100 %.
        let (score, x0, y0) = best;
        // No place within the view (a read-back region that ends short of this cell).
        if score <= 0.0 {
            return 0.0;
        }
        // On the source pixels: resampled to the reference geometry from 100 % the V's strokes
        // peak at 165-223 and only 1-10 of its pixels stay above 170, from 13-14.
        let bright = (0..t.height).flat_map(|r| (0..t.width).map(move |c| (c, r))).filter(|&(c, r)| view.peak_luma(x0 + c, y0 + r) >= HEADER_MIN_VALUE).count();
        if bright < MIN_BRIGHT_PIXELS {
            return 0.0;
        }
        score
    }
}
