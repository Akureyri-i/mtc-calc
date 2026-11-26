//! How the game lays its UI out at one display configuration (client size and Windows display
//! scaling), from rules measured on the game: every recognizer works in the
//! reference geometry (2560 x 1334 at 150 %), and a configuration maps each element group onto it
//! by a similarity, with the text scales the recognizers need there. Nothing here is searched at
//! run time.

use crate::aim::TextScale;
use crate::map::geometry::{MapTransform, MapView};
use crate::view::{FrameView, PixelFormat};

/// A display configuration: the client area (physical px) and the window's DPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiLayout {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
}

/// Reference client coordinates to this configuration's client coordinates: `p' = s p + t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Similarity {
    pub s: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Similarity {
    pub const IDENTITY: Self = Self { s: 1.0, tx: 0.0, ty: 0.0 };

    pub fn to_client(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (self.s * x + self.tx, self.s * y + self.ty)
    }

    pub fn to_reference(&self, (x, y): (f64, f64)) -> (f64, f64) {
        ((x - self.tx) / self.s, (y - self.ty) / self.s)
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }
}

/// The ammo bar's width at the reference (0.1446 x 1334 + 240 x 1.5): the HUD scale's unit.
const HUD_UNIT: f64 = 552.9;
/// The ammo bar's bottom edge in the reference client (its margin there is -1 + 8 d = 11 px).
const BAR_BOTTOM: f64 = 1323.0;
/// The readout label's on-screen horizontal and vertical scale against 150 % on 2560 x 1334, by
/// label font size 8-13: the readout reader's best label scale on window-height sweeps,
/// averaged per size. Sizes beyond follow the ratio
/// of the rendered "Elevation" widths (Montserrat Regular: 13 → 60 px, 14 → 62, 15 → 66, 16 → 71,
/// 17 → 76, 18 → 81, 19 → 84, 20 → 89).
const LABEL_SCALE: [(u32, f64, f64); 6] = [(8, 0.585, 0.560), (9, 0.671, 0.762), (10, 0.755, 0.745), (11, 0.786, 0.890), (12, 0.870, 0.901), (13, 0.991, 1.013)];
const LABEL_WIDTHS_BEYOND: [(u32, f64); 7] = [(14, 62.0), (15, 66.0), (16, 71.0), (17, 76.0), (18, 81.0), (19, 84.0), (20, 89.0)];
/// The digits' on-screen scale is this times the label's unrounded size over 13 (0.89-0.93 read
/// on all 46 sweep sizes, both axes).
const DIGIT_SCALE: f64 = 0.91;

impl UiLayout {
    pub const REFERENCE: Self = Self { width: 2560, height: 1334, dpi: 144 };

    /// The Windows display scaling (1 at 100 %).
    pub fn d(&self) -> f64 {
        f64::from(self.dpi) / 96.0
    }

    pub fn is_reference(&self) -> bool {
        *self == Self::REFERENCE
    }

    /// The game HUD's scale against the reference: it follows the client height and the display
    /// scaling, not the width (ammo bar width 0.1446 H + 240 d).
    pub fn hud_scale(&self) -> f64 {
        (0.1446 * f64::from(self.height) + 240.0 * self.d()) / HUD_UNIT
    }

    /// The bottom group (readout, ammo bar, loading cards, weapon label): scaled by the HUD scale
    /// about the client's centre line and the ammo bar's bottom edge, which stands -1 + 8 d px
    /// above the client's bottom.
    pub fn bottom(&self) -> Similarity {
        if self.is_reference() {
            return Similarity::IDENTITY;
        }
        let s = self.hud_scale();
        let margin = -1.0 + 8.0 * self.d();
        Similarity { s, tx: f64::from(self.width) / 2.0 - 1280.0 * s, ty: f64::from(self.height) - margin - BAR_BOTTOM * s }
    }

    /// The readout label's font size before rounding: H / (240 d) + 8 d - 2.55 (fitted on 54
    /// configurations; the game draws round(this)).
    pub fn label_size(&self) -> f64 {
        let d = self.d();
        f64::from(self.height) / (240.0 * d) + 8.0 * d - 2.55
    }

    /// The HUD's own drawings' scales here against the reference (exactly 1 there): the aim strip by
    /// the readout's text size (its rows sit beside the readout's, its type is the readout's), the
    /// panel by the HUD scale (built like the game's ammo cards, which scale so).
    pub fn overlay_scales(&self) -> (f32, f32) {
        if self.is_reference() {
            return (1.0, 1.0);
        }
        ((self.label_size() / Self::REFERENCE.label_size()) as f32, self.hud_scale() as f32)
    }

    /// The loading UI's text sizes on screen (px of the unhinted Montserrat the game draws them in:
    /// weapon label Regular, card name Medium, card value Regular), measured per Windows scaling on
    /// window-height sweeps: each style steps at its own
    /// heights; a step changes halfway between the heights measured on either side. Other
    /// scalings: the reference sizes times the readout's text scale (`overlay_scales`), within
    /// about 1 px of the game.
    pub fn ui_text_px(&self) -> [f32; 3] {
        // (client height below which, px) per style, the last for any taller window
        type Steps = &'static [(u32, f32)];
        let table: Option<[Steps; 3]> = match self.dpi {
            96 => Some([&[(740, 9.9), (940, 10.9), (1140, 11.6), (u32::MAX, 12.6)], &[(940, 10.9), (1100, 12.6), (1307, 13.4), (u32::MAX, 14.3)], &[(740, 9.8), (1140, 10.9), (u32::MAX, 12.7)]]),
            120 => Some([&[(900, 12.5), (1180, 13.25), (u32::MAX, 14.7)], &[(900, 12.7), (1140, 13.7), (u32::MAX, 14.4)], &[(900, 11.9), (1180, 12.7), (u32::MAX, 14.8)]]),
            144 => Some([&[(1100, 14.7), (u32::MAX, 15.4)], &[(1100, 14.75), (u32::MAX, 16.4)], &[(1100, 14.8), (u32::MAX, 15.45)]]),
            _ => None,
        };
        match table {
            Some(t) => t.map(|steps| steps.iter().find(|&&(below, _)| self.height < below).map_or(0.0, |&(_, px)| px)),
            None => {
                let (k, _) = self.overlay_scales();
                [15.4 * k, 16.4 * k, 15.45 * k]
            }
        }
    }

    /// The seat menu at the client's bottom left: scaled by the HUD scale about that corner (the
    /// seat circles measure 28.8 + 38 d px wide and sit 3.2 + 6 d px from the left edge, i.e. 86 s
    /// and 12 s).
    pub fn seats(&self) -> Similarity {
        if self.is_reference() {
            return Similarity::IDENTITY;
        }
        let s = self.hud_scale();
        Similarity { s, tx: 0.0, ty: f64::from(self.height) - f64::from(Self::REFERENCE.height) * s }
    }

    /// A map view's container in the client (left, top, side, px), by measured rules (the
    /// reference's own `MapView::container` there):
    /// - minimap: anchored top right, 22.55 g in from the right and 22.85 g below Roblox's 58 d
    ///   top bar, side 354.35 g, at its group scale g (`map_group_scale`);
    /// - full map: centred, side 0.6884 H - 24 d to the nearest even pixel;
    /// - scope map: centred on the client's middle row, 22.55 s in from the right, side
    ///   0.28125 W - 0.01575 H - 24 d + 1.56 (follows the width).
    pub fn map_container(&self, view: MapView) -> (f64, f64, f64) {
        let c = view.container();
        if self.is_reference() {
            return (c.x, c.y, c.size);
        }
        let (w, h, d) = (f64::from(self.width), f64::from(self.height), self.d());
        match view {
            MapView::Mini => {
                let g = self.map_group_scale(view);
                let side = 354.35 * g;
                (w - 22.55 * g - side, 58.0 * d + 22.85 * g, side)
            }
            MapView::Full => {
                let side = 2.0 * ((0.6884 * h - 24.0 * d) / 2.0).round();
                ((w - side) / 2.0, (h - side) / 2.0, side)
            }
            MapView::Scope => {
                let side = 0.28125 * w - 0.01575 * h - 24.0 * d + 1.56;
                (w - 22.55 * self.hud_scale() - side, h / 2.0 - side / 2.0, side)
            }
        }
    }

    /// The scale a map view's group (container, margins, icons) is drawn at: the HUD scale, but the
    /// minimap's at most 0.32 H / 354.35 (logical heights under about 690), where the game shrinks
    /// the whole group, margins and icons with it (the box found on screen at 8 capped sizes lies
    /// within 1.0 px of this, 2.3 px when only the side was capped; the arrow there is sized by it).
    pub fn map_group_scale(&self, view: MapView) -> f64 {
        let s = self.hud_scale();
        match view {
            MapView::Mini => s.min(0.32 * f64::from(self.height) / 354.35),
            MapView::Full | MapView::Scope => s,
        }
    }

    /// The reference client onto this one for a map view: its container onto the view's here.
    pub fn map_similarity(&self, view: MapView) -> Similarity {
        if self.is_reference() {
            return Similarity::IDENTITY;
        }
        let c = view.container();
        let (left, top, side) = self.map_container(view);
        let k = side / c.size;
        Similarity { s: k, tx: left - k * c.x, ty: top - k * c.y }
    }

    /// The map icons' size on a view mapped to the reference container against the reference's:
    /// icons (markers, arrows, objectives) follow the view's group scale, the container its own rule.
    pub fn map_icon_scale(&self, view: MapView) -> f64 {
        self.map_group_scale(view) / self.map_similarity(view).s
    }

    /// The readout reader's text scales for this configuration, in the reference geometry the
    /// bottom group is mapped to.
    pub fn readout_text(&self) -> TextScale {
        if self.is_reference() {
            return TextScale::REFERENCE;
        }
        let t = self.label_size();
        let n = (t + 0.5).floor() as u32;
        let (lx, ly) = match LABEL_SCALE.iter().find(|e| e.0 == n) {
            Some(&(_, x, y)) => (x, y),
            None => {
                let ratio = LABEL_WIDTHS_BEYOND.iter().find(|e| e.0 == n).map_or(f64::from(n) / 13.0, |e| e.1 / 60.0);
                (ratio, ratio)
            }
        };
        let digits = DIGIT_SCALE * t / 13.0;
        let s = self.hud_scale();
        TextScale { label: ((lx / s) as f32, (ly / s) as f32), digits: ((digits / s) as f32, (digits / s) as f32) }
    }
}

/// The minimap's and the full map's containers as found on screen (`crate::locate`): (left, top,
/// side) in client px, used instead of the rules once found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Located {
    pub mini: Option<(f64, f64, f64)>,
    pub full: Option<(f64, f64, f64)>,
}

impl Located {
    /// Nothing found: the rules.
    pub const NONE: Self = Self { mini: None, full: None };

    pub fn get(&self, view: MapView) -> Option<(f64, f64, f64)> {
        match view {
            MapView::Mini => self.mini,
            MapView::Full => self.full,
            MapView::Scope => None,
        }
    }
}

/// Where one captured frame shows the client, and the configuration it is laid out for: what the
/// HUD reads every recognizer's regions through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The client's top-left in the frame (the picture's, `picture_shift` taken off the geometry).
    pub origin: (i32, i32),
    pub layout: UiLayout,
    /// Map containers found on screen at this configuration (`Located::NONE`: the rules).
    pub located: Located,
}

impl Placement {
    /// The client origin the recognizers are given for the bottom group mapped to the reference
    /// geometry (the maximised reference window's: below its 34 px title bar).
    pub const VIRTUAL_ORIGIN: (i32, i32) = (0, 34);

    pub fn is_reference(&self) -> bool {
        self.layout.is_reference()
    }

    pub fn new(origin: (i32, i32), layout: UiLayout) -> Self {
        Self { origin, layout, located: Located::NONE }
    }

    /// A map view's container in the client (left, top, side): as found on screen, else by rule.
    pub fn map_container(&self, view: MapView) -> (f64, f64, f64) {
        if self.is_reference() {
            return self.layout.map_container(view);
        }
        self.located.get(view).unwrap_or_else(|| self.layout.map_container(view))
    }

    /// The reference client onto this one for a map view (`UiLayout::map_similarity`, from the
    /// container found on screen when there is one).
    pub fn map_similarity(&self, view: MapView) -> Similarity {
        if self.is_reference() {
            return Similarity::IDENTITY;
        }
        let c = view.container();
        let (left, top, side) = self.map_container(view);
        let k = side / c.size;
        Similarity { s: k, tx: left - k * c.x, ty: top - k * c.y }
    }

    /// The map icons' scale on a view mapped to the reference container (`UiLayout::map_icon_scale`).
    pub fn map_icon_scale(&self, view: MapView) -> f64 {
        self.layout.map_group_scale(view) / self.map_similarity(view).s
    }

    /// The client origin the bottom group's recognizers work at: the frame's own at the reference
    /// (read as captured), else `VIRTUAL_ORIGIN` (read from `bottom_remap`).
    pub fn bottom_origin(&self) -> (i32, i32) {
        if self.is_reference() { self.origin } else { Self::VIRTUAL_ORIGIN }
    }

    /// A point the bottom group's recognizers gave (at `bottom_origin`), in frame coordinates.
    pub fn bottom_to_frame(&self, (x, y): (f64, f64)) -> (f64, f64) {
        if self.is_reference() {
            return (x, y);
        }
        let (vx, vy) = Self::VIRTUAL_ORIGIN;
        let (cx, cy) = self.layout.bottom().to_client((x - f64::from(vx), y - f64::from(vy)));
        (cx + f64::from(self.origin.0), cy + f64::from(self.origin.1))
    }

    /// The readout's anchor x and its (label, value) line rows for elevation and azimuth on the
    /// frame, for an anchor the readout reader gave at `bottom_origin`.
    pub fn readout_rows(&self, anchor_x: i32) -> (i32, [(i32, i32); 2]) {
        let rows = crate::aim::AimReader::line_centers(self.bottom_origin());
        let at = |y: i32| self.bottom_to_frame((f64::from(anchor_x), f64::from(y)));
        let line = |(l, v): (i32, i32)| (at(l).1.round() as i32, at(v).1.round() as i32);
        (at(rows[0].0).0.round() as i32, [line(rows[0]), line(rows[1])])
    }

    /// The frame rectangle to read for a bottom-group region `rect` (at `bottom_origin`), kept
    /// inside a frame of `size`.
    pub fn bottom_source(&self, rect: (i32, i32, i32, i32), size: (u32, u32)) -> (i32, i32, i32, i32) {
        let (x, y, w, h) = if self.is_reference() {
            rect
        } else {
            let (vx, vy) = Self::VIRTUAL_ORIGIN;
            source_rect(&self.layout.bottom(), (rect.0 - vx, rect.1 - vy, rect.2, rect.3), self.origin, 1)
        };
        let (x0, y0) = (x.clamp(0, size.0 as i32), y.clamp(0, size.1 as i32));
        let (x1, y1) = ((x + w).clamp(0, size.0 as i32), (y + h).clamp(0, size.1 as i32));
        (x0, y0, x1 - x0, y1 - y0)
    }

    /// The bottom-group region `rect` (at `VIRTUAL_ORIGIN`) resampled from `source` (frame pixels
    /// covering `bottom_source(rect)`); off the reference only.
    pub fn bottom_remap(&self, source: &FrameView, rect: (i32, i32, i32, i32)) -> Remapped {
        let (vx, vy) = Self::VIRTUAL_ORIGIN;
        remap(source, self.origin, &self.layout.bottom(), (rect.0 - vx, rect.1 - vy, rect.2, rect.3), Self::VIRTUAL_ORIGIN)
    }

    /// The client rectangle the seat menu's reader is given: the frame's at the reference, else the
    /// reference client at `VIRTUAL_ORIGIN` (read from `seats_remap`).
    pub fn seats_client(&self, client: (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
        if self.is_reference() {
            return client;
        }
        let r = UiLayout::REFERENCE;
        (Self::VIRTUAL_ORIGIN.0, Self::VIRTUAL_ORIGIN.1, r.width, r.height)
    }

    /// As `bottom_source`, for the seat menu's region `rect` (at `seats_client`).
    pub fn seats_source(&self, rect: (i32, i32, i32, i32), size: (u32, u32)) -> (i32, i32, i32, i32) {
        let (x, y, w, h) = if self.is_reference() {
            rect
        } else {
            let (vx, vy) = Self::VIRTUAL_ORIGIN;
            source_rect(&self.layout.seats(), (rect.0 - vx, rect.1 - vy, rect.2, rect.3), self.origin, 1)
        };
        let (x0, y0) = (x.clamp(0, size.0 as i32), y.clamp(0, size.1 as i32));
        let (x1, y1) = ((x + w).clamp(0, size.0 as i32), (y + h).clamp(0, size.1 as i32));
        (x0, y0, x1 - x0, y1 - y0)
    }

    /// As `bottom_remap`, for the seat menu's region.
    pub fn seats_remap(&self, source: &FrameView, rect: (i32, i32, i32, i32)) -> Remapped {
        let (vx, vy) = Self::VIRTUAL_ORIGIN;
        remap(source, self.origin, &self.layout.seats(), (rect.0 - vx, rect.1 - vy, rect.2, rect.3), Self::VIRTUAL_ORIGIN)
    }

    /// A frame point (a click) where the seat menu's reader sees it.
    pub fn frame_to_seats(&self, (x, y): (f64, f64)) -> (f64, f64) {
        if self.is_reference() {
            return (x, y);
        }
        let (vx, vy) = Self::VIRTUAL_ORIGIN;
        let (rx, ry) = self.layout.seats().to_reference((x - f64::from(self.origin.0), y - f64::from(self.origin.1)));
        (rx + f64::from(vx), ry + f64::from(vy))
    }

    /// As `bottom_to_frame`, for a map view: a point its recognizers gave (at `bottom_origin`, in
    /// the view mapped to the reference container) in frame coordinates.
    pub fn map_to_frame(&self, view: MapView, (x, y): (f64, f64)) -> (f64, f64) {
        if self.is_reference() {
            return (x, y);
        }
        let (vx, vy) = Self::VIRTUAL_ORIGIN;
        let (cx, cy) = self.map_similarity(view).to_client((x - f64::from(vx), y - f64::from(vy)));
        (cx + f64::from(self.origin.0), cy + f64::from(self.origin.1))
    }

    /// A map transform the recognizers made (on the view mapped to the reference container, at
    /// `bottom_origin`) on the frame: the same map drawn where the view's container is here.
    pub fn map_transform_to_frame(&self, t: MapTransform) -> MapTransform {
        if self.is_reference() {
            return t;
        }
        let (left, top) = self.map_to_frame(t.view, (t.left, t.top));
        MapTransform { left, top, size: t.size * self.map_similarity(t.view).s, ..t }
    }

    /// A length on a map view's mapped picture (px) on the frame.
    pub fn map_length_to_frame(&self, view: MapView, px: f64) -> f64 {
        px * self.map_similarity(view).s
    }

    /// The size the HUD draws its map marks at on a view here: the map icons follow the view's
    /// group scale.
    pub fn map_marks_scale(&self, view: MapView) -> f32 {
        if self.is_reference() { 1.0 } else { self.layout.map_group_scale(view) as f32 }
    }

    /// A frame point (the cursor) where a map view's recognizers see it (at `bottom_origin`).
    pub fn frame_to_map(&self, view: MapView, (x, y): (f64, f64)) -> (f64, f64) {
        if self.is_reference() {
            return (x, y);
        }
        let (vx, vy) = Self::VIRTUAL_ORIGIN;
        let (rx, ry) = self.map_similarity(view).to_reference((x - f64::from(self.origin.0), y - f64::from(self.origin.1)));
        (rx + f64::from(vx), ry + f64::from(vy))
    }

    /// As `bottom_source`, for a map view's region `rect` (at `bottom_origin`).
    pub fn map_source(&self, view: MapView, rect: (i32, i32, i32, i32), size: (u32, u32)) -> (i32, i32, i32, i32) {
        let (x, y, w, h) = if self.is_reference() {
            rect
        } else {
            let (vx, vy) = Self::VIRTUAL_ORIGIN;
            source_rect(&self.map_similarity(view), (rect.0 - vx, rect.1 - vy, rect.2, rect.3), self.origin, 1)
        };
        let (x0, y0) = (x.clamp(0, size.0 as i32), y.clamp(0, size.1 as i32));
        let (x1, y1) = ((x + w).clamp(0, size.0 as i32), (y + h).clamp(0, size.1 as i32));
        (x0, y0, x1 - x0, y1 - y0)
    }

    /// As `bottom_remap`, for a map view: its region `rect` resampled from its container here onto
    /// the reference container (icons then show `UiLayout::map_icon_scale` times their size).
    pub fn map_remap(&self, view: MapView, source: &FrameView, rect: (i32, i32, i32, i32)) -> Remapped {
        let (vx, vy) = Self::VIRTUAL_ORIGIN;
        remap(source, self.origin, &self.map_similarity(view), (rect.0 - vx, rect.1 - vy, rect.2, rect.3), Self::VIRTUAL_ORIGIN)
    }
}

/// Rows and columns `picture_shift` looks at.
const SHIFT_ROWS: u32 = 24;
const SHIFT_COLUMNS: u32 = 8;

/// The two strips of a captured frame of `size` that `picture_shift` reads: along the bottom in
/// the middle third, along the right edge in the middle third (both clear of the rounded corners).
pub fn shift_probes(size: (u32, u32)) -> [(i32, i32, u32, u32); 2] {
    let (w, h) = size;
    [((w / 3) as i32, h.saturating_sub(SHIFT_ROWS) as i32, w / 3, SHIFT_ROWS.min(h)), (w.saturating_sub(SHIFT_COLUMNS) as i32, (h / 3) as i32, SHIFT_COLUMNS.min(w), h / 3)]
}

/// How far up and left of where its geometry puts it a restored window's picture is drawn: the
/// black rows below it and black columns right of it, read on `shift_probes` (`bottom`, `right`).
/// Measured: (1, 8) at 100 %, (1, 9) at 125 %, (2, 11) at 150 %; some windows at 100 % were
/// drawn in place (0, 0). A restored window's own border is grey, so content never counts.
/// `None` when a probe is black throughout (no picture to tell by).
pub fn picture_shift(bottom: &FrameView, right: &FrameView) -> Option<(i32, i32)> {
    let black = |p: [u8; 3]| p.iter().all(|&c| c <= 2);
    let (bx, by) = bottom.origin;
    let rows = (0..bottom.height as i32).rev().take_while(|&j| (0..bottom.width as i32).all(|i| black(bottom.px(bx + i, by + j)))).count();
    let (rx, ry) = right.origin;
    let columns = (0..right.width as i32).rev().take_while(|&i| (0..right.height as i32).all(|j| black(right.px(rx + i, ry + j)))).count();
    (rows < bottom.height as usize && columns < right.width as usize).then_some((columns as i32, rows as i32))
}

/// Pixels of `source` resampled into a reference-geometry rectangle: `rect` (x, y, w, h) in the
/// reference client, through `t` onto the source's client at `source_origin`. The result is
/// addressed at `rect` shifted by `virtual_origin` (the client origin the recognizers are given),
/// BGRA, bilinear, each pixel's fourth byte the brightest luma of its four source pixels.
pub struct Remapped {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Remapped {
    pub fn view(&self) -> FrameView<'_> {
        FrameView { data: &self.data, format: PixelFormat::BgraPeak, origin: (self.x, self.y), width: self.width, height: self.height, stride: self.width as usize * 4 }
    }
}

/// The source rectangle (x, y, w, h, frame px) a reference rectangle maps to, with `margin` px
/// round it for the bilinear samples.
pub fn source_rect(t: &Similarity, rect: (i32, i32, i32, i32), source_origin: (i32, i32), margin: i32) -> (i32, i32, i32, i32) {
    let (x0, y0) = t.to_client((f64::from(rect.0), f64::from(rect.1)));
    let (x1, y1) = t.to_client((f64::from(rect.0 + rect.2), f64::from(rect.1 + rect.3)));
    let (fx0, fy0) = (x0.floor() as i32 + source_origin.0 - margin, y0.floor() as i32 + source_origin.1 - margin);
    let (fx1, fy1) = (x1.ceil() as i32 + source_origin.0 + margin, y1.ceil() as i32 + source_origin.1 + margin);
    (fx0, fy0, fx1 - fx0, fy1 - fy0)
}

pub fn remap(source: &FrameView, source_origin: (i32, i32), t: &Similarity, rect: (i32, i32, i32, i32), virtual_origin: (i32, i32)) -> Remapped {
    // Weights in 1/1024: an output level within about 0.1 of the exact bilinear mean.
    const ONE: u32 = 1024;
    let (w, h) = (rect.2.max(0) as usize, rect.3.max(0) as usize);
    let mut data = vec![0u8; w * h * 4];
    // The map is a scale and a shift, so each output column samples the same two source columns
    // and each row the same two rows: found once per axis, the columns (rows) with both samples
    // inside the source one run (the rest stay black). Done per pixel an 860 x 590 loading
    // region took 15 ms.
    let axis = |n: usize, start: i32, shift: f64, offset: i32, view_start: i32, view_len: u32| -> (usize, Vec<(usize, u32)>) {
        let mut first = n;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            // The reference pixel's centre, in the source's frame.
            let c = t.s * (f64::from(start) + i as f64 + 0.5) + shift;
            let sc = c + f64::from(offset) - 0.5;
            let c0 = sc.floor() as i32;
            if c0 >= view_start && c0 + 2 <= view_start + view_len as i32 {
                first = first.min(i);
                out.push(((c0 - view_start) as usize, ((sc - f64::from(c0)) * f64::from(ONE) + 0.5) as u32));
            } else if first < n {
                break;
            }
        }
        (first, out)
    };
    let (i0, cols) = axis(w, rect.0, t.tx, source_origin.0, source.origin.0, source.width);
    let (j0, rows) = axis(h, rect.1, t.ty, source_origin.1, source.origin.1, source.height);
    if cols.is_empty() || rows.is_empty() {
        return Remapped { x: rect.0 + virtual_origin.0, y: rect.1 + virtual_origin.1, width: w as u32, height: h as u32, data };
    }
    // Each sampled source row interpolated across once (kept at full precision, 1024 times the
    // level), then each output row from its two source rows with one weight: the across pass
    // runs once per source row, the down pass vectorises (per pixel, 860 x 590 took 2.4 ms).
    let (bpp, [ri, gi, bi]) = match source.format {
        PixelFormat::Rgb => (3, [0usize, 1, 2]),
        PixelFormat::Bgra | PixelFormat::BgraPeak => (4, [2, 1, 0]),
    };
    // Luma in 1/65536 per channel level (0.299, 0.587, 0.114), rounded at the end.
    let weights = [19595u32, 38470, 7471];
    let n = cols.len();
    let across = |y: usize, row: &mut AcrossRow| {
        let line = &source.data[y * source.stride..];
        for (k, &(x0, wx)) in cols.iter().enumerate() {
            let (a, b) = (&line[x0 * bpp..x0 * bpp + 3], &line[(x0 + 1) * bpp..(x0 + 1) * bpp + 3]);
            let luma = |p: &[u8]| ((u32::from(p[ri]) * weights[0] + u32::from(p[gi]) * weights[1] + u32::from(p[bi]) * weights[2] + 32768) >> 16) as u8;
            row.b[k] = u32::from(a[bi]) * (ONE - wx) + u32::from(b[bi]) * wx;
            row.g[k] = u32::from(a[gi]) * (ONE - wx) + u32::from(b[gi]) * wx;
            row.r[k] = u32::from(a[ri]) * (ONE - wx) + u32::from(b[ri]) * wx;
            row.l[k] = luma(a).max(luma(b));
        }
    };
    let (mut upper, mut lower) = (AcrossRow::new(n), AcrossRow::new(n));
    let mut have: Option<usize> = None;
    for (j, &(y0, wy)) in rows.iter().enumerate() {
        match have {
            Some(y) if y == y0 => {}
            Some(y) if y + 1 == y0 => {
                std::mem::swap(&mut upper, &mut lower);
                across(y0 + 1, &mut lower);
            }
            _ => {
                across(y0, &mut upper);
                across(y0 + 1, &mut lower);
            }
        }
        have = Some(y0);
        let out = &mut data[((j0 + j) * w + i0) * 4..((j0 + j) * w + i0 + n) * 4];
        let mix = |t: u32, b: u32| ((t * (ONE - wy) + b * wy + ONE * ONE / 2) / (ONE * ONE)) as u8;
        for (k, o) in out.chunks_exact_mut(4).enumerate() {
            // The fourth byte: the brightest source luma it was made from (`PixelFormat::BgraPeak`).
            o.copy_from_slice(&[mix(upper.b[k], lower.b[k]), mix(upper.g[k], lower.g[k]), mix(upper.r[k], lower.r[k]), upper.l[k].max(lower.l[k])]);
        }
    }
    Remapped { x: rect.0 + virtual_origin.0, y: rect.1 + virtual_origin.1, width: w as u32, height: h as u32, data }
}

/// One source row interpolated across to the output columns (`remap`): each channel at 1024 times
/// its level, and the brighter luma of the two source pixels.
struct AcrossRow {
    b: Vec<u32>,
    g: Vec<u32>,
    r: Vec<u32>,
    l: Vec<u8>,
}

impl AcrossRow {
    fn new(n: usize) -> Self {
        Self { b: vec![0; n], g: vec![0; n], r: vec![0; n], l: vec![0; n] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_containers_follow_the_measured_rules() {
        // The reference: its own containers.
        for view in MapView::ALL {
            let c = view.container();
            assert_eq!(UiLayout::REFERENCE.map_container(view), (c.x, c.y, c.size));
        }
        // Measured by edges and by registering the map's canvas: (dpi, W, H, view, left, top,
        // side); the game draws them on whole pixels.
        let measured = [
            (96, 1920, 1080, MapView::Mini, 1650.0, 74.0, 254.0),
            (120, 1920, 840, MapView::Mini, 1634.0, 90.0, 269.0),
            (144, 1920, 1000, MapView::Mini, 1579.0, 108.0, 320.0),
            (144, 1920, 1080, MapView::Mini, 1568.0, 108.0, 331.0),
            // Capped at 0.32 H, the box found on screen.
            (120, 1920, 750, MapView::Mini, 1665.0, 88.0, 240.0),
            (120, 1920, 760, MapView::Mini, 1662.0, 88.0, 243.0),
            (144, 1920, 901, MapView::Mini, 1614.0, 105.0, 288.0),
            (96, 1280, 720, MapView::Full, 404.0, 124.0, 472.0),
            (96, 1920, 1080, MapView::Full, 600.0, 180.0, 720.0),
            (120, 2560, 1334, MapView::Full, 836.0, 223.0, 888.0),
            (96, 1600, 1000, MapView::Scope, 1172.0, 294.0, 412.0),
            (96, 1920, 1080, MapView::Scope, 1404.0, 290.0, 500.0),
            (96, 2560, 1334, MapView::Scope, 1866.0, 329.0, 676.0),
        ];
        for (dpi, width, height, view, left, top, side) in measured {
            let (l, t, s) = UiLayout { width, height, dpi }.map_container(view);
            assert!((l - left).abs() <= 1.5 && (t - top).abs() <= 1.5 && (s - side).abs() <= 1.5, "{view:?} {width}x{height} {dpi}: ({l:.1}, {t:.1}, {s:.1})");
        }
    }

    #[test]
    fn the_reference_maps_onto_itself() {
        let l = UiLayout::REFERENCE;
        assert!(l.bottom().is_identity());
        assert!((l.hud_scale() - 1.0).abs() < 1e-4);
        assert_eq!(l.readout_text(), TextScale::REFERENCE);
    }

    #[test]
    fn label_sizes_follow_the_measured_rule() {
        // Measured: (client height, dpi, label font size).
        for (h, dpi, n) in [(720, 96, 8), (768, 96, 9), (1000, 96, 10), (1240, 96, 11), (1334, 96, 11), (750, 120, 10), (1200, 120, 11), (1240, 120, 12), (1080, 144, 12), (1120, 144, 13), (1334, 144, 13)] {
            let l = UiLayout { width: 1920, height: h, dpi };
            assert_eq!((l.label_size() + 0.5).floor() as u32, n, "{h} at {dpi}");
        }
    }

    #[test]
    fn a_shifted_picture_is_told_by_its_black_margins() {
        // A 40 x 30 RGB frame drawn (1, 8) up-left: its last 8 rows and last column black, grey
        // border above and left of them.
        let (w, h) = (40u32, 30u32);
        let mut data = vec![90u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                if y >= h - 8 || x >= w - 1 {
                    data[i..i + 3].fill(0);
                } else if y == h - 9 || x == w - 2 {
                    data[i..i + 3].fill(47);
                }
            }
        }
        let frame = FrameView::rgb(&data, w, h);
        let [(bx, by, bw, bh), (rx, ry, rw, rh)] = shift_probes((w, h));
        let (bottom, right) = (frame.sub(bx, by, bw, bh), frame.sub(rx, ry, rw, rh));
        assert_eq!(picture_shift(&bottom, &right), Some((1, 8)));
        let black = vec![0u8; (w * h * 3) as usize];
        let dark = FrameView::rgb(&black, w, h);
        assert_eq!(picture_shift(&dark.sub(bx, by, bw, bh), &dark.sub(rx, ry, rw, rh)), None);
    }

    #[test]
    fn the_bottom_group_maps_to_the_frame_and_back() {
        let p = Placement::new((0, 23), UiLayout { width: 1920, height: 1080, dpi: 96 });
        let (vx, vy) = Placement::VIRTUAL_ORIGIN;
        // The ammo bar's box corner (reference client (1002, 1187)) lands where the layout puts it.
        let (fx, fy) = p.bottom_to_frame((f64::from(vx) + 1002.0, f64::from(vy) + 1187.0));
        let (cx, cy) = p.layout.bottom().to_client((1002.0, 1187.0));
        assert!((fx - cx).abs() < 1e-9 && (fy - cy - 23.0).abs() < 1e-9);
        let r = Placement::new((0, 34), UiLayout::REFERENCE);
        assert_eq!(r.bottom_origin(), (0, 34));
        assert_eq!(r.bottom_source((10, 20, 30, 40), (2560, 1368)), (10, 20, 30, 40));
    }

    #[test]
    fn the_bar_lands_where_measured_at_1280x720() {
        // 1280 x 720 at 100 %: the ammo bar's box measured at left 467, top 628, scale 0.623.
        let b = UiLayout { width: 1280, height: 720, dpi: 96 }.bottom();
        let (x, y) = b.to_client((1002.0, 1187.0));
        assert!((b.s - 0.623).abs() < 0.003 && (x - 467.0).abs() < 2.0 && (y - 628.0).abs() < 2.0, "{b:?} {x} {y}");
    }
}
