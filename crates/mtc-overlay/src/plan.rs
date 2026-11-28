//! The V1 plan panel, docked right of the full map: every target with its distance, the charge to
//! fire it with (in that charge's range ring colour), the flight time and the marker's precision;
//! the selected target's height, azimuth, precision and blast; its two arcs to pick from; the
//! current shell, what to load next (loading needs V3: press M), each charge's band and flight
//! time; where the gun position came from and the button to take it again, with the gear that
//! opens the settings window beside it (as on the fire-control panel, the gear shares a row with
//! the gun button; on V1 that panel is hidden, so one gear is on screen at a time).
//!
//! Built like the game's cards: translucent dark card with square corners, 1 px light lines,
//! Montserrat, grey captions over white values, the selection in solid amber with dark text (as
//! the game's menu and bar); rounded corners only where it copies the game's buttons and key
//! hints. Text is centred on its capitals' height, a little above the middle (optical centring).
//! Rows and arcs act on a click (release); the row under the pointer is reported so the map shows
//! that target.

use std::rc::Rc;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, IDWriteTextFormat,
};

use crate::map_marks::CHARGE_COLORS;
use crate::panel::{PanelAction, PanelLoad, inside, palette, rect};
use crate::lang::{t, tf};
use crate::render::{CapMetrics, Graphics, Surface, rgba};
use crate::window;

/// Card width, and the height (physical px) its rows shrink to stay within: the map is 882 px tall.
pub const WIDTH: i32 = 480;
pub const MAX_HEIGHT: i32 = 880;

/// The window size (px) of a card `height` tall at scale 1, drawn at `scale`.
fn scaled(height: i32, scale: f32) -> (i32, i32) {
    ((WIDTH as f32 * scale).ceil() as i32, (height as f32 * scale).ceil() as i32)
}
/// The card grows past `MAX_HEIGHT` rather than cut anything off, up to this (it docks 226 px down
/// a 1334 px client area).
pub const LIMIT_HEIGHT: i32 = 1100;
/// Rows shrink to this before the card reaches `LIMIT_HEIGHT`.
const ROW_TIGHT: f32 = 24.0;
const PAD: f32 = 16.0;
const NOTICE_H: f32 = 32.0;
const CAPTION_H: f32 = 32.0;
const ROW_H: f32 = 38.0;
/// Rows shrink to this when many targets would push the card past `MAX_HEIGHT`.
const ROW_MIN: f32 = 30.0;
const DETAIL_H: f32 = 58.0;
const ARC_H: f32 = 108.0;
const CHARGE_H: f32 = 150.0;
/// The terrain line, and the profile under it when open.
const TERRAIN_H: f32 = 44.0;
const PROFILE_H: f32 = 132.0;
/// Where the values of the lower sections (shell, next, terrain, gun) start: one column, clear
/// of the widest title.
const VALUE_X: f32 = PAD + 74.0;
/// The profile scales its ground (`HeightScale`) as if at least this high (m, and this share of
/// the distance: about 2.4 times the horizontal scale at most, or arcs climb as walls over flat
/// ground), and gives it this share of its height.
const PROFILE_MIN_RELIEF_M: f32 = 15.0;
const PROFILE_MIN_RELIEF_SHARE: f32 = 0.02;
const PROFILE_GROUND_SHARE: f32 = 0.4;
/// Arcs whose top stays within this many reliefs over the ground's knee draw linearly over the
/// whole profile (`HeightScale`), with this share of their height as headroom above.
const PROFILE_FLAT_RELIEFS: f32 = 2.0;
const PROFILE_HEADROOM: f32 = 0.12;
/// The shell row alone (no target selected).
const SHELL_H: f32 = 42.0;
/// A row of the shell list, opened under the SHELL row; the list's top margin.
const MENU_ROW_H: f32 = 34.0;
const MENU_TOP: f32 = 40.0;
const GUN_H: f32 = 52.0;
/// The squad row above the targets: whose markers the list follows.
const SQUAD_H: f32 = 36.0;
/// Where its letters start, clear of its title.
const SQUAD_LETTERS_X: f32 = 104.0;
/// Columns: the name's centre, the distance's right edge, the charge, the flight time, the
/// precision's right edge.
const COL_NAME: f32 = 38.0;
const COL_DIST: f32 = 170.0;
const COL_CHARGE: f32 = 190.0;
const COL_FLIGHT: f32 = 306.0;
/// The precision column ends before the box that folds the target list (as TERRAIN's).
const COL_PREC: f32 = WIDTH as f32 - PAD - 36.0;

#[derive(Clone, Debug, PartialEq)]
pub struct PlanRow {
    /// "A3", or the objective's letter.
    pub label: String,
    pub objective: bool,
    pub distance_m: Option<f64>,
    /// The charge to fire it with ("Medium") and its colour index; `None` without charges or
    /// when nothing reaches.
    pub charge: Option<(String, u8)>,
    /// Flight time with that charge on the target's arc (s); `None` when nothing reaches.
    pub time_s: Option<f64>,
    /// That arc is the high one.
    pub high_arc: bool,
    /// Some charge of the current shell reaches it (true while no shell is known).
    pub reachable: bool,
    /// The marker's location precision (m) and whether it is too coarse.
    pub precision: Option<(f32, bool)>,
}

/// One arc of the selected target with the charge to use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanArc {
    pub elevation: f64,
    pub time_s: f64,
    /// The elevation is beyond what the gun reaches.
    pub beyond_reach: bool,
    /// Where the terrain between stops it short of the target (m from the gun).
    pub blocked_m: Option<f32>,
    /// Where something narrower (a tree, a small house) may stop it first: a hint only.
    pub hint_m: Option<f32>,
}

/// One charge of the current shell for the selected target.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanChargeBox {
    pub name: String,
    /// Colour index (`CHARGE_COLORS`).
    pub index: u8,
    /// Its range band (km) within the gun's limits.
    pub band_km: Option<(f32, f32)>,
    /// Flight time on the target's arc (s), `None` when it does not reach.
    pub time_s: Option<f64>,
    pub loaded: bool,
    /// The charge to use.
    pub planned: bool,
}

/// An arc over the selected target's terrain profile.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileArc {
    /// Metres from the gun, metres above the gun's ground.
    pub points: Vec<(f32, f32)>,
    pub high: bool,
    /// Its charge's colour index (`CHARGE_COLORS`); `None` for a shell without charges.
    pub charge: Option<u8>,
    /// The arc the plan fires with.
    pub used: bool,
    /// Where the ground stops it, short of the target.
    pub stopped: Option<(f32, f32)>,
}

/// What the terrain line says about the selected target's arcs.
#[derive(Clone, Debug, PartialEq)]
pub enum TerrainNote {
    /// The arc to fire with clears the ground.
    Clear,
    /// The plan's other arc is stopped `at_m` from the gun by the terrain (`hard`), or may be by
    /// something narrower (a hint); the arc to fire with goes over it, chosen for
    /// that (`auto`) or by the player.
    OtherArc { high: bool, at_m: f32, auto: bool, hard: bool },
    /// The loaded charge's arc is stopped by the terrain (its name and colour index): the plan
    /// moved to another charge.
    LoadedCharge { name: String, index: u8, at_m: f32 },
    /// The arc to fire with is stopped by the terrain (`hard`: the player chose it, or no arc of
    /// any charge clears, `auto`), or may be by something narrower.
    Used { high: bool, at_m: f32, auto: bool, hard: bool },
    /// The arc to fire with passes under the heightmap's ground by less than the barrel's height:
    /// it may clear or not.
    Skims { high: bool, at_m: f32 },
}

/// The selected target's terrain profile along the line from the gun.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanTerrain {
    pub distance_m: f32,
    /// The target's ground above the gun's (m).
    pub target_m: f32,
    /// (m from the gun, m above the gun's ground), a little past the target.
    pub ground: Vec<(f32, f32)>,
    pub arcs: Vec<ProfileArc>,
    /// Where the arc to fire with skims the ground (`TerrainNote::Skims`).
    pub skims: Option<(f32, f32)>,
    pub note: TerrainNote,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanDetail {
    pub label: String,
    pub height_m: f64,
    pub azimuth: f64,
    pub precision: Option<(f32, bool)>,
    /// Damage reach and kill radius (m) of the current shell.
    pub blast_m: Option<(f32, f32)>,
    pub low: Option<PlanArc>,
    pub high: Option<PlanArc>,
    pub high_chosen: bool,
    /// The arc was chosen automatically.
    pub auto: bool,
    pub charges: Vec<PlanChargeBox>,
    pub terrain: Option<PlanTerrain>,
}

/// What to do next for the selected target (loading needs V3).
#[derive(Clone, Debug, PartialEq)]
pub enum PlanNext {
    /// The loaded charge reaches it.
    Ready,
    /// Press M, load this charge (name, colour index).
    Load(String, u8),
    /// Nothing known loaded: press M and load a shell.
    LoadShell,
    /// The rocket pod is empty: resupply before loading again.
    Resupply,
    /// Nothing reaches it.
    Nothing,
}

/// A squad member's letter on the squad row.
#[derive(Clone, Debug, PartialEq)]
pub struct SquadLetter {
    pub letter: char,
    /// Markers of that letter on the map.
    pub markers: u32,
    /// The player's own letter.
    pub own: bool,
}

/// The squad row: each letter with markers on the map, and the one whose markers are the targets.
#[derive(Clone, Debug, PartialEq)]
pub struct SquadRow {
    pub letters: Vec<SquadLetter>,
    pub following: char,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanState {
    /// Screen position of the card's top-left corner.
    pub at: (i32, i32),
    pub rows: Vec<PlanRow>,
    pub selected: Option<String>,
    /// The target under the pointer on the map (its row lights up).
    pub hover: Option<String>,
    /// Name of the current shell without its charge, the loaded charge (name, colour index) and
    /// the loading state.
    pub shell: Option<String>,
    /// The shell's data `type` ("TIME FUZE"): the game's icon before its name.
    pub shell_kind: Option<String>,
    pub loaded_charge: Option<(String, u8)>,
    /// The shell list behind the SHELL row (`None`: no ammunition data to list).
    pub menu: Option<crate::panel::PanelMenu>,
    /// The charge boxes set this shot's charge on a click (`None`: they do not); `Some(true)`: the
    /// loaded charge was set so.
    pub charge_manual: Option<bool>,
    pub load: PanelLoad,
    pub next: PlanNext,
    pub detail: Option<PlanDetail>,
    pub gun_set: bool,
    /// Where the gun position came from ("Map arrow · ±0.9 m").
    pub gun_source: Option<String>,
    /// What is missing for solutions.
    pub notice: Option<String>,
    /// The terrain profile is open (the line alone otherwise).
    pub terrain_open: bool,
    /// The settings window is open (the gear shows amber).
    pub settings_open: bool,
    /// Every target listed; folded, only the selected one.
    pub targets_open: bool,
    /// The squad row (`None`: one letter's markers on the map, or none; the row does not show).
    pub squad: Option<SquadRow>,
    /// The size the card is drawn at: 1, or smaller where it would not fit beside the full map of
    /// a smaller window (a planning card, kept readable while it fits).
    pub scale: f32,
}

/// A text format with its vertical metrics.
struct Font {
    format: IDWriteTextFormat,
    metrics: CapMetrics,
}

struct Fonts {
    caption: Font,
    caption_right: Font,
    title: Font,
    name: Font,
    letter: Font,
    value_right: Font,
    value16: Font,
    value16_right: Font,
    small: Font,
    small_right: Font,
    prec_right: Font,
    tof: Font,
    unit: Font,
    unit_right: Font,
    arc: Font,
    tag: Font,
    tag_right: Font,
    button: Font,
}

pub struct PlanWindow {
    hwnd: HWND,
    surface: Surface,
    fonts: Fonts,
    shown: Option<PlanState>,
    /// The language it was drawn in (a change redraws it).
    drawn_in: crate::lang::Language,
    placed: Option<(i32, i32, i32)>,
    hover: Option<(i32, i32)>,
    hits: Vec<(D2D_RECT_F, PanelAction)>,
    /// The row the pointer was last reported over.
    hover_row: Option<String>,
    press: Option<(i32, i32)>,
    /// The largest scale the window's surface holds the card at (made bigger when a card needs it).
    capacity: f32,
}

/// Colour of a charge index.
fn charge_color(index: u8) -> D2D1_COLOR_F {
    let (r, g, b) = CHARGE_COLORS[usize::from(index.min(2))];
    rgba(r, g, b, 1.0)
}

impl PlanWindow {
    pub fn new(graphics: &Rc<Graphics>) -> windows::core::Result<Self> {
        let hwnd = window::create_interactive(WIDTH, LIMIT_HEIGHT)?;
        let surface = Surface::new(graphics, hwnd, WIDTH as u32, LIMIT_HEIGHT as u32)?;
        Self::with_surface(hwnd, surface)
    }

    /// The card drawn offscreen where it would show.
    pub fn render_offscreen(graphics: &Rc<Graphics>, state: &PlanState) -> windows::core::Result<crate::Layer> {
        // Big enough for the card at its scale.
        let (surface_w, surface_h) = scaled(LIMIT_HEIGHT, state.scale.max(1.0));
        let mut window = Self::with_surface(HWND::default(), Surface::offscreen(graphics, surface_w as u32, surface_h as u32)?)?;
        let (w, height) = scaled(window.draw(state)?, state.scale);
        let full = window.surface.pixels()?;
        let row = surface_w as usize * 4;
        let bgra = (0..height as usize).flat_map(|y| full[y * row..y * row + w as usize * 4].iter().copied()).collect();
        Ok(crate::Layer { x: state.at.0, y: state.at.1, width: w as u32, height: height as u32, bgra })
    }

    fn with_surface(hwnd: HWND, surface: Surface) -> windows::core::Result<Self> {
        let font = |size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT| -> windows::core::Result<Font> {
            let format = surface.text_format(size, weight, align)?;
            let metrics = surface.cap_metrics(&format)?;
            Ok(Font { format, metrics })
        };
        let (l, r, c) = (DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_ALIGNMENT_CENTER);
        let (reg, med, bold) = (DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_BOLD);
        let fonts = Fonts {
            caption: font(12.0, reg, l)?,
            caption_right: font(12.0, reg, r)?,
            title: font(13.0, bold, l)?,
            name: font(17.0, bold, c)?,
            letter: font(13.0, bold, c)?,
            value_right: font(17.0, med, r)?,
            value16: font(16.0, med, l)?,
            value16_right: font(16.0, med, r)?,
            small: font(13.0, reg, l)?,
            small_right: font(13.0, reg, r)?,
            prec_right: font(15.0, med, r)?,
            tof: font(32.0, bold, l)?,
            unit: font(17.0, med, l)?,
            unit_right: font(17.0, med, r)?,
            arc: font(15.0, bold, l)?,
            tag: font(12.0, bold, l)?,
            tag_right: font(12.0, bold, r)?,
            button: font(14.0, bold, c)?,
        };
        Ok(Self { hwnd, surface, fonts, shown: None, drawn_in: crate::lang::Language::English, placed: None, hover: None, hits: Vec::new(), hover_row: None, press: None, capacity: 1.0 })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// The surface made again after a device loss (`Graphics::recover`), the shown card redrawn.
    pub fn rebuild(&mut self) -> windows::core::Result<()> {
        self.surface.rebuild()?;
        match self.shown.take() {
            Some(shown) => self.show(Some(&shown)),
            None => Ok(()),
        }
    }

    pub fn show(&mut self, state: Option<&PlanState>) -> windows::core::Result<()> {
        let Some(state) = state else {
            if self.shown.take().is_some() {
                window::hide(self.hwnd);
                self.placed = None;
                self.hover = None;
                self.hover_row = None;
                self.press = None;
            }
            return Ok(());
        };
        if self.shown.as_ref() == Some(state) && self.drawn_in == crate::lang::language() {
            return Ok(());
        }
        // A card drawn above the surface's scale (the game's HUD is larger than at the reference):
        // the surface made big enough first.
        if state.scale > self.capacity {
            let (w, h) = scaled(LIMIT_HEIGHT, state.scale);
            self.surface.resize_window((w as u32, h as u32))?;
            self.capacity = state.scale;
        }
        let (width, height) = scaled(self.draw(state)?, state.scale);
        let at = (state.at.0, state.at.1, height);
        if self.placed != Some(at) {
            window::place_sized(self.hwnd, at.0, at.1, width, height);
            self.placed = Some(at);
        }
        self.shown = Some(state.clone());
        self.drawn_in = crate::lang::language();
        Ok(())
    }

    /// The button went down at `at` (window px).
    pub fn press(&mut self, at: (i32, i32)) {
        self.press = Some(self.unscaled(at));
    }

    /// A window point in the card's scale-1 layout.
    fn unscaled(&self, (x, y): (i32, i32)) -> (i32, i32) {
        let k = self.shown.as_ref().map_or(1.0, |s| s.scale);
        ((x as f32 / k) as i32, (y as f32 / k) as i32)
    }

    /// The button came up: the click's action, where it went down.
    pub fn release(&mut self) -> Option<PanelAction> {
        let at = self.press.take()?;
        self.hits.iter().find(|(r, _)| inside(r, at)).map(|(_, a)| a.clone())
    }

    /// Pointer moved (window px) or left (`None`): redraws the hover highlight; the row now under
    /// the pointer when it changed.
    pub fn hover(&mut self, at: Option<(i32, i32)>) -> windows::core::Result<Option<PanelAction>> {
        let at = at.map(|p| self.unscaled(p));
        let before = self.hits.iter().position(|(r, _)| self.hover.is_some_and(|p| inside(r, p)));
        let after = self.hits.iter().position(|(r, _)| at.is_some_and(|p| inside(r, p)));
        self.hover = at;
        if before != after
            && let Some(state) = self.shown.clone()
        {
            self.draw(&state)?;
        }
        let row = after.and_then(|i| match &self.hits[i].1 {
            PanelAction::Select(label) => Some(label.clone()),
            _ => None,
        });
        if row != self.hover_row {
            self.hover_row = row.clone();
            return Ok(Some(PanelAction::Hover(row)));
        }
        Ok(None)
    }

    fn hovered(&self, r: &D2D_RECT_F) -> bool {
        self.hover.is_some_and(|p| inside(r, p))
    }

    /// The terrain profile shows: open, with a target's terrain to show.
    fn profile_shown(state: &PlanState) -> bool {
        state.terrain_open && state.detail.as_ref().is_some_and(|d| d.terrain.is_some())
    }

    /// The target rows listed: all, or folded only the selected one.
    fn rows_shown(state: &PlanState) -> Vec<&PlanRow> {
        state.rows.iter().filter(|r| state.targets_open || state.selected.as_deref() == Some(r.label.as_str())).collect()
    }

    fn fixed_height(state: &PlanState) -> f32 {
        Self::height_without_profile(state) + if Self::profile_shown(state) { PROFILE_H } else { 0.0 }
    }

    fn height_without_profile(state: &PlanState) -> f32 {
        let mut h = CAPTION_H + GUN_H;
        if state.squad.is_some() {
            h += SQUAD_H;
        }
        if state.notice.is_some() {
            h += NOTICE_H;
        }
        if let Some(d) = &state.detail {
            h += DETAIL_H + ARC_H + CHARGE_H;
            if d.terrain.is_some() {
                h += TERRAIN_H;
            }
        } else if state.shell.is_some() {
            h += SHELL_H;
        }
        h + Self::menu_height(state)
    }

    /// The open shell list's height (it pushes what is under the SHELL row down).
    fn menu_height(state: &PlanState) -> f32 {
        state.menu.as_ref().filter(|m| m.open && (state.shell.is_some() || state.detail.is_some())).map_or(0.0, |m| MENU_ROW_H * m.items.len() as f32 + 8.0)
    }

    /// Rows shrink (to `ROW_MIN`) to keep the card within the map's height, and further (to
    /// `ROW_TIGHT`) only to keep it within `LIMIT_HEIGHT`.
    fn row_height(state: &PlanState) -> f32 {
        let n = Self::rows_shown(state).len().max(1) as f32;
        let fixed = Self::fixed_height(state);
        let fit = |limit: i32| (limit as f32 - fixed) / n;
        let h = fit(MAX_HEIGHT).clamp(ROW_MIN, ROW_H);
        if fixed + h * n > LIMIT_HEIGHT as f32 { fit(LIMIT_HEIGHT).clamp(ROW_TIGHT, h) } else { h }
    }

    fn height(state: &PlanState) -> i32 {
        let h = Self::fixed_height(state) + Self::row_height(state) * Self::rows_shown(state).len() as f32;
        (h.ceil() as i32).min(LIMIT_HEIGHT)
    }

    /// `text` in `font` between `left` and `right`, its capitals centred on `cy`.
    fn txt(&self, text: &str, font: &Font, left: f32, right: f32, cy: f32, color: D2D1_COLOR_F) {
        self.surface.text_on_caps(text, &font.format, &font.metrics, left, right, cy, color);
    }

    fn width(&self, text: &str, font: &Font) -> windows::core::Result<f32> {
        self.surface.text_width(text, &font.format)
    }

    /// The game's bag capsule: three pills, `bags` filled.
    fn pills(&self, x: f32, cy: f32, bags: u8, color: D2D1_COLOR_F) {
        let s = &self.surface;
        for i in 0..3u8 {
            let bx = x + f32::from(i) * 8.0;
            if i < bags {
                s.rounded(rect(bx, cy - 6.0, bx + 6.0, cy + 6.0), 2.0, Some(color), None);
            } else {
                s.rounded(rect(bx + 0.5, cy - 5.5, bx + 5.5, cy + 5.5), 2.0, None, Some((D2D1_COLOR_F { a: color.a * 0.6, ..color }, 1.0)));
            }
        }
    }

    /// A small arc icon: a parabola from the gun to the target, flat or lobbed.
    fn arc_icon(&self, x: f32, y: f32, w: f32, h: f32, high: bool, color: D2D1_COLOR_F) -> windows::core::Result<()> {
        let lift = if high { 1.0 } else { 0.35 };
        let points: Vec<(f32, f32)> = (0..=16)
            .map(|i| {
                let u = i as f32 / 16.0;
                (x + w * u, y - h * lift * 4.0 * u * (1.0 - u))
            })
            .collect();
        self.surface.polyline(&points, color, 2.0)?;
        self.surface.circle(x + w, y, 2.5, color);
        Ok(())
    }

    /// The terrain line's words: amber when the ground stops an arc the plan would otherwise fire.
    fn terrain_note(&self, note: &TerrainNote, x: f32, right: f32, cy: f32) -> windows::core::Result<()> {
        let p = palette();
        let f = &self.fonts;
        let arc = |high: bool| if high { t("common.high") } else { t("common.low") };
        match note {
            TerrainNote::Clear => self.txt(t("terrain.clear"), &f.value16, x, right, cy, p.grey),
            TerrainNote::OtherArc { high, at_m, hard, .. } => {
                let key = if *hard { "terrain.hits_at" } else { "terrain.may_block_at" };
                self.txt(&tf(key, &[&arc(*high), &thousands(f64::from(*at_m))]), &f.value16, x, right, cy, p.amber)
            }
            TerrainNote::LoadedCharge { name, index, at_m } => {
                let color = charge_color(*index);
                self.pills(x, cy, index + 1, color);
                self.txt(name, &f.value16, x + 30.0, right, cy, color);
                let nx = x + 30.0 + self.width(name, &f.value16)? + 6.0;
                self.txt(&tf("terrain.loaded_hits_at", &[&thousands(f64::from(*at_m))]), &f.value16, nx, right, cy, p.amber);
            }
            TerrainNote::Skims { high, at_m } => self.txt(&tf("terrain.skims_at", &[&arc(*high), &thousands(f64::from(*at_m))]), &f.value16, x, right, cy, p.amber),
            TerrainNote::Used { high, at_m, auto, hard } => {
                let text = match (hard, auto) {
                    (true, true) => tf("terrain.none_clears", &[&arc(*high), &thousands(f64::from(*at_m))]),
                    (true, false) => tf("terrain.hits_at", &[&arc(*high), &thousands(f64::from(*at_m))]),
                    (false, _) => tf("terrain.may_block_at", &[&arc(*high), &thousands(f64::from(*at_m))]),
                };
                self.txt(&text, &f.value16, x, right, cy, p.amber);
            }
        }
        Ok(())
    }

    /// The terrain profile from the gun (left) to a little past the target: the ground, the arc to
    /// fire with in its charge's colour, arcs the ground stops dashed in theirs to an amber cross.
    /// Heights by `HeightScale`: the ground keeps its shape, arcs keep theirs.
    /// `loaded`: the loaded charge's colour index (`None`: unknown, or a shell without charges).
    fn profile(&self, t: &PlanTerrain, top: f32, loaded: Option<u8>) -> windows::core::Result<()> {
        let p = palette();
        let s = &self.surface;
        let f = &self.fonts;
        let (left, right) = (PAD + 8.0, WIDTH as f32 - PAD - 8.0);
        let (ptop, pbottom) = (top + 12.0, top + PROFILE_H - 30.0);
        let end = t.ground.last().map_or(t.distance_m, |g| g.0).max(1.0);
        let sx = |x: f32| left + (right - left) * x / end;
        let heights = t.ground.iter().map(|g| g.1).chain([0.0, t.target_m]);
        let (gmin, gmax) = heights.fold((f32::MAX, f32::MIN), |(lo, hi), h| (lo.min(h), hi.max(h)));
        let apex = t.arcs.iter().flat_map(|a| a.points.iter().map(|q| q.1)).fold(f32::MIN, f32::max);
        let scale = HeightScale::new(gmin, gmax, apex, t.distance_m, pbottom - ptop);
        let sy = |h: f32| pbottom - scale.px(h);
        s.push_clip(rect(left - 6.0, ptop - 6.0, right + 6.0, pbottom));
        // The ground, filled down to the frame.
        let mut outline: Vec<(f32, f32)> = vec![(sx(0.0), pbottom)];
        outline.extend(t.ground.iter().map(|&(x, h)| (sx(x), sy(h))));
        outline.push((sx(end), pbottom));
        s.polygon(&outline, Some(rgba(206, 210, 203, 0.14)), None)?;
        let ridge: Vec<(f32, f32)> = t.ground.iter().map(|&(x, h)| (sx(x), sy(h))).collect();
        if ridge.len() >= 2 {
            s.polyline(&ridge, rgba(235, 238, 228, 0.62), 1.5)?;
        }
        // Stopped arcs under the one to fire with. Lines by charge, as the map's range rings (the
        // two must not use solid and dashed the other way round): the loaded
        // charge solid and the charge the plan fires with long dashes, both thick; others thin short
        // dashes. An arc other than the one fired with is always one the ground stops (it ends at
        // the cross), so nothing else marks which is in use.
        let focus = t.arcs.iter().find(|a| a.used).and_then(|a| a.charge);
        let mut order: Vec<&ProfileArc> = t.arcs.iter().collect();
        order.sort_by_key(|a| a.used);
        for a in order {
            let points: Vec<(f32, f32)> = a.points.iter().map(|&(x, h)| (sx(x), sy(h))).collect();
            if points.len() < 2 {
                continue;
            }
            let color = a.charge.map_or(p.white, charge_color);
            let solid = a.charge.is_none() || loaded.is_none() || a.charge == loaded;
            if solid {
                s.polyline(&points, color, 2.5)?;
            } else if a.charge == focus {
                s.dashed_polyline(&points, color, 2.5, (10.0, 7.0))?;
            } else {
                s.dashed_polyline(&points, color, 1.5, (5.0, 5.0))?;
            }
        }
        s.pop_clip();
        // The gun and the target.
        s.circle(sx(0.0), sy(0.0), 3.5, p.white);
        let (tx, ty) = (sx(t.distance_m), sy(t.target_m));
        s.polygon(&[(tx, ty - 7.0), (tx + 7.0, ty), (tx, ty + 7.0), (tx - 7.0, ty)], Some(rgba(14, 17, 19, 0.9)), Some((p.amber, 2.0)))?;
        // Where the arc to fire with skims the ground: a ring round the spot.
        if let Some((x, h)) = t.skims {
            s.ring(sx(x), sy(h), 5.5, rgba(14, 17, 19, 0.9), 3.5);
            s.ring(sx(x), sy(h), 5.5, p.amber, 1.5);
        }
        // Where the ground stops an arc: a cross.
        for a in &t.arcs {
            if let Some((x, h)) = a.stopped {
                let (cx, cy) = (sx(x), sy(h));
                let arm = 5.0;
                for (dx, dy) in [(1.0, 1.0), (1.0, -1.0)] {
                    s.polyline(&[(cx - arm * dx, cy - arm * dy), (cx + arm * dx, cy + arm * dy)], rgba(14, 17, 19, 0.9), 4.5)?;
                }
                for (dx, dy) in [(1.0, 1.0), (1.0, -1.0)] {
                    s.polyline(&[(cx - arm * dx, cy - arm * dy), (cx + arm * dx, cy + arm * dy)], p.amber, 2.0)?;
                }
            }
        }
        // Distance ticks under the frame, the unit on the last.
        let step = [200.0, 500.0, 1000.0, 2000.0, 5000.0].into_iter().find(|&st| end / st <= 7.0).unwrap_or(10000.0);
        let ticks: Vec<f32> = (0..).map(|i| i as f32 * step).take_while(|&x| x <= end).collect();
        for (i, &x) in ticks.iter().enumerate() {
            let px = sx(x);
            s.fill_rect(rect(px - 0.5, pbottom, px + 0.5, pbottom + 4.0), p.dim);
            let km = x / 1000.0;
            let text = match (x == 0.0, i + 1 == ticks.len(), step < 1000.0) {
                (true, _, _) => "0".to_owned(),
                (false, true, true) => format!("{km:.1} km"),
                (false, true, false) => format!("{km:.0} km"),
                (false, false, true) => format!("{km:.1}"),
                (false, false, false) => format!("{km:.0}"),
            };
            let tw = self.width(&text, &f.caption)?;
            let tl = (px - tw / 2.0).clamp(left - 8.0, right + 8.0 - tw);
            self.txt(&text, &f.caption, tl, tl + tw + 2.0, pbottom + 14.0, p.grey);
        }
        Ok(())
    }

    /// Draws the card from the top down and returns its height.
    fn draw(&mut self, state: &PlanState) -> windows::core::Result<i32> {
        let p = palette();
        let w = WIDTH as f32;
        let height = Self::height(state);
        let h = height as f32;
        let row_h = Self::row_height(state);
        let f = &self.fonts;
        let mut hits = Vec::new();
        let s = &self.surface;
        s.set_scale(state.scale);
        s.begin();
        s.fill_rect(rect(0.0, 0.0, w, h), rgba(14, 17, 19, 0.80));
        let sep = |y: f32| s.fill_rect(rect(0.0, y, w, y + 1.0), rgba(235, 238, 228, 0.18));
        let edge = rgba(235, 238, 228, 0.43);
        let mut y = 0.0;

        // What is missing for solutions, only while it is missing.
        if let Some(notice) = &state.notice {
            self.txt(notice, &f.value16, PAD, w - PAD, y + NOTICE_H / 2.0, p.amber);
            y += NOTICE_H;
            sep(y);
        }

        // Whose markers the list follows: each letter with markers on the map, the player's
        // marked, the one followed as a selection (an amber tint and frame); a click follows it.
        if let Some(squad) = &state.squad {
            let cy = y + SQUAD_H / 2.0;
            self.txt(t("plan.markers"), &f.title, PAD, w, cy, p.grey);
            let mut x = SQUAD_LETTERS_X;
            for l in &squad.letters {
                let letter = l.letter.to_string();
                let count = l.markers.to_string();
                let (lw, cw) = (self.width(&letter, &f.arc)?, self.width(&count, &f.small)?);
                let you = if l.own { 8.0 + self.width(t("plan.you"), &f.tag)? } else { 0.0 };
                let chip = rect(x, cy - 13.0, x + 12.0 + lw + 8.0 + cw + you + 12.0, cy + 13.0);
                let following = l.letter == squad.following;
                let hovered = self.hovered(&chip);
                if following {
                    s.fill_rect(chip, rgba(228, 174, 71, if hovered { 0.26 } else { 0.18 }));
                    s.stroke_rect(chip, p.amber, 1.0);
                } else {
                    if hovered {
                        s.fill_rect(chip, p.hover);
                    }
                    s.stroke_rect(chip, if hovered { p.white } else { edge }, 1.0);
                }
                let mut tx = chip.left + 12.0;
                self.txt(&letter, &f.arc, tx, tx + lw + 2.0, cy, p.white);
                tx += lw + 8.0;
                self.txt(&count, &f.small, tx, tx + cw + 2.0, cy, if following { p.grey } else { p.dim });
                if l.own {
                    tx += cw + 8.0;
                    self.txt(t("plan.you"), &f.tag, tx, chip.right, cy, if following { p.amber } else { p.grey });
                }
                hits.push((chip, PanelAction::Follow(l.letter)));
                x = chip.right + 8.0;
            }
            y += SQUAD_H;
            sep(y);
        }

        // The targets: the caption row folds the list to the selected target (as TERRAIN).
        let cy = y + CAPTION_H / 2.0;
        let caption = rect(0.0, y, w, y + CAPTION_H);
        let caption_hovered = self.hovered(&caption);
        if caption_hovered {
            s.fill_rect(caption, p.hover);
        }
        self.txt(t("common.targets"), &f.title, PAD, w, cy, p.grey);
        self.txt(t("common.distance"), &f.caption_right, 0.0, COL_DIST, cy, p.grey);
        self.txt(t("common.charge"), &f.caption, COL_CHARGE, w, cy, p.grey);
        self.txt(t("plan.flight"), &f.caption, COL_FLIGHT, w, cy, p.grey);
        self.txt("± m", &f.caption_right, 0.0, COL_PREC, cy, p.grey);
        let fold = rect(w - PAD - 26.0, cy - 11.0, w - PAD, cy + 11.0);
        s.stroke_rect(fold, if caption_hovered { p.white } else { edge }, 1.0);
        let (fx, dir) = ((fold.left + fold.right) / 2.0, if state.targets_open { -1.0 } else { 1.0 });
        s.polyline(&[(fx - 5.0, cy - 2.5 * dir), (fx, cy + 2.5 * dir), (fx + 5.0, cy - 2.5 * dir)], p.white, 1.5)?;
        hits.push((caption, PanelAction::Targets(!state.targets_open)));
        y += CAPTION_H;
        let mut last_objective = None;
        for row in Self::rows_shown(state) {
            if last_objective == Some(false) && row.objective {
                sep(y);
            }
            last_objective = Some(row.objective);
            let r = rect(0.0, y, w, y + row_h);
            let selected = state.selected.as_deref() == Some(row.label.as_str());
            let hovered = self.hovered(&r) || state.hover.as_deref() == Some(row.label.as_str());
            // The selected row keeps every value's own colour (the charge's, a coarse precision's
            // amber): an amber tint and frame mark it, not a fill that would turn them dark.
            if selected {
                s.fill_rect(r, rgba(228, 174, 71, if hovered { 0.26 } else { 0.18 }));
                s.stroke_rect(rect(1.0, y + 1.0, w - 1.0, y + row_h - 1.0), p.amber, 2.0);
            } else if hovered {
                s.fill_rect(r, rgba(255, 255, 255, 0.13));
            }
            let fg = if row.reachable { p.white } else { p.dim };
            let name_color = if selected { p.amber } else { fg };
            let cy = y + row_h / 2.0;
            if row.objective {
                let d = 12.0;
                s.polygon(&[(COL_NAME, cy - d), (COL_NAME + d, cy), (COL_NAME, cy + d), (COL_NAME - d, cy)], None, Some((name_color, 2.0)))?;
                self.txt(&row.label, &f.letter, COL_NAME - 20.0, COL_NAME + 20.0, cy, name_color);
            } else {
                self.txt(&row.label, &f.name, COL_NAME - 30.0, COL_NAME + 30.0, cy, name_color);
            }
            if let Some(d) = row.distance_m {
                self.txt(&format!("{} m", thousands(d)), &f.value_right, 0.0, COL_DIST, cy, fg);
            }
            match (&row.charge, row.time_s) {
                (charge, Some(t)) => {
                    if let Some((name, index)) = charge {
                        let color = charge_color(*index);
                        self.pills(COL_CHARGE, cy, index + 1, color);
                        self.txt(name, &f.value16, COL_CHARGE + 30.0, COL_FLIGHT, cy, color);
                    }
                    self.arc_icon(COL_FLIGHT, cy + 5.0, 22.0, 11.0, row.high_arc, p.grey)?;
                    self.txt(&format!("{t:.1} s"), &f.value16, COL_FLIGHT + 30.0, COL_PREC - 26.0, cy, fg);
                }
                (_, None) if !row.reachable => self.txt(t("common.out_of_range"), &f.small, COL_CHARGE, COL_PREC, cy, fg),
                _ => {}
            }
            if let Some((m, coarse)) = row.precision {
                let color = if coarse { p.amber } else { p.grey };
                self.txt(&format!("{m:.1}"), &f.prec_right, 0.0, COL_PREC, cy, color);
            }
            hits.push((r, PanelAction::Select(row.label.clone())));
            y += row_h;
        }
        sep(y);

        // The selected target: its name boxed like the game's weapon label; height, azimuth,
        // precision, blast; then its two arcs.
        if let Some(d) = &state.detail {
            let cy = y + DETAIL_H / 2.0;
            s.stroke_rect(rect(PAD + 1.0, y + 12.0, PAD + 55.0, y + DETAIL_H - 12.0), p.amber, 2.0);
            self.txt(&d.label, &f.name, PAD, PAD + 56.0, cy, p.white);
            let columns = [86.0, 178.0, 270.0];
            self.txt(t("common.height"), &f.caption, columns[0], w, cy - 9.0, p.grey);
            self.txt(&format!("{:+.1} m", d.height_m), &f.value16, columns[0], w, cy + 10.0, p.white);
            self.txt(t("common.azimuth"), &f.caption, columns[1], w, cy - 9.0, p.grey);
            self.txt(&format!("{:06.2}°", d.azimuth), &f.value16, columns[1], w, cy + 10.0, p.white);
            if let Some((m, coarse)) = d.precision {
                self.txt(t("plan.precision"), &f.caption, columns[2], w, cy - 9.0, p.grey);
                self.txt(&format!("±{m:.1} m"), &f.value16, columns[2], w, cy + 10.0, if coarse { p.amber } else { p.white });
            }
            if let Some((reach, kill)) = d.blast_m {
                self.txt(t("plan.blast"), &f.caption_right, 0.0, w - PAD, cy - 9.0, p.grey);
                self.txt(&format!("{reach:.1} / {kill:.1} m"), &f.value16_right, 0.0, w - PAD, cy + 10.0, p.white);
            }
            y += DETAIL_H;
            sep(y);

            // The arcs: a two-way switch with a visible edge.
            let (bx0, bx1, by0, by1) = (14.0, w - 14.0, y + 12.0, y + ARC_H - 12.0);
            s.stroke_rect(rect(bx0 + 0.5, by0 + 0.5, bx1 - 0.5, by1 - 0.5), edge, 1.0);
            let half = (bx1 - bx0) / 2.0;
            for (i, (name, arc)) in [(t("common.low"), d.low), (t("common.high"), d.high)].into_iter().enumerate() {
                let x0 = bx0 + half * i as f32;
                let cell = rect(x0 + 1.0, by0 + 1.0, x0 + half - 1.0, by1 - 1.0);
                let chosen = arc.is_some() && d.high_chosen == (i == 1);
                if chosen {
                    s.fill_rect(cell, p.amber);
                } else if arc.is_some() && self.hovered(&cell) {
                    s.fill_rect(cell, p.hover);
                } else {
                    s.fill_rect(cell, rgba(255, 255, 255, 0.05));
                }
                let (c, sc) = if chosen {
                    (p.ink, p.ink)
                } else if arc.is_some() {
                    (p.white, p.grey)
                } else {
                    (p.dim, p.dim)
                };
                self.arc_icon(x0 + 14.0, by0 + 26.0, 34.0, 16.0, i == 1, c)?;
                self.txt(name, &f.arc, x0 + 58.0, x0 + half, by0 + 20.0, sc);
                // The terrain stops it before it was chosen automatically; something narrower
                // may (a hint).
                let tag = match (arc.and_then(|a| a.blocked_m), arc.and_then(|a| a.hint_m), chosen && d.auto) {
                    (Some(_), _, _) => Some((t("plan.blocked"), if chosen { p.ink_alert } else { p.amber })),
                    (None, Some(_), _) => Some((t("plan.may_hit"), if chosen { p.ink } else { p.amber })),
                    (None, None, true) => Some((t("plan.auto"), sc)),
                    (None, None, false) => None,
                };
                if let Some((tag, color)) = tag {
                    let nw = self.width(name, &f.arc)?;
                    self.txt(tag, &f.tag, x0 + 58.0 + nw + 8.0, x0 + half, by0 + 20.0, color);
                }
                match arc {
                    Some(a) => {
                        let tof = format!("{:.1}", a.time_s);
                        self.txt(&tof, &f.tof, x0 + 14.0, x0 + half, by1 - 24.0, c);
                        let tw = self.width(&tof, &f.tof)?;
                        self.txt("s", &f.unit, x0 + 14.0 + tw + 4.0, x0 + half, by1 - 24.0, c);
                        let el_color = match (a.beyond_reach, chosen) {
                            (true, true) => p.ink_alert,
                            (true, false) => p.amber,
                            (false, _) => sc,
                        };
                        self.txt(&format!("{:.2}°", a.elevation), &f.unit_right, x0, x0 + half - 14.0, by1 - 24.0, el_color);
                        hits.push((cell, PanelAction::Arc(i == 1)));
                    }
                    None => self.txt("—", &f.tof, x0 + 14.0, x0 + half, by1 - 24.0, p.dim),
                }
            }
            y += ARC_H;
            sep(y);

            // The terrain along the line: what it does to the arcs, and the profile.
            if let Some(t) = &d.terrain {
                let row = rect(0.0, y, w, y + TERRAIN_H);
                let hovered = self.hovered(&row);
                if hovered {
                    s.fill_rect(row, p.hover);
                }
                let cy = y + TERRAIN_H / 2.0;
                self.txt(crate::lang::t("plan.terrain"), &f.title, PAD, w, cy, p.grey);
                self.terrain_note(&t.note, VALUE_X, w - PAD - 36.0, cy)?;
                let b = rect(w - PAD - 26.0, cy - 13.0, w - PAD, cy + 13.0);
                s.stroke_rect(b, if hovered { p.white } else { edge }, 1.0);
                let open = Self::profile_shown(state);
                let (bx, dir) = ((b.left + b.right) / 2.0, if open { -1.0 } else { 1.0 });
                s.polyline(&[(bx - 5.0, cy - 2.5 * dir), (bx, cy + 2.5 * dir), (bx + 5.0, cy - 2.5 * dir)], p.white, 1.5)?;
                hits.push((row, PanelAction::Terrain(!open)));
                y += TERRAIN_H;
                if open {
                    self.profile(t, y, state.loaded_charge.as_ref().map(|(_, c)| *c))?;
                    y += PROFILE_H;
                }
                sep(y);
            }
        }

        // The current shell and its loading state; for a selected target what to do next and
        // the charges.
        if state.shell.is_some() || state.detail.is_some() {
            let cy = y + 21.0;
            self.txt(t("common.shell"), &f.title, PAD, w, cy, p.grey);
            let mut x = VALUE_X;
            let value_box = rect(VALUE_X - 8.0, y + 5.0, w - 100.0, y + 37.0);
            if state.menu.as_ref().is_some_and(|m| m.open) || (state.menu.is_some() && self.hovered(&value_box)) {
                s.fill_rect(value_box, p.hover);
            }
            let mut end;
            if let Some(shell) = &state.shell {
                if let Some(icon) = state.shell_kind.as_deref().and_then(crate::shell_icons::for_kind) {
                    s.image(icon, crate::shell_icons::SIDE, rect(x - 2.0, cy - 13.0, x + 24.0, cy + 13.0))?;
                    x += 30.0;
                }
                self.txt(shell, &f.value16, x, w - 90.0, cy, p.white);
                end = x + self.width(shell, &f.value16)?;
                if let Some((name, index)) = &state.loaded_charge {
                    let cx = end + 14.0;
                    self.pills(cx, cy, index + 1, charge_color(*index));
                    self.txt(name, &f.value16, cx + 30.0, w - 90.0, cy, charge_color(*index));
                    end = cx + 30.0 + self.width(name, &f.value16)?;
                }
            } else {
                self.txt(t("plan.none_known"), &f.value16, x, w, cy, p.dim);
                end = x + self.width(t("plan.none_known"), &f.value16)?;
            }
            if let Some(menu) = &state.menu {
                if menu.manual {
                    self.txt(t("common.manual"), &f.tag, end + 10.0, w, cy, p.amber);
                    end += 10.0 + self.width(t("common.manual"), &f.tag)?;
                }
                let (cx, open) = (end + 14.0, menu.open);
                let caret = if open { [(cx - 5.0, cy + 2.5), (cx + 5.0, cy + 2.5), (cx, cy - 3.0)] } else { [(cx - 5.0, cy - 2.5), (cx + 5.0, cy - 2.5), (cx, cy + 3.0)] };
                s.polygon(&caret, Some(if open || self.hovered(&value_box) { p.white } else { p.grey }), None)?;
                hits.push((value_box, PanelAction::Menu(!open)));
            }
            let (tag, color) = match state.load {
                PanelLoad::Loaded => (t("common.loaded"), p.green),
                PanelLoad::Loading => (t("common.loading"), p.amber),
                PanelLoad::Fired => (t("common.empty"), p.amber),
                _ => ("", p.grey),
            };
            self.txt(tag, &f.tag_right, 0.0, w - PAD, cy, color);
            // The open list under the SHELL row: a framed drop-down under the value column, the
            // rows below it moved down by its height.
            if let Some(menu) = state.menu.as_ref().filter(|m| m.open) {
                let (left, right) = (VALUE_X - 8.0, w - PAD);
                let top = y + MENU_TOP;
                let iconned = menu.items.iter().any(|i| i.kind.as_deref().and_then(crate::shell_icons::for_kind).is_some());
                s.fill_rect(rect(left, top, right, top + MENU_ROW_H * menu.items.len() as f32), rgba(10, 13, 15, 0.55));
                for (i, item) in menu.items.iter().enumerate() {
                    let r = rect(left, top + i as f32 * MENU_ROW_H, right, top + (i + 1) as f32 * MENU_ROW_H);
                    let ry = (r.top + r.bottom) / 2.0;
                    if item.current {
                        s.fill_rect(r, rgba(228, 174, 71, 0.22));
                        s.stroke_rect(r, p.amber, 1.0);
                    } else if self.hovered(&r) {
                        s.fill_rect(r, p.hover);
                    }
                    if let Some(icon) = item.kind.as_deref().and_then(crate::shell_icons::for_kind) {
                        s.image(icon, crate::shell_icons::SIDE, rect(left + 8.0, ry - 13.0, left + 34.0, ry + 13.0))?;
                    }
                    let label = crate::panel::menu_label(item);
                    self.txt(&label, &f.value16, if iconned { left + 42.0 } else { left + 10.0 }, right - 110.0, ry, p.white);
                    if let Some(d) = &item.detail {
                        self.txt(d, &f.tag_right, 0.0, right - 10.0, ry, if item.current { p.amber } else { p.grey });
                    }
                    hits.push((r, PanelAction::Pick(item.pick.clone())));
                }
                s.stroke_rect(rect(left, top, right, top + MENU_ROW_H * menu.items.len() as f32), edge, 1.0);
                y += Self::menu_height(state);
            }
            if let Some(d) = &state.detail {
                let ny = y + 50.0;
                self.txt(t("plan.next"), &f.title, PAD, w, ny, p.grey);
                let key = rect(VALUE_X, ny - 12.0, VALUE_X + 26.0, ny + 12.0);
                let key_cap = |label: &str| {
                    s.rounded(key, 5.0, Some(rgba(214, 218, 212, 1.0)), None);
                    self.txt(label, &f.button, key.left, key.right, ny, p.ink);
                };
                match &state.next {
                    PlanNext::Ready if state.load == PanelLoad::Loading => self.txt(t("plan.loading_right_charge"), &f.value16, VALUE_X, w, ny, p.amber),
                    PlanNext::Ready => self.txt(t("plan.ready"), &f.value16, VALUE_X, w, ny, p.green),
                    PlanNext::Resupply => self.txt(t("plan.resupply"), &f.value16, VALUE_X, w, ny, p.amber),
                    PlanNext::Load(name, index) => {
                        key_cap("M");
                        self.txt(t("plan.then_load"), &f.value16, key.right + 10.0, w, ny, p.white);
                        let x = key.right + 10.0 + self.width(t("plan.then_load"), &f.value16)? + 8.0;
                        self.pills(x, ny, index + 1, charge_color(*index));
                        self.txt(name, &f.value16, x + 30.0, w, ny, charge_color(*index));
                    }
                    PlanNext::LoadShell => {
                        key_cap("M");
                        self.txt(t("plan.then_load_shell"), &f.value16, key.right + 10.0, w, ny, p.white);
                    }
                    PlanNext::Nothing => self.txt(t("plan.no_charge_reaches"), &f.value16, VALUE_X, w, ny, p.dim),
                }
                let n = d.charges.len().max(1) as f32;
                let cw = (w - 28.0 - 8.0 * (n - 1.0)) / n;
                for (k, c) in d.charges.iter().enumerate() {
                    let x0 = 14.0 + k as f32 * (cw + 8.0);
                    let b = rect(x0, y + 76.0, x0 + cw, y + 136.0);
                    let reach = c.time_s.is_some();
                    let color = charge_color(c.index);
                    // The frame: the charge's colour for the one to use; the loaded one's top
                    // edge leaves a gap for its LOADED legend (beside the name it would crowd
                    // "Medium").
                    let (frame, t) = if c.planned { (color, 2.0) } else { (edge, 1.0) };
                    // A click sets this shot's charge (the box set so, again: as read).
                    if let Some(manual) = state.charge_manual {
                        if self.hovered(&b) {
                            s.fill_rect(b, p.hover);
                        }
                        hits.push((b, PanelAction::Pick(crate::panel::MenuPick::Charge(if c.loaded && manual { None } else { Some(c.index) }))));
                    }
                    let manual = c.loaded && state.charge_manual == Some(true);
                    let legend = if manual { crate::lang::t("common.manual") } else { crate::lang::t("common.loaded") };
                    let gap = if c.loaded {
                        let tw = self.width(legend, &f.tag)?;
                        Some((b.right - 12.0 - tw - 5.0, b.right - 12.0 + 5.0))
                    } else {
                        None
                    };
                    match gap {
                        Some((g0, g1)) => {
                            s.fill_rect(rect(b.left, b.top, g0, b.top + t), frame);
                            s.fill_rect(rect(g1, b.top, b.right, b.top + t), frame);
                            self.txt(legend, &f.tag, g0 + 5.0, g1, b.top + t / 2.0, if manual { p.amber } else { p.green });
                        }
                        None => s.fill_rect(rect(b.left, b.top, b.right, b.top + t), frame),
                    }
                    s.fill_rect(rect(b.left, b.bottom - t, b.right, b.bottom), frame);
                    s.fill_rect(rect(b.left, b.top, b.left + t, b.bottom), frame);
                    s.fill_rect(rect(b.right - t, b.top, b.right, b.bottom), frame);
                    let name_color = if reach { color } else { p.dim };
                    self.pills(x0 + 12.0, y + 96.0, c.index + 1, name_color);
                    self.txt(&c.name, &f.value16, x0 + 42.0, x0 + cw, y + 96.0, name_color);
                    if let Some((lo, hi)) = c.band_km {
                        self.txt(&format!("{lo:.1}–{hi:.1} km"), &f.small, x0 + 12.0, x0 + cw, y + 120.0, if reach { color } else { D2D1_COLOR_F { a: 0.6, ..color } });
                    }
                    self.txt(&c.time_s.map_or("—".to_owned(), |t| format!("{t:.1} s")), &f.small_right, x0, x0 + cw - 10.0, y + 120.0, if reach { p.grey } else { p.dim });
                }
                y += CHARGE_H;
            } else {
                y += SHELL_H;
            }
            sep(y);
        }

        // Where the gun position came from; take it again.
        let cy = y + GUN_H / 2.0;
        self.txt(t("plan.gun"), &f.title, PAD, w, cy, p.grey);
        let source = state.gun_source.as_deref().unwrap_or(t("plan.gun_unknown"));
        self.txt(source, &f.value16, VALUE_X, w - PAD - 150.0 - 46.0, cy, if state.gun_set { p.white } else { p.amber });
        let gear = rect(w - PAD - 144.0 - 46.0, y + 11.0, w - PAD - 152.0, y + GUN_H - 11.0);
        let gear_hover = self.hovered(&gear);
        s.rounded(gear, 6.0, Some(if gear_hover { rgba(44, 52, 58, 0.95) } else { rgba(24, 30, 34, 0.95) }), Some((if state.settings_open { p.amber } else if gear_hover { p.white } else { edge }, 1.0)));
        crate::settings::gear(s, (gear.left + gear.right) / 2.0, cy, if state.settings_open { p.amber } else if gear_hover { p.white } else { p.grey })?;
        hits.push((gear, PanelAction::Settings(!state.settings_open)));
        let gun = rect(w - PAD - 144.0, y + 11.0, w - PAD, y + GUN_H - 11.0);
        if state.gun_set {
            let hover = self.hovered(&gun);
            s.rounded(gun, 6.0, Some(if hover { rgba(44, 52, 58, 0.95) } else { rgba(24, 30, 34, 0.95) }), Some((if hover { p.white } else { edge }, 1.0)));
            self.txt(t("common.update_gun"), &f.button, gun.left, gun.right, cy, p.white);
        } else {
            s.rounded(gun, 6.0, Some(p.amber), None);
            self.txt(t("common.set_gun"), &f.button, gun.left, gun.right, cy, p.ink);
        }
        hits.push((gun, PanelAction::UpdateGun));
        s.end()?;
        self.hits = hits;
        Ok(height)
    }
}

/// The terrain profile's heights (m above the gun's ground) to px above the frame's bottom.
///
/// Flat arcs that stay near the ground (their top within two reliefs over the `knee`, a little
/// over the highest ground): linear from a little under the lowest ground to the highest arc with
/// some headroom, the whole frame (on Fulvia Gap A3, a 5.9° arc 1.8 km out drew in the bottom
/// third of the frame under the lobbed top below).
///
/// Higher arcs, ground a few tens of metres high under arcs hundreds to thousands of metres high:
/// true to scale, the ground is a flat line; scaled for the ground, every arc is a wall. So
/// heights are linear up to the knee over `PROFILE_GROUND_SHARE` of the frame, and logarithmic
/// above, to the top of a lobbed arc (60 % of the distance high, a 67° arc's top) or the highest
/// arc drawn if higher: a low arc stays visibly lower than a high one. The log's scale makes its
/// slope at the knee the linear part's, so arcs bend there without a kink (five mappings were
/// compared on sample profiles: square-root arches, kinked asinh, clipped, and this).
struct HeightScale {
    floor: f32,
    knee: f32,
    top: f32,
    /// The linear part's height and the whole frame's (px): the whole frame for flat arcs.
    linear: f32,
    span: f32,
    /// The log's scale (m).
    soft: f32,
}

impl HeightScale {
    fn new(ground_min: f32, ground_max: f32, apex: f32, distance_m: f32, span: f32) -> Self {
        let relief = (ground_max - ground_min).max(PROFILE_MIN_RELIEF_M).max(PROFILE_MIN_RELIEF_SHARE * distance_m);
        let floor = ground_min - 0.25 * relief;
        let knee = ground_max + 0.6 * relief;
        let ceiling = apex.max(ground_max);
        if ceiling <= knee + PROFILE_FLAT_RELIEFS * relief {
            let top = ceiling + PROFILE_HEADROOM * (ceiling - floor);
            return Self { floor, knee: top, top, linear: span, span, soft: 1.0 };
        }
        let top = (0.6 * distance_m).max(apex).max(knee + relief);
        let linear = span * PROFILE_GROUND_SHARE;
        // The log's slope at the knee, (span - linear) / (soft · ln(1 + range / soft)), falls as
        // `soft` grows: halve the bracket to the linear part's slope.
        let slope = linear / (knee - floor);
        let range = top - knee;
        let (mut lo, mut hi) = (1e-3_f32, range * 100.0);
        for _ in 0..64 {
            let mid = (lo + hi) / 2.0;
            if (span - linear) / (mid * (1.0 + range / mid).ln()) > slope {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Self { floor, knee, top, linear, span, soft: (lo + hi) / 2.0 }
    }

    fn px(&self, h: f32) -> f32 {
        if h <= self.knee {
            self.linear * (h - self.floor) / (self.knee - self.floor)
        } else {
            self.linear + (self.span - self.linear) * (1.0 + (h - self.knee) / self.soft).ln() / (1.0 + (self.top - self.knee) / self.soft).ln()
        }
    }
}

/// "3467" → "3 467" (a space groups thousands, as the game's panels write metres).
fn thousands(v: f64) -> String {
    let n = v.round() as i64;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3.5 km line over 40 m of ground, 90 px high: the ground keeps its share, the scale has no
    /// step or kink at the knee, and a low arc's top stays well under a high one's.
    #[test]
    fn the_profile_scale_keeps_the_ground_and_tells_arcs_apart() {
        let scale = HeightScale::new(-10.0, 30.0, 1900.0, 3467.0, 90.0);
        assert!(scale.px(-10.0) > 0.0 && scale.px(30.0) < 90.0 * PROFILE_GROUND_SHARE);
        let (k, e) = (scale.knee, 0.01);
        let below = (scale.px(k) - scale.px(k - e)) / e;
        let above = (scale.px(k + e) - scale.px(k)) / e;
        assert!((scale.px(k + 1e-3) - scale.px(k)).abs() < 0.01, "no step at the knee");
        assert!((above / below - 1.0).abs() < 0.02, "slopes {below} below and {above} above the knee");
        assert!((scale.px(scale.top) - 90.0).abs() < 0.01, "the lobbed reference reaches the top");
        let (low, high) = (scale.px(420.0), scale.px(1900.0));
        assert!(low < 0.85 * high, "a 420 m top at {low:.1} px against a 1900 m one at {high:.1} px");
    }

    /// A low arc climbing well over the ground: the top is the lobbed reference, not the arc,
    /// which stays low.
    #[test]
    fn a_lone_low_arc_does_not_fill_the_profile() {
        let scale = HeightScale::new(0.0, 20.0, 400.0, 3467.0, 90.0);
        assert!(scale.px(400.0) < 0.8 * 90.0, "{:.1} px", scale.px(400.0));
    }

    /// A flat arc near the ground (Fulvia Gap A3: a 5.9° arc 1.8 km out over 37 m of ground, its
    /// top about 50 m up): linear over the whole frame, the arc's top under the headroom.
    #[test]
    fn a_flat_arc_fills_the_profile_linearly() {
        let scale = HeightScale::new(-2.0, 38.0, 52.0, 1830.0, 90.0);
        let (a, b, c) = (scale.px(0.0), scale.px(20.0), scale.px(40.0));
        assert!(((b - a) - (c - b)).abs() < 1e-3, "linear: {a:.2} {b:.2} {c:.2}");
        let top = scale.px(52.0);
        assert!(top > 0.8 * 90.0 && top < 90.0, "the arc's top at {top:.1} px of 90");
    }
}
