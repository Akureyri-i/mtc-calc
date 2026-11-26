//! Reading the zoomed-V1 grid labels ("Z 19": column letters, row number) to pin the absolute
//! cell of a fitted grid.
//!
//! Every cell carries its label left-aligned just inside its top-left corner, white Titillium Web
//! with a dark outline. Glyphs are segmented by ink columns, the split between letters and digits
//! is the widest gap (the space), and each glyph is classified by correlation against templates
//! (harvested from recordings, the few letters never recorded rendered from the font).
//! Each readable label votes for the grid column/row of the
//! first visible line; the majority wins.
//!
//! Marker labels ("A7", "B21") are green but in the same font. Their letters are read with
//! templates harvested from marker labels where recorded (`assets/marker_letters.json`,
//! `examples/harvest_marker_letters.rs`) alongside the grid templates: with the grid templates
//! alone a marker label's B correlates with B and 8 alike (0.835 against 0.836) and was read as a
//! digit.

use std::collections::HashMap;

use serde::Deserialize;

use crate::map::geometry::MapTransform;
use crate::map::zoom::GridFit;
use crate::view::FrameView;

/// A glyph column holds ink when some pixel exceeds this ink strength.
const INK_PRESENT: f32 = 40.0;
/// A label is used when every glyph correlates at least this well with its class.
const MIN_GLYPH_SCORE: f32 = 0.55;
const MAX_GLYPH_WIDTH: usize = 12;
/// A glyph has ink in at least this many rows (cap height 11).
const MIN_GLYPH_ROWS: usize = 6;
const SHIFT: i32 = 1;
/// Letters a marker label can start with: one per squad member, by joining order (a squad has
/// five or six members at most). Every letter still takes part in telling a letter from a digit
/// (an A half under an objective icon reads as some letter, so it is not read as a digit), but
/// another letter is reported as cut: in a public match such an A read as T, narrow enough to pass
/// as whole, and the player's A4 was taken for a squad member's.
const SQUAD_LETTERS: std::ops::RangeInclusive<char> = 'A'..='F';

#[derive(Deserialize)]
struct Asset {
    #[serde(rename = "box")]
    label_box: LabelBox,
    ink: Ink,
    height: usize,
    glyphs: HashMap<char, GlyphAsset>,
}

#[derive(Deserialize, Clone, Copy)]
struct LabelBox {
    dx: i32,
    dy: i32,
    width: i32,
    height: i32,
}

#[derive(Deserialize, Clone, Copy)]
struct Ink {
    min_value: f32,
    max_saturation: f32,
}

/// Letter templates harvested from marker labels.
#[derive(Deserialize)]
struct MarkerAsset {
    height: usize,
    glyphs: HashMap<char, GlyphAsset>,
}

#[derive(Deserialize)]
struct GlyphAsset {
    width: usize,
    rows: Vec<f32>,
}

#[derive(Clone)]
struct Glyph {
    ch: char,
    width: usize,
    data: Vec<f32>,
}

/// Grid column/row of the first visible lines, with the votes behind them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelVote {
    pub column: i64,
    pub row: i64,
    pub column_votes: usize,
    pub row_votes: usize,
    /// Labels read (each votes once per axis).
    pub labels: usize,
}

pub struct LabelReader {
    label_box: LabelBox,
    ink: Ink,
    height: usize,
    letters: Vec<Glyph>,
    digits: Vec<Glyph>,
    /// Letters for marker labels: the ones harvested from marker labels, then every grid letter
    /// (a letter cut by an icon leaves a part that the grid templates take for a letter, where the
    /// marker A alone lost it to a digit: "A2" read "?42").
    marker_letters: Vec<Glyph>,
}

impl Default for LabelReader {
    fn default() -> Self {
        Self::new()
    }
}

/// Column index of label letters: A = 0 … Z = 25, AA = 26 (bijective base 26).
pub fn column_index(letters: &str) -> i64 {
    letters.bytes().fold(0i64, |n, b| n * 26 + i64::from(b - b'A') + 1) - 1
}

impl LabelReader {
    pub fn new() -> Self {
        let asset: Asset = serde_json::from_str(include_str!("../../assets/grid_glyphs.json")).expect("grid glyph asset");
        let marker: MarkerAsset = serde_json::from_str(include_str!("../../assets/marker_letters.json")).expect("marker letter asset");
        assert_eq!(marker.height, asset.height, "marker letters have the grid glyphs' height");
        let mut marker_letters: Vec<Glyph> = marker.glyphs.into_iter().map(|(ch, g)| Glyph { ch, width: g.width, data: g.rows }).collect();
        let mut letters = Vec::new();
        let mut digits = Vec::new();
        for (ch, g) in asset.glyphs {
            if !ch.is_ascii_digit() {
                marker_letters.push(Glyph { ch, width: g.width, data: g.rows.clone() });
            }
            let glyph = Glyph {
                ch,
                width: g.width,
                data: g.rows,
            };
            if ch.is_ascii_digit() {
                digits.push(glyph);
            } else {
                letters.push(glyph);
            }
        }
        Self {
            label_box: asset.label_box,
            ink: asset.ink,
            height: asset.height,
            letters,
            digits,
            marker_letters,
        }
    }

    /// Width (px) of a marker label's letter, from its template (the harvested one first).
    pub(crate) fn marker_letter_width(&self, ch: char) -> Option<usize> {
        self.marker_letters.iter().find(|g| g.ch == ch).map(|g| g.width)
    }

    /// Ink strength of a frame pixel: value above the threshold where unsaturated.
    fn ink_at(&self, view: &FrameView, x: i32, y: i32) -> f32 {
        let p = view.px(x, y);
        let max = p[0].max(p[1]).max(p[2]);
        let min = p[0].min(p[1]).min(p[2]);
        let sat = if max == 0 { 0.0 } else { f32::from(max - min) * 255.0 / f32::from(max) };
        if sat < self.ink.max_saturation {
            (f32::from(max) - self.ink.min_value).max(0.0)
        } else {
            0.0
        }
    }

    /// Best correlation of `glyph` (h×w) with a template, ±1 px shifts, on a common canvas.
    fn classify<'g>(&self, glyph: &[f32], w: usize, templates: &'g [Glyph]) -> (&'g Glyph, f32) {
        let h = self.height;
        let cw = MAX_GLYPH_WIDTH + 2;
        let ch = h + 2;
        let place = |data: &[f32], width: usize, dx: i32, dy: i32| {
            let mut canvas = vec![0.0f32; cw * ch];
            for y in 0..h {
                for x in 0..width {
                    let (cx, cy) = (x as i32 + 1 + dx, y as i32 + 1 + dy);
                    if cx >= 0 && cy >= 0 && (cx as usize) < cw && (cy as usize) < ch {
                        canvas[cy as usize * cw + cx as usize] = data[y * width + x];
                    }
                }
            }
            canvas
        };
        let observed = place(glyph, w, 0, 0);
        let mut best = (&templates[0], f32::MIN);
        for t in templates {
            for dy in -SHIFT..=SHIFT {
                for dx in -SHIFT..=SHIFT {
                    let s = crate::map::raster::masked_ncc(&observed, &place(&t.data, t.width, dx, dy), None);
                    if s > best.1 {
                        best = (t, s);
                    }
                }
            }
        }
        best
    }

    /// Text of the grid label whose box starts at frame (x0, y0): (letters, number) or `None`.
    fn read_label(&self, view: &FrameView, x0: i32, y0: i32) -> Option<(String, i64)> {
        let b = self.label_box;
        let (w, h) = (b.width as usize, b.height as usize);
        let mut ink = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                ink[y * w + x] = self.ink_at(view, x0 + x as i32, y0 + y as i32);
            }
        }
        self.read_text(&ink, w, h)
    }

    /// Letters followed by a number ("Z19", "A3") from an ink-strength box `w`×`h`: glyphs are
    /// ink column runs, letters and digits split at the widest gap.
    pub(crate) fn read_text(&self, ink: &[f32], w: usize, h: usize) -> Option<(String, i64)> {
        let col_has = |x: usize| (0..h).any(|y| ink[y * w + x] > INK_PRESENT);
        let mut runs = Vec::new();
        let mut start = None;
        for x in 0..=w {
            let has = x < w && col_has(x);
            match (has, start) {
                (true, None) => start = Some(x),
                (false, Some(a)) => {
                    runs.push((a, x));
                    start = None;
                }
                _ => {}
            }
        }
        // Glyphs span most of the cap height; slivers of neighbouring outlines do not.
        let inked_rows = |&(a, b): &(usize, usize)| (0..h).filter(|&y| (a..b).any(|x| ink[y * w + x] > INK_PRESENT)).count();
        runs.retain(|r| inked_rows(r) >= MIN_GLYPH_ROWS);
        if !(2..=4).contains(&runs.len()) || runs.iter().any(|&(a, b)| b - a > MAX_GLYPH_WIDTH) {
            return None;
        }
        // The space between letters and digits is the widest gap.
        let split = (1..runs.len()).max_by_key(|&i| runs[i].0 - runs[i - 1].1)?;
        if split > 2 || runs.len() - split > 2 {
            return None;
        }
        let (first, last) = (runs.first()?.0, runs.last()?.1);
        let top = (0..h).find(|&y| (first..last).any(|x| ink[y * w + x] > INK_PRESENT))?;
        if top + self.height > h {
            return None;
        }
        let mut letters = String::new();
        let mut digits = String::new();
        for (i, &(a, bx)) in runs.iter().enumerate() {
            let gw = bx - a;
            let glyph: Vec<f32> = (0..self.height)
                .flat_map(|y| (a..bx).map(move |x| (y, x)))
                .map(|(y, x)| ink[(top + y) * w + x])
                .collect();
            let (t, score) = self.classify(&glyph, gw, if i < split { &self.letters } else { &self.digits });
            if score < MIN_GLYPH_SCORE {
                return None;
            }
            if i < split { letters.push(t.ch) } else { digits.push(t.ch) }
        }
        let number: i64 = digits.parse().ok()?;
        (number >= 1).then_some((letters, number))
    }

    /// Reads a marker label ("A7", "B12", "A104"): one letter, then 1–3 digits, no space. Returns
    /// the letter (`None` when the first glyph is not one, or not a squad letter: cut by an icon or
    /// missing) and the number.
    pub(crate) fn read_marker_text(&self, ink: &[f32], w: usize, h: usize) -> Option<(Option<char>, u32)> {
        let glyphs = self.marker_glyphs(ink, w, h)?;
        // The first glyph is the letter when it reads as one at least as well as as a digit, or when
        // it reads best as 0: a marker number never starts with 0. D is the letter most like a digit
        // (its grid template correlates 0.80 with the 0, more than the B with the 8, 0.77, which was
        // read as a digit before B was harvested from marker labels), and no squad of four or more has
        // been recorded to harvest it from.
        let (first_width, first) = &glyphs[0];
        let (lt, ls) = self.classify(first, *first_width, &self.marker_letters);
        let (dt, ds) = self.classify(first, *first_width, &self.digits);
        let letter = if glyphs.len() < 2 || ls < MIN_GLYPH_SCORE {
            None
        } else if ls >= ds {
            Some(lt.ch)
        } else if dt.ch == '0' {
            // A letter for sure: the squad letter it is most like (not the O of the grid font).
            let squad: Vec<Glyph> = self.marker_letters.iter().filter(|g| SQUAD_LETTERS.contains(&g.ch)).cloned().collect();
            Some(self.classify(first, *first_width, &squad).0.ch)
        } else {
            None
        };
        let digit_glyphs = &glyphs[usize::from(letter.is_some())..];
        if digit_glyphs.is_empty() || digit_glyphs.len() > 3 {
            return None;
        }
        let mut digits = String::new();
        for (width, glyph) in digit_glyphs {
            let (t, score) = self.classify(glyph, *width, &self.digits);
            if score < MIN_GLYPH_SCORE {
                return None;
            }
            digits.push(t.ch);
        }
        let number: u32 = digits.parse().ok()?;
        (number >= 1).then_some((letter.filter(|c| SQUAD_LETTERS.contains(c)), number))
    }

    /// The glyphs of a marker label's ink box, left to right: width and ink, `height` rows from
    /// the label's top. `None` unless there are 1–4.
    pub(crate) fn marker_glyphs(&self, ink: &[f32], w: usize, h: usize) -> Option<Vec<(usize, Vec<f32>)>> {
        let col_has = |x: usize| (0..h).any(|y| ink[y * w + x] > INK_PRESENT);
        let mut runs = Vec::new();
        let mut start = None;
        for x in 0..=w {
            let has = x < w && col_has(x);
            match (has, start) {
                (true, None) => start = Some(x),
                (false, Some(a)) => {
                    runs.push((a, x));
                    start = None;
                }
                _ => {}
            }
        }
        let inked_rows = |&(a, b): &(usize, usize)| (0..h).filter(|&y| (a..b).any(|x| ink[y * w + x] > INK_PRESENT)).count();
        runs.retain(|r| inked_rows(r) >= MIN_GLYPH_ROWS);
        if !(1..=4).contains(&runs.len()) || runs.iter().any(|&(a, b)| b - a > MAX_GLYPH_WIDTH) {
            return None;
        }
        let (first, last) = (runs.first()?.0, runs.last()?.1);
        let top = (0..h).find(|&y| (first..last).any(|x| ink[y * w + x] > INK_PRESENT))?;
        if top + self.height > h {
            return None;
        }
        let glyph = |(a, b): (usize, usize)| -> Vec<f32> {
            (0..self.height).flat_map(|y| (a..b).map(move |x| (y, x))).map(|(y, x)| ink[(top + y) * w + x]).collect()
        };
        Some(runs.into_iter().map(|(a, b)| (b - a, glyph((a, b)))).collect())
    }

    /// Reads every label inside the container and votes for the grid column/row of the first
    /// visible vertical/horizontal line.
    pub fn read(&self, view: &FrameView, t: &MapTransform, grid: &GridFit) -> Option<LabelVote> {
        let b = self.label_box;
        let right = t.left + t.size;
        let bottom = t.top + t.size;
        let mut columns: HashMap<i64, usize> = HashMap::new();
        let mut rows: HashMap<i64, usize> = HashMap::new();
        let mut labels = 0;
        let mut k = 0i64;
        loop {
            let xc = grid.first_line.0 + k as f64 * grid.period;
            let x0 = xc.round() as i32 + b.dx;
            if f64::from(x0 + b.width) >= right {
                break;
            }
            let mut j = 0i64;
            loop {
                let yc = grid.first_line.1 + j as f64 * grid.period;
                let y0 = yc.round() as i32 + b.dy;
                if f64::from(y0 + b.height) >= bottom {
                    break;
                }
                if view.covers(x0, y0, b.width, b.height)
                    && let Some((letters, number)) = self.read_label(view, x0, y0)
                {
                    labels += 1;
                    *columns.entry(column_index(&letters) - k).or_default() += 1;
                    *rows.entry(number - 1 - j).or_default() += 1;
                }
                j += 1;
            }
            k += 1;
        }
        let (&column, &column_votes) = columns.iter().max_by_key(|e| *e.1)?;
        let (&row, &row_votes) = rows.iter().max_by_key(|e| *e.1)?;
        Some(LabelVote {
            column,
            row,
            column_votes,
            row_votes,
            labels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{LabelReader, column_index};

    /// A marker label's ink set from the grid templates, glyphs one column apart, a row of margin.
    fn label_ink(reader: &LabelReader, text: &str) -> (Vec<f32>, usize, usize) {
        let glyphs: Vec<_> = text.chars().map(|c| reader.letters.iter().chain(&reader.digits).find(|g| g.ch == c).expect("template")).collect();
        let (h, w) = (reader.height + 2, glyphs.iter().map(|g| g.width + 1).sum::<usize>() + 1);
        let mut ink = vec![0.0; w * h];
        let mut x0 = 1;
        for g in glyphs {
            for y in 0..reader.height {
                for x in 0..g.width {
                    ink[(y + 1) * w + x0 + x] = g.data[y * g.width + x];
                }
            }
            x0 += g.width + 1;
        }
        (ink, w, h)
    }

    #[test]
    fn squad_letters_d_e_f_read_as_letters() {
        let reader = LabelReader::new();
        for (text, letter, number) in [("D3", 'D', 3), ("D10", 'D', 10), ("E7", 'E', 7), ("F12", 'F', 12), ("A4", 'A', 4)] {
            let (ink, w, h) = label_ink(&reader, text);
            assert_eq!(reader.read_marker_text(&ink, w, h), Some((Some(letter), number)), "{text}");
        }
        // A D drawn so like a 0 that the 0 template wins (the worst case: the 0 itself): still the
        // letter D, not the number 12 with no letter.
        let (ink, w, h) = label_ink(&reader, "012");
        assert_eq!(reader.read_marker_text(&ink, w, h), Some((Some('D'), 12)));
    }

    #[test]
    fn column_letters_are_bijective_base_26() {
        assert_eq!(column_index("A"), 0);
        assert_eq!(column_index("Z"), 25);
        assert_eq!(column_index("AA"), 26);
        assert_eq!(column_index("AC"), 28);
        assert_eq!(column_index("BC"), 54);
    }
}
