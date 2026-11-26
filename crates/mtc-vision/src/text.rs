//! UI text reader for the loading UI (weapon label, card shell name and values): light text in
//! the game's Montserrat, matched against templates rendered from the font shipped with Roblox
//! (`assets/ui_glyphs.json`; styles calibrated by correlating
//! rendered strings with recorded frames).
//!
//! Glyphs are the runs of inked columns; the baseline is the most common bottom row. Each run is
//! scored against every template by normalised correlation on the baseline (±1 px), a run wider
//! than any template is split greedily, and a gap wider than a space is a space.
//!
//! Some glyphs are close in this font (5/S, 1/I/l, 0/O, 8/B, 2/Z); readers of names compare with
//! [`same_glyph`], and numbers are read with [`digits`].

use serde::Deserialize;

use crate::map::raster::{Plane, resize};
use crate::view::FrameView;

/// Template pixels below this (0–255) are not ink when finding a template's columns.
const MIN_TEMPLATE_INK: f32 = 40.0 / 255.0;
/// Ink threshold between the box's background and text levels.
const INK_SHARE: f32 = 0.45;
/// A run whose best template scores below this reads as `?`.
const MIN_GLYPH_SCORE: f32 = 0.5;
/// A run whose best single template scores below this is also tried as two touching glyphs
/// ("71" in "171 m/s"), each side from its `PAIR_CANDIDATES` best templates.
const SPLIT_BELOW: f32 = 0.6;
/// A pair must beat the single glyph by this much.
const PAIR_MARGIN: f32 = 0.08;
const PAIR_CANDIDATES: usize = 10;
/// Templates kept from the centred pass for the ±1 px search (more for the sides of a split,
/// whose candidates are then thinned to distinct glyphs).
const COARSE_KEEP: usize = 12;
const COARSE_KEEP_SIDE: usize = 40;
/// Left-glyph candidates tried when splitting a run too wide for one glyph, and how many glyphs
/// deep such a run is split before the rest reads as `?`.
const WIDE_CANDIDATES: usize = 6;
const MAX_WIDE_DEPTH: u32 = 6;
/// A line with more ink runs than this is not text (the longest strings read are about 20
/// characters): scenery behind an absent label would otherwise be matched run by run.
const MAX_RUNS: usize = 40;
/// A run wider than a digit is read as touching digits when they score this on average.
const MIN_DIGIT_RUN_SCORE: f32 = 0.65;
/// Combined template width minus run width allowed for a pair: a gap is negative, an overlap
/// (bold glyphs kerned together) positive.
const PAIR_FILL: (i32, i32) = (-2, 4);
/// Glyphs narrower than this (".", "-", ",") are not tried in pairs: they fit parts of others.
const MIN_PAIR_WIDTH: usize = 3;
/// Touching glyphs a run may hold when a line is matched against whole candidate strings (a whole
/// name can be one run: "L33A1" over a busy scene behind the translucent card).
const MAX_RUN_GLYPHS: usize = 8;
/// Templates of each glyph tried per side when touching glyphs are laid over one run.
const MATCH_SIDE_KEEP: usize = 3;
/// A candidate string's estimated width (`Style::width_of`) against the text's ink span: on the
/// 358 labelled card names −1 to +10 px wider (touching glyphs, kerning), at most 15 %. Allowed:
/// this much narrower, and this many px or this share of the span wider.
const MATCH_SPAN_NARROWER: f64 = 4.0;
const MATCH_SPAN_WIDER: (f64, f64) = (6.0, 0.18);
/// Every run laid under a candidate's glyphs must score this: a wrong candidate otherwise
/// passes on a mean held up by its right glyphs ("3VO2..." over "3VO28" scored 0.77).
const MATCH_RUN_MIN_SCORE: f32 = 0.45;
/// Gap (px) between ink runs, as a share of the font's space width, from which a space is read:
/// words are 7–8 px apart, the letters of "mm" 4 px (weapon label, space 4.0 px).
const SPACE_SHARE: f64 = 1.35;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextStyle {
    /// Weapon label ("155 mm M284"): Montserrat Regular 16 px.
    Weapon,
    /// Card shell name ("M107 High..."): Medium 17 px.
    Name,
    /// Card value ("684 m/s"): Medium 15.5 px.
    Value,
}

#[derive(Deserialize)]
struct GlyphFile {
    styles: StylesFile,
}

#[derive(Deserialize)]
struct StylesFile {
    weapon: StyleFile,
    name: StyleFile,
    value: StyleFile,
}

#[derive(Deserialize)]
struct StyleFile {
    space: f64,
    glyphs: Vec<GlyphEntry>,
}

#[derive(Deserialize)]
struct GlyphEntry {
    ch: char,
    width: usize,
    height: usize,
    top: i32,
    pixels: String,
}

/// A glyph template made at run time (`TextReader::from_styles`): grey levels 0-255 row by row,
/// its first row relative to the baseline (negative: above).
#[derive(Clone, Debug)]
pub struct GlyphImage {
    pub ch: char,
    pub width: usize,
    pub height: usize,
    pub top: i32,
    pub pixels: Vec<u8>,
}

/// One text style's templates: the space's width (px) and the glyphs.
#[derive(Clone, Debug, Default)]
pub struct GlyphStyle {
    pub space: f64,
    pub glyphs: Vec<GlyphImage>,
}

#[derive(Clone)]
struct Glyph {
    ch: char,
    width: usize,
    height: usize,
    /// First row relative to the baseline (negative: above).
    top: i32,
    pixels: Vec<f32>,
}

struct Style {
    space: f64,
    glyphs: Vec<Glyph>,
    max_width: usize,
}

impl Style {
    /// `scale`: the style's horizontal and vertical text scale in this configuration (see
    /// [`TextReader::with_scales`]); the templates are resampled to it.
    fn from(file: StyleFile, (sx, sy): (f32, f32)) -> Self {
        let glyphs: Vec<Glyph> = file
            .glyphs
            .into_iter()
            .map(|g| {
                let pixels: Vec<f32> = (0..g.width * g.height).map(|i| f32::from(u8::from_str_radix(&g.pixels[2 * i..2 * i + 2], 16).expect("hex")) / 255.0).collect();
                if (sx, sy) == (1.0, 1.0) {
                    return Glyph { ch: g.ch, width: g.width, height: g.height, top: g.top, pixels };
                }
                let (w, h) = (((g.width as f32 * sx).round() as usize).max(1), ((g.height as f32 * sy).round() as usize).max(1));
                let scaled = resize(&Plane { width: g.width, height: g.height, data: pixels }, w, h);
                Glyph { ch: g.ch, width: w, height: h, top: (g.top as f32 * sy).round() as i32, pixels: scaled.data }
            })
            .collect();
        let max_width = glyphs.iter().map(|g| g.width).max().unwrap_or(0);
        Self { space: file.space * f64::from(sx), glyphs, max_width }
    }

    /// About how wide `text` is drawn (px, first ink to last): each glyph's median template
    /// width, a pixel between glyphs, a space where the text has one.
    fn width_of(&self, text: &str) -> f64 {
        let mut total = 0.0;
        let mut first = true;
        for c in text.chars() {
            if c == ' ' {
                total += self.space;
                continue;
            }
            let mut widths: Vec<usize> = self.glyphs.iter().filter(|g| g.ch == c).map(|g| g.width).collect();
            widths.sort_unstable();
            total += widths.get(widths.len() / 2).copied().unwrap_or(0) as f64 + if first { 0.0 } else { 1.0 };
            first = false;
        }
        total
    }
}

pub struct TextReader {
    weapon: Style,
    name: Style,
    value: Style,
}

impl Default for TextReader {
    fn default() -> Self {
        Self::new()
    }
}

/// A grey patch of the text box: ink strength 0–1 per pixel.
struct Patch {
    width: usize,
    height: usize,
    ink: Vec<f32>,
}

impl Patch {
    fn at(&self, x: i32, y: i32) -> f32 {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            0.0
        } else {
            self.ink[y as usize * self.width + x as usize]
        }
    }
}

impl TextReader {
    pub fn new() -> Self {
        Self::with_scales([(1.0, 1.0); 3])
    }

    /// `scales`: the weapon label's, card name's and card value's horizontal and vertical text
    /// scale in this configuration, against 150 % on 2560 x 1334 after the loading UI is mapped
    /// to that geometry (measured).
    pub fn with_scales(scales: [(f32, f32); 3]) -> Self {
        let file: GlyphFile = serde_json::from_str(include_str!("../assets/ui_glyphs.json")).expect("ui glyph asset");
        Self {
            weapon: Style::from(file.styles.weapon, scales[0]),
            name: Style::from(file.styles.name, scales[1]),
            value: Style::from(file.styles.value, scales[2]),
        }
    }

    /// Templates drawn for this configuration (weapon label, card name, card value) instead of the
    /// 150 % asset: at another display configuration the game draws these styles at other whole
    /// font sizes, unhinted.
    pub fn from_styles(styles: [GlyphStyle; 3]) -> Self {
        let style = |g: GlyphStyle| {
            let glyphs: Vec<Glyph> = g
                .glyphs
                .into_iter()
                .map(|i| Glyph { ch: i.ch, width: i.width, height: i.height, top: i.top, pixels: i.pixels.iter().map(|&v| f32::from(v) / 255.0).collect() })
                .collect();
            let max_width = glyphs.iter().map(|g| g.width).max().unwrap_or(0);
            Style { space: g.space, glyphs, max_width }
        };
        let [weapon, name, value] = styles;
        Self { weapon: style(weapon), name: style(name), value: style(value) }
    }

    /// As `from_styles`, from a glyph file in the asset's format (development: templates drawn for
    /// a configuration by `crates/mtc-overlay/examples/ui_glyphs_json.rs`).
    pub fn from_json(json: &str) -> Self {
        let file: GlyphFile = serde_json::from_str(json).expect("glyph file");
        Self::from_styles([file.styles.weapon, file.styles.name, file.styles.value].map(|f| GlyphStyle {
            space: f.space,
            glyphs: f.glyphs.into_iter().map(|g| GlyphImage { ch: g.ch, width: g.width, height: g.height, top: g.top, pixels: (0..g.width * g.height).map(|i| u8::from_str_radix(&g.pixels[2 * i..2 * i + 2], 16).expect("hex")).collect() }).collect(),
        }))
    }

    fn style(&self, style: TextStyle) -> &Style {
        match style {
            TextStyle::Weapon => &self.weapon,
            TextStyle::Name => &self.name,
            TextStyle::Value => &self.value,
        }
    }

    /// Reads one line of light text inside the frame box `(x0, y0, x1, y1)`. Pixels whose lowest
    /// channel is at least `min_ink` are ink; ink strength scales from there to white.
    pub fn read(&self, view: &FrameView, style: TextStyle, rect: (i32, i32, i32, i32), min_ink: u8) -> Option<String> {
        self.read_line(view, style, rect, min_ink, false, false)
    }

    /// The number at the start of the line, e.g. 684 in "684 m/s": digit templates only (no
    /// letter look-alikes), ending at the first run wider than any digit (the "m" of "m/s").
    pub fn read_number(&self, view: &FrameView, style: TextStyle, rect: (i32, i32, i32, i32), min_ink: u8) -> Option<u32> {
        let text = self.read_line(view, style, rect, min_ink, false, true)?;
        text.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
    }

    /// `read_number` for a line centred in the box (as `read_centred`).
    pub fn read_number_centred(&self, view: &FrameView, style: TextStyle, rect: (i32, i32, i32, i32), min_ink: u8) -> Option<u32> {
        let text = self.read_line(view, style, rect, min_ink, true, true)?;
        text.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
    }

    /// Like `read`, for a line centred in the box: only the glyphs chained (gaps under three spaces)
    /// to the one nearest the centre, and ink from 60 % of the brightest pixel on (dim labels over
    /// a busy scene).
    pub fn read_centred(&self, view: &FrameView, style: TextStyle, rect: (i32, i32, i32, i32), min_ink: u8) -> Option<String> {
        self.read_line(view, style, rect, min_ink, true, false)
    }

    /// The box's ink (from partway between the background, its median, and the text, its 98th
    /// percentile; `centred`: from 60 % of the brightest pixel too) and its runs of inked columns.
    fn patch_runs(view: &FrameView, (x0, y0, x1, y1): (i32, i32, i32, i32), min_ink: u8, centred: bool) -> Option<(Patch, Vec<(usize, usize)>)> {
        if x1 <= x0 || y1 <= y0 || !view.covers(x0, y0, x1 - x0, y1 - y0) {
            return None;
        }
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let grey: Vec<u8> = (0..w * h)
            .map(|i| {
                let p = view.px(x0 + (i % w) as i32, y0 + (i / w) as i32);
                p[0].min(p[1]).min(p[2])
            })
            .collect();
        // Thin grey strokes stay whole, a bright scene behind a translucent panel stays out.
        let mut sorted = grey.clone();
        sorted.sort_unstable();
        let background = f32::from(sorted[sorted.len() / 2]);
        let text_level = f32::from(sorted[sorted.len() * 98 / 100]);
        let adaptive = background + INK_SHARE * (text_level - background);
        let floor = f32::from(min_ink).max(adaptive).max(if centred { 0.6 * f32::from(sorted[sorted.len() - 1]) } else { 0.0 });
        let ink: Vec<f32> = grey
            .iter()
            .map(|&g| {
                let v = f32::from(g);
                if v >= floor { ((v - floor) / (255.0 - floor)).max(0.05) } else { 0.0 }
            })
            .collect();
        let patch = Patch { width: w, height: h, ink };
        let col_inked = |x: usize| (0..h).any(|y| patch.ink[y * w + x] > 0.0);
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut start = None;
        for x in 0..=w {
            match (x < w && col_inked(x), start) {
                (true, None) => start = Some(x),
                (false, Some(a)) => {
                    runs.push((a, x));
                    start = None;
                }
                _ => {}
            }
        }
        Some((patch, runs))
    }

    fn read_line(&self, view: &FrameView, style: TextStyle, rect: (i32, i32, i32, i32), min_ink: u8, centred: bool, digits_only: bool) -> Option<String> {
        let (patch, mut runs) = Self::patch_runs(view, rect, min_ink, centred)?;
        let (w, h) = (patch.width, patch.height);
        let col_inked = |x: usize| (0..h).any(|y| patch.ink[y * w + x] > 0.0);
        let full = self.style(style);
        let digits_style;
        let st = if digits_only {
            let glyphs: Vec<Glyph> = full.glyphs.iter().filter(|g| g.ch.is_ascii_digit()).map(Glyph::clone).collect();
            let max_width = glyphs.iter().map(|g| g.width).max().unwrap_or(0);
            digits_style = Style { space: full.space, glyphs, max_width };
            &digits_style
        } else {
            full
        };
        if centred && !runs.is_empty() {
            let mid = w / 2;
            let k = (0..runs.len()).min_by_key(|&i| runs[i].0.abs_diff(mid).min(runs[i].1.abs_diff(mid))).expect("runs");
            let gap = (3.0 * st.space) as usize;
            let mut lo = k;
            while lo > 0 && runs[lo].0 - runs[lo - 1].1 < gap {
                lo -= 1;
            }
            let mut hi = k;
            while hi + 1 < runs.len() && runs[hi + 1].0 - runs[hi].1 < gap {
                hi += 1;
            }
            runs = runs[lo..=hi].to_vec();
        }
        if runs.is_empty() || runs.len() > MAX_RUNS {
            return None;
        }
        let baseline = baseline_of(&patch, &runs);
        let mut text = String::new();
        let mut last_end: Option<usize> = None;
        for &(a, b) in &runs {
            // Digits have one advance, so a narrow "1" leaves a word-sized gap: numbers end at
            // the first run wider than a digit instead.
            if let Some(e) = last_end
                && !digits_only
                && (a - e) as f64 >= SPACE_SHARE * st.space
            {
                text.push(' ');
            }
            if digits_only && b - a > st.max_width + 1 {
                // Touching digits read well as digits; the "m" of "m/s" (x-height only) does not.
                let mut x = a as i32;
                let (mut chars, mut total) = (String::new(), 0.0f32);
                while x < b as i32 {
                    let remaining = b as i32 - x;
                    let (score, ch, width) = self.best_scored(st, &patch, x, remaining, baseline, remaining as usize > st.max_width + 1);
                    chars.push(ch);
                    total += score;
                    x += width.max(1) as i32;
                    while x < b as i32 && !col_inked(x as usize) {
                        x += 1;
                    }
                }
                if chars.is_empty() || total / (chars.len() as f32) < MIN_DIGIT_RUN_SCORE {
                    break;
                }
                text.push_str(&chars);
                continue;
            }
            last_end = Some(b);
            if b - a > st.max_width + 1 {
                let inked = |x: i32| x >= 0 && (x as usize) < w && col_inked(x as usize);
                let memo = std::cell::RefCell::new(std::collections::HashMap::new());
                text.extend(self.read_wide(st, &patch, a as i32, b as i32, baseline, &inked, 0, &memo));
            } else {
                text.extend(self.read_run(st, &patch, a as i32, b as i32, baseline));
            }
        }
        Some(text)
    }

    /// Which of `candidates` (whole strings: the names a card can show) the line in the box
    /// is. Each candidate's characters (spaces left out) are laid over the box's runs of ink in
    /// order, a run taking one glyph or two or three touching ones (`run_as`), and scored with
    /// those characters' templates only; a candidate scores the mean over its characters. No
    /// glyph is guessed among look-alikes ("3OF61" against "3OF01") and touching glyphs need no
    /// cut of their own. Best first: (candidate index, score); a candidate whose characters do
    /// not fit the runs is left out.
    pub fn match_line(&self, view: &FrameView, style: TextStyle, rect: (i32, i32, i32, i32), min_ink: u8, candidates: &[String]) -> Vec<(usize, f32)> {
        self.match_runs(view, style, rect, min_ink, candidates, 3.0, (MATCH_SPAN_NARROWER, 0.0))
    }

    /// As `match_line`, for a weapon label: its letters stand 2–3 px apart and its words 7–8 px
    /// (150 %), so the width estimate (`Style::width_of`: 1 px and a space) falls short of the ink
    /// by more than a card name's; with no cut-short variants among the candidates a candidate may
    /// be as much narrower as it may be wider.
    pub fn match_label(&self, view: &FrameView, style: TextStyle, rect: (i32, i32, i32, i32), min_ink: u8, candidates: &[String]) -> Vec<(usize, f32)> {
        self.match_runs(view, style, rect, min_ink, candidates, 3.0, MATCH_SPAN_WIDER)
    }

    /// Development: a candidate's estimated width (`Style::width_of`) and the ink span of the
    /// line in the box as `match_line` takes it.
    pub fn width_and_span(&self, view: &FrameView, style: TextStyle, rect: (i32, i32, i32, i32), min_ink: u8, text: &str) -> Option<(f64, f64)> {
        let (_, mut runs) = Self::patch_runs(view, rect, min_ink, false)?;
        let st = self.style(style);
        if let Some(end) = runs.windows(2).position(|p| (p[1].0 - p[0].1) as f64 >= 3.0 * st.space) {
            runs.truncate(end + 1);
        }
        let span = (runs.last()?.1 - runs.first()?.0) as f64;
        Some((st.width_of(text), span))
    }

    /// As `match_line`, for the line's first word only (the number of "259 m/s", whose unit and
    /// faint slash are left out): candidates are words.
    pub fn match_leading(&self, view: &FrameView, style: TextStyle, rect: (i32, i32, i32, i32), min_ink: u8, candidates: &[String]) -> Vec<(usize, f32)> {
        self.match_runs(view, style, rect, min_ink, candidates, SPACE_SHARE, (MATCH_SPAN_NARROWER, 0.0))
    }

    /// Candidates laid over the leftmost runs chained by gaps under `gap_spaces` spaces; a
    /// candidate's estimated width may fall short of the ink span by `narrower` (px, or that share
    /// of the span when more).
    #[allow(clippy::too_many_arguments)]
    fn match_runs(&self, view: &FrameView, style: TextStyle, rect: (i32, i32, i32, i32), min_ink: u8, candidates: &[String], gap_spaces: f64, narrower: (f64, f64)) -> Vec<(usize, f32)> {
        let Some((patch, mut runs)) = Self::patch_runs(view, rect, min_ink, false) else { return Vec::new() };
        let st = self.style(style);
        // The text: the leftmost runs chained by gaps under `gap_spaces` spaces (three for a name:
        // the M110 card's charge marks sit right of its name).
        if let Some(end) = runs.windows(2).position(|p| (p[1].0 - p[0].1) as f64 >= gap_spaces * st.space) {
            runs.truncate(end + 1);
        }
        if runs.is_empty() || runs.len() > MAX_RUNS {
            return Vec::new();
        }
        let baseline = baseline_of(&patch, &runs);
        let span = (runs[runs.len() - 1].1 - runs[0].0) as f64;
        let memo: RunMemo = std::cell::RefCell::new(std::collections::HashMap::new());
        let mut out = Vec::new();
        for (i, candidate) in candidates.iter().enumerate() {
            let chars: Vec<char> = candidate.chars().filter(|c| *c != ' ').collect();
            let n = chars.len();
            if n < runs.len() || n > MAX_RUN_GLYPHS * runs.len() || chars.iter().any(|c| !st.glyphs.iter().any(|g| g.ch == *c)) {
                continue;
            }
            // A candidate far longer or shorter than the ink is not it (most cut-short variants).
            let excess = st.width_of(candidate) - span;
            if excess < -narrower.0.max(narrower.1 * span) || excess > MATCH_SPAN_WIDER.0.max(MATCH_SPAN_WIDER.1 * span) {
                continue;
            }
            // Best total score with `j` characters laid over the runs so far.
            let mut best = vec![f32::NEG_INFINITY; n + 1];
            best[0] = 0.0;
            for &(a, b) in &runs {
                let mut next = vec![f32::NEG_INFINITY; n + 1];
                for j in 0..n {
                    if best[j] == f32::NEG_INFINITY {
                        continue;
                    }
                    for k in 1..=MAX_RUN_GLYPHS.min(n - j) {
                        let score = self.run_as(st, &patch, a as i32, b as i32, baseline, &chars[j..j + k], &memo);
                        if score >= MATCH_RUN_MIN_SCORE && best[j] + score * k as f32 > next[j + k] {
                            next[j + k] = best[j] + score * k as f32;
                        }
                    }
                }
                best = next;
            }
            if best[n] > f32::NEG_INFINITY {
                out.push((i, best[n] / n as f32));
            }
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }

    /// The run `a..b` as the glyphs `chars` (one to three) side by side: the best placement of
    /// their templates (the left one flush with the run's start, the right one with its end, each
    /// ±1 px up or down), `NEG_INFINITY` when no templates fit the run's width.
    fn run_as(&self, st: &Style, patch: &Patch, a: i32, b: i32, baseline: i32, chars: &[char], memo: &RunMemo) -> f32 {
        let key = (a, b, chars.iter().collect::<String>());
        if let Some(&score) = memo.borrow().get(&key) {
            return score;
        }
        let score = self.run_as_uncached(st, patch, a, b, baseline, chars, memo);
        memo.borrow_mut().insert(key, score);
        score
    }

    fn run_as_uncached(&self, st: &Style, patch: &Patch, a: i32, b: i32, baseline: i32, chars: &[char], memo: &RunMemo) -> f32 {
        let w = b - a;
        let of = |c: char| st.glyphs.iter().filter(move |g| g.ch == c);
        // Glyphs side by side span their widths, give or take the pair fill: skip what cannot fit.
        if chars.len() > 1 {
            let (lo, hi) = chars.iter().fold((0i32, 0i32), |(lo, hi), &c| {
                let ws = of(c).map(|g| g.width as i32);
                (lo + ws.clone().min().unwrap_or(0), hi + ws.max().unwrap_or(0))
            });
            let joints = chars.len() as i32 - 1;
            if w < lo - PAIR_FILL.1 * joints || w > hi - PAIR_FILL.0 * joints {
                return f32::NEG_INFINITY;
            }
        }
        let mut best = f32::NEG_INFINITY;
        match chars {
            [c] => {
                for g in of(*c).filter(|g| (g.width as i32 - w).abs() <= 3) {
                    let gw = g.width as i32;
                    for dx in -1..=1 {
                        for dy in -1..=1 {
                            let ox = a + (w - gw) / 2 + dx;
                            best = best.max(ncc(patch, g, ox, baseline + g.top + dy, ox.min(a), (ox + gw).max(b), baseline));
                        }
                    }
                }
            }
            [l, r] => {
                // Each side's best few templates flush with its end, then the pairs of those.
                let side = |c: char, left: bool| -> Vec<&Glyph> {
                    let scored = two_pass(of(c).filter(|g| (g.width as i32) <= w + PAIR_FILL.1), MATCH_SIDE_KEEP, |g, dx, dy| {
                        let gw = g.width as i32;
                        let ox = if left { a + dx } else { b - gw + dx };
                        ncc(patch, g, ox, baseline + g.top + dy, ox, ox + gw, baseline)
                    });
                    scored.into_iter().map(|(_, g)| g).collect()
                };
                let (lefts, rights) = (side(*l, true), side(*r, false));
                for &lg in &lefts {
                    for &rg in &rights {
                        let fill = (lg.width + rg.width) as i32 - w;
                        if !(PAIR_FILL.0..=PAIR_FILL.1).contains(&fill) {
                            continue;
                        }
                        for dy in -1..=1 {
                            best = best.max(ncc_pair(patch, lg, (a, baseline + lg.top + dy), rg, (b - rg.width as i32, baseline + rg.top + dy), a - 1, b + 1, baseline));
                        }
                    }
                }
            }
            [l, rest @ ..] => {
                // The left glyph flush with the start (its best few templates), the other two as
                // a pair over the rest.
                let lefts = two_pass(of(*l).filter(|g| (g.width as i32) < w - 2), MATCH_SIDE_KEEP, |g, dx, dy| ncc(patch, g, a + dx, baseline + g.top + dy, a + dx, a + dx + g.width as i32, baseline));
                for (_, lg) in lefts {
                    let gw = lg.width as i32;
                    let left = (-1..=1).map(|dy| ncc(patch, lg, a, baseline + lg.top + dy, a, a + gw, baseline)).fold(f32::NEG_INFINITY, f32::max);
                    for overlap in 0..=2 {
                        let pair = self.run_as(st, patch, a + gw - overlap, b, baseline, rest, memo);
                        if pair > f32::NEG_INFINITY {
                            best = best.max((left + pair * rest.len() as f32) / chars.len() as f32);
                        }
                    }
                }
            }
            [] => {}
        }
        best
    }

    /// One run read as a glyph, or as two touching glyphs when that fits better.
    fn read_run(&self, st: &Style, patch: &Patch, a: i32, b: i32, baseline: i32) -> Vec<char> {
        self.read_run_scored(st, patch, a, b, baseline).1
    }

    /// A run too wide for one glyph: a left glyph and the rest read as one or two glyphs, the
    /// split with the best mean score among the left glyph's best candidates. Only the outermost
    /// split tries several candidates (`depth` 0); deeper ones take the best (the search would
    /// grow exponentially on a long run of clutter).
    /// `memo`: the reading of the rest of the run from each start already read (deeper levels
    /// take one candidate, so it depends on the start only). Without it each level tries four
    /// overlaps of the next: a wide blob of scenery took 38 s.
    #[allow(clippy::too_many_arguments)]
    fn read_wide(&self, st: &Style, patch: &Patch, a: i32, b: i32, baseline: i32, inked: &dyn Fn(i32) -> bool, depth: u32, memo: &std::cell::RefCell<std::collections::HashMap<i32, Vec<char>>>) -> Vec<char> {
        if depth > MAX_WIDE_DEPTH {
            return vec!['?'];
        }
        let w = b - a;
        let mut candidates = two_pass(st.glyphs.iter().filter(|g| g.width >= MIN_PAIR_WIDTH && (g.width as i32) < w - 2), COARSE_KEEP_SIDE, |g, dx, dy| {
            let ox = a + dx;
            ncc(patch, g, ox, baseline + g.top + dy, ox, ox + g.width as i32, baseline)
        });
        let mut seen = Vec::new();
        candidates.retain(|(_, g)| {
            let new = !seen.contains(&g.ch);
            seen.push(g.ch);
            new
        });
        candidates.truncate(if depth == 0 { WIDE_CANDIDATES } else { 1 });
        let mut best: Option<(f32, Vec<char>)> = None;
        for &(score, g) in &candidates {
            for overlap in 0..=3 {
                let mut rest = a + g.width as i32 - overlap;
                while rest < b && !inked(rest) {
                    rest += 1;
                }
                if rest < a + 3 || b - rest < 3 {
                    continue;
                }
                let (rest_score, rest_chars) = if (b - rest) as usize > st.max_width + 1 {
                    let known = memo.borrow().get(&rest).cloned();
                    let chars = known.unwrap_or_else(|| {
                        let chars = self.read_wide(st, patch, rest, b, baseline, inked, depth + 1, memo);
                        memo.borrow_mut().insert(rest, chars.clone());
                        chars
                    });
                    (MIN_GLYPH_SCORE, chars)
                } else {
                    self.read_run_scored(st, patch, rest, b, baseline)
                };
                let mean = (score + rest_score * rest_chars.len() as f32) / (1 + rest_chars.len()) as f32;
                if best.as_ref().is_none_or(|(m, _)| mean > *m) {
                    let mut chars = vec![g.ch];
                    chars.extend(rest_chars);
                    best = Some((mean, chars));
                }
            }
        }
        best.map_or_else(|| vec!['?'], |(_, chars)| chars)
    }

    /// `read_run` with the mean score of its glyphs.
    fn read_run_scored(&self, st: &Style, patch: &Patch, a: i32, b: i32, baseline: i32) -> (f32, Vec<char>) {
        let w = b - a;
        let (single, ch) = self.whole(st, patch, a, w, baseline);
        let single_char = if single >= MIN_GLYPH_SCORE { ch } else { '?' };
        if single >= SPLIT_BELOW || w < 6 {
            return (single, vec![single_char]);
        }
        // Best placement of each template flush with the run's left or right end.
        let side = |left: bool| {
            let mut scored = two_pass(st.glyphs.iter().filter(|g| g.width >= MIN_PAIR_WIDTH && (g.width as i32) <= w - 2), COARSE_KEEP_SIDE, |g, dx, dy| {
                let gw = g.width as i32;
                let ox = if left { a + dx } else { b - gw + dx };
                ncc(patch, g, ox, baseline + g.top + dy, ox, ox + gw, baseline)
            });
            let mut seen = Vec::new();
            scored.retain(|(_, g)| {
                let new = !seen.contains(&(g.ch, g.width));
                seen.push((g.ch, g.width));
                new
            });
            scored.truncate(PAIR_CANDIDATES);
            scored
        };
        let (lefts, rights) = (side(true), side(false));
        // Each width-compatible pair scored as one picture over the run, like a single glyph.
        let mut pair: Option<(f32, char, char)> = None;
        for &(_, l) in &lefts {
            for &(_, r) in &rights {
                let fill = (l.width + r.width) as i32 - w;
                if !(PAIR_FILL.0..=PAIR_FILL.1).contains(&fill) {
                    continue;
                }
                let mut best = f32::MIN;
                for dy in -1..=1 {
                    best = best.max(ncc_pair(patch, l, (a, baseline + l.top + dy), r, (b - r.width as i32, baseline + r.top + dy), a - 1, b + 1, baseline));
                }
                if pair.is_none_or(|p| best > p.0) {
                    pair = Some((best, l.ch, r.ch));
                }
            }
        }
        match pair {
            Some((score, l, r)) if score > single + PAIR_MARGIN && score >= MIN_GLYPH_SCORE => (score, vec![l, r]),
            _ => (single, vec![single_char]),
        }
    }

    /// Best template spanning the run `x..x + w` and its score.
    fn whole(&self, st: &Style, patch: &Patch, x: i32, w: i32, baseline: i32) -> (f32, char) {
        let scored = two_pass(st.glyphs.iter().filter(|g| (g.width as i32 - w).abs() <= 2), COARSE_KEEP, |g, dx, dy| {
            let gw = g.width as i32;
            let ox = x + (w - gw) / 2 + dx;
            ncc(patch, g, ox, baseline + g.top + dy, ox.min(x), (ox + gw).max(x + w), baseline)
        });
        scored.first().map_or((f32::MIN, '?'), |&(score, g)| (score, g.ch))
    }

    /// Best template for the run starting at `x` (`remaining` columns to its end) and its score
    /// (`MIN_GLYPH_SCORE` and `?` when nothing fits). With `split` the template may cover only the
    /// run's start; otherwise it should span the run.
    fn best_scored(&self, st: &Style, patch: &Patch, x: i32, remaining: i32, baseline: i32, split: bool) -> (f32, char, usize) {
        let fits = |g: &&Glyph| {
            let gw = g.width as i32;
            if split { gw <= remaining + 1 } else { (gw - remaining).abs() <= 2 }
        };
        let place = |g: &Glyph, dx: i32| x + if split { dx } else { (remaining - g.width as i32) / 2 + dx };
        let mut best = (MIN_GLYPH_SCORE, '?', remaining as usize);
        for g in two_pass(st.glyphs.iter().filter(fits), COARSE_KEEP, |g, dx, dy| {
            let ox = place(g, dx);
            let gw = g.width as i32;
            let (wx0, wx1) = if split { (ox, ox + gw) } else { (ox.min(x), (ox + gw).max(x + remaining)) };
            ncc(patch, g, ox, baseline + g.top + dy, wx0, wx1, baseline)
        })
        .into_iter()
        .take(1)
        .map(|(_, g)| g)
        {
            // The winner's best placement, for its width.
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let ox = place(g, dx);
                    let gw = g.width as i32;
                    let (wx0, wx1) = if split { (ox, ox + gw) } else { (ox.min(x), (ox + gw).max(x + remaining)) };
                    let score = ncc(patch, g, ox, baseline + g.top + dy, wx0, wx1, baseline);
                    if score > best.0 {
                        best = (score, g.ch, if split { (ox + gw - x).max(1) as usize } else { remaining as usize });
                    }
                }
            }
        }
        best
    }
}

/// The baseline of a box: the most common bottom row of its runs.
fn baseline_of(patch: &Patch, runs: &[(usize, usize)]) -> i32 {
    let (w, h) = (patch.width, patch.height);
    let mut bottoms = vec![0u32; h + 1];
    for &(a, b) in runs {
        let bottom = (0..h).rev().find(|&y| (a..b).any(|x| patch.ink[y * w + x] > 0.0)).map_or(0, |y| y + 1);
        bottoms[bottom] += 1;
    }
    (0..=h).max_by_key(|&y| (bottoms[y], y)).unwrap_or(h) as i32
}

/// `run_as` results of one line: (run start, end, glyphs) → score.
type RunMemo = std::cell::RefCell<std::collections::HashMap<(i32, i32, String), f32>>;

/// Scores glyphs in two passes: `score(glyph, dx, dy)` at the centred placement for all, then
/// the best `keep` over ±1 px; best first.
fn two_pass<'a>(glyphs: impl Iterator<Item = &'a Glyph>, keep: usize, score: impl Fn(&Glyph, i32, i32) -> f32) -> Vec<(f32, &'a Glyph)> {
    let mut coarse: Vec<(f32, &Glyph)> = glyphs.map(|g| (score(g, 0, 0), g)).collect();
    coarse.sort_by(|a, b| b.0.total_cmp(&a.0));
    coarse.truncate(keep);
    let mut fine: Vec<(f32, &Glyph)> = coarse
        .into_iter()
        .map(|(_, g)| {
            let mut best = f32::MIN;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    best = best.max(score(g, dx, dy));
                }
            }
            (best, g)
        })
        .collect();
    fine.sort_by(|a, b| b.0.total_cmp(&a.0));
    fine
}

/// Like `ncc`, for two glyphs placed together (the brighter template pixel where they meet).
#[allow(clippy::too_many_arguments)]
fn ncc_pair(patch: &Patch, l: &Glyph, lo: (i32, i32), r: &Glyph, ro: (i32, i32), wx0: i32, wx1: i32, baseline: i32) -> f32 {
    let wy0 = lo.1.min(ro.1).min(baseline - 14);
    let wy1 = (lo.1 + l.height as i32).max(ro.1 + r.height as i32).max(baseline + 4);
    let value = |g: &Glyph, o: (i32, i32), x: i32, y: i32| {
        let (gx, gy) = (x - o.0, y - o.1);
        if gx >= 0 && gy >= 0 && (gx as usize) < g.width && (gy as usize) < g.height {
            let v = g.pixels[gy as usize * g.width + gx as usize];
            if v >= MIN_TEMPLATE_INK { v } else { 0.0 }
        } else {
            0.0
        }
    };
    let (mut sa, mut sb, mut saa, mut sbb, mut sab, mut n) = (0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
    for y in wy0..wy1 {
        for x in wx0..wx1 {
            let a = patch.at(x, y);
            let b = value(l, lo, x, y).max(value(r, ro, x, y));
            sa += a;
            sb += b;
            saa += a * a;
            sbb += b * b;
            sab += a * b;
            n += 1.0;
        }
    }
    let cov = sab - sa * sb / n;
    let (va, vb) = (saa - sa * sa / n, sbb - sb * sb / n);
    if va <= 0.0 || vb <= 0.0 { 0.0 } else { cov / (va * vb).sqrt() }
}

/// Normalised correlation of glyph `g` placed at (`ox`, `oy`) with the patch, over columns
/// `wx0..wx1` and the rows from the higher of the glyph top and 14 px above the baseline down to
/// 4 px below it.
fn ncc(patch: &Patch, g: &Glyph, ox: i32, oy: i32, wx0: i32, wx1: i32, baseline: i32) -> f32 {
    let wy0 = oy.min(baseline - 14);
    let wy1 = (oy + g.height as i32).max(baseline + 4);
    let (mut sa, mut sb, mut saa, mut sbb, mut sab, mut n) = (0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
    for y in wy0..wy1 {
        for x in wx0..wx1 {
            let a = patch.at(x, y);
            let (gx, gy) = (x - ox, y - oy);
            let b = if gx >= 0 && gy >= 0 && (gx as usize) < g.width && (gy as usize) < g.height {
                let v = g.pixels[gy as usize * g.width + gx as usize];
                if v >= MIN_TEMPLATE_INK { v } else { 0.0 }
            } else {
                0.0
            };
            sa += a;
            sb += b;
            saa += a * a;
            sbb += b * b;
            sab += a * b;
            n += 1.0;
        }
    }
    if n == 0.0 {
        return 0.0;
    }
    let cov = sab - sa * sb / n;
    let va = saa - sa * sa / n;
    let vb = sbb - sb * sb / n;
    if va <= 0.0 || vb <= 0.0 { 0.0 } else { cov / (va * vb).sqrt() }
}

/// Whether two read characters can be the same glyph (shapes this reader confuses).
pub fn same_glyph(a: char, b: char) -> bool {
    let class = |c: char| match c {
        '5' | 'S' | 's' => 'S',
        '1' | 'I' | 'l' | 'i' | '|' => 'I',
        '0' | 'O' | 'o' => 'O',
        '8' | 'B' => 'B',
        '2' | 'Z' | 'z' => 'Z',
        c => c,
    };
    class(a) == class(b)
}

/// The text read as a number's digits: look-alike letters taken as digits.
pub fn digits(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'S' | 's' => '5',
            'I' | 'l' | 'i' | '|' => '1',
            'O' | 'o' => '0',
            'B' => '8',
            'Z' | 'z' => '2',
            c => c,
        })
        .collect()
}
