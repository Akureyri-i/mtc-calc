//! The settings window: one window, opened by the gear on the fire-control panel or on V1's plan
//! panel (only one of them is on screen at a time) and later from the tray, closed by its cross or
//! the gear again. Settings are not changed mid-fight, so it is sized like an ordinary program's
//! window and centred over the game, rather than kept small to cover less of it; it keeps the
//! game's look: built like the game's ammo tooltip — a near-opaque dark card with a lighter title
//! strip and a thin light edge — section captions small, grey and upper-case as on the game's vote
//! screen, labels in sentence case, choices as joined square cells with the selected one in the
//! game's amber with dark text, as on its loading menu.
//!
//! What it sets: the aim tolerance by angle or by the miss on the ground, elevation and azimuth
//! each; which charge to suggest when the loaded one does not reach; whether the landing prediction
//! shows; what the small target labels on V1 show, nothing by default; the drag follow's
//! fine-tuning (calibration: values that start from what is measured on this machine); sharing
//! statistics, a problem report and the log files. The HUD keeps the values and sends them back
//! with every change, so the window shows what is in force.
//!
//! Laid out at 150 % display scaling and drawn at the screen's scaling like an ordinary program
//! (`SettingsState::scale`, at most 1: the 150 % size).
//!
//! It takes clicks without taking focus (the game keeps the keyboard); clicks act on release.

use std::rc::Rc;

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    IDWriteTextFormat,
};

use crate::panel::{PanelAction, inside, palette, rect};
use crate::render::{CapMetrics, Graphics, Surface, rgba};
use crate::window;

/// Window size at 150 % display scaling (physical pixels; `SettingsState::scale` times this on
/// screen).
pub const WIDTH: i32 = 880;
const HEIGHT: i32 = 1236;
const PAD: f32 = 32.0;
const HEADER_H: f32 = 64.0;
/// A section's caption, with the space above it.
const CAPTION_H: f32 = 46.0;
/// A setting: its name over a line saying what it does, the control at the right.
const ROW_H: f32 = 76.0;
/// The map labels' setting: name and line, its four choices under them.
const LABELS_ROW_H: f32 = 128.0;
const FOOTER_H: f32 = 54.0;
const CONTROL_H: f32 = 42.0;
/// A stepper: − and + buttons round the value.
const STEP_W: f32 = 46.0;
const STEP_VALUE_W: f32 = 132.0;

/// How the aim strip judges "on target".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToleranceMode {
    /// Within an angle of the target's elevation and azimuth.
    Angle,
    /// Within a miss on the ground, along and across the line of fire, turned into angles by the
    /// trajectory to the target: the default.
    #[default]
    Impact,
}

/// Which reaching charge to recommend when the loaded one does not reach.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChargeRule {
    /// The charge closest to the loaded one.
    #[default]
    NearestCharge,
    /// The charge whose arc is closest to where the gun points now (the least gun movement).
    LeastMove,
}

/// What the small labels beside the targets on V1 show: nothing by default, the blast written where
/// the selected target's ring is too small to draw included — the map already carries a lot. A
/// location too coarse to aim by shows anyway.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabelFields {
    /// Flight time, in the charge's colour.
    pub time: bool,
    /// Elevation of the arc in use, in the charge's colour.
    pub elevation: bool,
    pub distance: bool,
    /// The marker's location precision.
    pub precision: bool,
    /// The selected target's blast radius, where its ring is too small to draw.
    pub blast: bool,
}


impl LabelFields {
    /// Whether any of the target's figures shows (the blast aside).
    pub fn any(&self) -> bool {
        self.time || self.elevation || self.distance || self.precision
    }

    pub fn get(&self, field: LabelField) -> bool {
        match field {
            LabelField::Time => self.time,
            LabelField::Elevation => self.elevation,
            LabelField::Distance => self.distance,
            LabelField::Precision => self.precision,
            LabelField::Blast => self.blast,
        }
    }

    pub fn set(&mut self, field: LabelField, on: bool) {
        match field {
            LabelField::Time => self.time = on,
            LabelField::Elevation => self.elevation = on,
            LabelField::Distance => self.distance = on,
            LabelField::Precision => self.precision = on,
            LabelField::Blast => self.blast = on,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelField {
    Time,
    Elevation,
    Distance,
    Precision,
    Blast,
}

/// A value set with − and +.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stepper {
    /// The elevation tolerance (an angle or the miss along the line of fire, by the mode).
    Elevation,
    /// The azimuth tolerance (an angle or the miss across the line of fire).
    Azimuth,
    /// The drag follow's fine-tuning.
    DragOffset,
}

/// One change asked for in the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Setting {
    Pred(bool),
    Mode(ToleranceMode),
    /// One step down (−1) or up (+1).
    Step(Stepper, i32),
    Rule(ChargeRule),
    Label(LabelField, bool),
    /// Send the statistics.
    Share(bool),
    /// Put screenshots of what the HUD reads into a problem report.
    ReportShots(bool),
    /// Take the private server's debug map as the map until a real one is identified.
    DebugMap(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsState {
    /// Screen position of the game window's client centre: the window shows centred on it.
    pub centre: (i32, i32),
    pub pred: bool,
    pub mode: ToleranceMode,
    /// Elevation and azimuth tolerance by angle (°).
    pub angle: (f64, f64),
    /// Tolerated miss along and across the line of fire (m).
    pub impact: (f64, f64),
    pub rule: ChargeRule,
    pub labels: LabelFields,
    /// The drag follow's fine-tuning (ms, positive: later); 0 leaves it automatic.
    pub drag_offset_ms: i32,
    /// Where each stepper's value stands in its range: (at the bottom, at the top).
    pub limits: [(bool, bool); 3],
    /// The statistics are sent.
    pub share: bool,
    /// A problem report gets screenshots of what the HUD reads.
    pub report_shots: bool,
    /// The debug map is taken as the map (until a real one is identified).
    pub debug_map: bool,
    /// What the last problem report did (where it was saved), shown under its row.
    pub report_note: Option<String>,
    /// The screen's display scaling over 150 %, at most 1.
    pub scale: f32,
}

/// The startup question about sending statistics (asked at every start until the player sends them
/// or ticks "Don't ask again"), drawn as the settings window's card.
#[derive(Clone, Debug, PartialEq)]
pub struct ConsentState {
    /// Screen position of the game window's client centre: the card shows centred on it.
    pub centre: (i32, i32),
    /// "Don't ask again" is ticked.
    pub dont_ask: bool,
    /// As `SettingsState::scale`.
    pub scale: f32,
}

/// The question's card at 150 % (physical px).
const CONSENT_W: f32 = 780.0;
const CONSENT_H: f32 = 284.0;

/// The gear: a 16 px wheel of eight teeth round a hub, outlined in `color`, centred at (`cx`, `cy`).
pub(crate) fn gear(s: &Surface, cx: f32, cy: f32, color: D2D1_COLOR_F) -> windows::core::Result<()> {
    let (outer, inner) = (7.5f32, 5.4f32);
    let mut points = Vec::with_capacity(32);
    for k in 0..8 {
        let a = std::f32::consts::TAU * k as f32 / 8.0;
        for (da, r) in [(-0.30f32, inner), (-0.17, outer), (0.17, outer), (0.30, inner)] {
            points.push((cx + r * (a + da).cos(), cy + r * (a + da).sin()));
        }
    }
    s.polygon(&points, None, Some((color, 1.5)))?;
    s.ring(cx, cy, 2.3, color, 1.5);
    Ok(())
}

/// A text format with its vertical metrics, for text centred on its capitals.
struct Font {
    format: IDWriteTextFormat,
    metrics: CapMetrics,
}

struct Fonts {
    title: Font,
    caption: Font,
    name: Font,
    line: Font,
    cell: Font,
    value: Font,
}

/// The window's colours on top of the panels' palette.
struct Shades {
    card: D2D1_COLOR_F,
    strip: D2D1_COLOR_F,
    edge: D2D1_COLOR_F,
    rule: D2D1_COLOR_F,
    cell: D2D1_COLOR_F,
    cell_edge: D2D1_COLOR_F,
    line: D2D1_COLOR_F,
}

fn shades() -> Shades {
    Shades {
        // The game's tooltip: (10, 13, 16) nearly opaque, the title strip (25, 29, 29).
        card: rgba(10, 13, 16, 0.94),
        strip: rgba(25, 29, 30, 0.96),
        edge: rgba(235, 238, 228, 0.32),
        rule: rgba(235, 238, 228, 0.10),
        cell: rgba(255, 255, 255, 0.05),
        cell_edge: rgba(235, 238, 228, 0.28),
        line: rgba(206, 210, 203, 0.64),
    }
}

pub struct SettingsWindow {
    hwnd: HWND,
    surface: Surface,
    fonts: Fonts,
    shown: Option<SettingsState>,
    placed: Option<(i32, i32)>,
    hover: Option<(i32, i32)>,
    hits: Vec<(D2D_RECT_F, PanelAction)>,
    pressed: Option<(i32, i32)>,
    /// The scale it is drawn at (`SettingsState::scale`); hits and pointer are in layout px.
    scale: f32,
    /// This window shows the startup question instead (`show_consent`).
    consent: Option<ConsentState>,
}

impl SettingsWindow {
    pub fn new(graphics: &Rc<Graphics>) -> windows::core::Result<Self> {
        let hwnd = window::create_interactive(WIDTH, HEIGHT)?;
        let surface = Surface::new(graphics, hwnd, WIDTH as u32, HEIGHT as u32)?;
        Self::with_surface(hwnd, surface)
    }

    /// The window drawn offscreen where it would show, `hover` (window px) highlighted.
    pub fn render_offscreen(graphics: &Rc<Graphics>, state: &SettingsState, hover: Option<(i32, i32)>) -> windows::core::Result<crate::Layer> {
        let mut window = Self::with_surface(HWND::default(), Surface::offscreen(graphics, WIDTH as u32, HEIGHT as u32)?)?;
        window.scale = state.scale;
        window.hover = hover.map(|p| window.unscaled(p));
        window.draw(state)?;
        let (x, y) = Self::at(state);
        let (w, h) = Self::size(state);
        let full = window.surface.pixels()?;
        let row = WIDTH as usize * 4;
        let bgra = (0..h as usize).flat_map(|y| full[y * row..y * row + w as usize * 4].iter().copied()).collect();
        Ok(crate::Layer { x, y, width: w as u32, height: h as u32, bgra })
    }

    /// Its size on screen.
    fn size(state: &SettingsState) -> (i32, i32) {
        ((WIDTH as f32 * state.scale).ceil() as i32, (HEIGHT as f32 * state.scale).ceil() as i32)
    }

    fn at(state: &SettingsState) -> (i32, i32) {
        let (w, h) = Self::size(state);
        (state.centre.0 - w / 2, state.centre.1 - h / 2)
    }

    /// A point on the window (px) where the layout has it.
    fn unscaled(&self, (x, y): (i32, i32)) -> (i32, i32) {
        ((x as f32 / self.scale).round() as i32, (y as f32 / self.scale).round() as i32)
    }

    fn with_surface(hwnd: HWND, surface: Surface) -> windows::core::Result<Self> {
        let font = |size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT| -> windows::core::Result<Font> {
            let format = surface.text_format(size, weight, align)?;
            let metrics = surface.cap_metrics(&format)?;
            Ok(Font { format, metrics })
        };
        let fonts = Fonts {
            title: font(24.0, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            caption: font(13.5, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            name: font(18.0, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            line: font(14.5, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            cell: font(15.5, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_CENTER)?,
            value: font(18.0, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        };
        Ok(Self { hwnd, surface, fonts, shown: None, placed: None, hover: None, hits: Vec::new(), pressed: None, scale: 1.0, consent: None })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// On screen.
    pub fn visible(&self) -> bool {
        (self.shown.is_some() || self.consent.is_some()) && self.placed.is_some()
    }

    pub fn rebuild(&mut self) -> windows::core::Result<()> {
        self.surface.rebuild()?;
        if let Some(consent) = self.consent.take() {
            return self.show_consent(Some(&consent));
        }
        match self.shown.take() {
            Some(shown) => self.show(Some(&shown)),
            None => Ok(()),
        }
    }

    /// The startup question drawn offscreen where it would show.
    pub fn render_consent_offscreen(graphics: &Rc<Graphics>, state: &ConsentState, hover: Option<(i32, i32)>) -> windows::core::Result<crate::Layer> {
        let mut window = Self::with_surface(HWND::default(), Surface::offscreen(graphics, WIDTH as u32, HEIGHT as u32)?)?;
        window.scale = state.scale;
        window.hover = hover.map(|p| window.unscaled(p));
        window.draw_consent(state)?;
        let (x, y, w, h) = Self::consent_rect(state);
        let full = window.surface.pixels()?;
        let row = WIDTH as usize * 4;
        let bgra = (0..h as usize).flat_map(|y| full[y * row..y * row + w as usize * 4].iter().copied()).collect();
        Ok(crate::Layer { x, y, width: w as u32, height: h as u32, bgra })
    }

    /// Where the question's card shows on screen: (left, top, width, height).
    fn consent_rect(state: &ConsentState) -> (i32, i32, i32, i32) {
        let (w, h) = ((CONSENT_W * state.scale).ceil() as i32, (CONSENT_H * state.scale).ceil() as i32);
        (state.centre.0 - w / 2, state.centre.1 - h / 2, w, h)
    }

    /// Shows the startup question, or hides it with `None`.
    pub fn show_consent(&mut self, state: Option<&ConsentState>) -> windows::core::Result<()> {
        let Some(state) = state else {
            if self.consent.take().is_some() {
                window::hide(self.hwnd);
                self.placed = None;
                self.hover = None;
            }
            return Ok(());
        };
        if self.consent.as_ref() == Some(state) {
            return Ok(());
        }
        self.scale = state.scale;
        self.draw_consent(state)?;
        let (x, y, w, h) = Self::consent_rect(state);
        if self.placed != Some((x, y)) {
            window::place_sized(self.hwnd, x, y, w, h);
            self.placed = Some((x, y));
        }
        self.consent = Some(state.clone());
        Ok(())
    }

    /// Draws the startup question: what is sent, what is not, where to change it; "Don't ask
    /// again", "Not now" and "Send statistics".
    fn draw_consent(&mut self, state: &ConsentState) -> windows::core::Result<()> {
        use crate::panel::ConsentChoice;
        let (p, sh) = (palette(), shades());
        let (w, h) = (CONSENT_W, CONSENT_H);
        self.hits.clear();
        self.surface.set_scale(self.scale);
        self.surface.begin();
        self.surface.fill_rect(rect(0.0, 0.0, w, h), sh.card);
        self.surface.fill_rect(rect(0.0, 0.0, w, HEADER_H), sh.strip);
        self.txt("Help improve the calculator", &self.fonts.title, PAD, w - PAD, HEADER_H / 2.0, p.white);
        self.surface.fill_rect(rect(0.0, HEADER_H - 1.0, w, HEADER_H), sh.edge);
        let right = w - PAD;
        self.txt("Send anonymous statistics to help tune the ballistics and the recognition?", &self.fonts.name, PAD, right, HEADER_H + 40.0, p.white);
        self.txt("Shot angles and misses, recognition and performance numbers, sent now and then.", &self.fonts.line, PAD, right, HEADER_H + 76.0, sh.line);
        self.txt("Plain numbers only: no screenshots, no player names. View or change it in Settings.", &self.fonts.line, PAD, right, HEADER_H + 102.0, sh.line);
        let cy = h - PAD - CONTROL_H / 2.0;
        self.cells(PAD, cy, 220.0, &[("Don't ask again", state.dont_ask, PanelAction::Consent(ConsentChoice::DontAsk(!state.dont_ask)))], true)?;
        let send = rect(right - 210.0, cy - CONTROL_H / 2.0, right, cy + CONTROL_H / 2.0);
        self.surface.fill_rect(send, if self.hovered(&send) { rgba(240, 190, 90, 1.0) } else { p.amber });
        self.txt("Send statistics", &self.fonts.cell, send.left, send.right, cy, p.ink);
        self.hits.push((send, PanelAction::Consent(ConsentChoice::Send)));
        self.cells(right - 210.0 - 12.0 - 150.0, cy, 150.0, &[("Not now", false, PanelAction::Consent(ConsentChoice::NotNow))], false)?;
        self.surface.stroke_rect(rect(0.5, 0.5, w - 0.5, h - 0.5), sh.edge, 1.0);
        self.surface.end()
    }

    pub fn show(&mut self, state: Option<&SettingsState>) -> windows::core::Result<()> {
        let Some(state) = state else {
            if self.shown.take().is_some() {
                window::hide(self.hwnd);
                self.placed = None;
                self.hover = None;
            }
            return Ok(());
        };
        if self.shown.as_ref() == Some(state) {
            return Ok(());
        }
        self.scale = state.scale;
        self.draw(state)?;
        let at = Self::at(state);
        if self.placed != Some(at) {
            let (w, h) = Self::size(state);
            window::place_sized(self.hwnd, at.0, at.1, w, h);
            self.placed = Some(at);
        }
        self.shown = Some(state.clone());
        Ok(())
    }

    pub fn press(&mut self, at: (i32, i32)) {
        self.pressed = Some(self.unscaled(at));
    }

    /// The button came up: the click where it went down.
    pub fn release(&mut self) -> Option<PanelAction> {
        let at = self.pressed.take()?;
        self.hits.iter().find(|(r, _)| inside(r, at)).map(|(_, a)| a.clone())
    }

    pub fn hover(&mut self, at: Option<(i32, i32)>) -> windows::core::Result<()> {
        let at = at.map(|p| self.unscaled(p));
        let before = self.hits.iter().position(|(r, _)| self.hover.is_some_and(|p| inside(r, p)));
        let after = self.hits.iter().position(|(r, _)| at.is_some_and(|p| inside(r, p)));
        self.hover = at;
        if before != after {
            if let Some(consent) = self.consent.clone() {
                self.draw_consent(&consent)?;
            } else if let Some(state) = self.shown.clone() {
                self.draw(&state)?;
            }
        }
        Ok(())
    }

    fn hovered(&self, r: &D2D_RECT_F) -> bool {
        self.hover.is_some_and(|p| inside(r, p))
    }

    fn txt(&self, text: &str, font: &Font, left: f32, right: f32, cy: f32, color: D2D1_COLOR_F) {
        self.surface.text_on_caps(text, &font.format, &font.metrics, left, right, cy, color);
    }

    /// Joined square cells of `width` each, from `left`, centred on `cy`; the selected ones amber.
    /// `check`: each cell is picked on its own (a box before its text, ticked when picked).
    fn cells(&mut self, left: f32, cy: f32, width: f32, cells: &[(&str, bool, PanelAction)], check: bool) -> windows::core::Result<()> {
        let (p, sh) = (palette(), shades());
        let (top, bottom) = (cy - CONTROL_H / 2.0, cy + CONTROL_H / 2.0);
        for (i, (text, selected, action)) in cells.iter().enumerate() {
            let x = left + i as f32 * width;
            let r = rect(x, top, x + width, bottom);
            let fill = if *selected {
                p.amber
            } else if self.hovered(&r) {
                p.hover
            } else {
                sh.cell
            };
            self.surface.fill_rect(r, fill);
            let ink = if *selected { p.ink } else { p.white };
            if check {
                // The box and the text centred together.
                const BOX: f32 = 15.0;
                let text_w = self.surface.text_width(text, &self.fonts.cell.format)?;
                let bx = (r.left + r.right) / 2.0 - (BOX + 10.0 + text_w) / 2.0;
                let b = rect(bx, cy - BOX / 2.0, bx + BOX, cy + BOX / 2.0);
                let edge = if *selected { ink } else { rgba(242, 243, 238, 0.70) };
                self.surface.stroke_rect(rect(b.left + 0.75, b.top + 0.75, b.right - 0.75, b.bottom - 0.75), edge, 1.5);
                if *selected {
                    self.surface.polyline(&[(b.left + 3.5, cy + 0.5), (b.left + 6.5, cy + 3.5), (b.right - 3.0, cy - 3.5)], ink, 2.0)?;
                }
                self.txt(text, &self.fonts.cell, b.right + 10.0 - 400.0, b.right + 10.0 + text_w + 400.0, cy, ink);
            } else {
                self.txt(text, &self.fonts.cell, r.left, r.right, cy, ink);
            }
            self.hits.push((r, action.clone()));
        }
        let all = rect(left, top, left + width * cells.len() as f32, bottom);
        for i in 1..cells.len() {
            let x = left + i as f32 * width;
            self.surface.fill_rect(rect(x, top, x + 1.0, bottom), sh.cell_edge);
        }
        self.surface.stroke_rect(rect(all.left + 0.5, all.top + 0.5, all.right - 0.5, all.bottom - 0.5), sh.cell_edge, 1.0);
        Ok(())
    }

    /// − value + ending at `right`, centred on `cy`; a button at the end of the range is dim and
    /// does nothing.
    fn stepper(&mut self, right: f32, cy: f32, value: &str, stepper: Stepper, (at_min, at_max): (bool, bool)) {
        let (p, sh) = (palette(), shades());
        let (top, bottom) = (cy - CONTROL_H / 2.0, cy + CONTROL_H / 2.0);
        let left = right - 2.0 * STEP_W - STEP_VALUE_W;
        for (step, end, x) in [(-1, at_min, left), (1, at_max, right - STEP_W)] {
            let r = rect(x, top, x + STEP_W, bottom);
            self.surface.fill_rect(r, if !end && self.hovered(&r) { p.hover } else { sh.cell });
            let color = if end { p.dim } else { p.white };
            let (mx, my) = (x + STEP_W / 2.0, cy);
            self.surface.fill_rect(rect(mx - 6.0, my - 0.8, mx + 6.0, my + 0.8), color);
            if step > 0 {
                self.surface.fill_rect(rect(mx - 0.8, my - 6.0, mx + 0.8, my + 6.0), color);
            }
            if !end {
                self.hits.push((r, PanelAction::Set(Setting::Step(stepper, step))));
            }
        }
        self.txt(value, &self.fonts.value, left + STEP_W, right - STEP_W, cy, p.white);
        for x in [left + STEP_W, right - STEP_W] {
            self.surface.fill_rect(rect(x, top, x + 1.0, bottom), sh.cell_edge);
        }
        self.surface.stroke_rect(rect(left + 0.5, top + 0.5, right - 0.5, bottom - 0.5), sh.cell_edge, 1.0);
    }

    /// A setting's name and the line under it saying what it does, in the row starting at `y`.
    fn named(&self, y: f32, name: &str, line: &str, right: f32) {
        let (p, sh) = (palette(), shades());
        self.txt(name, &self.fonts.name, PAD, right, y + 27.0, p.white);
        self.txt(line, &self.fonts.line, PAD, right, y + 52.0, sh.line);
    }

    fn caption(&self, y: f32, text: &str) {
        self.txt(text, &self.fonts.caption, PAD, WIDTH as f32 - PAD, y + CAPTION_H - 14.0, palette().grey);
    }

    /// Draws the window.
    fn draw(&mut self, state: &SettingsState) -> windows::core::Result<()> {
        let (p, sh) = (palette(), shades());
        let (w, h) = (WIDTH as f32, HEIGHT as f32);
        let right = w - PAD;
        self.hits.clear();
        self.surface.set_scale(self.scale);
        self.surface.begin();
        self.surface.fill_rect(rect(0.0, 0.0, w, h), sh.card);
        self.surface.fill_rect(rect(0.0, 0.0, w, HEADER_H), sh.strip);
        let rule = |s: &Surface, y: f32| s.fill_rect(rect(PAD, y, w - PAD, y + 1.0), sh.rule);

        // Title strip: the gear, the title, the close cross.
        gear(&self.surface, PAD + 8.0, HEADER_H / 2.0, p.grey)?;
        self.txt("Settings", &self.fonts.title, PAD + 28.0, w - HEADER_H, HEADER_H / 2.0, p.white);
        let close = rect(w - HEADER_H, 0.0, w, HEADER_H);
        if self.hovered(&close) {
            self.surface.fill_rect(close, p.hover);
        }
        let cross = if self.hovered(&close) { p.white } else { p.grey };
        let (cx, cy) = (w - HEADER_H / 2.0, HEADER_H / 2.0);
        self.surface.polyline(&[(cx - 7.0, cy - 7.0), (cx + 7.0, cy + 7.0)], cross, 1.8)?;
        self.surface.polyline(&[(cx - 7.0, cy + 7.0), (cx + 7.0, cy - 7.0)], cross, 1.8)?;
        self.hits.push((close, PanelAction::Settings(false)));
        self.surface.fill_rect(rect(0.0, HEADER_H - 1.0, w, HEADER_H), sh.edge);
        let mut y = HEADER_H;

        // Aiming: when the strip turns green.
        self.caption(y, "AIMING");
        y += CAPTION_H;
        let angle = state.mode == ToleranceMode::Angle;
        self.named(y, "Aim tolerance", "When the aim indicator turns green.", right - 340.0);
        self.cells(right - 340.0, y + ROW_H / 2.0, 170.0, &[
            ("By angle", angle, PanelAction::Set(Setting::Mode(ToleranceMode::Angle))),
            ("By miss distance", !angle, PanelAction::Set(Setting::Mode(ToleranceMode::Impact))),
        ], false)?;
        y += ROW_H;
        let steppers = if angle {
            [
                ("Elevation", "How far off the solution's elevation still counts as on target.", format!("±{:.2}°", state.angle.0)),
                ("Azimuth", "How far off the solution's azimuth still counts as on target.", format!("±{:.2}°", state.angle.1)),
            ]
        } else {
            [
                ("Range miss", "How far short or long it may land, by the trajectory to the target.", format!("±{:.0} m", state.impact.0)),
                ("Sideways miss", "How far left or right of the target it may land.", format!("±{:.0} m", state.impact.1)),
            ]
        };
        for (i, (name, line, value)) in steppers.iter().enumerate() {
            rule(&self.surface, y);
            self.named(y, name, line, right - 2.0 * STEP_W - STEP_VALUE_W - 16.0);
            let stepper = if i == 0 { Stepper::Elevation } else { Stepper::Azimuth };
            self.stepper(right, y + ROW_H / 2.0, value, stepper, state.limits[i]);
            y += ROW_H;
        }

        // Ammunition.
        self.caption(y, "AMMUNITION");
        y += CAPTION_H;
        let nearest = state.rule == ChargeRule::NearestCharge;
        self.named(y, "Charge advice", "Which charge to suggest when the loaded one can't reach.", right - 380.0);
        self.cells(right - 380.0, y + ROW_H / 2.0, 190.0, &[
            ("Closest charge", nearest, PanelAction::Set(Setting::Rule(ChargeRule::NearestCharge))),
            ("Least gun travel", !nearest, PanelAction::Set(Setting::Rule(ChargeRule::LeastMove))),
        ], false)?;
        y += ROW_H;

        // Map.
        self.caption(y, "MAP");
        y += CAPTION_H;
        self.named(y, "Map", "The debug map of a private server: used until a real map is recognised.", right - 300.0);
        self.cells(right - 300.0, y + ROW_H / 2.0, 150.0, &[
            ("Auto", !state.debug_map, PanelAction::Set(Setting::DebugMap(false))),
            ("Debug map", state.debug_map, PanelAction::Set(Setting::DebugMap(true))),
        ], false)?;
        y += ROW_H;
        rule(&self.surface, y);
        self.named(y, "Predicted impact", "Where the loaded shell lands at the gun's current angles.", right - 240.0);
        self.cells(right - 240.0, y + ROW_H / 2.0, 120.0, &[("Show", state.pred, PanelAction::Set(Setting::Pred(true))), ("Hide", !state.pred, PanelAction::Set(Setting::Pred(false)))], false)?;
        y += ROW_H;
        rule(&self.surface, y);
        self.named(y, "Target labels on the big map", "What the small label beside each target shows. Blast: the selected one's, when its ring is too small.", right);
        let l = state.labels;
        let field = |f: LabelField| PanelAction::Set(Setting::Label(f, !l.get(f)));
        let width = (right - PAD) / 5.0;
        self.cells(PAD, y + 76.0 + CONTROL_H / 2.0, width, &[
            ("Flight time", l.time, field(LabelField::Time)),
            ("Elevation", l.elevation, field(LabelField::Elevation)),
            ("Distance", l.distance, field(LabelField::Distance)),
            ("Precision", l.precision, field(LabelField::Precision)),
            ("Blast", l.blast, field(LabelField::Blast)),
        ], true)?;
        y += LABELS_ROW_H;

        // Calibration: values that start from what is measured on this machine.
        self.caption(y, "CALIBRATION");
        y += CAPTION_H;
        self.named(y, "Drag timing", "Moves the marks earlier or later if they lag or lead a dragged map.", right - 2.0 * STEP_W - STEP_VALUE_W - 16.0);
        let offset = match state.drag_offset_ms {
            0 => "Auto".to_owned(),
            ms => format!("{ms:+} ms").replace('-', "−"),
        };
        self.stepper(right, y + ROW_H / 2.0, &offset, Stepper::DragOffset, state.limits[2]);
        y += ROW_H;

        // Diagnostics: the statistics sent, a problem report, the logs.
        self.caption(y, "DIAGNOSTICS");
        y += CAPTION_H;
        let (button_w, gap) = (150.0, 12.0);
        self.named(y, "Share statistics", "Anonymous numbers only: no screenshots, no names.", right - 240.0 - gap - button_w);
        self.cells(right - 240.0 - gap - button_w, y + ROW_H / 2.0, 120.0, &[
            ("Send", state.share, PanelAction::Set(Setting::Share(true))),
            ("Don't send", !state.share, PanelAction::Set(Setting::Share(false))),
        ], false)?;
        self.cells(right - button_w, y + ROW_H / 2.0, button_w, &[("View data", false, PanelAction::Diag(crate::panel::DiagAction::ViewData))], false)?;
        y += ROW_H;
        rule(&self.surface, y);
        let report_line = state.report_note.as_deref().unwrap_or("Saves the last 10 minutes of logs for you to send us.");
        self.named(y, "Problem report", report_line, right - 190.0 - gap - button_w);
        self.cells(right - 190.0 - gap - button_w, y + ROW_H / 2.0, 190.0, &[("Screenshots", state.report_shots, PanelAction::Set(Setting::ReportShots(!state.report_shots)))], true)?;
        self.cells(right - button_w, y + ROW_H / 2.0, button_w, &[("Create report", false, PanelAction::Diag(crate::panel::DiagAction::CreateReport))], false)?;
        y += ROW_H;
        rule(&self.surface, y);
        self.named(y, "Log files", "Kept on this computer for 30 days.", right - button_w);
        self.cells(right - button_w, y + ROW_H / 2.0, button_w, &[("Open folder", false, PanelAction::Diag(crate::panel::DiagAction::OpenLogs))], false)?;
        y += ROW_H;

        // Footer.
        self.surface.fill_rect(rect(0.0, y + 10.0, w, y + 11.0), sh.rule);
        self.txt("Changes apply at once and are kept.", &self.fonts.line, PAD, right, y + 10.0 + (FOOTER_H - 10.0) / 2.0, sh.line);
        y += FOOTER_H;
        debug_assert!((y - h).abs() < 0.5, "laid out {y} px of {h}");
        self.surface.stroke_rect(rect(0.5, 0.5, w - 0.5, h - 0.5), sh.edge, 1.0);
        self.surface.end()
    }
}
