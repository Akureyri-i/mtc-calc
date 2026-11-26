//! Target marks over the game's maps, drawn in the game's own map style: 2 px diamond outlines like
//! its markers (no shadow), labels in Titillium Web with a dark outline like its marker labels, all
//! centred on the diamond the game draws.
//!
//! - The selected target, the calculator's red target point: a red diamond around the game's, with
//!   its blast drawn at map scale underneath — the damage reach and the kill radius of the current
//!   shell, and for a cluster shell the area its submunitions cover (dashed). A selected objective
//!   gets the red diamond outside its icon.
//! - A marker whose diamond or label an objective icon covers: a white diamond and its label where
//!   the game's are.
//! - Every marker: its location precision at the lower right, mirroring the game's label at the
//!   upper right (`±2.0 m`); amber with a prompt to zoom in when coarser than `coarse_m`. On the
//!   minimap (V3, no labels in the game) the label instead.
//! - A blast too small to show around the selected diamond (3VO28's bomblet: 9.8 m, ~15 px even
//!   at V1's 5× zoom, inside the 20 px diamond) is written under the precision (`blast 9.8 m`).
//!
//! On V1 a planning layer (`Planning`) replaces the precision text: one small label per target
//! where the precision was, showing what the settings pick (`LabelFields`: flight time and
//! elevation in the charge's colour, distance, precision; nothing by default — a location too
//! coarse to aim by shows anyway; the selected one adds its blast when too small to draw), placed
//! clear of the game's diamonds, labels and icons and of each other (shortened, else left out: the
//! plan panel lists every target) and kept in its place while it fits; the current shell's
//! charges' range rings clipped to the map's picture; the hovered target's arcs in a popover.
//!
//! The window is click-through and covers the map container with a margin for text; marks
//! outside the container (a zoomed V1) are left out, and the blast is clipped to it.
//!
//! While V1 is dragged (`PanFollow`) the marks are drawn once with `PAD` px around the window and
//! then only moved: the visual's offset follows the cursor, handed to the compositor as often as
//! the cursor moves, without drawing (drawing and presenting each time reached the screen 5–7 ms
//! later, and later still while the GPU drew the game). Marks the HUD sends meanwhile that are the
//! drawn ones moved with the map change nothing; anything else, or a pan near the edge of what is
//! drawn, is drawn again in the same compositor commit as the new offset (a DirectComposition
//! surface, `Surface::layered`).
//!
//! A marker being dragged on V1 is drawn in a window over the whole desktop (`DragWindow`) whose
//! content the overlay thread moves to the cursor plus the grab offset at its own pace: a captured
//! frame shows the game some tens of milliseconds late, too late for a mark the hand is moving.

use std::rc::Rc;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F as Rect;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, IDWriteTextFormat};

use crate::render::{CapMetrics, Graphics, Surface, rgba};
use crate::window;

/// Space around the container for text (px).
pub const MARGIN: i32 = 60;
/// Largest container (V1) plus margins.
const MAX_SIDE: i32 = 882 + 2 * MARGIN + 2;
/// Room drawn around the window while a drag is followed (px): the map moves this far before the
/// marks are drawn again.
const PAD: i32 = 480;
/// The marks are drawn again when the pan comes this close (px) to `PAD`.
const PAD_SLACK: f32 = 120.0;
/// Positions this close (px) are the same drawing.
const SAME_PX: f32 = 0.3;
/// The game's marker diamond: centre relative to the marker point (pixel
/// centroid) and half-diagonal, on the big maps and the minimap.
const DIAMOND_CENTRE: (f32, f32) = (-1.1, -0.7);
const DIAMOND_CENTRE_SMALL: (f32, f32) = (-0.7, -0.7);
const DIAMOND_HALF: f32 = 14.0;
const DIAMOND_HALF_SMALL: f32 = 6.0;
/// The selected target's diamond around the game's, and around an objective icon (41 px).
const SELECTED_HALF: f32 = 20.0;
const SELECTED_HALF_SMALL: f32 = 10.0;
const OBJECTIVE_HALF: f32 = 27.0;
/// Line width of the game's diamonds.
const STROKE: f32 = 2.0;
/// The predicted landing's sight: ring radius, tick length and line width (px) on the big
/// maps and on the minimap; its colour, and the dark edge around it (px each side, opacity).
const PRED: (f32, f32, f32) = (11.0, 4.0, 2.0);
const PRED_SMALL: (f32, f32, f32) = (6.0, 3.0, 1.5);
const PRED_COLOUR: (u8, u8, u8) = (246, 246, 240);
const PRED_EDGE: f32 = 1.0;
const PRED_EDGE_ALPHA: f32 = 0.6;
/// The game's marker label relative to the marker point: left edge, and the label box's top and
/// bottom; the precision mirrors it below.
const LABEL_LEFT: f32 = 9.3;
const LABEL_TOP: f32 = -27.0;
const LABEL_BOTTOM: f32 = -16.0;

#[derive(Clone, Debug, PartialEq)]
pub struct MapMark {
    pub label: String,
    /// Screen position of the marker point.
    pub x: f32,
    pub y: f32,
    pub selected: bool,
    pub objective: bool,
    /// Location precision (m), for markers.
    pub precision_m: Option<f32>,
    /// The game's own diamond or label is hidden (an objective icon over it).
    pub covered: bool,
}

/// The selected target's blast at the map's scale (screen px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blast {
    /// Damage reach and kill radius.
    pub reach_px: f32,
    pub kill_px: f32,
    /// Damage reach in metres, written when the circle hides under the diamond.
    pub reach_m: f32,
    /// Area a cluster shell's submunitions cover (radius).
    pub spread_px: Option<f32>,
}

/// The charges' colours, Low to High: muted (saturation 0.39-0.58; the game keeps saturated
/// colour for meaning — teams, markers, selection), at least 5:1 on the label plate, and apart
/// from the game's greens, reds and ambers (each colour means one thing).
pub const CHARGE_COLORS: [(u8, u8, u8); 3] = [(106, 196, 184), (140, 170, 224), (210, 152, 204)];
/// A shell without charges (L33A1, rockets): its range ring and its targets' flight times. A muted
/// yellow-green: not a charge's hue, nor the game's green, red, amber or yellow, nor white; ΔE ≥ 35
/// from all of them and 9.9:1 on the label plate; coral, the other hue left, reads as the red of
/// the enemy's objectives beside them.
pub const UNCHARGED_COLOR: (u8, u8, u8) = (190, 206, 130);

/// How a range ring is drawn, in three tiers of emphasis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingKind {
    /// The loaded charge: solid.
    Loaded,
    /// The charge the selected target needs: long dashes.
    Focus,
    /// The other charges: thin, faint dashes.
    Other,
}

/// One charge's minimum or maximum range around the gun.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeRing {
    pub radius_px: f32,
    /// Index into `CHARGE_COLORS`; `None` for a shell without charges (drawn white).
    pub charge: Option<u8>,
    pub kind: RingKind,
    /// The minimum range (dotted), else the maximum.
    pub min: bool,
}

/// A target's label on V1.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetLabel {
    pub label: String,
    /// Flight time (s) with the charge to use, `None` when nothing reaches it.
    pub flight_s: Option<f32>,
    /// That charge (index into `CHARGE_COLORS`); `None` for a shell without charges.
    pub charge: Option<u8>,
    /// Elevation (°) of the arc in use, `None` when nothing reaches it.
    pub elevation: Option<f32>,
    pub distance_m: f32,
}

/// The hovered target's firing data.
#[derive(Clone, Debug, PartialEq)]
pub struct Hover {
    pub label: String,
    /// The charge to use: its name and colour index.
    pub charge: Option<(String, u8)>,
    pub distance_m: f32,
    /// Elevation (°) and flight time (s) on each arc.
    pub low: Option<(f32, f32)>,
    pub high: Option<(f32, f32)>,
    /// The high arc is the one in use.
    pub high_chosen: bool,
}

/// The V1 planning layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Planning {
    /// Screen px of the gun, the rings' centre.
    pub gun: (f32, f32),
    /// Screen rectangle of the map's picture (left, top, right, bottom).
    pub picture: (f32, f32, f32, f32),
    pub rings: Vec<RangeRing>,
    /// One per target (markers in `marks`, and objectives).
    pub labels: Vec<TargetLabel>,
    /// What the small labels show.
    pub fields: crate::settings::LabelFields,
    pub hover: Option<Hover>,
    /// Where the ground stops an arc to the selected target short of it (screen px).
    pub stopped: Option<(f32, f32)>,
}

/// The map being dragged on V1: the marks keep up with the cursor between the HUD's frames, moved
/// on every overlay pass (`MapWindow::follow`). The game moves its map 1:1 with the cursor, about
/// 70 times a second, while the HUD sees ~50 of its frames a second, each well after it was shown
/// (measured with `--latency`): updated only on those, the marks were one to three screen
/// refreshes behind the game's steps in a third of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanFollow {
    /// The moment whose cursor these marks were placed for.
    pub at: std::time::Instant,
    /// How far the marks may move (px, left, up, right, down) before the map stops at its edge.
    pub limits: (f32, f32, f32, f32),
    /// Where (screen px) and when the button went down: the game moves the map only once the
    /// cursor is `DRAG_THRESHOLD_PX` from there, and trails it by that much after.
    pub press: (f64, f64),
    pub pressed_at: std::time::Instant,
}

/// How far the cursor goes from the press before the game drags the map (screen px); the HUD's
/// canvas uses the same (`mtc_vision::map::canvas::DRAG_THRESHOLD_PX`, where it is measured).
pub const DRAG_THRESHOLD_PX: f64 = 6.7;

#[derive(Clone, Debug, PartialEq)]
pub struct MapMarks {
    /// Screen rectangle of the map container.
    pub left: i32,
    pub top: i32,
    pub size: i32,
    /// The minimap (V3): smaller marks, labels on every marker.
    pub small: bool,
    pub marks: Vec<MapMark>,
    /// Precision (m) worse than this is flagged.
    pub coarse_m: f32,
    /// The selected target's blast, when the shell is known.
    pub blast: Option<Blast>,
    /// The V1 planning layer; `None` on V2 and V3.
    pub planning: Option<Planning>,
    /// The map is being dragged: follow the cursor between frames.
    pub follow: Option<PanFollow>,
    /// Where the loaded shell lands fired now (screen px).
    pub pred: Option<(f32, f32)>,
    /// The size the marks and their labels are drawn at (1 at the reference configuration; the
    /// game's map icons follow its HUD scale). Positions stay screen px.
    pub scale: f32,
}

/// The stop bar's half length and width (px).
const STOP_HALF: f32 = 9.0;
const STOP_WIDTH: f32 = 3.0;
/// The bar keeps this far (px) from the selected target's diamond or icon centre.
const STOP_CLEAR_PX: f32 = 30.0;
/// The line of fire up to the bar starts this far (px) from the gun, clear of its arrow.
const STOP_LINE_FROM_PX: f32 = 16.0;

/// `Planning::gun` before the gun was seen: nowhere, and it stays there when the map moves.
pub const NO_GUN: (f32, f32) = (-1.0e5, -1.0e5);

impl MapMarks {
    /// The marks laid out for drawing at scale 1 about the container's top-left corner (the
    /// surface then scales the drawing by `scale` about it): positions and map-scale lengths
    /// divided by `scale` from there, the window's margin kept.
    fn unscaled(&self) -> MapMarks {
        let k = self.scale;
        let (l, t) = (self.left as f32, self.top as f32);
        let at = |(x, y): (f32, f32)| (l + (x - l) / k, t + (y - t) / k);
        let mut m = self.clone();
        m.size = (self.size as f32 / k).ceil() as i32;
        m.scale = 1.0;
        for mark in &mut m.marks {
            (mark.x, mark.y) = at((mark.x, mark.y));
        }
        if let Some(b) = m.blast.as_mut() {
            b.reach_px /= k;
            b.kill_px /= k;
            b.spread_px = b.spread_px.map(|s| s / k);
        }
        if let Some(p) = m.planning.as_mut() {
            if p.gun != NO_GUN {
                p.gun = at(p.gun);
            }
            // An unknown picture is the whole plane (f32::MIN/MAX).
            if p.picture.0 > f32::MIN && p.picture.2 < f32::MAX {
                let (a, b) = (at((p.picture.0, p.picture.1)), at((p.picture.2, p.picture.3)));
                p.picture = (a.0, a.1, b.0, b.1);
            }
            for ring in &mut p.rings {
                ring.radius_px /= k;
            }
            p.stopped = p.stopped.map(at);
        }
        m.pred = m.pred.map(at);
        m
    }

    /// The marks moved by `(dx, dy)` px with the map (the container stays).
    fn shifted(&self, (dx, dy): (f32, f32)) -> MapMarks {
        let mut m = self.clone();
        for k in &mut m.marks {
            k.x += dx;
            k.y += dy;
        }
        if let Some(p) = m.planning.as_mut() {
            if p.gun != NO_GUN {
                p.gun = (p.gun.0 + dx, p.gun.1 + dy);
            }
            p.picture = (p.picture.0 + dx, p.picture.1 + dy, p.picture.2 + dx, p.picture.3 + dy);
            if let Some(q) = p.stopped.as_mut() {
                *q = (q.0 + dx, q.1 + dy);
            }
        }
        if let Some(p) = m.pred.as_mut() {
            *p = (p.0 + dx, p.1 + dy);
        }
        m
    }
}

/// What tells two drawings of the marks apart, when they are not the same marks moved with the
/// map (the first difference found; for the overlay's log of why it drew).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    /// The container (window) moved or changed size, or V2/V3 became V1.
    Container,
    /// A target came or went.
    Count,
    /// A target's label, selection, precision or cover.
    MarkState,
    /// A target moved apart from the others.
    Position,
    Gun,
    /// The map's picture rectangle.
    Picture,
    /// The range rings (a zoom changes their radius).
    Rings,
    /// The planning labels (flight time, charge).
    Labels,
    Hover,
    Blast,
    /// The predicted landing moved apart from the map (the gun was laid).
    Pred,
    /// Where the ground stops the selected target's arc.
    Terrain,
    Other,
}

impl Change {
    pub fn name(self) -> &'static str {
        match self {
            Change::Container => "container",
            Change::Count => "targets added/removed",
            Change::MarkState => "target fields",
            Change::Position => "target positions",
            Change::Gun => "gun",
            Change::Picture => "map extent",
            Change::Rings => "range rings",
            Change::Labels => "labels",
            Change::Hover => "hover card",
            Change::Blast => "blast",
            Change::Pred => "predicted impact",
            Change::Terrain => "terrain obstruction",
            Change::Other => "other",
        }
    }
}

/// A marker being dragged: drawn like its map mark, where the game shows it (`drag_point`): at its
/// place (the press plus `grab`, screen px from the cursor to the marker point) until the cursor
/// passed the threshold, then on the lattice point nearest the cursor, or nearest `released_at`
/// once the button came up.
#[derive(Clone, Debug, PartialEq)]
pub struct DragMark {
    pub mark: MapMark,
    pub coarse_m: f32,
    pub grab: (i32, i32),
    pub released_at: Option<(i32, i32)>,
    /// The lattice the game puts a dragged marker on (5 m on the ground from the map's centre):
    /// a lattice point (screen px) and the step (px). `None` (no
    /// registration): the mark goes to the cursor.
    pub snap: Option<((f64, f64), f64)>,
    /// Where (screen px) and when the button went down: the game moves the marker only once the
    /// cursor is `MARKER_DRAG_THRESHOLD_PX` from there.
    pub press: (f64, f64),
    pub pressed_at: std::time::Instant,
    /// The mark's drawing size (`MapMarks::scale`).
    pub scale: f32,
}

/// How far the cursor goes from the press before the game moves a dragged marker (screen px): the
/// marker holds still until then, and past it sits on the lattice point nearest the cursor, not
/// trailing it (in recorded marker drags on Snowy Fields the first move came with the cursor
/// 6.0–7.1 px from the press in a slow drag; taken as the map's threshold).
pub const MARKER_DRAG_THRESHOLD_PX: f64 = 6.7;

/// Where the game shows a dragged marker (screen px) for the cursor `cursor`, or for where the
/// button came up: at its place until the cursor passed the threshold (`start`, `None` before),
/// then on the lattice point nearest the cursor's pixel, the grab offset dropped (over 6461
/// recorded frames: per axis on the predicted point in 97–98 % of frames at 5×, 94–96 % in
/// slow drags at 1.8×, the rest with the cursor within 1 px of a cell boundary).
fn drag_point(drag: &DragMark, cursor: (f64, f64), start: Option<(f64, f64)>) -> (f64, f64) {
    if start.is_none() {
        return (drag.press.0.round() + f64::from(drag.grab.0), drag.press.1.round() + f64::from(drag.grab.1));
    }
    // The game reads whole pixels.
    let raw = drag.released_at.map_or((cursor.0.round(), cursor.1.round()), |(x, y)| (f64::from(x), f64::from(y)));
    match drag.snap.filter(|s| s.1 > 0.5) {
        Some((a, step)) => (a.0 + ((raw.0 - a.0) / step).round() * step, a.1 + ((raw.1 - a.1) / step).round() * step),
        None => raw,
    }
}

/// The drag window's size and where the marker point sits in it (room for the label above, the
/// precision to the lower right).
const DRAG_W: i32 = 270;
const DRAG_H: i32 = 115;
const DRAG_ANCHOR: i32 = MARGIN;

pub struct DragWindow {
    window: MapWindow,
    shown: Option<DragMark>,
    /// The window's screen rectangle (the whole desktop) while shown.
    placed: Option<(i32, i32, i32, i32)>,
    /// The mark's top left in the window (px), as handed over.
    pan: Option<(i32, i32)>,
    /// Positions handed to the compositor so far.
    commits: u64,
}

impl DragWindow {
    pub fn new(graphics: &Rc<Graphics>) -> windows::core::Result<Self> {
        let (_, _, w, h) = window::desktop();
        let hwnd = window::create(w, h)?;
        let surface = Surface::layered(graphics, hwnd, (DRAG_W as u32, DRAG_H as u32), (0.0, 0.0))?;
        Ok(Self { window: MapWindow::with_surface(hwnd, surface)?, shown: None, placed: None, pan: None, commits: 0 })
    }

    /// The surface made again after a device loss (`Graphics::recover`), the shown mark redrawn.
    pub fn rebuild(&mut self, cursor: (i32, i32)) -> windows::core::Result<()> {
        self.window.surface.rebuild()?;
        self.pan = None;
        match self.shown.take() {
            Some(shown) => self.show(Some(&shown), cursor),
            None => Ok(()),
        }
    }

    pub fn active(&self) -> bool {
        self.shown.is_some()
    }

    pub fn show(&mut self, drag: Option<&DragMark>, cursor: (i32, i32)) -> windows::core::Result<()> {
        let Some(drag) = drag else {
            if self.shown.take().is_some() {
                window::hide(self.window.hwnd);
                self.placed = None;
                self.pan = None;
            }
            return Ok(());
        };
        if self.shown.as_ref().map(|d| &d.mark) != Some(&drag.mark) {
            let mark = MapMark { x: DRAG_ANCHOR as f32, y: DRAG_ANCHOR as f32, ..drag.mark.clone() };
            // The map drawing with the container at the window's margin: the mark lands on the anchor.
            self.window.draw(&MapMarks { left: MARGIN, top: MARGIN, size: DRAG_W, small: false, marks: vec![mark], coarse_m: drag.coarse_m, blast: None, planning: None, follow: None, pred: None, scale: drag.scale }, 0)?;
            self.window.surface.commit()?;
        }
        self.shown = Some(drag.clone());
        let desktop = window::desktop();
        if self.placed != Some(desktop) {
            window::place_sized(self.window.hwnd, desktop.0, desktop.1, desktop.2, desktop.3);
            self.placed = Some(desktop);
        }
        // Placed where it was pressed when it appears; after that the overlay thread moves it (to
        // the cursor the game shows the marker at, not the newest one).
        if self.pan.is_none() { self.follow((f64::from(cursor.0), f64::from(cursor.1)), None) } else { Ok(()) }
    }

    /// The press of the drag shown (screen px, moment), if any.
    pub fn press(&self) -> Option<((f64, f64), std::time::Instant)> {
        self.shown.as_ref().map(|d| (d.press, d.pressed_at))
    }

    /// Moves the mark to where the game shows the marker for the cursor `cursor` (screen px), or
    /// for where the button came up: `start` is where the cursor passed the threshold from the press
    /// (`None` before: the marker holds still). See `drag_point`.
    pub fn follow(&mut self, cursor: (f64, f64), start: Option<(f64, f64)>) -> windows::core::Result<()> {
        let (Some(drag), Some((left, top, _, _))) = (&self.shown, self.placed) else { return Ok(()) };
        let point = drag_point(drag, cursor, start);
        let at = (point.0.round() as i32 - DRAG_ANCHOR - left, point.1.round() as i32 - DRAG_ANCHOR - top);
        if self.pan != Some(at) {
            self.window.surface.set_pan((at.0 as f32, at.1 as f32))?;
            self.window.surface.commit()?;
            self.pan = Some(at);
            self.commits += 1;
        }
        Ok(())
    }

    /// Positions handed to the compositor so far (`pace` times them).
    pub fn commits(&self) -> u64 {
        self.commits
    }
}

pub struct MapWindow {
    hwnd: HWND,
    surface: Surface,
    /// The window's surface is made for containers up to this side (px, margins included), and
    /// made again larger for a larger one (another display configuration's full map); not
    /// offscreen.
    layered: bool,
    side: i32,
    label: IDWriteTextFormat,
    small: IDWriteTextFormat,
    /// The planning labels: the flight time, the rest, and right-aligned (Titillium Web Bold).
    tag_first: (IDWriteTextFormat, CapMetrics),
    tag: (IDWriteTextFormat, CapMetrics),
    tag_right: (IDWriteTextFormat, CapMetrics),
    /// Where each target's label went last (candidate index): kept there while it fits.
    slots: std::cell::RefCell<std::collections::HashMap<String, usize>>,
    /// The marks as drawn (while following a drag, as they came).
    shown: Option<MapMarks>,
    placed: Option<(i32, i32, i32)>,
    /// A drag being followed.
    following: Option<Following>,
    /// Drawings since the last `take_redraws`, by why.
    redraws: std::collections::BTreeMap<Redraw, u32>,
    /// Positions handed to the compositor while following a drag, so far.
    commits: u64,
}

/// A drag being followed (`PanFollow`): the marks drawn once with room around them, then moved
/// as a whole to where the game's map shows them.
struct Following {
    /// The marks drawn, as they came.
    base: MapMarks,
    /// The newest marks are `base` moved by this (px).
    moved: (f32, f32),
    /// The newest marks' moment and the map's edges from there.
    newest: PanFollow,
    /// Marks that came since the last pass.
    incoming: Option<MapMarks>,
    /// The pan handed over (whole px).
    pan: (f32, f32),
}

/// How far `new` is `base` moved with the map (px), when nothing else differs; else the first
/// difference.
fn translation(base: &MapMarks, new: &MapMarks) -> Result<(f32, f32), Change> {
    let anchor = |m: &MapMarks| {
        m.marks.first().map(|k| (k.x, k.y)).or_else(|| {
            let p = m.planning.as_ref()?;
            if p.picture.0 > f32::MIN && p.picture.1 > f32::MIN {
                Some((p.picture.0, p.picture.1))
            } else {
                (p.gun != NO_GUN).then_some(p.gun)
            }
        })
    };
    let v = match (anchor(base), anchor(new)) {
        (Some(a), Some(b)) => (b.0 - a.0, b.1 - a.1),
        _ => (0.0, 0.0),
    };
    let near = |a: f32, b: f32| (a - b).abs() <= SAME_PX;
    let moved = |(x, y): (f32, f32), (u, w): (f32, f32)| near(x + v.0, u) && near(y + v.1, w);
    if (base.left, base.top, base.size, base.small) != (new.left, new.top, new.size, new.small) || base.planning.is_some() != new.planning.is_some() {
        return Err(Change::Container);
    }
    if base.marks.len() != new.marks.len() {
        return Err(Change::Count);
    }
    for (m, n) in base.marks.iter().zip(&new.marks) {
        if (&m.label, m.selected, m.objective, m.precision_m, m.covered) != (&n.label, n.selected, n.objective, n.precision_m, n.covered) {
            return Err(Change::MarkState);
        }
        if !moved((m.x, m.y), (n.x, n.y)) {
            return Err(Change::Position);
        }
    }
    if base.blast != new.blast {
        return Err(Change::Blast);
    }
    if base.coarse_m != new.coarse_m {
        return Err(Change::Other);
    }
    match (base.pred, new.pred) {
        (Some(a), Some(b)) if moved(a, b) => {}
        (None, None) => {}
        _ => return Err(Change::Pred),
    }
    if let (Some(p), Some(q)) = (&base.planning, &new.planning) {
        let gun_same = if p.gun == NO_GUN || q.gun == NO_GUN { p.gun == q.gun } else { moved(p.gun, q.gun) };
        if !gun_same {
            return Err(Change::Gun);
        }
        match (p.stopped, q.stopped) {
            (Some(a), Some(b)) if moved(a, b) => {}
            (None, None) => {}
            _ => return Err(Change::Terrain),
        }
        // An unknown picture is the whole plane (f32::MIN/MAX stay put when moved).
        if !(moved((p.picture.0, p.picture.1), (q.picture.0, q.picture.1)) && moved((p.picture.2, p.picture.3), (q.picture.2, q.picture.3))) {
            return Err(Change::Picture);
        }
        if p.rings != q.rings {
            return Err(Change::Rings);
        }
        if p.labels != q.labels || p.fields != q.fields {
            return Err(Change::Labels);
        }
        if p.hover != q.hover {
            return Err(Change::Hover);
        }
    }
    Ok(v)
}

/// Why the marks were drawn (for the overlay's log).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Redraw {
    /// Shown after being hidden.
    Appeared,
    /// Not following a drag: moved more than `SAME_PX` (a correction at rest), or `Change`.
    Moved,
    Still(Change),
    /// A drag began: drawn with room around.
    DragStart,
    /// Following a drag: near the edge of what was drawn, or `Change`.
    Edge,
    Dragging(Change),
    /// The button came up: drawn as the marks are.
    DragEnd,
}

impl Redraw {
    pub fn name(self) -> String {
        match self {
            Redraw::Appeared => "shown".to_owned(),
            Redraw::Moved => "still: moved".to_owned(),
            Redraw::Still(c) => format!("still: {}", c.name()),
            Redraw::DragStart => "drag start".to_owned(),
            Redraw::Edge => "drag at edge".to_owned(),
            Redraw::Dragging(c) => format!("dragging: {}", c.name()),
            Redraw::DragEnd => "drag end".to_owned(),
        }
    }
}

/// The container in window coordinates.
fn container_rect(marks: &MapMarks) -> Rect {
    let (a, b) = (MARGIN as f32, (MARGIN + marks.size) as f32);
    Rect { left: a, top: a, right: b, bottom: b }
}

/// A rectangle (left, top, right, bottom).
type Box2 = (f32, f32, f32, f32);

/// Where a target's label may go (top-left corners, in order of preference) for a label `w` × `h`:
/// an objective's below, right, left, above its icon; a marker's to the lower right of its point
/// (where the game's precision text was), left of, below, above its diamond; the selected
/// marker's a little further out (its red diamond).
fn candidates_for(objective: bool, selected: bool, (cx, cy): (f32, f32), x: f32, y: f32, w: f32, h: f32) -> Vec<(f32, f32)> {
    if objective {
        vec![(cx - w / 2.0, cy + 25.0), (cx + 25.0, cy - h / 2.0), (cx - 25.0 - w, cy - h / 2.0), (cx - w / 2.0, cy - 25.0 - h)]
    } else if selected {
        vec![(x + 13.0, y + 16.0), (cx - 26.0 - w, cy - h / 2.0), (cx - w / 2.0, cy + 26.0), (cx - w - 10.0, cy - h - 16.0)]
    } else {
        vec![(x + 8.0, y + 11.0), (cx - 20.0 - w, cy - h / 2.0), (cx - w / 2.0, cy + 22.0), (cx - w - 6.0, cy - h - 12.0)]
    }
}

fn overlaps(a: Box2, b: Box2) -> bool {
    const PAD: f32 = 2.0;
    a.0 < b.2 + PAD && b.0 < a.2 + PAD && a.1 < b.3 + PAD && b.1 < a.3 + PAD
}

fn within(a: Box2, b: Box2) -> bool {
    a.0 >= b.0 && a.1 >= b.1 && a.2 <= b.2 && a.3 <= b.3
}

/// The planning label plate: the game's button colour ("Clear all").
const PLATE: (u8, u8, u8, f32) = (24, 30, 34, 0.89);
/// Label height and line spacing (px); padding inside; gap between parts of a line.
const TAG_H: f32 = 24.0;
const TAG_LINE: f32 = 18.0;
const TAG_PAD: f32 = 7.0;
const TAG_GAP: f32 = 8.0;

fn diamond(cx: f32, cy: f32, half: f32) -> [(f32, f32); 4] {
    [(cx, cy - half), (cx + half, cy), (cx, cy + half), (cx - half, cy)]
}

impl MapWindow {
    pub fn new(graphics: &Rc<Graphics>) -> windows::core::Result<Self> {
        let hwnd = window::create(MAX_SIDE, MAX_SIDE)?;
        let full = (MAX_SIDE + 2 * PAD) as u32;
        let mut window = Self::with_surface(hwnd, Surface::layered(graphics, hwnd, (full, full), (-PAD as f32, -PAD as f32))?)?;
        window.layered = true;
        Ok(window)
    }

    /// The surface made for a container of `size` px (margins added) if it is not already.
    fn fit(&mut self, size: i32) -> windows::core::Result<()> {
        let needed = size + 2 * MARGIN + 2;
        if needed > self.side && self.layered {
            let full = (needed + 2 * PAD) as u32;
            self.surface.resize_layered((full, full))?;
            self.side = needed;
        }
        Ok(())
    }

    /// The marks drawn offscreen where they would show.
    pub fn render_offscreen(graphics: &Rc<Graphics>, marks: &MapMarks) -> windows::core::Result<crate::Layer> {
        let canvas = MAX_SIDE.max(marks.size + 2 * MARGIN + 2);
        let window = Self::with_surface(HWND::default(), Surface::offscreen(graphics, canvas as u32, canvas as u32)?)?;
        window.draw(marks, 0)?;
        let side = (marks.size + 2 * MARGIN) as usize;
        let full = window.surface.pixels()?;
        let row = canvas as usize * 4;
        let bgra = (0..side).flat_map(|y| full[y * row..y * row + side * 4].iter().copied()).collect();
        Ok(crate::Layer { x: marks.left - MARGIN, y: marks.top - MARGIN, width: side as u32, height: side as u32, bgra })
    }

    /// Development: `marks` drawn `n` times on one offscreen surface, each time shifted by a pixel
    /// (what a pan does every frame); the time per drawing.
    #[doc(hidden)]
    pub fn benchmark_draw(graphics: &Rc<Graphics>, marks: &MapMarks, n: u32) -> windows::core::Result<std::time::Duration> {
        let window = Self::with_surface(HWND::default(), Surface::offscreen(graphics, MAX_SIDE as u32, MAX_SIDE as u32)?)?;
        window.draw(marks, 0)?;
        let started = std::time::Instant::now();
        for k in 0..n {
            let shifted = MapMarks { marks: marks.marks.iter().map(|m| MapMark { x: m.x + (k % 7) as f32, ..m.clone() }).collect(), ..marks.clone() };
            window.draw(&shifted, 0)?;
        }
        Ok(started.elapsed() / n.max(1))
    }

    fn with_surface(hwnd: HWND, surface: Surface) -> windows::core::Result<Self> {
        // The game's marker labels: Titillium Web Regular, ~14.5 px.
        let label = surface.map_label_format(14.5, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?;
        let small = surface.map_label_format(12.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?;
        let with_metrics = |format: IDWriteTextFormat| -> windows::core::Result<(IDWriteTextFormat, CapMetrics)> {
            let m = surface.cap_metrics(&format)?;
            Ok((format, m))
        };
        let tag_first = with_metrics(surface.map_label_format(16.0, DWRITE_FONT_WEIGHT_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING)?)?;
        let tag = with_metrics(surface.map_label_format(15.0, DWRITE_FONT_WEIGHT_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING)?)?;
        let tag_right = with_metrics(surface.map_label_format(15.0, DWRITE_FONT_WEIGHT_BOLD, DWRITE_TEXT_ALIGNMENT_TRAILING)?)?;
        Ok(Self { hwnd, surface, layered: false, side: MAX_SIDE, label, small, tag_first, tag, tag_right, slots: Default::default(), shown: None, placed: None, following: None, redraws: Default::default(), commits: 0 })
    }

    /// The drawings since the last call, by why.
    pub fn take_redraws(&mut self) -> std::collections::BTreeMap<Redraw, u32> {
        std::mem::take(&mut self.redraws)
    }

    fn count(&mut self, why: Redraw) {
        *self.redraws.entry(why).or_default() += 1;
    }

    /// The surface made again after a device loss (`Graphics::recover`), the shown marks redrawn.
    pub fn rebuild(&mut self) -> windows::core::Result<()> {
        self.surface.rebuild()?;
        self.following = None;
        match self.shown.take() {
            Some(shown) => self.show(Some(&shown)).map(|_| ()),
            None => Ok(()),
        }
    }

    /// Following a drag (the marks move with the cursor between the HUD's frames).
    pub fn following(&self) -> bool {
        self.following.is_some()
    }

    /// V1 is on screen (the planning layer shows): a drag may start any moment.
    pub fn on_v1(&self) -> bool {
        self.shown.as_ref().is_some_and(|m| m.planning.is_some())
    }

    /// Where and when the button of the followed drag went down (screen px).
    pub fn press(&self) -> Option<((f64, f64), std::time::Instant)> {
        self.following.as_ref().map(|f| (f.newest.press, f.newest.pressed_at))
    }

    /// While following: folds in the marks that came, and moves the drawing to where the game's map
    /// is at moment `target` (within the map's edges); whether it drew. `cursor_at` gives how far
    /// the game has moved the map at a moment (the cursor's place, less the threshold the map
    /// waits for; any fixed origin).
    pub fn follow(&mut self, cursor_at: &dyn Fn(std::time::Instant) -> Option<(f64, f64)>, target: std::time::Instant) -> windows::core::Result<bool> {
        let Some(mut f) = self.following.take() else { return Ok(false) };
        let mut redraw: Option<MapMarks> = None;
        if let Some(new) = f.incoming.take()
            && let Some(newest) = new.follow
        {
            f.newest = newest;
            match translation(&f.base, &new) {
                Ok(moved) => f.moved = moved,
                // Something else changed (a label, the selection, a target refined).
                Err(change) => {
                    self.count(Redraw::Dragging(change));
                    redraw = Some(new);
                }
            }
        }
        // How far the game's map went from the newest marks' moment, by the cursor (1:1).
        let step = match (cursor_at(target), cursor_at(f.newest.at)) {
            (Some(now), Some(then)) => {
                let (l, t, r, b) = f.newest.limits;
                (((now.0 - then.0) as f32).clamp(l, r), ((now.1 - then.1) as f32).clamp(t, b))
            }
            _ => (0.0, 0.0),
        };
        let mut pan = (f.moved.0 + step.0, f.moved.1 + step.1);
        // Near the edge of what is drawn: drawn again where the marks are now, for moment `target`
        // (the map's edges as far again closer).
        let reach = PAD as f32 - PAD_SLACK;
        if redraw.is_none() && (pan.0.abs() > reach || pan.1.abs() > reach) {
            self.count(Redraw::Edge);
            let (l, t, r, b) = f.newest.limits;
            let now = PanFollow { at: target, limits: (l - step.0, t - step.1, r - step.0, b - step.1), ..f.newest };
            redraw = Some(MapMarks { follow: Some(now), ..f.base.shifted(pan) });
        }
        let drew = redraw.is_some();
        if let Some(marks) = redraw {
            self.draw(&marks, PAD + MARGIN)?;
            if let Some(newest) = marks.follow {
                f.newest = newest;
            }
            f.base = marks;
            f.moved = (0.0, 0.0);
            pan = match (cursor_at(target), cursor_at(f.newest.at)) {
                (Some(now), Some(then)) => {
                    let (l, t, r, b) = f.newest.limits;
                    (((now.0 - then.0) as f32).clamp(l, r), ((now.1 - then.1) as f32).clamp(t, b))
                }
                _ => (0.0, 0.0),
            };
        }
        // Whole pixels, as the game moves its map (the cursor's).
        let pan = (pan.0.round(), pan.1.round());
        if drew || pan != f.pan {
            self.surface.set_pan(pan)?;
            self.surface.commit()?;
            f.pan = pan;
            self.commits += 1;
        }
        self.following = Some(f);
        Ok(drew)
    }

    /// Positions handed to the compositor while following a drag, so far (`pace` times them).
    pub fn commits(&self) -> u64 {
        self.commits
    }

    /// Shows `marks` (or hides the window); whether it drew.
    pub fn show(&mut self, marks: Option<&MapMarks>) -> windows::core::Result<bool> {
        let Some(marks) = marks else {
            self.following = None;
            if self.shown.take().is_some() {
                window::hide(self.hwnd);
                self.placed = None;
            }
            return Ok(false);
        };
        if self.shown.as_ref() == Some(marks) {
            return Ok(false);
        }
        self.fit(marks.size)?;
        let drew = match (marks.follow, self.following.as_mut()) {
            // Folded in at the next pass (`follow`), which knows where the cursor went.
            (Some(_), Some(f)) => {
                f.incoming = Some(marks.clone());
                false
            }
            // A drag starts: drawn once with room around, shown inside the container.
            (Some(newest), None) => {
                self.count(Redraw::DragStart);
                self.draw(marks, PAD + MARGIN)?;
                self.surface.set_clip(Some(container_rect(marks)))?;
                self.surface.set_pan((0.0, 0.0))?;
                self.surface.commit()?;
                self.following = Some(Following { base: marks.clone(), moved: (0.0, 0.0), newest, incoming: None, pan: (0.0, 0.0) });
                true
            }
            (None, Some(_)) => {
                self.count(Redraw::DragEnd);
                self.following = None;
                self.draw(marks, 0)?;
                self.surface.set_clip(None)?;
                self.surface.set_pan((0.0, 0.0))?;
                self.surface.commit()?;
                true
            }
            (None, None) => {
                // At rest, the drawn marks within `SAME_PX` of where they now are stay (a smoothed
                // nudge onto the diamonds moves them by tenths of a pixel). Compared with what was
                // drawn, so a slow drift is drawn once it adds up.
                let why = match self.shown.as_ref() {
                    None => Redraw::Appeared,
                    Some(drawn) => match translation(drawn, marks) {
                        Ok(v) if v.0.abs() <= SAME_PX && v.1.abs() <= SAME_PX => return Ok(false),
                        Ok(_) => Redraw::Moved,
                        Err(change) => Redraw::Still(change),
                    },
                };
                self.count(why);
                self.following = None;
                self.draw(marks, 0)?;
                self.surface.set_clip(None)?;
                self.surface.set_pan((0.0, 0.0))?;
                self.surface.commit()?;
                true
            }
        };
        let at = (marks.left - MARGIN, marks.top - MARGIN, marks.size + 2 * MARGIN);
        if self.placed != Some(at) {
            window::place_sized(self.hwnd, at.0, at.1, at.2, at.2);
            self.placed = Some(at);
        }
        self.shown = Some(marks.clone());
        Ok(drew)
    }

    /// Draws `marks`: those in the container, or within `reach` px around it while a drag is
    /// followed (the map brings them in); at `marks.scale` about the container's corner.
    fn draw(&self, marks: &MapMarks, reach: i32) -> windows::core::Result<()> {
        let s = &self.surface;
        let unscaled;
        let (marks, reach) = if marks.scale == 1.0 {
            s.set_scale(1.0);
            (marks, reach)
        } else {
            s.set_scale_about(marks.scale, (MARGIN as f32, MARGIN as f32));
            unscaled = marks.unscaled();
            (&unscaled, (reach as f32 / marks.scale).ceil() as i32)
        };
        let red = rgba(232, 62, 50, 1.0);
        let white = rgba(255, 255, 255, 0.95);
        let amber = rgba(226, 174, 70, 1.0);
        let outline = rgba(0, 0, 0, 0.75);
        let (ox, oy) = ((marks.left - MARGIN) as f32, (marks.top - MARGIN) as f32);
        let (x0, y0) = (MARGIN as f32, MARGIN as f32);
        let r = reach as f32;
        let (x0, y0, x1, y1) = (x0 - r, y0 - r, x0 + marks.size as f32 + r, y0 + marks.size as f32 + r);
        let inside = |x: f32, y: f32| x >= x0 && y >= y0 && x < x1 && y < y1;
        let (centre, half, selected_half) =
            if marks.small { (DIAMOND_CENTRE_SMALL, DIAMOND_HALF_SMALL, SELECTED_HALF_SMALL) } else { (DIAMOND_CENTRE, DIAMOND_HALF, SELECTED_HALF) };
        s.start()?;
        if let Some(planning) = &marks.planning {
            self.draw_rings(planning, (ox, oy), (x0, y0, x1, y1))?;
        }
        if crate::PROBE_MARKER.load(std::sync::atomic::Ordering::Relaxed) && !marks.small {
            s.fill_rect(D2D_RECT_F { left: 0.0, top: 0.0, right: 4.0, bottom: 4.0 }, rgba(255, 0, 255, 1.0));
            // And on the selected marker's diamond centre: the drag probe finds it on screen beside
            // the game's diamond.
            if let Some(m) = marks.marks.iter().find(|m| m.selected && !m.objective) {
                let (x, y) = (m.x - ox + centre.0, m.y - oy + centre.1);
                s.fill_rect(D2D_RECT_F { left: x - 2.0, top: y - 2.0, right: x + 2.0, bottom: y + 2.0 }, rgba(255, 0, 255, 1.0));
            }
        }
        // The selected target's blast first, under everything, clipped to the container.
        if let (Some(blast), Some(m)) = (marks.blast, marks.marks.iter().find(|m| m.selected)) {
            let (cx, cy) = (m.x - ox + if m.objective { 0.0 } else { centre.0 }, m.y - oy + if m.objective { 0.0 } else { centre.1 });
            s.push_clip(D2D_RECT_F { left: x0, top: y0, right: x1, bottom: y1 });
            s.circle(cx, cy, blast.reach_px, rgba(232, 62, 50, 0.10));
            s.ring(cx, cy, blast.reach_px, rgba(232, 62, 50, 0.55), 1.0);
            s.circle(cx, cy, blast.kill_px, rgba(232, 62, 50, 0.20));
            s.ring(cx, cy, blast.kill_px, rgba(232, 62, 50, 0.75), 1.0);
            if let Some(spread) = blast.spread_px {
                s.dashed_ring(cx, cy, spread, amber, 1.5, 32);
            }
            s.pop_clip();
        }
        // Selected last, so it draws over the others.
        let mut order: Vec<&MapMark> = marks.marks.iter().filter(|m| inside(m.x - ox, m.y - oy)).collect();
        order.sort_by_key(|m| m.selected);
        for m in order {
            let (px, py) = (m.x - ox, m.y - oy);
            if m.objective {
                if m.selected {
                    s.polygon(&diamond(px, py, OBJECTIVE_HALF), None, Some((red, STROKE)))?;
                }
                continue;
            }
            let (cx, cy) = (px + centre.0, py + centre.1);
            if m.covered && !marks.small {
                s.polygon(&diamond(cx, cy, half), None, Some((white, STROKE)))?;
            }
            if m.selected {
                s.polygon(&diamond(cx, cy, selected_half), None, Some((red, STROKE)))?;
            }
            if marks.small {
                // The minimap shows no labels: the label where the big maps put it.
                let color = if m.selected { red } else { white };
                s.outlined_text(&m.label, &self.small, D2D_RECT_F { left: px + 5.0, top: py - 17.0, right: px + 60.0, bottom: py - 3.0 }, color, outline);
                continue;
            }
            if m.covered {
                s.outlined_text(&m.label, &self.label, D2D_RECT_F { left: px + LABEL_LEFT, top: py + LABEL_TOP - 3.0, right: px + 120.0, bottom: py + LABEL_BOTTOM + 3.0 }, white, outline);
            }
            if let Some(p) = m.precision_m.filter(|_| marks.planning.is_none()) {
                let coarse = p > marks.coarse_m;
                let text = if coarse { format!("±{p:.1} m · zoom in") } else { format!("±{p:.1} m") };
                // Mirrors the game's label below the marker point.
                s.outlined_text(&text, &self.label, D2D_RECT_F { left: px + LABEL_LEFT, top: py - LABEL_BOTTOM - 3.0, right: px + 200.0, bottom: py - LABEL_TOP + 3.0 }, if coarse { amber } else { white }, outline);
            }
            // The blast circle hides under the diamonds: its size in words, a line lower.
            if let Some(blast) = marks.blast.filter(|b| marks.planning.is_none() && m.selected && b.spread_px.is_none() && b.reach_px < selected_half + STROKE) {
                let line = 15.0;
                let text = format!("blast {:.1} m", blast.reach_m);
                s.outlined_text(&text, &self.label, D2D_RECT_F { left: px + LABEL_LEFT, top: py - LABEL_BOTTOM - 3.0 + line, right: px + 200.0, bottom: py - LABEL_TOP + 3.0 + line }, white, outline);
            }
        }
        if let Some(planning) = &marks.planning {
            // Not over the selected target's diamond or icon (the panel says it then).
            let target = marks.marks.iter().find(|m| m.selected).map(|m| if m.objective { (m.x - ox, m.y - oy) } else { (m.x - ox + centre.0, m.y - oy + centre.1) });
            let clear = |(x, y): (f32, f32)| target.is_none_or(|(tx, ty)| (x - tx).hypot(y - ty) > STOP_CLEAR_PX);
            if let Some((sx, sy)) = planning.stopped.map(|(x, y)| (x - ox, y - oy)).filter(|&(x, y)| inside(x, y) && clear((x, y))) {
                self.draw_stopped(sx, sy, (planning.gun.0 - ox, planning.gun.1 - oy))?;
            }
            self.draw_labels(marks, planning, (ox, oy), (x0, y0, x1, y1), centre)?;
        }
        // The predicted landing on top: it is what the player lays the gun by.
        if let Some((px, py)) = marks.pred.map(|(x, y)| (x - ox, y - oy)).filter(|&(x, y)| inside(x, y)) {
            self.draw_pred(px, py, marks.small)?;
        }
        s.end()
    }

    /// The predicted landing: a white sight ring with four ticks pointing in, the middle left clear
    /// for what it is laid on (a target's diamond, the game's X), over a dark edge for bright
    /// ground.
    fn draw_pred(&self, x: f32, y: f32, small: bool) -> windows::core::Result<()> {
        let s = &self.surface;
        let (r, tick, width) = if small { PRED_SMALL } else { PRED };
        let ticks = |extra: f32| [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)].map(|(dx, dy)| [(x + dx * (r + extra), y + dy * (r + extra)), (x + dx * (r - tick - extra), y + dy * (r - tick - extra))]);
        let edge = rgba(0, 0, 0, PRED_EDGE_ALPHA);
        s.ring(x, y, r, edge, width + 2.0 * PRED_EDGE);
        for t in ticks(PRED_EDGE) {
            s.polyline(&t, edge, width + 2.0 * PRED_EDGE)?;
        }
        let white = rgba(PRED_COLOUR.0, PRED_COLOUR.1, PRED_COLOUR.2, 1.0);
        s.ring(x, y, r, white, width);
        for t in ticks(0.0) {
            s.polyline(&t, white, width)?;
        }
        Ok(())
    }

    /// Where the ground stops an arc to the selected target: an amber bar across the line
    /// from the gun, over a dark edge, as the panel's terrain profile marks it.
    fn draw_stopped(&self, x: f32, y: f32, gun: (f32, f32)) -> windows::core::Result<()> {
        let s = &self.surface;
        let (dx, dy) = (x - gun.0, y - gun.1);
        let len = dx.hypot(dy);
        // Across the line; upright without a gun to take it from.
        let (nx, ny) = if len > 1.0 && gun != NO_GUN { (-dy / len, dx / len) } else { (1.0, 0.0) };
        let half = STOP_HALF;
        let amber = rgba(226, 174, 70, 1.0);
        // The line of fire up to it, clear of the gun's arrow.
        if len > STOP_LINE_FROM_PX + 4.0 && gun != NO_GUN {
            let from = (gun.0 + dx / len * STOP_LINE_FROM_PX, gun.1 + dy / len * STOP_LINE_FROM_PX);
            s.dashed_polyline(&[from, (x, y)], rgba(0, 0, 0, 0.6), 3.5, (6.0, 5.0))?;
            s.dashed_polyline(&[from, (x, y)], amber, 1.5, (6.0, 5.0))?;
        }
        let bar = [(x - nx * half, y - ny * half), (x + nx * half, y + ny * half)];
        s.polyline(&bar, rgba(0, 0, 0, 0.75), STOP_WIDTH + 2.0)?;
        s.polyline(&bar, amber, STOP_WIDTH)?;
        Ok(())
    }

    /// The charges' range rings, clipped to the map's picture in the container, a dark edge under
    /// each so they show on bright ground.
    fn draw_rings(&self, planning: &Planning, (ox, oy): (f32, f32), container: Box2) -> windows::core::Result<()> {
        let s = &self.surface;
        let p = planning.picture;
        let clip = (container.0.max(p.0 - ox), container.1.max(p.1 - oy), container.2.min(p.2 - ox), container.3.min(p.3 - oy));
        if clip.0 >= clip.2 || clip.1 >= clip.3 {
            return Ok(());
        }
        s.push_clip(D2D_RECT_F { left: clip.0, top: clip.1, right: clip.2, bottom: clip.3 });
        let (cx, cy) = (planning.gun.0 - ox, planning.gun.1 - oy);
        for ring in &planning.rings {
            if ring.radius_px < 6.0 {
                continue;
            }
            let (r, g, b) = ring.charge.map_or(UNCHARGED_COLOR, |c| CHARGE_COLORS[usize::from(c.min(2))]);
            let (width, dash, alpha) = match (ring.kind, ring.min) {
                (RingKind::Loaded, false) => (3.0, None, 1.0),
                (RingKind::Focus, false) => (3.0, Some((14.0, 10.0)), 1.0),
                (RingKind::Loaded | RingKind::Focus, true) => (2.0, Some((2.5, 3.5)), 0.9),
                (RingKind::Other, false) => (1.5, Some((8.0, 8.0)), 0.55),
                (RingKind::Other, true) => continue,
            };
            let area = D2D_RECT_F { left: clip.0, top: clip.1, right: clip.2, bottom: clip.3 };
            s.styled_ring_in(cx, cy, ring.radius_px, area, rgba(0, 0, 0, 0.43 * alpha), width + 3.0, dash)?;
            s.styled_ring_in(cx, cy, ring.radius_px, area, rgba(r, g, b, 0.94 * alpha), width, dash)?;
        }
        s.pop_clip();
        Ok(())
    }

    /// One label per target, placed in order (the selected target, the hovered one, markers by
    /// number, objectives) at the first free candidate place, the last one first; the hovered
    /// target's grows into its whole card.
    fn draw_labels(&self, marks: &MapMarks, planning: &Planning, (ox, oy): (f32, f32), container: Box2, centre: (f32, f32)) -> windows::core::Result<()> {
        let s = &self.surface;
        let white = rgba(242, 243, 238, 1.0);
        let dim = rgba(138, 143, 140, 1.0);
        let amber = rgba(226, 174, 70, 1.0);
        let red = rgba(232, 62, 50, 1.0);
        let plate = rgba(PLATE.0, PLATE.1, PLATE.2, PLATE.3);
        let p = planning.picture;
        let bounds = (container.0.max(p.0 - ox), container.1.max(p.1 - oy), container.2.min(p.2 - ox), container.3.min(p.3 - oy));
        let uncharged = rgba(UNCHARGED_COLOR.0, UNCHARGED_COLOR.1, UNCHARGED_COLOR.2, 1.0);
        let charge_color = |c: Option<u8>| c.map_or(uncharged, |c| {
            let (r, g, b) = CHARGE_COLORS[usize::from(c.min(2))];
            rgba(r, g, b, 1.0)
        });
        // Window-local positions: the marker point, the diamond or icon centre.
        let visible: Vec<&MapMark> = marks.marks.iter().filter(|m| {
            let (x, y) = (m.x - ox, m.y - oy);
            x >= container.0 && y >= container.1 && x < container.2 && y < container.3
        }).collect();
        let at = |m: &MapMark| {
            let (x, y) = (m.x - ox, m.y - oy);
            if m.objective { (x, y, x, y) } else { (x, y, x + centre.0, y + centre.1) }
        };
        // What labels must leave clear, by owner.
        let mut obstacles: Vec<(Option<&str>, Box2)> = Vec::new();
        for m in &visible {
            let (x, y, cx, cy) = at(m);
            if m.objective {
                obstacles.push((Some(&m.label), (cx - 22.0, cy - 22.0, cx + 22.0, cy + 22.0)));
            } else {
                obstacles.push((Some(&m.label), (cx - 16.0, cy - 16.0, cx + 16.0, cy + 16.0)));
                obstacles.push((Some(&m.label), (x + 8.0, y - 29.0, x + 34.0, y - 12.0)));
            }
            if m.selected {
                obstacles.push((Some(&m.label), (cx - 22.0, cy - 22.0, cx + 22.0, cy + 22.0)));
            }
        }
        let (gx, gy) = (planning.gun.0 - ox, planning.gun.1 - oy);
        obstacles.push((None, (gx - 18.0, gy - 18.0, gx + 18.0, gy + 18.0)));
        let number = |label: &str| label.trim_start_matches(|c: char| c.is_ascii_alphabetic()).parse::<u32>().unwrap_or(0);
        let hovered = planning.hover.as_ref().map(|h| h.label.as_str());
        let mut order = visible.clone();
        order.sort_by_key(|m| (!m.selected, Some(m.label.as_str()) != hovered, m.objective, number(&m.label), m.label.clone()));
        let mut placed: Vec<Box2> = Vec::new();
        let mut placed_by: Vec<(String, Box2)> = Vec::new();
        let mut slots = self.slots.borrow_mut();
        for m in order {
            let Some(info) = planning.labels.iter().find(|l| l.label == m.label) else { continue };
            // Lines of (text, colour): what the settings pick; a location too coarse to aim by
            // always; the selected target's blast when too small to draw.
            let fields = planning.fields;
            let tint = if info.flight_s.is_some() { charge_color(info.charge) } else { dim };
            let mut first: Vec<(String, windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F)> = Vec::new();
            if fields.time {
                first.push((info.flight_s.map_or("—".to_owned(), |t| format!("{t:.0}s")), tint));
            }
            if fields.elevation {
                first.push((info.elevation.map_or("—".to_owned(), |e| format!("{e:.1}°")), tint));
            }
            if fields.distance {
                first.push((format!("{:.1} km", info.distance_m / 1000.0), white));
            }
            if let Some(prec) = m.precision_m {
                let coarse = prec > marks.coarse_m;
                if fields.precision || coarse {
                    first.push((if coarse { format!("±{prec:.1} m · zoom in") } else { format!("±{prec:.1} m") }, if coarse { amber } else { white }));
                }
            }
            let mut lines = Vec::new();
            if !first.is_empty() {
                lines.push(first);
            }
            if m.selected
                && fields.blast
                && let Some(blast) = marks.blast
            {
                lines.push(vec![(format!("blast {:.1} m", blast.reach_m), white)]);
            }
            if lines.is_empty() {
                continue;
            }
            let size = |lines: &Vec<Vec<(String, windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F)>>| -> windows::core::Result<(f32, f32)> {
                let mut w = 0.0f32;
                for (i, line) in lines.iter().enumerate() {
                    let mut lw = 0.0;
                    for (j, (text, _)) in line.iter().enumerate() {
                        lw += s.text_width(text, if i == 0 && j == 0 { &self.tag_first.0 } else { &self.tag.0 })? + if j > 0 { TAG_GAP } else { 0.0 };
                    }
                    w = w.max(lw);
                }
                Ok((w + 2.0 * TAG_PAD, TAG_H + TAG_LINE * (lines.len() as f32 - 1.0)))
            };
            let (x, y, cx, cy) = at(m);
            let candidates = |w: f32, h: f32| candidates_for(m.objective, m.selected, (cx, cy), x, y, w, h);
            let free = |r: Box2| within(r, bounds) && !obstacles.iter().any(|(owner, o)| *owner != Some(m.label.as_str()) && overlaps(r, *o)) && !placed.iter().any(|o| overlaps(r, *o));
            let fit = |lines: &Vec<Vec<(String, _)>>| -> windows::core::Result<Option<(usize, Box2)>> {
                let (w, h) = size(lines)?;
                let spots = candidates(w, h);
                let last = slots.get(&m.label).copied();
                let tries = last.into_iter().chain(0..spots.len());
                Ok(tries.filter_map(|i| spots.get(i).map(|&(sx, sy)| (i, (sx, sy, sx + w, sy + h)))).find(|&(_, r)| free(r)))
            };
            let mut got = fit(&lines)?;
            if got.is_none() && !m.selected && lines[0].len() > 1 {
                lines[0].truncate(1);
                got = fit(&lines)?;
            }
            if got.is_none() && m.selected {
                let (w, h) = size(&lines)?;
                let (sx, sy) = candidates(w, h)[0];
                got = Some((0, (sx, sy, sx + w, sy + h)));
            }
            let Some((slot, r)) = got else { continue };
            slots.insert(m.label.clone(), slot);
            placed.push(r);
            placed_by.push((m.label.clone(), r));
            if Some(m.label.as_str()) == hovered {
                continue;
            }
            s.fill_rect(D2D_RECT_F { left: r.0, top: r.1, right: r.2, bottom: r.3 }, plate);
            if m.selected {
                s.stroke_rect(D2D_RECT_F { left: r.0 + 1.0, top: r.1 + 1.0, right: r.2 - 1.0, bottom: r.3 - 1.0 }, red, 2.0);
            }
            let top = (r.1 + r.3) / 2.0 - TAG_LINE / 2.0 * (lines.len() as f32 - 1.0);
            for (i, line) in lines.iter().enumerate() {
                let mut tx = r.0 + TAG_PAD;
                for (j, (text, color)) in line.iter().enumerate() {
                    let (format, metrics) = if i == 0 && j == 0 { (&self.tag_first.0, &self.tag_first.1) } else { (&self.tag.0, &self.tag.1) };
                    s.text_on_caps(text, format, metrics, tx, r.2, top + TAG_LINE * i as f32, *color);
                    tx += s.text_width(text, format)? + TAG_GAP;
                }
            }
        }
        drop(slots);
        // The hovered target: a white ring where it is, and its label grown in place into the
        // whole card (one label, small until the pointer is on it).
        if let Some(hover) = &planning.hover
            && let Some(m) = visible.iter().find(|m| m.label == hover.label)
        {
            let (_, _, cx, cy) = at(m);
            let radius = if m.objective { 28.0 } else { 24.0 };
            s.ring(cx, cy, radius, rgba(0, 0, 0, 0.45), 4.5);
            s.ring(cx, cy, radius, white, 1.5);
            let caption = rgba(201, 208, 211, 1.0);
            let color = hover.charge.as_ref().map_or(dim, |(_, c)| charge_color(Some(*c)));
            let charge_text = hover.charge.as_ref().map_or("out of range".to_owned(), |(n, _)| n.clone());
            // The arc in use carries the charge's colour, as the small label's flight time did.
            let arc_row = |name: &str, arc: Option<(f32, f32)>, used: bool| -> [(String, windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F); 3] {
                [
                    (name.to_owned(), caption),
                    (arc.map_or("—".to_owned(), |a| format!("{:.2}°", a.0)), white),
                    (arc.map_or(String::new(), |a| format!("{:.1} s", a.1)), if used && arc.is_some() { color } else { white }),
                ]
            };
            let mut rows: Vec<[(String, windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F); 3]> = vec![
                [(hover.label.clone(), white), (charge_text, color), (format!("{:.1} km", hover.distance_m / 1000.0), white)],
                arc_row("LOW", hover.low, !hover.high_chosen),
                arc_row("HIGH", hover.high, hover.high_chosen),
            ];
            let precision = m.precision_m.map(|p| {
                let coarse = p > marks.coarse_m;
                (if coarse { format!("±{p:.1} m · zoom in") } else { format!("±{p:.1} m") }, if coarse { amber } else { white })
            });
            let blast = marks.blast.map(|b| (format!("blast {:.1} m", b.reach_m), white));
            let last = (precision.is_some() || blast.is_some()).then(|| {
                let (p, pc) = precision.clone().unwrap_or((String::new(), white));
                let (b, bc) = blast.clone().unwrap_or((String::new(), white));
                [(p, pc), (String::new(), white), (b, bc)]
            });
            if let Some(row) = last.clone() {
                rows.push(row);
            }
            let mut widths = [0.0f32; 3];
            for (i, row) in rows.iter().enumerate().take(3) {
                for (j, (text, _)) in row.iter().enumerate() {
                    widths[j] = widths[j].max(s.text_width(text, if i == 0 { &self.tag_first.0 } else { &self.tag.0 })?);
                }
            }
            let col1 = widths[0].max(42.0) + 10.0;
            let mut pw = TAG_PAD + 3.0 + col1 + widths[1] + 18.0 + widths[2] + TAG_PAD + 3.0;
            if let Some(row) = &last {
                pw = pw.max(2.0 * (TAG_PAD + 3.0) + s.text_width(&row[0].0, &self.tag.0)? + 18.0 + s.text_width(&row[2].0, &self.tag.0)?);
            }
            let ph = 16.0 + 24.0 * rows.len() as f32;
            // Where its small label is, grown away from the marker: from the label's corner
            // nearest the marker; kept in the map, off other markers where it can be.
            let small = placed_by.iter().find(|(l, _)| *l == hover.label).map_or((cx + 20.0, cy - 12.0, cx + 60.0, cy + 12.0), |(_, r)| *r);
            let corner_x = if (small.0 - cx).abs() <= (small.2 - cx).abs() { small.0 } else { small.2 - pw };
            let corner_y = if (small.1 - cy).abs() <= (small.3 - cy).abs() { small.1 } else { small.3 - ph };
            let fit = |x: f32, y: f32| (x.clamp(container.0 + 2.0, (container.2 - pw - 2.0).max(container.0 + 2.0)), y.clamp(container.1 + 2.0, (container.3 - ph - 2.0).max(container.1 + 2.0)));
            let own = (cx - radius, cy - radius, cx + radius, cy + radius);
            let mut spots = vec![fit(corner_x, corner_y)];
            spots.extend(candidates_for(m.objective, m.selected, (cx, cy), at(m).0, at(m).1, pw, ph).into_iter().map(|(x, y)| fit(x, y)));
            let clear = |&(x, y): &(f32, f32)| {
                let r = (x, y, x + pw, y + ph);
                !overlaps(r, own) && !obstacles.iter().any(|(owner, o)| *owner != Some(hover.label.as_str()) && overlaps(r, *o))
            };
            let (sx, sy) = spots.iter().copied().find(|p| clear(p)).or_else(|| spots.iter().copied().find(|&(x, y)| !overlaps((x, y, x + pw, y + ph), own))).unwrap_or(spots[0]);
            let r = D2D_RECT_F { left: sx, top: sy, right: sx + pw, bottom: sy + ph };
            s.fill_rect(r, plate);
            if m.selected {
                s.stroke_rect(D2D_RECT_F { left: sx + 1.0, top: sy + 1.0, right: sx + pw - 1.0, bottom: sy + ph - 1.0 }, red, 2.0);
            } else {
                s.stroke_rect(D2D_RECT_F { left: sx + 0.5, top: sy + 0.5, right: sx + pw - 0.5, bottom: sy + ph - 0.5 }, white, 1.0);
            }
            for (i, row) in rows.iter().enumerate() {
                let rcy = sy + 8.0 + 12.0 + 24.0 * i as f32;
                let first = if i == 0 { &self.tag_first } else { &self.tag };
                s.text_on_caps(&row[0].0, &first.0, &first.1, sx + TAG_PAD + 3.0, sx + pw, rcy, row[0].1);
                s.text_on_caps(&row[1].0, &first.0, &first.1, sx + TAG_PAD + 3.0 + col1, sx + pw, rcy, row[1].1);
                s.text_on_caps(&row[2].0, &self.tag_right.0, &self.tag_right.1, sx, sx + pw - TAG_PAD - 3.0, rcy, row[2].1);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Two markers, moved by `by` with the map, placed for moment `at`.
    fn marks(at: Instant, by: (f32, f32), limits: (f32, f32, f32, f32)) -> MapMarks {
        let mark = |label: &str, x: f32, y: f32| MapMark { label: label.into(), x: x + by.0, y: y + by.1, selected: false, objective: false, precision_m: Some(2.0), covered: false };
        MapMarks {
            left: 400,
            top: 200,
            size: 882,
            small: false,
            marks: vec![mark("A1", 700.0, 500.0), mark("A2", 900.0, 650.0)],
            coarse_m: 10.0,
            blast: None,
            planning: None,
            follow: Some(PanFollow { at, limits, press: (0.0, 0.0), pressed_at: at }),
            pred: None,
            scale: 1.0,
        }
    }

    const FREE: (f32, f32, f32, f32) = (-5000.0, -5000.0, 5000.0, 5000.0);

    #[test]
    fn a_dragged_marker_holds_then_sits_on_the_lattice_point_nearest_the_cursor() {
        let t = Instant::now();
        let mark = MapMark { label: "A1".into(), x: 0.0, y: 0.0, selected: false, objective: false, precision_m: None, covered: false };
        // Pressed 6 px left of and 5 px below the marker point; lattice of 10.2 px through (1000, 600).
        let mut drag = DragMark { mark, coarse_m: 5.0, grab: (6, -5), released_at: None, snap: Some(((1000.0, 600.0), 10.2)), press: (1241.0, 697.0), pressed_at: t, scale: 1.0 };
        // Before the threshold: where it was, wherever the cursor is.
        assert_eq!(drag_point(&drag, (1245.0, 699.0), None), (1247.0, 692.0));
        // Past it: the lattice point nearest the cursor's pixel (24.0 and 9.6 steps off: 24 and 10),
        // not the cursor plus the grab offset.
        let p = drag_point(&drag, (1244.8, 698.3), Some((1247.7, 697.0)));
        assert!((p.0 - 1244.8).abs() < 1e-9 && (p.1 - 702.0).abs() < 1e-9, "{p:?}");
        // Released: the lattice point nearest where the button came up.
        drag.released_at = Some((1266, 690));
        let p = drag_point(&drag, (1300.0, 650.0), Some((1247.7, 697.0)));
        assert!((p.0 - 1265.2).abs() < 1e-9 && (p.1 - 691.8).abs() < 1e-9, "{p:?}");
        // No registration: at the cursor.
        drag.snap = None;
        assert_eq!(drag_point(&drag, (1300.0, 650.0), Some((1247.7, 697.0))), (1266.0, 690.0));
    }

    /// A map marks window drawing offscreen (pan and clip do nothing there).
    fn window() -> MapWindow {
        let graphics = Graphics::new(std::path::Path::new("no-fonts")).expect("graphics");
        MapWindow::with_surface(HWND::default(), Surface::offscreen(&graphics, MAX_SIDE as u32, MAX_SIDE as u32).expect("surface")).expect("window")
    }

    fn pan(w: &MapWindow) -> (f32, f32) {
        w.following.as_ref().expect("following").pan
    }

    #[test]
    fn marks_moved_with_the_map_are_a_translation() {
        let t = Instant::now();
        let base = marks(t, (0.0, 0.0), FREE);
        let v = translation(&base, &marks(t + ms(12), (12.4, -3.0), FREE)).expect("translation");
        assert!((v.0 - 12.4).abs() < 1e-3 && (v.1 + 3.0).abs() < 1e-3);
        let mut relabelled = marks(t, (5.0, 0.0), FREE);
        relabelled.marks[1].label = "A3".into();
        assert_eq!(translation(&base, &relabelled), Err(Change::MarkState));
        let mut apart = marks(t, (5.0, 0.0), FREE);
        apart.marks[1].x += 2.0;
        assert_eq!(translation(&base, &apart), Err(Change::Position));
    }

    fn planned(t: Instant, by: (f32, f32), gun: (f32, f32)) -> MapMarks {
        let planning = Planning { gun, picture: (600.0 + by.0, 400.0 + by.1, 1400.0 + by.0, 1200.0 + by.1), rings: Vec::new(), labels: Vec::new(), fields: Default::default(), hover: None, stopped: None };
        MapMarks { planning: Some(planning), ..marks(t, by, FREE) }
    }

    #[test]
    fn a_gun_not_yet_seen_stays_nowhere_when_the_map_moves() {
        let t = Instant::now();
        // No gun: the placeholder does not move with the marks, and that is no change.
        assert!(translation(&planned(t, (0.0, 0.0), NO_GUN), &planned(t, (25.0, 8.0), NO_GUN)).is_ok());
        assert_eq!(planned(t, (0.0, 0.0), NO_GUN).shifted((25.0, 8.0)).planning.map(|p| p.gun), Some(NO_GUN));
        // A seen gun moves with them; one that moved apart, or appeared, is a change.
        assert!(translation(&planned(t, (0.0, 0.0), (900.0, 700.0)), &planned(t, (25.0, 8.0), (925.0, 708.0))).is_ok());
        assert_eq!(translation(&planned(t, (0.0, 0.0), (900.0, 700.0)), &planned(t, (25.0, 8.0), (926.0, 708.0))), Err(Change::Gun));
        assert_eq!(translation(&planned(t, (0.0, 0.0), NO_GUN), &planned(t, (25.0, 8.0), (925.0, 708.0))), Err(Change::Gun));
    }

    #[test]
    fn at_rest_a_shift_within_a_third_of_a_pixel_is_not_drawn() {
        let t = Instant::now();
        let mut w = window();
        let rest = |by: (f32, f32)| MapMarks { follow: None, ..marks(t, by, FREE) };
        assert!(w.show(Some(&rest((0.0, 0.0)))).expect("show"));
        // A nudge of a tenth of a pixel, twice: compared with what was drawn, still within.
        assert!(!w.show(Some(&rest((0.1, 0.0)))).expect("show"));
        assert!(!w.show(Some(&rest((0.25, -0.1)))).expect("show"));
        // Drifted past it: drawn.
        assert!(w.show(Some(&rest((0.4, 0.0)))).expect("show"));
        assert_eq!(w.take_redraws().into_iter().collect::<Vec<_>>(), vec![(Redraw::Appeared, 1), (Redraw::Moved, 1)]);
    }

    /// The cursor goes 1 px/ms right and 0.5 px/ms up from (100, 100) at `t0`.
    fn cursor(t0: Instant) -> impl Fn(Instant) -> Option<(f64, f64)> {
        move |t: Instant| {
            let ms = t.saturating_duration_since(t0).as_secs_f64() * 1000.0;
            Some((100.0 + ms, 100.0 - 0.5 * ms))
        }
    }

    #[test]
    fn a_followed_drag_moves_by_the_cursor_and_draws_only_on_change() {
        let t0 = Instant::now();
        let c = cursor(t0);
        let mut w = window();
        assert!(w.show(Some(&marks(t0, (0.0, 0.0), FREE))).expect("show"));
        assert!(w.following());
        // Where the game's map shows the cursor of moment 30 ms: 30 px right, 15 up.
        assert!(!w.follow(&c, t0 + ms(30)).expect("follow"));
        assert_eq!(pan(&w), (30.0, -15.0));
        // The HUD's marks of moment 20 ms are the drawn ones moved with the map: nothing drawn.
        w.show(Some(&marks(t0 + ms(20), (20.0, -10.0), FREE))).expect("show");
        assert!(!w.follow(&c, t0 + ms(30)).expect("follow"));
        assert_eq!(pan(&w), (30.0, -15.0));
        // A correction of the canvas moves every mark alike (4 px more than the map's move): the
        // drawing moves there, still nothing drawn.
        w.show(Some(&marks(t0 + ms(24), (28.0, -12.0), FREE))).expect("show");
        assert!(!w.follow(&c, t0 + ms(30)).expect("follow"));
        assert_eq!(pan(&w), (34.0, -15.0));
        // Another target selected: drawn again, with the pan from the new marks' moment.
        let mut selected = marks(t0 + ms(26), (30.0, -13.0), FREE);
        selected.marks[0].selected = true;
        w.show(Some(&selected)).expect("show");
        assert!(w.follow(&c, t0 + ms(30)).expect("follow"));
        assert_eq!(pan(&w), (4.0, -2.0));
        // The button comes up: drawn as they are, no longer followed.
        w.show(Some(&MapMarks { follow: None, ..marks(t0 + ms(40), (40.0, -20.0), FREE) })).expect("show");
        assert!(!w.following());
    }

    #[test]
    fn the_follow_stops_at_the_map_edge() {
        let t0 = Instant::now();
        let mut w = window();
        w.show(Some(&marks(t0, (0.0, 0.0), (-5.0, -5.0, 8.0, 8.0)))).expect("show");
        w.follow(&cursor(t0), t0 + ms(30)).expect("follow");
        assert_eq!(pan(&w), (8.0, -5.0));
    }

    #[test]
    fn a_long_drag_is_drawn_again_before_the_edge_of_what_is_drawn() {
        let t0 = Instant::now();
        let c = cursor(t0);
        let mut w = window();
        w.show(Some(&marks(t0, (0.0, 0.0), FREE))).expect("show");
        // 400 px right, past PAD − PAD_SLACK: drawn again where the marks are now.
        assert!(w.follow(&c, t0 + ms(400)).expect("follow"));
        assert_eq!(pan(&w), (0.0, 0.0));
        // 10 ms on, 10 px further, without drawing.
        assert!(!w.follow(&c, t0 + ms(410)).expect("follow"));
        assert_eq!(pan(&w), (10.0, -5.0));
        // The HUD's marks of moment 405 ms fold in as a move of what was drawn.
        w.show(Some(&marks(t0 + ms(405), (405.0, -202.5), FREE))).expect("show");
        assert!(!w.follow(&c, t0 + ms(410)).expect("follow"));
        assert_eq!(pan(&w), (10.0, -5.0));
    }
}
