//! Aim guidance drawn beside the game's own angle readout, in the game's visual language:
//! Montserrat, white text with a soft dark shadow, a small grey label line above the value
//! line. No panel behind it, like the readout it extends.
//!
//! Per axis one column beside the game's readout column, built like it — a small label line for
//! reference over a large value line for the live number — then the indicator lamp:
//!
//! - Label line: the target, fixed until the target changes (`Target +12.98°`, in the readout's
//!   angle format).
//! - Value line: how far is left to go and which way — an amber arrow with a dark edge, a little
//!   taller than the digits, and the amount in the readout's white (`▲ 6.07°`; `▲▼` raise or lower, `◀▶` turn left
//!   or right). Within tolerance the arrow stays, in white, for whoever aims finer than the
//!   tolerance; at 0.00° it goes.
//! - The indicator lamp on the value line, between that number and the game's current
//!   value: a white rim, dark inside while out of tolerance, green within it.
//!
//! Text is the readout's white and grey with the game's drop shadow, so it reads as well as the
//! readout on any ground (amber or green numbers fade on sand and grass); colour is kept to the
//! small marks, which carry the same shadow.
//!
//! The type is the readout's (label Montserrat Regular 13 px, (201, 208, 211); value Medium 18 px —
//! the sizes whose ink matches the readout's labels and values in width, height and rows), on the
//! readout's text rows (a 1 px optical drop was tried and looked too low). The lamp's right edge is
//! the HUD's right edge, shared with the panel.
//!
//! Off the reference configuration (150 %, 2560×1334) the whole strip is drawn at `AimStrip::scale`:
//! the readout's text size there against the reference's, so its type stays the
//! readout's within a pixel.

use std::rc::Rc;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    IDWriteTextFormat,
};

use crate::render::{Graphics, SHADOW_ALPHA, SHADOW_OFFSET, Surface, rgba};
use crate::window;

/// Window size in physical pixels at scale 1; the HUD's right edge lies `EDGE_X` from the window's
/// left. The surface is made for scales up to `MAX_SCALE` and the window shows its scaled part.
const WIDTH: i32 = 200;
const HEIGHT: i32 = 112;
const EDGE_X: i32 = 190;
const MAX_SCALE: f32 = 1.6;
/// The column (centre and half width, from the HUD's right edge): wide enough for
/// "Target 180.00°" and "▶ 180.00°".
const COLUMN_CENTRE: f32 = -78.0;
const COLUMN_HALF: f32 = 50.0;
/// Lamp radius (as tall as the digits); its right edge is the HUD's right edge.
const LAMP_R: f32 = 7.0;
/// The direction arrow: an isosceles triangle this long along its direction and this wide across
/// it (the digits are 13 px tall), and its gap to the amount.
const ARROW_LENGTH: f32 = 15.0;
const ARROW_WIDTH: f32 = 18.0;
const ARROW_GAP: f32 = 6.0;
/// The HUD's right edge (the lamp's) lies this far left of the readout's centre line, ~25 px
/// short of the readout's values.
const GAP_TO_READOUT: i32 = 60;

/// Half width of the game's loading countdown bar, centred on the window (px).
const COUNTDOWN_HALF: i32 = 225;
/// Least gap between the HUD's right edge and the countdown bar.
const COUNTDOWN_GAP: i32 = 32;

/// Right edge shared by the strip's lamps and the panel (screen px) for a readout centred at
/// `readout_x` in a window centred at `centre_x`: `GAP_TO_READOUT` left of the readout, but never
/// closer than `COUNTDOWN_GAP` to the loading countdown bar (vehicles with one or two slots put the
/// readout near the middle). `scale` is the strip's, `hud` the game HUD's (the countdown bar's).
pub fn hud_right(readout_x: i32, centre_x: i32, scale: f32, hud: f32) -> i32 {
    let px = |v: i32, k: f32| (v as f32 * k).round() as i32;
    (readout_x - px(GAP_TO_READOUT, scale)).min(centre_x - px(COUNTDOWN_HALF + COUNTDOWN_GAP, hud))
}

/// Top-left corner of the strip's window for the HUD's right edge `right` and the elevation label
/// line `label_y`, and the window's size, at `scale`.
fn window_rect(right: i32, label_y: i32, scale: f32) -> (i32, i32, i32, i32) {
    let px = |v: i32| (v as f32 * scale).round() as i32;
    (right - px(EDGE_X), label_y - px(12), (WIDTH as f32 * scale).ceil() as i32, (HEIGHT as f32 * scale).ceil() as i32)
}

const SURFACE: (u32, u32) = ((WIDTH as f32 * MAX_SCALE) as u32, (HEIGHT as f32 * MAX_SCALE) as u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Elevation,
    Azimuth,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimRow {
    pub axis: Axis,
    pub current: f64,
    pub target: f64,
    pub tolerance: f64,
    /// Screen y of this axis's label line and value line centres.
    pub label_y: i32,
    pub value_y: i32,
    /// The target is outside the elevation the gun is known to reach: the label is amber.
    pub beyond_reach: bool,
}

impl AimRow {
    /// Signed correction to apply (target − current), azimuth wrapped to ±180°.
    pub fn correction(&self) -> f64 {
        let d = self.target - self.current;
        match self.axis {
            Axis::Elevation => d,
            Axis::Azimuth => (d + 540.0).rem_euclid(360.0) - 180.0,
        }
    }

    pub fn on_target(&self) -> bool {
        self.correction().abs() <= self.tolerance
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimStrip {
    /// Screen x of the HUD's right edge (`hud_right`).
    pub right: i32,
    pub rows: [AimRow; 2],
    /// Comparison mode: each column shows the readout's own words and current angle
    /// (`Elevation` over `+28.97°`) in the strip's type, to compare the two renderings.
    pub mirror: bool,
    /// The scale everything is drawn at (1 at the reference configuration).
    pub scale: f32,
}

pub struct StripWindow {
    hwnd: HWND,
    surface: Surface,
    label: IDWriteTextFormat,
    value: IDWriteTextFormat,
    value_left: IDWriteTextFormat,
    shown: Option<AimStrip>,
    position: Option<(i32, i32, i32, i32)>,
}

impl StripWindow {
    pub fn new(graphics: &Rc<Graphics>) -> windows::core::Result<Self> {
        let hwnd = window::create(SURFACE.0 as i32, SURFACE.1 as i32)?;
        let surface = Surface::new(graphics, hwnd, SURFACE.0, SURFACE.1)?;
        Self::with_surface(hwnd, surface)
    }

    /// The strip drawn offscreen where it would show.
    pub fn render_offscreen(graphics: &Rc<Graphics>, strip: &AimStrip) -> windows::core::Result<crate::Layer> {
        let window = Self::with_surface(HWND::default(), Surface::offscreen(graphics, SURFACE.0, SURFACE.1)?)?;
        let (x, y, w, h) = window_rect(strip.right, strip.rows[0].label_y, strip.scale);
        window.draw(strip, y)?;
        let full = window.surface.pixels()?;
        let row = SURFACE.0 as usize * 4;
        let bgra = (0..h as usize).flat_map(|r| full[r * row..r * row + w as usize * 4].iter().copied()).collect();
        Ok(crate::Layer { x, y, width: w as u32, height: h as u32, bgra })
    }

    fn with_surface(hwnd: HWND, surface: Surface) -> windows::core::Result<Self> {
        let label = surface.text_format(13.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER)?;
        let value = surface.text_format(18.0, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_CENTER)?;
        let value_left = surface.text_format(18.0, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_LEADING)?;
        Ok(Self { hwnd, surface, label, value, value_left, shown: None, position: None })
    }

    /// The surface made again after a device loss (`Graphics::recover`), the shown strip redrawn.
    pub fn rebuild(&mut self) -> windows::core::Result<()> {
        self.surface.rebuild()?;
        match self.shown.take() {
            Some(shown) => self.show(Some(&shown)),
            None => Ok(()),
        }
    }

    pub fn show(&mut self, strip: Option<&AimStrip>) -> windows::core::Result<()> {
        let Some(strip) = strip else {
            if self.shown.take().is_some() {
                window::hide(self.hwnd);
                self.position = None;
            }
            return Ok(());
        };
        if self.shown.as_ref() == Some(strip) {
            return Ok(());
        }
        // Window top sits so the elevation label line lands at its row inside the window.
        let rect = window_rect(strip.right, strip.rows[0].label_y, strip.scale);
        self.draw(strip, rect.1)?;
        if self.position != Some(rect) {
            window::place_sized(self.hwnd, rect.0, rect.1, rect.2, rect.3);
            self.position = Some(rect);
        }
        self.shown = Some(*strip);
        Ok(())
    }

    fn draw(&self, strip: &AimStrip, window_y: i32) -> windows::core::Result<()> {
        let s = &self.surface;
        let white = rgba(255, 255, 255, 1.0); // the readout's values are pure white
        let grey = rgba(201, 208, 211, 1.0); // the readout's label colour
        let amber = rgba(226, 174, 70, 1.0); // the game's selection amber
        let shadow = rgba(0, 0, 0, SHADOW_ALPHA);
        let (sx, sy) = SHADOW_OFFSET;
        let green = rgba(118, 214, 128, 1.0);
        let edge = EDGE_X as f32;
        let (left, right) = (edge + COLUMN_CENTRE - COLUMN_HALF, edge + COLUMN_CENTRE + COLUMN_HALF);
        let centre = edge + COLUMN_CENTRE;
        let lamp_x = edge - LAMP_R;
        // Drawn in scale-1 coordinates; the surface scales them.
        s.set_scale(strip.scale);
        s.begin();
        for row in &strip.rows {
            let label_y = (row.label_y - window_y) as f32 / strip.scale;
            let value_y = (row.value_y - window_y) as f32 / strip.scale;
            let ok = row.on_target();
            let correction = row.correction();

            if strip.mirror {
                let (word, angle) = match row.axis {
                    Axis::Elevation => ("Elevation", format!("{:+.2}°", row.current)),
                    Axis::Azimuth => ("Azimuth", format!("{:06.2}°", row.current.rem_euclid(360.0))),
                };
                s.label_text(word, &self.label, D2D_RECT_F { left, top: label_y - 10.0, right, bottom: label_y + 10.0 }, grey);
                s.text(&angle, &self.value, D2D_RECT_F { left, top: value_y - 13.0, right, bottom: value_y + 15.0 }, white);
                continue;
            }

            // Label line: the target, for reference.
            let target_text = match row.axis {
                Axis::Elevation => format!("Target {:+.2}°", row.target),
                Axis::Azimuth => format!("Target {:06.2}°", row.target.rem_euclid(360.0)),
            };
            s.label_text(&target_text, &self.label, D2D_RECT_F { left, top: label_y - 10.0, right, bottom: label_y + 10.0 }, if row.beyond_reach { amber } else { grey });

            // Value line: the amount left, with its direction while out of tolerance. 1 px below
            // the line centre puts the digits on the readout value's rows (value_y − 6 …
            // value_y + 6); arrow and lamp are centred on them, cy.
            let cy = value_y + 0.5;
            let amount = format!("{:.2}°", correction.abs());
            let text_rect = |l: f32, r: f32| D2D_RECT_F { left: l, top: value_y - 13.0, right: r, bottom: value_y + 15.0 };
            if amount == "0.00°" {
                s.text(&amount, &self.value, text_rect(left, right), white);
            } else {
                // The arrow's box: as wide as its widest side either way.
                let arrow_w = match row.axis {
                    Axis::Elevation => ARROW_WIDTH,
                    Axis::Azimuth => ARROW_LENGTH,
                };
                let width = arrow_w + ARROW_GAP + s.text_width(&amount, &self.value_left)?;
                let start = centre - width / 2.0;
                let (ax, ay) = (start + arrow_w / 2.0, cy);
                // The tip half a pixel farther than the base: the triangle's ink looks centred.
                let (l, w) = (ARROW_LENGTH / 2.0, ARROW_WIDTH / 2.0);
                let arrow = match (row.axis, correction > 0.0) {
                    (Axis::Elevation, true) => [(ax, ay - l - 0.5), (ax - w, ay + l - 0.5), (ax + w, ay + l - 0.5)],
                    (Axis::Elevation, false) => [(ax, ay + l - 0.5), (ax - w, ay - l - 0.5), (ax + w, ay - l - 0.5)],
                    (Axis::Azimuth, true) => [(ax + l + 0.5, ay - 0.5), (ax - l + 0.5, ay - w - 0.5), (ax - l + 0.5, ay + w - 0.5)],
                    (Axis::Azimuth, false) => [(ax - l - 0.5, ay - 0.5), (ax + l - 0.5, ay - w - 0.5), (ax + l - 0.5, ay + w - 0.5)],
                };
                // A dark edge keeps the arrow apart from sand and snow as well as grass.
                s.triangle(arrow.map(|(x, y)| (x + sx, y + sy)), shadow)?;
                s.polygon(&arrow, Some(if ok { white } else { amber }), Some((rgba(0, 0, 0, 0.6), 1.0)))?;
                s.text(&amount, &self.value_left, text_rect(start + arrow_w + ARROW_GAP, start + width + 20.0), white);
            }

            // The lamp: a white rim with the text's drop shadow, so it shows on light and dark
            // ground alike; unlit it is dark inside, lit green — told apart on green ground too.
            s.circle(lamp_x + sx, cy + sy, LAMP_R, shadow);
            s.circle(lamp_x, cy, LAMP_R - 1.0, if ok { green } else { rgba(0, 0, 0, 0.45) });
            s.ring(lamp_x, cy, LAMP_R - 0.75, white, 1.5);
        }
        s.end()
    }
}
