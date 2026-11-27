//! The seat menu in the bottom-left corner: which seats the vehicle has, which are taken, and
//! which one is the player's own.
//!
//! Each seat is a row: a round icon (steering wheel: driver, cross-hair: gunner, cap: commander,
//! three shells: loader, seat: passenger) and the occupant's name or "Empty". The rows stack up
//! from the bottom-left corner, beside whatever the seat shows there (the gunner's weapon and
//! TRT/BRC/GUN buttons, nothing for the loader, the speedometer while driving, where the menu is
//! drawn 0.945 the size). A passenger group (overlapping seat icons and a count) is not a row.
//!
//! - Rows: grey template matching of the five icon glyphs (`assets/seat_glyphs.json`, cut from
//!   recordings), first on a 4× reduced image for candidates, then at full
//!   resolution around them; the rows form one column, and the passenger group (icons side by
//!   side 18–34 px apart) is dropped.
//! - Taken: a taken seat's ring is drawn in colour (green, or a part-filled arc going red, yellow,
//!   green), an empty one's is grey. On the 348 labelled frames every taken row had at least 21%
//!   of its ring coloured and every empty one none.
//! - Own seat: by the player's name, compared as an image (high-passed grey, so a bright
//!   background does not wash it out) with the name in the row the HUD knows is the player's
//!   (`SeatReader::learn`, when the ammo bar shows the gunner's or loader's layout). The name is
//!   kept in memory only; nothing about it is written anywhere. Labelled frames: own rows 0.80
//!   and above, other players' names 0.23 and below.

use serde::Deserialize;

use crate::FrameView;

/// The searched corner, client px from the bottom-left (2560×1334 client; the tallest menu seen,
/// six rows beside the speedometer, starts 320 px above the bottom).
pub const REGION_WIDTH: u32 = 900;
pub const REGION_HEIGHT: u32 = 468;

/// Icon ring radius in the usual layout (px).
const RING_RADIUS: f32 = 23.0;
/// Full-resolution match score a row needs, and that at least one row of the column needs.
const ROW_MIN: f32 = 0.6;
const COLUMN_MIN: f32 = 0.8;
/// Reduced-image score that makes a candidate.
const COARSE_MIN: f32 = 0.55;
const COARSE: usize = 4;
/// Grey a candidate's middle must reach somewhere (the white glyph).
const GLYPH_MIN: f32 = 200.0;
/// Share of the ring drawn in colour that makes a seat taken.
const TAKEN_MIN: f32 = 0.08;
/// Name comparison score that makes a row the player's own.
const OWN_MIN: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeatKind {
    Driver,
    Gunner,
    Commander,
    Loader,
    Passenger,
}

impl SeatKind {
    fn from_name(name: &str) -> Self {
        match name {
            "driver" => Self::Driver,
            "gunner" => Self::Gunner,
            "commander" => Self::Commander,
            "loader" => Self::Loader,
            "passenger" => Self::Passenger,
            other => panic!("seat glyph asset: unknown kind {other}"),
        }
    }
}

/// The menu's size: the usual one, or the smaller one beside the speedometer while driving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuLayout {
    Usual,
    Driving,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeatRow {
    pub kind: SeatKind,
    /// Icon centre, frame px.
    pub center: (f32, f32),
    pub taken: bool,
    /// The player's own seat (only when the name is known, `SeatMenu::own_known`).
    pub mine: bool,
    pub score: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeatMenu {
    pub layout: MenuLayout,
    /// Top to bottom.
    pub rows: Vec<SeatRow>,
    /// Whether the player's name is known, so that `mine` tells.
    pub own_known: bool,
}

impl SeatMenu {
    /// The player's own seat, when the name is known and found.
    pub fn mine(&self) -> Option<SeatKind> {
        self.rows.iter().find(|r| r.mine).map(|r| r.kind)
    }

    /// Whether someone other than the player sits in a seat of `kind`.
    pub fn taken_by_other(&self, kind: SeatKind) -> bool {
        self.rows.iter().any(|r| r.kind == kind && r.taken && !r.mine)
    }

    /// The menu's size relative to the usual one.
    pub fn scale(&self) -> f32 {
        layout_scale(self.layout)
    }
}

fn layout_scale(layout: MenuLayout) -> f32 {
    match layout {
        MenuLayout::Usual => 1.0,
        MenuLayout::Driving => 0.945,
    }
}

/// A zero-mean grey template.
#[derive(Clone)]
struct Template {
    w: usize,
    h: usize,
    data: Vec<f32>,
    norm: f32,
}

impl Template {
    fn new(w: usize, h: usize, mut data: Vec<f32>) -> Self {
        let mean = data.iter().sum::<f32>() / data.len() as f32;
        data.iter_mut().for_each(|v| *v -= mean);
        let norm = data.iter().map(|v| v * v).sum::<f32>().sqrt();
        Self { w, h, data, norm }
    }

    /// Box-reduced by `COARSE` (cells clipped at the edge).
    fn reduced(&self) -> Self {
        let (w, h) = (self.w.div_ceil(COARSE), self.h.div_ceil(COARSE));
        let mut out = vec![0.0; w * h];
        for (y, row) in out.chunks_mut(w).enumerate() {
            for (x, v) in row.iter_mut().enumerate() {
                let (mut sum, mut n) = (0.0, 0.0);
                for sy in y * COARSE..((y + 1) * COARSE).min(self.h) {
                    for sx in x * COARSE..((x + 1) * COARSE).min(self.w) {
                        sum += self.data[sy * self.w + sx];
                        n += 1.0;
                    }
                }
                *v = sum / n;
            }
        }
        Self::new(w, h, out)
    }

    /// Bilinearly resized by `f`.
    fn resized(&self, f: f32) -> Self {
        let (w, h) = (((self.w as f32) * f).round() as usize, ((self.h as f32) * f).round() as usize);
        let mut out = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let sx = ((x as f32 + 0.5) / f - 0.5).clamp(0.0, (self.w - 1) as f32);
                let sy = ((y as f32 + 0.5) / f - 0.5).clamp(0.0, (self.h - 1) as f32);
                let (x0, y0) = (sx as usize, sy as usize);
                let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
                let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
                let at = |xx: usize, yy: usize| self.data[yy * self.w + xx];
                out[y * w + x] = (at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx) * (1.0 - fy) + (at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx) * fy;
            }
        }
        Self::new(w, h, out)
    }
}

/// A grey image for windowed normalised correlation.
struct Gray {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

impl Gray {
    fn new(w: usize, h: usize, px: Vec<f32>) -> Self {
        Self { w, h, px }
    }

    fn reduced(&self) -> Self {
        let (w, h) = (self.w / COARSE, self.h / COARSE);
        let mut px = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut s = 0.0;
                for sy in y * COARSE..(y + 1) * COARSE {
                    for sx in x * COARSE..(x + 1) * COARSE {
                        s += self.px[sy * self.w + sx];
                    }
                }
                px[y * w + x] = s / (COARSE * COARSE) as f32;
            }
        }
        Self::new(w, h, px)
    }

    /// The best (kind, score, x, y) of same-sized templates over the windows with top-left in
    /// the given ranges; each window's mean and spread are worked out once for all templates.
    fn best_of(&self, templates: &[(SeatKind, Template)], (x0, x1): (usize, usize), (y0, y1): (usize, usize)) -> Option<(SeatKind, f32, usize, usize)> {
        let size = templates.first()?.1.w;
        let n = (size * size) as f32;
        // Sums over the patch the windows cover.
        let (pw, ph) = (x1 - x0 + size, y1 - y0 + size);
        let (mut sum, mut sq) = (vec![0.0f32; (pw + 1) * (ph + 1)], vec![0.0f32; (pw + 1) * (ph + 1)]);
        for y in 0..ph {
            let (mut rs, mut rq) = (0.0, 0.0);
            for x in 0..pw {
                let v = self.px[(y0 + y) * self.w + x0 + x];
                rs += v;
                rq += v * v;
                sum[(y + 1) * (pw + 1) + x + 1] = sum[y * (pw + 1) + x + 1] + rs;
                sq[(y + 1) * (pw + 1) + x + 1] = sq[y * (pw + 1) + x + 1] + rq;
            }
        }
        let window = |t: &[f32], x: usize, y: usize| t[(y + size) * (pw + 1) + x + size] + t[y * (pw + 1) + x] - t[y * (pw + 1) + x + size] - t[(y + size) * (pw + 1) + x];
        let mut best: Option<(SeatKind, f32, usize, usize)> = None;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (s, q) = (window(&sum, x - x0, y - y0), window(&sq, x - x0, y - y0));
                let sd = (q - s * s / n).max(0.0).sqrt();
                if sd <= 1e-3 {
                    continue;
                }
                for (kind, t) in templates {
                    let mut num = 0.0f32;
                    for ty in 0..size {
                        let row = &self.px[(y + ty) * self.w + x..(y + ty) * self.w + x + size];
                        num += row.iter().zip(&t.data[ty * size..(ty + 1) * size]).map(|(a, b)| a * b).sum::<f32>();
                    }
                    let score = num / (sd * t.norm);
                    if best.is_none_or(|b| score > b.1) {
                        best = Some((*kind, score, x, y));
                    }
                }
            }
        }
        best
    }

    /// Normalised correlation of `t` with the window at (x, y), -1..1.
    fn ncc(&self, t: &Template, x: usize, y: usize) -> f32 {
        let (mut num, mut s, mut q) = (0.0f32, 0.0f32, 0.0f32);
        for ty in 0..t.h {
            let row = &self.px[(y + ty) * self.w + x..(y + ty) * self.w + x + t.w];
            let trow = &t.data[ty * t.w..(ty + 1) * t.w];
            for (a, b) in row.iter().zip(trow) {
                num += a * b;
                s += a;
                q += a * a;
            }
        }
        let var = q - s * s / (t.w * t.h) as f32;
        if var <= 1e-3 || t.norm <= 1e-6 {
            return 0.0;
        }
        num / (var.sqrt() * t.norm)
    }
}

struct LayoutGlyphs {
    layout: MenuLayout,
    size: usize,
    full: Vec<(SeatKind, Template)>,
    /// The glyphs reduced, for finding candidates.
    coarse: Vec<Template>,
}

/// The player's name as last seen in the seat menu: high-passed grey, trimmed to the text.
struct OwnName {
    template: Template,
    scale: f32,
}

pub struct SeatReader {
    layouts: Vec<LayoutGlyphs>,
    own: Option<OwnName>,
}

#[derive(Deserialize)]
struct Asset {
    layouts: std::collections::BTreeMap<String, AssetLayout>,
}

#[derive(Deserialize)]
struct AssetLayout {
    size: usize,
    kinds: std::collections::BTreeMap<String, Vec<u8>>,
}

impl Default for SeatReader {
    fn default() -> Self {
        Self::new()
    }
}

/// One candidate's best match in one layout.
#[derive(Clone, Copy)]
struct Hit {
    kind: SeatKind,
    score: f32,
    center: (f32, f32),
}

impl SeatReader {
    pub fn new() -> Self {
        let asset: Asset = serde_json::from_str(include_str!("../assets/seat_glyphs.json")).expect("seat glyph asset");
        let layouts = asset
            .layouts
            .into_iter()
            .map(|(name, l)| {
                let layout = match name.as_str() {
                    "full" => MenuLayout::Usual,
                    "driving" => MenuLayout::Driving,
                    other => panic!("seat glyph asset: unknown layout {other}"),
                };
                let full: Vec<(SeatKind, Template)> =
                    l.kinds.iter().map(|(k, px)| (SeatKind::from_name(k), Template::new(l.size, l.size, px.iter().map(|&v| f32::from(v)).collect()))).collect();
                // Reduced 4×, the two sizes' glyphs are alike: candidates come from the usual ones.
                let coarse = if layout == MenuLayout::Usual { full.iter().map(|(_, t)| t.reduced()).collect() } else { Vec::new() };
                LayoutGlyphs { layout, size: l.size, full, coarse }
            })
            .collect();
        Self { layouts, own: None }
    }

    /// The frame region to read (x, y, width, height): the client's bottom-left corner.
    pub fn region(client: (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
        let (x, y, w, h) = client;
        (x, y + h as i32 - REGION_HEIGHT as i32, REGION_WIDTH.min(w), REGION_HEIGHT.min(h))
    }

    /// Whether the player's name is known.
    pub fn own_known(&self) -> bool {
        self.own.is_some()
    }

    /// Reads the seat menu; `None` when none is shown (map views, menus, on foot away from a
    /// vehicle). `client` is the client rectangle in frame px; the view covers `region(client)`.
    pub fn read(&self, view: &FrameView, client: (i32, i32, u32, u32)) -> Option<SeatMenu> {
        let (rx, ry, rw, rh) = Self::region(client);
        // The client and the frame disagree for a frame or two while the display configuration
        // changes (the region already follows the new one): nothing to read then.
        if !view.covers(rx, ry, rw as i32, rh as i32) {
            return None;
        }
        let gray = gray_of(view, rx, ry, rw as usize, rh as usize);
        let coarse = gray.reduced();

        // Candidates: local maxima of the best reduced-image score over every glyph, among the
        // windows whose middle (the glyph, drawn white) has a near-white pixel.
        let (cw, ch) = (coarse.w, coarse.h);
        let mut white = vec![0u32; (cw + 1) * (ch + 1)];
        for y in 0..ch {
            let mut run = 0;
            for x in 0..cw {
                let block = (y * COARSE..(y + 1) * COARSE).any(|yy| gray.px[yy * gray.w + x * COARSE..yy * gray.w + (x + 1) * COARSE].iter().any(|&v| v >= GLYPH_MIN));
                run += u32::from(block);
                white[(y + 1) * (cw + 1) + x + 1] = white[y * (cw + 1) + x + 1] + run;
            }
        }
        let white_in = |x: usize, y: usize, w: usize, h: usize| {
            let s = cw + 1;
            white[(y + h) * s + x + w] + white[y * s + x] - white[y * s + x + w] - white[(y + h) * s + x]
        };
        let mut best = vec![f32::MIN; cw * ch];
        for l in &self.layouts {
            for t in &l.coarse {
                if t.w > cw || t.h > ch {
                    continue;
                }
                for y in 0..=ch - t.h {
                    for x in 0..=cw - t.w {
                        if white_in(x + 2, y + 2, t.w - 4, t.h - 4) == 0 {
                            continue;
                        }
                        let s = coarse.ncc(t, x, y);
                        let i = y * cw + x;
                        if s > best[i] {
                            best[i] = s;
                        }
                    }
                }
            }
        }
        let mut candidates = Vec::new();
        for y in 0..ch {
            for x in 0..cw {
                let s = best[y * cw + x];
                if s < COARSE_MIN {
                    continue;
                }
                let local_max = (y.saturating_sub(1)..=(y + 1).min(ch - 1)).all(|yy| (x.saturating_sub(1)..=(x + 1).min(cw - 1)).all(|xx| best[yy * cw + xx] <= s));
                if local_max {
                    // Top-left of the reduced window, in region px.
                    candidates.push((x * COARSE, y * COARSE));
                }
            }
        }

        // Each candidate's best glyph per layout, at full resolution within ±3 px.
        let mut per_layout: Vec<Vec<Hit>> = vec![Vec::new(); self.layouts.len()];
        for &(cx, cy) in &candidates {
            for (li, l) in self.layouts.iter().enumerate() {
                let (x0, y0) = (cx.saturating_sub(3), cy.saturating_sub(3));
                let (x1, y1) = ((cx + 3).min(gray.w - l.size), (cy + 3).min(gray.h - l.size));
                if x0 > x1 || y0 > y1 {
                    continue;
                }
                if let Some((kind, score, x, y)) = gray.best_of(&l.full, (x0, x1), (y0, y1)).filter(|h| h.1 >= ROW_MIN) {
                    let half = l.size as f32 / 2.0;
                    per_layout[li].push(Hit { kind, score, center: (rx as f32 + x as f32 + half, ry as f32 + y as f32 + half) });
                }
            }
        }

        // One column per layout; the layout whose column matches best.
        let (li, column) = per_layout
            .into_iter()
            .enumerate()
            .map(|(li, hits)| (li, column(hits)))
            .max_by(|a, b| total(&a.1).total_cmp(&total(&b.1)))?;
        if !column.iter().any(|h| h.score >= COLUMN_MIN) {
            return None;
        }
        let layout = self.layouts[li].layout;
        let scale = layout_scale(layout);
        let mut rows: Vec<SeatRow> = column
            .into_iter()
            .map(|h| SeatRow { kind: h.kind, center: h.center, taken: ring_coloured(view, h.center, RING_RADIUS * scale) >= TAKEN_MIN, mine: false, score: h.score })
            .collect();
        rows.sort_by(|a, b| a.center.1.total_cmp(&b.center.1));
        let mut menu = SeatMenu { layout, rows, own_known: false };
        self.mark_own(view, &mut menu);
        Some(menu)
    }

    /// Marks the player's own row in `menu` by the remembered name (none when not known).
    pub fn mark_own(&self, view: &FrameView, menu: &mut SeatMenu) {
        menu.rows.iter_mut().for_each(|r| r.mine = false);
        menu.own_known = self.own.is_some();
        let Some(own) = &self.own else { return };
        let scale = menu.scale();
        let best = menu.rows.iter().enumerate().filter(|(_, r)| r.taken).map(|(i, r)| (i, name_score(view, r.center, scale, own))).max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, s)) = best
            && s >= OWN_MIN
        {
            menu.rows[i].mine = true;
        }
    }

    /// Remembers the player's name from the taken row of `kind` (the seat the HUD knows the
    /// player is in); `false` when there is not exactly one such row. In memory only.
    pub fn learn(&mut self, view: &FrameView, menu: &SeatMenu, kind: SeatKind) -> bool {
        let mut taken = menu.rows.iter().filter(|r| r.kind == kind && r.taken);
        let (Some(row), None) = (taken.next(), taken.next()) else {
            return false;
        };
        let s = menu.scale();
        let (x0, y0) = ((row.center.0 + 38.0 * s).round() as i32, (row.center.1 - 15.0 * s).round() as i32);
        let (w, h) = ((180.0 * s).round() as i32, (30.0 * s).round() as i32);
        let Some(hp) = high_pass(view, x0, y0, w, h) else { return false };
        // Trim to the text: columns with a stroke edge in them.
        let col_energy: Vec<f32> = (0..w as usize).map(|x| (0..h as usize).map(|y| hp[y * w as usize + x].abs()).fold(0.0, f32::max)).collect();
        let text: Vec<usize> = (0..col_energy.len()).filter(|&x| col_energy[x] > 30.0).collect();
        let (Some(&first), Some(&last)) = (text.first(), text.last()) else { return false };
        let (a, b) = (first.saturating_sub(3), (last + 4).min(w as usize));
        let tw = b - a;
        let data: Vec<f32> = (0..h as usize).flat_map(|y| (a..b).map(move |x| (y, x))).map(|(y, x)| hp[y * w as usize + x]).collect();
        self.own = Some(OwnName { template: Template::new(tw, h as usize, data), scale: s });
        true
    }

    /// Forgets the player's name.
    pub fn forget(&mut self) {
        self.own = None;
    }
}

fn total(hits: &[Hit]) -> f32 {
    hits.iter().map(|h| h.score).sum()
}

/// The hits that form the menu: duplicates merged, the passenger group dropped, then the
/// column (shared x) with the best total score.
fn column(mut hits: Vec<Hit>) -> Vec<Hit> {
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Hit> = Vec::new();
    for h in hits {
        if kept.iter().all(|k| (k.center.0 - h.center.0).abs() > 20.0 || (k.center.1 - h.center.1).abs() > 20.0) {
            kept.push(h);
        }
    }
    let group: Vec<bool> = kept
        .iter()
        .map(|a| kept.iter().any(|b| !std::ptr::eq(a, b) && (a.center.1 - b.center.1).abs() <= 4.0 && (18.0..=34.0).contains(&(a.center.0 - b.center.0).abs())))
        .collect();
    let rows: Vec<Hit> = kept.into_iter().zip(group).filter(|(_, g)| !g).map(|(h, _)| h).collect();
    let mut columns: Vec<Vec<Hit>> = Vec::new();
    for h in rows {
        match columns.iter_mut().find(|c| (c[0].center.0 - h.center.0).abs() <= 5.0) {
            Some(c) => c.push(h),
            None => columns.push(vec![h]),
        }
    }
    columns.into_iter().max_by(|a, b| total(a).total_cmp(&total(b))).unwrap_or_default()
}

/// BT.601 grey of a frame rectangle.
fn gray_of(view: &FrameView, x0: i32, y0: i32, w: usize, h: usize) -> Gray {
    let mut px = Vec::with_capacity(w * h);
    let bytes = match view.format {
        crate::PixelFormat::Rgb => 3,
        crate::PixelFormat::Bgra | crate::PixelFormat::BgraPeak => 4,
    };
    let (r, b) = match view.format {
        crate::PixelFormat::Rgb => (0, 2),
        crate::PixelFormat::Bgra | crate::PixelFormat::BgraPeak => (2, 0),
    };
    for y in 0..h {
        let start = (y0 - view.origin.1 + y as i32) as usize * view.stride + (x0 - view.origin.0) as usize * bytes;
        let row = &view.data[start..start + w * bytes];
        px.extend(row.chunks_exact(bytes).map(|p| 0.299 * f32::from(p[r]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[b])));
    }
    Gray::new(w, h, px)
}

/// Share of the icon ring drawn in colour (any hue: green, yellow, red), sampled on three
/// circles around the ring radius.
fn ring_coloured(view: &FrameView, center: (f32, f32), radius: f32) -> f32 {
    let (mut coloured, mut n) = (0, 0);
    for dr in [-0.7f32, 0.0, 0.7] {
        for i in 0..96 {
            let a = i as f32 * std::f32::consts::TAU / 96.0;
            let x = (center.0 + (radius + dr) * a.cos()).round() as i32;
            let y = (center.1 + (radius + dr) * a.sin()).round() as i32;
            if !view.covers(x, y, 1, 1) {
                continue;
            }
            let [r, g, b] = view.px(x, y);
            let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
            n += 1;
            if mx > 140 && mx - mn > 70 {
                coloured += 1;
            }
        }
    }
    if n == 0 { 0.0 } else { coloured as f32 / n as f32 }
}

/// Grey minus its Gaussian blur (sigma 3 px) over a frame rectangle, row-major; `None` when the
/// rectangle is outside the view.
fn high_pass(view: &FrameView, x0: i32, y0: i32, w: i32, h: i32) -> Option<Vec<f32>> {
    const R: i32 = 9;
    let kernel: Vec<f32> = {
        let k: Vec<f32> = (-R..=R).map(|i| (-(i * i) as f32 / 18.0).exp()).collect();
        let s: f32 = k.iter().sum();
        k.into_iter().map(|v| v / s).collect()
    };
    if !view.covers(x0, y0, w, h) {
        return None;
    }
    // Grey with a margin of R, clamped to the view.
    let (pw, ph) = (w + 2 * R, h + 2 * R);
    let (vx0, vy0) = view.origin;
    let (vx1, vy1) = (vx0 + view.width as i32 - 1, vy0 + view.height as i32 - 1);
    let mut g = vec![0.0f32; (pw * ph) as usize];
    for y in 0..ph {
        for x in 0..pw {
            let fx = (x0 - R + x).clamp(vx0, vx1);
            let fy = (y0 - R + y).clamp(vy0, vy1);
            g[(y * pw + x) as usize] = view.luma(fx, fy);
        }
    }
    let mut tmp = vec![0.0f32; (pw * ph) as usize];
    for y in 0..ph {
        for x in R..pw - R {
            tmp[(y * pw + x) as usize] = (-R..=R).map(|k| g[(y * pw + x + k) as usize] * kernel[(k + R) as usize]).sum();
        }
    }
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in R..ph - R {
        for x in R..pw - R {
            let blur: f32 = (-R..=R).map(|k| tmp[((y + k) * pw + x) as usize] * kernel[(k + R) as usize]).sum();
            out.push(g[(y * pw + x) as usize] - blur);
        }
    }
    Some(out)
}

/// How well the name right of a row's icon matches the player's (best over a small window and,
/// across the two layouts, a little scale search).
fn name_score(view: &FrameView, center: (f32, f32), scale: f32, own: &OwnName) -> f32 {
    let ratio = scale / own.scale;
    let factors: &[f32] = if (ratio - 1.0).abs() < 1e-3 { &[1.0] } else { &[0.985, 1.0, 1.015] };
    let mut best = -1.0f32;
    for &f in factors {
        let t = if (ratio * f - 1.0).abs() < 1e-3 { own.template.clone() } else { own.template.resized(ratio * f) };
        let (x0, y0) = ((center.0 + 30.0 * scale).round() as i32, (center.1 - 20.0 * scale).round() as i32);
        let (ww, wh) = (t.w as i32 + 16, t.h as i32 + 10);
        let Some(hp) = high_pass(view, x0, y0, ww, wh) else { continue };
        let win = Gray::new(ww as usize, wh as usize, hp);
        for y in 0..=(wh as usize - t.h) {
            for x in 0..=(ww as usize - t.w) {
                best = best.max(win.ncc(&t, x, y));
            }
        }
    }
    best
}

#[cfg(test)]
mod region_tests {
    use super::*;

    /// A client rectangle reaching past the frame (the display configuration changing) reads
    /// nothing instead of indexing past the pixels.
    #[test]
    fn a_region_past_the_frame_reads_nothing() {
        let (w, h) = (1280u32, 720u32);
        let data = vec![0u8; (w * h * 3) as usize];
        let view = FrameView::rgb(&data, w, h);
        assert!(SeatReader::new().read(&view, (0, 34, 1280, 1316)).is_none());
    }
}
