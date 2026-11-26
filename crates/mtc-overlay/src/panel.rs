//! The fire-control panel: the targets to pick, the gun update button
//! and the selected target's solution, built like the game's ammo cards: a translucent dark card
//! (about 60 % opaque, measured on the cards) with 1 px light lines across its full width between
//! sections, Montserrat Medium, grey labels and white values without shadows, and choices as
//! joined cells whose selected cell is filled with the game's amber — like the cards' charge rows.
//! Objectives carry a diamond like the game's objective icons.
//!
//! It sits above the aim strip, right edges aligned, so the HUD stays one group beside the
//! game's angle readout, and grows upwards only as far as its content: empty sections are left
//! out. The window is exactly the card: it takes clicks without taking focus, and no transparent
//! part of it swallows clicks meant for the game.
//!
//! The gear beside the padlock opens the settings window (`settings.rs`); amber while it
//! is open.
//!
//! The padlock at the top left unlocks the card: unlocked (open padlock, amber edge) it can be
//! dragged anywhere and reports where it was dropped; locked, presses are only clicks. Clicks act
//! on release, so a drag never selects anything. The low and high arcs are cells to click: the
//! chosen one is amber and the aim strip guides to it.
//!
//! Off the reference configuration the card is drawn at `PanelState::scale`, the game's HUD scale
//! (its ammo cards scale so): laid out at scale 1, the surface scales the drawing,
//! the window takes the scaled size and pointer positions are scaled back.

use std::rc::Rc;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, IDWriteTextFormat,
};

use crate::render::{Graphics, Surface, rgba};
use crate::window;

/// Window width and largest height in physical pixels.
pub const WIDTH: i32 = 336;
const MAX_HEIGHT: i32 = 300;
/// The surface is made for scales up to this; the window shows the scaled card.
const MAX_SCALE: f32 = 1.6;
const SURFACE: (u32, u32) = ((WIDTH as f32 * MAX_SCALE) as u32, (MAX_HEIGHT as f32 * MAX_SCALE) as u32);
const PAD: f32 = 12.0;
/// Width of one target cell: six markers fill the card.
const CELL_W: f32 = WIDTH as f32 / 6.0;
/// A press moves the card once the pointer has gone this far (px), else it is a click.
const DRAG_START_PX: i32 = 4;
/// The padlock's box (top left of the header), the gear's after it, and where the header label
/// starts after them.
const LOCK_LEFT: f32 = PAD;
const GEAR_LEFT: f32 = PAD + 18.0;
const LABEL_LEFT: f32 = PAD + 42.0;
/// The gun button's width, at the right of the header.
const GUN_W: f32 = 106.0;
const HEADER_H: f32 = 34.0;
const CELL_H: f32 = 36.0;
const ROW_H: f32 = 30.0;

#[derive(Clone, Debug, PartialEq)]
pub struct PanelTarget {
    /// "A3", or the objective's letter.
    pub label: String,
    pub objective: bool,
    /// The current shell reaches it (true while no shell is known).
    pub reachable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelArc {
    pub elevation: f64,
    pub time_of_flight: f64,
    /// The arc the aim strip guides to.
    pub active: bool,
    /// Outside the elevation the gun is known to reach: its angle is drawn amber.
    pub beyond_reach: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PanelSolution {
    pub distance_m: f64,
    pub height_m: f64,
    pub azimuth: f64,
    pub low: Option<PanelArc>,
    pub high: Option<PanelArc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelLoad {
    /// Nothing known loaded.
    Empty,
    Loading,
    Loaded,
    /// The loaded shell was fired (a rocket pod: its last rocket) and nothing is loaded since.
    Fired,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PanelState {
    /// Screen position of the card's bottom-right corner.
    pub anchor: (i32, i32),
    pub targets: Vec<PanelTarget>,
    pub selected: Option<String>,
    /// The selected target's solution.
    pub solution: Option<PanelSolution>,
    /// Name of the current shell without its charge ("M107 VT").
    pub shell: Option<String>,
    pub load: PanelLoad,
    /// The charge of the current shell ("Low"), when it has charges.
    pub charge: Option<String>,
    /// Recommended charge ("High"), when the shell has charges.
    pub recommended: Option<String>,
    pub gun_set: bool,
    /// Shown in place of the arcs when there is something to fix.
    pub notice: Option<String>,
    /// The selected marker's location precision (m) and whether it is too coarse.
    pub precision: Option<(f32, bool)>,
    /// Scoped: one line with the selected target, its distance and the charge.
    pub compact: bool,
    /// The card stays where it is; unlocked it can be dragged.
    pub locked: bool,
    /// The settings window is open (the gear shows amber).
    pub settings_open: bool,
    /// The scale the card is drawn at (1 at the reference configuration).
    pub scale: f32,
}

/// The diagnostics buttons in the settings window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagAction {
    /// Open the statistics waiting to be sent, as text.
    ViewData,
    /// Make a problem report in a folder and open it.
    CreateReport,
    /// Open the folder of the log files.
    OpenLogs,
}

/// An answer in the startup question about sending statistics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentChoice {
    /// Send them (the question does not come back).
    Send,
    /// Not now: asked again at the next start, or never with "Don't ask again" ticked.
    NotNow,
    /// "Don't ask again" ticked or not.
    DontAsk(bool),
}

/// What a click or a drag on the panel asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelAction {
    Diag(DiagAction),
    Consent(ConsentChoice),
    Select(String),
    UpdateGun,
    /// Aim with the high arc (`true`) or the low arc.
    Arc(bool),
    /// Lock (`true`) or unlock the card's position.
    Lock(bool),
    /// The card was dropped with its bottom-right corner here (screen px).
    Moved((i32, i32)),
    /// The pointer is over this target's row on the plan panel (`None`: over no row).
    Hover(Option<String>),
    /// Open (`true`) or close the plan panel's terrain profile.
    Terrain(bool),
    /// Open (`true`) or close the settings window.
    Settings(bool),
    /// List every target on the plan panel (`true`) or only the selected one.
    Targets(bool),
    /// A change in the settings window.
    Set(crate::settings::Setting),
}

/// A press on the card: where (window px and screen px) and where the card was then.
#[derive(Clone, Copy, Debug)]
struct Press {
    at: (i32, i32),
    screen: (i32, i32),
    window: (i32, i32),
    dragging: bool,
}

pub struct PanelWindow {
    hwnd: HWND,
    surface: Surface,
    label: IDWriteTextFormat,
    label_right: IDWriteTextFormat,
    caption: IDWriteTextFormat,
    caption_right: IDWriteTextFormat,
    cell: IDWriteTextFormat,
    value: IDWriteTextFormat,
    value_right: IDWriteTextFormat,
    headline: IDWriteTextFormat,
    button: IDWriteTextFormat,
    shown: Option<PanelState>,
    /// Window position and height (screen px).
    placed: Option<(i32, i32, i32)>,
    /// The pointer over the card, in its scale-1 layout.
    hover: Option<(i32, i32)>,
    /// The scale of the card on screen.
    scale: f32,
    /// Clickable areas of the last drawing.
    hits: Vec<(D2D_RECT_F, PanelAction)>,
    press: Option<Press>,
    /// After a drop: the anchor the HUD still sends until it takes the new one, and the new one.
    dropped: Option<((i32, i32), (i32, i32))>,
    /// The current drag's moves, logged on the drop.
    trace: Option<DragTrace>,
}

/// How closely a drag followed the pointer: moves applied and the gaps between them.
#[derive(Clone, Copy, Debug)]
struct DragTrace {
    started: std::time::Instant,
    last: std::time::Instant,
    moves: u32,
    longest: std::time::Duration,
}

pub(crate) struct Palette {
    pub(crate) white: D2D1_COLOR_F,
    pub(crate) grey: D2D1_COLOR_F,
    pub(crate) dim: D2D1_COLOR_F,
    pub(crate) amber: D2D1_COLOR_F,
    /// Text on amber: dark, as the game writes on its amber selections (the loading menu's
    /// chosen charge, the bar's loaded slot; measured about (24-60, 18-45, 7-18)).
    pub(crate) ink: D2D1_COLOR_F,
    /// Something to notice, on amber (amber text would not show there).
    pub(crate) ink_alert: D2D1_COLOR_F,
    pub(crate) green: D2D1_COLOR_F,
    pub(crate) line: D2D1_COLOR_F,
    pub(crate) hover: D2D1_COLOR_F,
}

pub(crate) fn palette() -> Palette {
    Palette {
        white: rgba(242, 243, 238, 1.0),
        grey: rgba(206, 210, 203, 0.78),
        dim: rgba(206, 210, 203, 0.42),
        amber: rgba(226, 174, 70, 1.0),
        ink: rgba(30, 24, 12, 1.0),
        ink_alert: rgba(140, 28, 20, 1.0),
        green: rgba(118, 214, 128, 1.0),
        line: rgba(235, 238, 228, 0.34),
        hover: rgba(255, 255, 255, 0.10),
    }
}

pub(crate) fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}

/// The window size (px) of a card `height` tall at scale 1, drawn at `scale`.
fn scaled(height: i32, scale: f32) -> (i32, i32) {
    ((WIDTH as f32 * scale).ceil() as i32, (height as f32 * scale).ceil() as i32)
}

pub(crate) fn inside(r: &D2D_RECT_F, (x, y): (i32, i32)) -> bool {
    let (x, y) = (x as f32, y as f32);
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// A padlock 12 px wide and 15 px tall with its top left at (`x`, `y`): the shackle closed into
/// the body, or lifted with its left leg free.
fn padlock(s: &Surface, x: f32, y: f32, locked: bool, color: D2D1_COLOR_F) -> windows::core::Result<()> {
    let body_top = y + 7.0;
    let lift = if locked { 0.0 } else { 3.5 };
    let (left, right) = (x + 2.5, x + 9.5);
    let r = (right - left) / 2.0;
    let (cx, cy) = (x + 6.0, y + 1.0 + r - lift);
    let mut points = vec![(left, if locked { body_top } else { cy + 1.5 })];
    points.extend((0..=8).map(|k| {
        let a = std::f32::consts::PI * (1.0 - k as f32 / 8.0);
        (cx + r * a.cos(), cy - r * a.sin())
    }));
    points.push((right, body_top + 1.0));
    s.polyline(&points, color, 2.0)?;
    s.fill_rect(rect(x, body_top, x + 12.0, body_top + 8.0), color);
    Ok(())
}

impl PanelWindow {
    pub fn new(graphics: &Rc<Graphics>) -> windows::core::Result<Self> {
        let hwnd = window::create_interactive(SURFACE.0 as i32, SURFACE.1 as i32)?;
        let surface = Surface::new(graphics, hwnd, SURFACE.0, SURFACE.1)?;
        Self::with_surface(hwnd, surface)
    }

    /// The panel drawn offscreen where it would show.
    pub fn render_offscreen(graphics: &Rc<Graphics>, state: &PanelState) -> windows::core::Result<crate::Layer> {
        let mut window = Self::with_surface(HWND::default(), Surface::offscreen(graphics, SURFACE.0, SURFACE.1)?)?;
        let (w, h) = scaled(window.draw(state)?, state.scale);
        let full = window.surface.pixels()?;
        let row = SURFACE.0 as usize * 4;
        let bgra = (0..h as usize).flat_map(|r| full[r * row..r * row + w as usize * 4].iter().copied()).collect();
        Ok(crate::Layer { x: state.anchor.0 - w, y: state.anchor.1 - h, width: w as u32, height: h as u32, bgra })
    }

    fn with_surface(hwnd: HWND, surface: Surface) -> windows::core::Result<Self> {
        let format = |size, weight, align| surface.text_format(size, weight, align);
        Ok(Self {
            label: format(15.0, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            label_right: format(15.0, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
            caption: format(12.5, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            caption_right: format(12.5, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
            cell: format(17.0, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_CENTER)?,
            value: format(17.0, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            value_right: format(17.0, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
            headline: format(23.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            button: format(13.5, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT_CENTER)?,
            hwnd,
            surface,
            shown: None,
            placed: None,
            hover: None,
            scale: 1.0,
            hits: Vec::new(),
            press: None,
            dropped: None,
            trace: None,
        })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Whether the card is following the pointer: the overlay then passes about every
    /// millisecond, as for a dragged map or marker, instead of waiting up to 15 ms per pass.
    pub fn dragging(&self) -> bool {
        self.press.is_some_and(|p| p.dragging)
    }

    /// The surface made again after a device loss (`Graphics::recover`), the shown panel redrawn.
    pub fn rebuild(&mut self) -> windows::core::Result<()> {
        self.surface.rebuild()?;
        match self.shown.take() {
            Some(shown) => self.show(Some(&shown)),
            None => Ok(()),
        }
    }

    pub fn show(&mut self, state: Option<&PanelState>) -> windows::core::Result<()> {
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
        let (width, height) = scaled(self.draw(state)?, state.scale);
        let anchor = match self.dropped {
            Some((stale, new)) if state.anchor == stale => new,
            _ => {
                self.dropped = None;
                state.anchor
            }
        };
        match (self.press, self.placed) {
            // Mid-drag the card stays under the pointer.
            (Some(p), Some((x, y, _))) if p.dragging => {
                window::place_sized(self.hwnd, x, y, width, height);
                self.placed = Some((x, y, height));
            }
            _ => self.place(anchor, width, height),
        }
        self.shown = Some(state.clone());
        Ok(())
    }

    /// The button went down at `at` (window px).
    pub fn press(&mut self, at: (i32, i32)) {
        if let Some((x, y, _)) = self.placed {
            self.press = Some(Press { at, screen: (x + at.0, y + at.1), window: (x, y), dragging: false });
            self.trace = None;
        }
    }

    /// The pointer moved to `at` (window px) with the button down: an unlocked card follows it.
    /// Whether the card is being dragged.
    pub fn drag(&mut self, at: (i32, i32)) -> bool {
        let (Some(press), Some(state), Some((x, y, height))) = (self.press.as_mut(), self.shown.as_ref(), self.placed) else { return false };
        if state.locked {
            return false;
        }
        let (dx, dy) = (x + at.0 - press.screen.0, y + at.1 - press.screen.1);
        if !press.dragging && dx.abs().max(dy.abs()) < DRAG_START_PX {
            return false;
        }
        press.dragging = true;
        let to = (press.window.0 + dx, press.window.1 + dy);
        window::place_sized(self.hwnd, to.0, to.1, scaled(0, self.scale).0, height);
        self.placed = Some((to.0, to.1, height));
        let now = std::time::Instant::now();
        self.trace = Some(match self.trace {
            Some(t) => DragTrace { last: now, moves: t.moves + 1, longest: t.longest.max(now - t.last), ..t },
            None => DragTrace { started: now, last: now, moves: 1, longest: std::time::Duration::ZERO },
        });
        true
    }

    /// The button came up: the end of a drag (where the card went), or a click where it went down.
    pub fn release(&mut self) -> Option<PanelAction> {
        let press = self.press.take()?;
        if !press.dragging {
            return self.click(press.at);
        }
        if let Some(t) = self.trace.take() {
            let span = (t.last - t.started).as_secs_f64() * 1000.0;
            log::info!(
                "[overlay] panel drag: {} moves, {span:.0} ms, mean gap {:.1} ms, longest {:.1} ms",
                t.moves,
                span / f64::from(t.moves.saturating_sub(1).max(1)),
                t.longest.as_secs_f64() * 1000.0
            );
        }
        let (x, y, height) = self.placed?;
        let anchor = (x + scaled(0, self.scale).0, y + height);
        if let Some(state) = &self.shown {
            self.dropped = Some((state.anchor, anchor));
        }
        Some(PanelAction::Moved(anchor))
    }

    fn place(&mut self, anchor: (i32, i32), width: i32, height: i32) {
        let at = (anchor.0 - width, anchor.1 - height, height);
        if self.placed != Some(at) {
            window::place_sized(self.hwnd, at.0, at.1, width, height);
            self.placed = Some(at);
        }
    }

    /// A window point in the card's scale-1 layout.
    fn unscaled(&self, (x, y): (i32, i32)) -> (i32, i32) {
        ((x as f32 / self.scale) as i32, (y as f32 / self.scale) as i32)
    }

    /// Pointer moved (window px) or left (`None`): redraws the hover highlight.
    pub fn hover(&mut self, at: Option<(i32, i32)>) -> windows::core::Result<()> {
        let at = at.map(|p| self.unscaled(p));
        let before = self.hits.iter().position(|(r, _)| self.hover.is_some_and(|p| inside(r, p)));
        let after = self.hits.iter().position(|(r, _)| at.is_some_and(|p| inside(r, p)));
        self.hover = at;
        if before != after
            && let Some(state) = self.shown.clone()
        {
            self.draw(&state)?;
        }
        Ok(())
    }

    /// The action under a click (window px).
    pub fn click(&self, at: (i32, i32)) -> Option<PanelAction> {
        let at = self.unscaled(at);
        self.hits.iter().find(|(r, _)| inside(r, at)).map(|(_, a)| a.clone())
    }

    fn hovered(&self, r: &D2D_RECT_F) -> bool {
        self.hover.is_some_and(|p| inside(r, p))
    }

    /// The one-line card used while scoped: target, distance, charge, or what is missing.
    fn draw_compact(&mut self, state: &PanelState) -> windows::core::Result<i32> {
        let p = palette();
        let (w, h) = (WIDTH as f32, HEADER_H);
        self.hits.clear();
        let s = &self.surface;
        s.set_scale(state.scale);
        s.begin();
        s.fill_rect(rect(0.0, 0.0, w, h), rgba(10, 13, 15, 0.60));
        let mut x = PAD;
        if let Some(label) = &state.selected {
            s.flat_text(label, &self.value, rect(x, 0.0, x + 50.0, h), p.amber);
            x += s.text_width(label, &self.value)? + 14.0;
        }
        if let Some(notice) = &state.notice {
            s.flat_text(notice, &self.label, rect(x, 0.0, w - PAD, h), p.amber);
        } else if let Some(sol) = &state.solution {
            s.flat_text(&format!("{:.0} m", sol.distance_m), &self.value, rect(x, 0.0, x + 100.0, h), p.white);
            let charge = match (&state.charge, &state.recommended) {
                (now, Some(best)) if now.as_ref() != Some(best) => Some((format!("use {best}"), p.amber)),
                (Some(now), _) => Some((now.clone(), p.white)),
                (None, _) => None,
            };
            if let Some((text, color)) = charge {
                s.flat_text(&text, &self.value_right, rect(w - PAD - 150.0, 0.0, w - PAD, h), color);
            }
        } else if state.selected.is_none() {
            s.flat_text("No target selected", &self.label, rect(x, 0.0, w - PAD, h), p.dim);
        }
        s.stroke_rect(rect(0.0, 0.0, w, h), if state.locked { p.line } else { p.amber }, 1.0);
        s.end()?;
        Ok(h as i32)
    }

    /// Draws the card from the top down and returns its height.
    fn draw(&mut self, state: &PanelState) -> windows::core::Result<i32> {
        if state.compact {
            return self.draw_compact(state);
        }
        let p = palette();
        let w = WIDTH as f32;
        let height = self.height(state);
        let h = height as f32;
        self.hits.clear();
        let s = &self.surface;
        s.set_scale(state.scale);
        s.begin();
        s.fill_rect(rect(0.0, 0.0, w, h), rgba(10, 13, 15, 0.60));
        let line = |y: f32| s.fill_rect(rect(0.0, y, w, y + 1.0), p.line);

        // Header: the padlock, the label and the gun button.
        let lock = rect(0.0, 0.0, GEAR_LEFT - 2.0, HEADER_H);
        let lock_color = if !state.locked { p.amber } else if self.hovered(&lock) { p.white } else { p.grey };
        padlock(s, LOCK_LEFT, (HEADER_H - 15.0) / 2.0, state.locked, lock_color)?;
        self.hits.push((lock, PanelAction::Lock(!state.locked)));
        let gear = rect(GEAR_LEFT - 2.0, 0.0, LABEL_LEFT - 6.0, HEADER_H);
        let gear_color = if state.settings_open { p.amber } else if self.hovered(&gear) { p.white } else { p.grey };
        crate::settings::gear(s, GEAR_LEFT + 8.0, HEADER_H / 2.0, gear_color)?;
        self.hits.push((gear, PanelAction::Settings(!state.settings_open)));
        s.flat_text("TARGETS", &self.label, rect(LABEL_LEFT, 0.0, 200.0, HEADER_H), p.grey);
        // Too coarse a location: what to do, in the free space of the header; the
        // amber precision under it names the target.
        if let (Some((_, true)), Some(_)) = (state.precision, &state.selected) {
            let x = LABEL_LEFT + s.text_width("TARGETS", &self.label)? + 8.0;
            s.flat_text("Zoom in V1", &self.caption, rect(x, 0.0, w - PAD - GUN_W - 4.0, HEADER_H), p.amber);
        }
        let gun = rect(w - PAD - GUN_W, 6.0, w - PAD, HEADER_H - 6.0);
        let gun_hover = self.hovered(&gun);
        if state.gun_set {
            if gun_hover {
                s.fill_rect(gun, p.hover);
            }
            s.stroke_rect(gun, if gun_hover { p.white } else { p.line }, 1.0);
            s.flat_text("UPDATE GUN", &self.button, gun, p.white);
        } else {
            s.fill_rect(gun, p.amber);
            s.flat_text("SET GUN", &self.button, gun, p.ink);
        }
        self.hits.push((gun, PanelAction::UpdateGun));
        let mut y = HEADER_H;
        line(y);

        // Target cells: a row of markers, a row of objectives.
        for objective in [false, true] {
            let row: Vec<&PanelTarget> = state.targets.iter().filter(|t| t.objective == objective).collect();
            if row.is_empty() {
                continue;
            }
            for (i, t) in row.iter().enumerate().take(6) {
                let x = i as f32 * CELL_W;
                let r = rect(x, y + 1.0, x + CELL_W, y + CELL_H);
                let selected = state.selected.as_deref() == Some(t.label.as_str());
                if selected {
                    s.fill_rect(r, p.amber);
                } else if self.hovered(&r) {
                    s.fill_rect(r, p.hover);
                }
                if i > 0 {
                    s.fill_rect(rect(x, y + 1.0, x + 1.0, y + CELL_H), p.line);
                }
                let color = if selected {
                    p.ink
                } else if t.reachable {
                    p.white
                } else {
                    p.dim
                };
                if objective {
                    let (cx, cy, d) = (x + CELL_W / 2.0, y + CELL_H / 2.0 + 0.5, 13.0);
                    let diamond = [(cx, cy - d), (cx + d, cy), (cx, cy + d), (cx - d, cy)];
                    s.polygon(&diamond, None, Some((color, 1.2)))?;
                }
                s.flat_text(&t.label, &self.cell, rect(r.left, r.top, r.right, r.bottom + 1.0), color);
                self.hits.push((r, PanelAction::Select(t.label.clone())));
            }
            y += CELL_H;
            line(y);
        }

        // The selected target (its cell above is amber): distance, height difference, azimuth,
        // small labels above the values like the game's readout; then both arcs.
        if let (Some(_), Some(sol)) = (&state.selected, &state.solution) {
            let columns = [PAD, PAD + 132.0];
            let (label_y, value_y) = (y + 6.0, y + 20.0);
            for (x, name) in columns.iter().zip(["DISTANCE", "HEIGHT"]) {
                s.flat_text(name, &self.caption, rect(*x, label_y, x + 110.0, label_y + 14.0), p.grey);
            }
            // The marker's precision right after DISTANCE; amber when too coarse.
            if let Some((metres, coarse)) = state.precision {
                let x = columns[0] + s.text_width("DISTANCE", &self.caption)? + 6.0;
                s.flat_text(&format!("±{metres:.1} m"), &self.caption, rect(x, label_y, columns[1] - 4.0, label_y + 14.0), if coarse { p.amber } else { p.grey });
            }
            s.flat_text("AZIMUTH", &self.caption_right, rect(w - PAD - 110.0, label_y, w - PAD, label_y + 14.0), p.grey);
            s.flat_text(&format!("{:.0} m", sol.distance_m), &self.headline, rect(columns[0], value_y, columns[1], value_y + 28.0), p.white);
            s.flat_text(&format!("{:+.1} m", sol.height_m), &self.value, rect(columns[1], value_y + 1.0, columns[1] + 100.0, value_y + 28.0), p.white);
            s.flat_text(&format!("{:06.2}°", sol.azimuth), &self.value_right, rect(w - PAD - 110.0, value_y + 1.0, w - PAD, value_y + 28.0), p.white);
            y += 54.0;
            if state.notice.is_none() {
                line(y);
                // Two cells like the target cells: the chosen arc amber, the other one to click.
                for (i, (name, arc)) in [("LOW", sol.low), ("HIGH", sol.high)].into_iter().enumerate() {
                    let x = i as f32 * w / 2.0;
                    let cell = rect(x, y + 1.0, x + w / 2.0, y + ROW_H);
                    let chosen = arc.is_some_and(|a| a.active);
                    if chosen {
                        s.fill_rect(cell, p.amber);
                    } else if arc.is_some() && self.hovered(&cell) {
                        s.fill_rect(cell, p.hover);
                    }
                    if i > 0 {
                        s.fill_rect(rect(x, y, x + 1.0, y + ROW_H), p.line);
                    }
                    s.flat_text(name, &self.label, rect(x + PAD, y, x + PAD + 48.0, y + ROW_H), if chosen { p.ink } else { p.grey });
                    match arc {
                        Some(a) => {
                            // Beyond the gun's reach: amber on the dark cell, dark red on the amber one.
                            let angle_color = match (a.beyond_reach, chosen) {
                                (true, false) => p.amber,
                                (true, true) => p.ink_alert,
                                (false, true) => p.ink,
                                (false, false) => p.white,
                            };
                            s.flat_text(&format!("{:.2}°", a.elevation), &self.value, rect(x + PAD + 48.0, y, x + PAD + 110.0, y + ROW_H), angle_color);
                            // Seconds written like the game's countdown ("17.1s").
                            s.flat_text(&format!("{:.1}s", a.time_of_flight), &self.label_right, rect(x + PAD + 100.0, y, x + w / 2.0 - PAD, y + ROW_H), if chosen { p.ink } else { p.white });
                            self.hits.push((cell, PanelAction::Arc(i == 1)));
                        }
                        None => s.flat_text("—", &self.value, rect(x + PAD + 48.0, y, x + PAD + 110.0, y + ROW_H), p.dim),
                    }
                }
                y += ROW_H;
            }
            line(y);
        } else if state.notice.is_none() && !state.targets.is_empty() {
            s.flat_text("Select a target", &self.label, rect(PAD, y, w - PAD, y + ROW_H), p.dim);
            y += ROW_H;
            line(y);
        }

        // What is missing for a solution.
        if let Some(notice) = &state.notice {
            s.flat_text(notice, &self.label, rect(PAD, y, w - PAD, y + ROW_H), p.amber);
            y += ROW_H;
            line(y);
        }

        // Shell with its loading state; its charge with the recommended one when that differs.
        s.flat_text("SHELL", &self.label, rect(PAD, y, PAD + 76.0, y + ROW_H), p.grey);
        match &state.shell {
            Some(shell) => {
                let (tag, tag_color) = match state.load {
                    PanelLoad::Loaded => ("LOADED", p.green),
                    PanelLoad::Loading => ("LOADING", p.amber),
                    PanelLoad::Fired => ("EMPTY", p.amber),
                    PanelLoad::Empty => ("", p.grey),
                };
                s.flat_text(shell, &self.value, rect(PAD + 76.0, y, w - PAD - 78.0, y + ROW_H), p.white);
                s.flat_text(tag, &self.label_right, rect(w - PAD - 76.0, y, w - PAD, y + ROW_H), tag_color);
            }
            // Not seen loading since the HUD started (or nothing loaded yet).
            None => s.flat_text("unknown", &self.value, rect(PAD + 76.0, y, w - PAD, y + ROW_H), p.dim),
        }
        y += ROW_H;
        if state.charge.is_some() || state.recommended.is_some() {
            line(y);
            s.flat_text("CHARGE", &self.label, rect(PAD, y, PAD + 76.0, y + ROW_H), p.grey);
            if let Some(now) = &state.charge {
                s.flat_text(now, &self.value, rect(PAD + 76.0, y, PAD + 170.0, y + ROW_H), p.white);
            }
            if let Some(best) = state.recommended.as_ref().filter(|r| state.charge.as_ref() != Some(*r)) {
                s.flat_text(&format!("use {best}"), &self.value_right, rect(w - PAD - 150.0, y, w - PAD, y + ROW_H), p.amber);
            }
            y += ROW_H;
        }
        debug_assert!((y - h).abs() < 0.5, "drawn height {y} vs measured {h}");
        s.stroke_rect(rect(0.0, 0.0, w, h), if state.locked { p.line } else { p.amber }, 1.0);
        s.end()?;
        Ok(height)
    }

    /// Height of the card for `state`, section by section as `draw` lays them out.
    fn height(&self, state: &PanelState) -> i32 {
        let mut h = HEADER_H;
        for objective in [false, true] {
            if state.targets.iter().any(|t| t.objective == objective) {
                h += CELL_H;
            }
        }
        if state.selected.is_some() && state.solution.is_some() {
            h += 54.0;
            if state.notice.is_none() {
                h += ROW_H;
            }
        } else if state.notice.is_none() && !state.targets.is_empty() {
            h += ROW_H;
        }
        if state.notice.is_some() {
            h += ROW_H;
        }
        h += ROW_H;
        if state.charge.is_some() || state.recommended.is_some() {
            h += ROW_H;
        }
        (h as i32).min(MAX_HEIGHT)
    }
}
