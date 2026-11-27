//! Loading UI: the ammo cards with their charge selection, the loading countdown, and the loaded
//! slot (the gunner's bar header filled yellow, or the loader's simplified bar with a
//! yellow-framed box). Geometry measured on eight vehicles.
//!
//! The cards are centred on screen, one per shell, `CARD_PITCH` apart. They are found by the
//! light frame lines of their number row; the largest card count whose every card shows the
//! lines wins (fewer centred cards line up with the middle ones of more). The loader's menu is
//! the gunner's moved down `LOADER_SHIFT`; loading on foot beside the gun (towed guns, or a
//! dismounted crew member) moves it down `DISMOUNTED_SHIFT`.
//!
//! Hovering a card opens a detail box (mass, fuze setting, …) centred on that card just above
//! its number row; it covers the charge panels of that card and its neighbours, whose charge is
//! then reported as hidden.

use crate::bar::BarLayout;
use crate::text::{TextReader, TextStyle};
use crate::view::FrameView;

/// Card midline of a single card (frame x; cards are centred on screen) and card pitch.
const CARD_CENTRE_X: f64 = 1279.5;
pub const CARD_PITCH: f64 = 146.67;
/// Card frame half width (frame px from the midline to the border lines).
const CARD_HALF_WIDTH: f64 = 70.75;
/// Client y of the number row's top and bottom frame lines (gunner seat).
const NUMBER_ROW: (i32, i32) = (873, 899);
/// Client y of the charge panel's top line and the height of one charge row (gunner seat).
const CHARGE_TOP: f64 = 751.0;
const CHARGE_ROW: f64 = 38.5;
/// The loader's menu sits this much lower (no bar or angle readout below it), the menu on foot
/// lower still (the on-foot hotbar below it).
pub const LOADER_SHIFT: i32 = 150;
pub const DISMOUNTED_SHIFT: i32 = 191;
/// A frame line is this much brighter (channel mean) than the pixels two rows away.
const LINE_CONTRAST: i32 = 12;
/// Bright ground behind the UI: each channel at least this (the snow on Arctic Airbase is about
/// (221, 247, 252); ordinary ground far darker).
const BRIGHT_GROUND: u8 = 150;
/// Share of a card's width the line must cover.
const LINE_COVER: f64 = 0.6;
/// Selected charge row fill, about (226, 174, 70).
const MIN_SELECTED_SHARE: f64 = 0.5;
/// Countdown bar interior (frame x, client y). The bar is pure black; its fill is a flat colour
/// whose stripes and edges keep two channels equal: grey, green (done), red, grey-white or cyan
/// stripes (the loader's seat: red while counting, cyan after the quick reload key); the text is
/// white.
const COUNTDOWN: (i32, i32, i32, i32) = (1058, 996, 1502, 1019);
/// A column is the bar's when this share of its pixels are; this share of columns must be (the
/// loader's quick-reload marks are full-height columns).
const MIN_BAR_COLUMN_SHARE: f64 = 0.9;
const MIN_BAR_COLUMNS: f64 = 0.85;
/// Columns just inside and outside the bar's left and right ends (frame x).
const COUNTDOWN_EDGES: [(i32, i32); 2] = [(1057, 1052), (1502, 1508)];
/// How far into the bar the second column checked at each end lies, and the brightness step that
/// marks an end where the ground outside looks like the bar's colours.
const COUNTDOWN_EDGE_DEEPER: i32 = 3;
const COUNTDOWN_EDGE_STEP: i32 = 40;
const MIN_EDGE_SHARE: f64 = 0.6;
/// The countdown's unfilled part is pure black; anything brighter is its fill (or the seconds).
const COUNTDOWN_BLACK_MAX: u8 = 12;

/// The countdown bar: its interior (x0, y0, x1, y1; client px, ends exclusive) and the columns
/// just inside and outside its left and right ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CountdownBox {
    pub interior: (i32, i32, i32, i32),
    pub edges: [(i32, i32); 2],
}

/// Where the countdown bar is at the reference configuration.
pub const COUNTDOWN_BOX: CountdownBox = CountdownBox { interior: COUNTDOWN, edges: COUNTDOWN_EDGES };

/// Off the reference configuration the countdown bar does not follow the bottom group's rule: at
/// 100 % 1920x1080 it was 300 px wide (296 logical px at the reference, 300 there) with its top
/// at client y 805, which mapped to the reference geometry is 419 px wide and 47 px higher than at
/// the reference. So it is found where it shows (`find_countdown`):
/// centred on the client as at the reference, in these rows (client y, reference geometry), as
/// wide and as tall as this (px), its rows' runs of bar pixels through the middle agreeing on both
/// ends.
const COUNTDOWN_SEARCH_Y: (i32, i32) = (840, 1110);
const COUNTDOWN_SEARCH_WIDTH: (i32, i32) = (300, 640);
const COUNTDOWN_SEARCH_HEIGHT: (i32, i32) = (14, 44);
/// Pixels off the bar's colours a run may cross: the loader's quick-reload mark (9 px wide mapped
/// from 100 % 1920x1080), the green line at the fill's end and the seconds' edges.
const COUNTDOWN_SEARCH_GAP: i32 = 14;
/// Bar pixels in a row that end a gap.
const COUNTDOWN_SEARCH_SOLID: i32 = 3;
/// Hover detail box: about 381 px wide, centred on the hovered card, bottom just above the
/// number row; dark and nearly neutral, about (12, 16, 20). Checked band above the number row.
const DETAIL_HALF_WIDTH: f64 = 191.0;
const DETAIL_BAND: (i32, i32) = (-110, -20);
const DETAIL_MAX_SPREAD: u8 = 10;
const DETAIL_MAX_MEAN: i32 = 24;
const MIN_DETAIL_SHARE: f64 = 0.5;
/// The box's left and right edges (frame px from the hovered card's midline): the box is darker
/// inside by 58–93 (channel mean, band average) on the recorded boxes.
const DETAIL_EDGES: (f64, f64) = (-191.0, 190.0);
const MIN_DETAIL_EDGE_STEP: f64 = 30.0;

/// Charge bag capsule at a card's top-left (gunner seat): a cap, then one white segment per bag,
/// segments 8–14 px long with grey dividers up to 6 px. Centre row (client y), search rows, the
/// window's offset from the card midline and width, where the first segment may start, and the
/// segments' brightness (lowest channel).
const BAG_ROW: i32 = 914;
const BAG_ROW_SEARCH: i32 = 3;
const BAG_WINDOW: (f64, i32) = (-69.0, 76);
const BAG_SEGMENT: (i32, i32) = (8, 14);
const BAG_FIRST_START: (i32, i32) = (5, 20);
const BAG_MAX_GAP: i32 = 6;
/// A segment's lowest channel is at least this (segments are opaque white, 241–255 at their edges);
/// the dividers between them show the ground through, up to 233 over snow (157–229 over grass).
const BAG_MIN_BRIGHT: u8 = 240;
/// The same capsule on the gunner's bar, above each slot's shell icon: centre row (client y,
/// rows 1222–1235 on the recorded bars) and where its first segment may start from the cell's
/// left edge. White on the dark fill of the loaded (highlighted) slot; dim on the others.
const BAR_BAG_ROW: i32 = 1229;
const BAR_BAG_FIRST_START: (i32, i32) = (5, 30);
/// Card text boxes (frame px from the card midline, client y, gunner seat): shell name, and the
/// value column of the VEL row (muzzle velocity at the selected charge).
const NAME_BOX: (f64, f64, i32, i32) = (-66.0, 68.0, 1006, 1034);
const VALUE_BOX: (f64, f64, i32, i32) = (-16.0, 68.0, 1063, 1092);
/// Light text: lowest channel from this on can be ink (the reader adapts above it).
const TEXT_MIN_INK: u8 = 90;
/// Gunner's weapon label: a yellow-framed box above the bar (top frame line at client y 1149,
/// bottom 1185), searched across this frame x range.
const WEAPON_BOX_TOP: i32 = 1149;
/// Rows above and below `WEAPON_BOX_TOP` searched for the frame line: mapped from 100 % with the
/// ammo bar's transform the box sits up to 2 px higher, its own position being set apart from
/// the bar's.
const WEAPON_BOX_REACH: i32 = 3;
const WEAPON_BOX_HEIGHT: i32 = 36;
const WEAPON_BOX_SEARCH: (i32, i32) = (860, 1720);
const MIN_WEAPON_BOX_WIDTH: i32 = 60;
/// Loader's weapon label: centred in the strip under the cards or the simplified bar (client y);
/// dimmer under the simplified bar. On foot the strip is `DISMOUNTED_SHIFT - LOADER_SHIFT` lower.
const LOADER_LABEL: (i32, i32, i32, i32) = (1000, 1294, 1560, 1314);
const LOADER_LABEL_MIN_INK: u8 = 90;
/// The gunner's weapon box matched against the data's labels (`read_weapon_label_among`) takes the
/// best when it scores this and leads the next by this much. On every recording the label shown
/// scored 0.66–1.0 (lowest on the soft 125 % text under 900 px high) and no other label fitted,
/// but for the Centurion AVRE's "165 mm L9A1", where "105 mm L7A1" scored 0.88 against 0.98: a
/// lead of 0.106 at 150 %; under half of that the box is read glyph by glyph instead.
const WEAPON_MATCH_MIN_SCORE: f32 = 0.5;
const WEAPON_MATCH_MIN_LEAD: f32 = 0.05;
/// Gunner's bar: header band of a cell (client y) and its inset from the cell edges (frame px).
const BAR_HEADER: (i32, i32) = (1195, 1213);
const BAR_HEADER_INSET: i32 = 6;
/// The bar collapsed under the full map: one row of cells in the open bar's columns, the loaded
/// slot's number on an amber fill at client y 1294–1322 (on a 2S19 recording: slot 2 at frame x
/// 1117–1222, amber share 0.99; every cell 0.00 once the shell was fired). The band read
/// keeps 2 px off its top and bottom edges.
const STRIP_ROWS: (i32, i32) = (1296, 1320);
/// A collapsed cell without the amber fill is the bar's translucent dark: medians (27–48, 29–49,
/// 32–53) over the grey test ground, up to (61, 84, 28) over grass: no channel of the median
/// above this.
const STRIP_DARK_MAX: u8 = 110;
/// Loader's simplified bar after loading: one box per shell, centred on screen, 110 px apart;
/// box frame half width and the client y of its top and bottom frame lines.
const LOADER_BAR_PITCH: f64 = 110.0;
const LOADER_BAR_HALF_WIDTH: f64 = 53.0;
const LOADER_BAR_ROWS: (i32, i32) = (1222, 1281);
/// Most cards (and loader bar boxes) a menu has.
const MAX_SLOTS: usize = 4;

/// The client region (x, y, width, height) the HUD reads back for all of the loading UI: the
/// cards with the detail box's edges, the countdown, both bars and the weapon labels. Every
/// reader here works on a view of just this region.
pub const LOADING_REGION: (i32, i32, i32, i32) = (860, 740, 860, 590);
/// The loaded box's frame is translucent yellow, (147, 120, 56) over grass to (184, 150, 80) over
/// sand; the others are light grey. A frame pixel counts as yellow when it is this much yellower
/// ((r + g) / 2 − b) than the box inside three rows in.
const MIN_FRAME_YELLOWING: i32 = 30;
const MIN_FRAME_YELLOW_SHARE: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charge {
    Low,
    Medium,
    High,
}

const CHARGES: [Charge; 3] = [Charge::Low, Charge::Medium, Charge::High];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuSeat {
    Gunner,
    Loader,
    /// On foot beside the gun.
    Dismounted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardCharge {
    /// No charge panel: the shell is loaded without charges.
    None,
    Selected(Charge),
    /// The panel is under the hover detail box.
    Hidden,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// Frame x of the card's midline.
    pub centre_x: f64,
    pub charge: CardCharge,
    /// Segments of the charge bag capsule (0 without one). Not covered by the hover detail box;
    /// rockets show a one-segment capsule without a charge panel.
    pub bags: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub seat: MenuSeat,
    pub cards: Vec<Card>,
}

/// Frame x of the midline of card `i` of `n`.
pub fn card_centre(i: usize, n: usize) -> f64 {
    CARD_CENTRE_X + CARD_PITCH * (i as f64 - (n as f64 - 1.0) / 2.0)
}

/// Client y of the middle of the `charge` row on the cards of a menu in `seat`.
pub fn charge_row_centre(seat: MenuSeat, charge: Charge) -> f64 {
    let shift = match seat {
        MenuSeat::Gunner => 0,
        MenuSeat::Loader => LOADER_SHIFT,
        MenuSeat::Dismounted => DISMOUNTED_SHIFT,
    };
    let k = CHARGES.iter().position(|&c| c == charge).unwrap_or(0) as f64;
    f64::from(shift) + CHARGE_TOP + CHARGE_ROW * (k + 0.5)
}

/// Frame px from a card's midline to its side border lines.
pub const CARD_HALF: f64 = CARD_HALF_WIDTH;

/// Client x of the left edge of a menu of `n` cards (the loader's simplified bar is narrower).
pub fn menu_left(n: usize) -> f64 {
    card_centre(0, n) - CARD_HALF_WIDTH
}

/// Client y of the bottom of the loader's weapon strip under the cards or the simplified bar; the
/// dismounted menu's cards end on the same line.
pub const LOADER_STRIP_BOTTOM: i32 = 1322;

fn mean(p: [u8; 3]) -> i32 {
    (i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2])) / 3
}

/// Share of `x0..x1` that is a light line on frame row `y` (±1): brighter than two rows away on
/// the card's side (`inside`: +1 below the line, -1 above it), and on the other side too unless
/// the scene there is bright ground (snow, sky: a card's line over it is darker than it; without
/// this the mortar's cards on Arctic Airbase were counted as one).
fn line_share(view: &FrameView, x0: i32, x1: i32, y: i32, inside: i32) -> f64 {
    let share = |y: i32| {
        let lit = (x0..x1).filter(|&x| {
            let v = mean(view.px(x, y));
            let outside = view.px(x, y - 2 * inside);
            // Outside the card the ground may show through brighter than the line: snow under the
            // translucent icon panel (about 161 against the line's 146).
            v > mean(view.px(x, y + 2 * inside)) + LINE_CONTRAST && (v > mean(outside) + LINE_CONTRAST || bright_ground(outside) || mean(outside) >= v)
        });
        lit.count() as f64 / f64::from((x1 - x0).max(1))
    };
    share(y - 1).max(share(y)).max(share(y + 1))
}

/// A pixel of bright ground behind the UI (snow, sky): its lowest channel above this.
fn bright_ground(p: [u8; 3]) -> bool {
    p[0].min(p[1]).min(p[2]) >= BRIGHT_GROUND
}

/// A pixel of the action-bar colour's fill (the selected charge row, the loaded slot's header):
/// amber, about (228, 176, 72). Bright and warm rather than a narrow band of levels, so a picture
/// made brighter still counts: Windows HDR captured as 8 bits lifts it to (255, 255, 106).
fn is_selected_fill(p: [u8; 3]) -> bool {
    let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
    r > 200 && g >= 150 && b < 130 && r + g - 2 * b >= 220 && r >= g - 8
}

/// Countdown bar pixel: neutral (black, grey, white), green (r = b) or red and cyan (g = b) fill.
fn bar_pixel(p: [u8; 3]) -> bool {
    let [r, g, b] = p;
    let eq = |a: u8, c: u8| a.abs_diff(c) <= 1;
    (eq(r, g) && eq(g, b)) || (eq(r, b) && g > r) || eq(g, b)
}

/// The ammo cards, if a loading menu is open.
pub fn read_menu(view: &FrameView, client_origin: (i32, i32)) -> Option<Menu> {
    let (ox, oy) = client_origin;
    for (seat, shift) in [(MenuSeat::Gunner, 0), (MenuSeat::Loader, LOADER_SHIFT), (MenuSeat::Dismounted, DISMOUNTED_SHIFT)] {
        let rows = (oy + NUMBER_ROW.0 + shift, oy + NUMBER_ROW.1 + shift);
        // The widest menu's number row (a read-back region holds just the menu).
        let (left, right) = (ox + menu_left(MAX_SLOTS) as i32, ox + (card_centre(MAX_SLOTS - 1, MAX_SLOTS) + CARD_HALF_WIDTH) as i32 + 1);
        if !view.covers(left, rows.0 - 3, right - left, rows.1 - rows.0 + 7) {
            continue;
        }
        let shows = |c: f64| {
            let (x0, x1) = ((c - CARD_HALF_WIDTH) as i32 + 6, (c + CARD_HALF_WIDTH) as i32 - 6);
            (line_share(view, x0, x1, rows.0, 1) + line_share(view, x0, x1, rows.1, -1)) / 2.0 >= LINE_COVER
        };
        let Some(n) = (1..=MAX_SLOTS).rev().find(|&n| (0..n).all(|i| shows(f64::from(ox) + card_centre(i, n)))) else { continue };
        let centres: Vec<f64> = (0..n).map(|i| f64::from(ox) + card_centre(i, n)).collect();
        // The box is centred on the hovered card: dark above it, with the box's edges where they
        // belong (a dark vehicle behind the cards has no such edges).
        let detail = centres
            .iter()
            .map(|&c| (c, detail_box_share(view, c, rows.0)))
            .filter(|&(c, share)| share >= MIN_DETAIL_SHARE && detail_box_edge_step(view, c, rows.0) >= MIN_DETAIL_EDGE_STEP)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(c, _)| c);
        let cards = centres
            .iter()
            .map(|&c| {
                let hidden = detail.is_some_and(|d| (c - d).abs() < DETAIL_HALF_WIDTH + CARD_HALF_WIDTH);
                Card {
                    centre_x: c,
                    charge: if hidden { CardCharge::Hidden } else { read_charge(view, c, oy + shift) },
                    bags: read_bags(view, c, oy + shift),
                }
            })
            .collect();
        return Some(Menu { seat, cards });
    }
    None
}

/// Share of detail-box pixels in the band where the hover detail box of the card at `centre_x`
/// would be (number row top at frame y `row`).
fn detail_box_share(view: &FrameView, centre_x: f64, row: i32) -> f64 {
    let (x0, x1) = ((centre_x - DETAIL_HALF_WIDTH) as i32 + 10, (centre_x + DETAIL_HALF_WIDTH) as i32 - 10);
    let (y0, y1) = (row + DETAIL_BAND.0, row + DETAIL_BAND.1);
    if !view.covers(x0, y0, x1 - x0, y1 - y0) {
        return 0.0;
    }
    let dark = (y0..y1)
        .step_by(2)
        .flat_map(|y| (x0..x1).step_by(2).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = view.px(x, y);
            p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]) <= DETAIL_MAX_SPREAD && mean(p) < DETAIL_MAX_MEAN
        })
        .count();
    let total = ((y1 - y0 + 1) / 2) * ((x1 - x0 + 1) / 2);
    dark as f64 / f64::from(total.max(1))
}

/// The stronger of the steps (outside minus inside, channel mean over the band) at the box's left
/// and right edges, each the best within ±2 px of where it belongs.
fn detail_box_edge_step(view: &FrameView, centre_x: f64, row: i32) -> f64 {
    let (y0, y1) = (row + DETAIL_BAND.0, row + DETAIL_BAND.1);
    let column = |x: i32| (y0..y1).map(|y| f64::from(mean(view.px(x, y)))).sum::<f64>() / f64::from(y1 - y0);
    let around = |x: i32, from: i32, to: i32| (from..=to).map(|d| column(x + d)).sum::<f64>() / f64::from(to - from + 1);
    let left = (centre_x + DETAIL_EDGES.0).round() as i32;
    let right = (centre_x + DETAIL_EDGES.1).round() as i32;
    if !view.covers(left - 8, y0, right - left + 16, y1 - y0) {
        return 0.0;
    }
    let best = |edge: i32, inside: (i32, i32), outside: (i32, i32)| {
        (-2..=2).map(|d| around(edge + d, outside.0, outside.1) - around(edge + d, inside.0, inside.1)).fold(f64::MIN, f64::max)
    };
    best(left, (1, 3), (-4, -2)).max(best(right, (-4, -2), (1, 3)))
}

/// Charge bags on the card at `centre_x`: the most common segment count over the search rows.
fn read_bags(view: &FrameView, centre_x: f64, top_y: i32) -> u32 {
    bag_segments(view, (centre_x + BAG_WINDOW.0).round() as i32, BAG_WINDOW.1, top_y + BAG_ROW, BAG_FIRST_START)
}

/// Charge bags on slot `slot` of the gunner's bar: read on the loaded slot, the one highlighted
/// (`gunner_bar_highlight`). Bags are the charge: 1 Low, 2 Medium, 3 High on every recorded
/// card; a teammate's loading shows here, with no menu on the gunner's screen.
pub fn gunner_bar_bags(view: &FrameView, client_origin: (i32, i32), layout: &BarLayout, slot: usize) -> u32 {
    bag_segments(view, layout.cell_x(slot), BarLayout::CELL_WIDTH, client_origin.1 + BAR_BAG_ROW, BAR_BAG_FIRST_START)
}

/// White capsule segments in a window `width` px from `x0` around row `row`: the most common
/// count over the search rows.
fn bag_segments(view: &FrameView, x0: i32, width: i32, row: i32, first_start: (i32, i32)) -> u32 {
    if !view.covers(x0, row - BAG_ROW_SEARCH, width, 2 * BAG_ROW_SEARCH + 1) {
        return 0;
    }
    let mut counts = [0u32; 8];
    for y in row - BAG_ROW_SEARCH..=row + BAG_ROW_SEARCH {
        let bright = |x: i32| {
            let p = view.px(x0 + x, y);
            p[0].min(p[1]).min(p[2]) >= BAG_MIN_BRIGHT
        };
        let mut runs = Vec::new();
        let mut start = None;
        for x in 0..=width {
            match (x < width && bright(x), start) {
                (true, None) => start = Some(x),
                (false, Some(a)) => {
                    runs.push((a, x));
                    start = None;
                }
                _ => {}
            }
        }
        // Segments chained by short gaps; specks inside a divider are skipped, a long run ends it.
        let mut segments = 0;
        let mut last_end = None;
        for (a, b) in runs {
            if b - a < BAG_SEGMENT.0 {
                continue;
            }
            let fits = b - a <= BAG_SEGMENT.1 && last_end.map_or((first_start.0..=first_start.1).contains(&a), |e| a - e <= BAG_MAX_GAP);
            if !fits {
                break;
            }
            segments += 1;
            last_end = Some(b);
        }
        counts[segments.min(7)] += 1;
    }
    (0..8).max_by_key(|&k| (counts[k], std::cmp::Reverse(k))).map_or(0, |k| k as u32)
}

fn read_charge(view: &FrameView, centre_x: f64, top_y: i32) -> CardCharge {
    let (x0, x1) = ((centre_x - CARD_HALF_WIDTH) as i32 + 4, (centre_x + CARD_HALF_WIDTH) as i32 - 4);
    let panel_top = top_y + CHARGE_TOP as i32;
    let has_line = view.covers(x0, panel_top - 3, x1 - x0, 7) && line_share(view, x0 + 2, x1 - 2, panel_top, 1) >= LINE_COVER;
    let filled = |k: usize| {
        let y0 = top_y + (CHARGE_TOP + CHARGE_ROW * k as f64) as i32 + 4;
        let y1 = top_y + (CHARGE_TOP + CHARGE_ROW * (k + 1) as f64) as i32 - 4;
        let total = ((x1 - x0) * (y1 - y0)).max(1);
        let yellow = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| is_selected_fill(view.px(x, y))).count();
        yellow as f64 / f64::from(total)
    };
    let charge = (0..3).map(|k| (k, filled(k))).filter(|&(_, s)| s >= MIN_SELECTED_SHARE).max_by(|a, b| a.1.total_cmp(&b.1)).map(|(k, _)| CHARGES[k]);
    match charge {
        Some(c) => CardCharge::Selected(c),
        // A panel with no row filled: not seen yet (menu opening); report it hidden.
        None if has_line => CardCharge::Hidden,
        None => CardCharge::None,
    }
}

fn yellowness(p: [u8; 3]) -> i32 {
    (i32::from(p[0]) + i32::from(p[1])) / 2 - i32::from(p[2])
}

/// Shell slot (0-based) whose header in the gunner's bar is filled yellow: the loaded slot (or
/// the one loading).
pub fn gunner_bar_highlight(view: &FrameView, client_origin: (i32, i32), layout: &BarLayout) -> Option<usize> {
    (0..layout.shell_slots).find(|&i| amber_share(view, layout, i, (client_origin.1 + BAR_HEADER.0, client_origin.1 + BAR_HEADER.1)) >= MIN_SELECTED_SHARE)
}

/// Share of a cell's band (frame rows `y0..y1`, inset from the cell's sides) in the amber fill;
/// 0 when the band is outside the view.
fn amber_share(view: &FrameView, layout: &BarLayout, i: usize, (y0, y1): (i32, i32)) -> f64 {
    let x0 = layout.cell_x(i) + BAR_HEADER_INSET;
    let x1 = layout.cell_x(i) + BarLayout::CELL_WIDTH - BAR_HEADER_INSET;
    if !view.covers(x0, y0, x1 - x0, y1 - y0) {
        return 0.0;
    }
    let yellow = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| is_selected_fill(view.px(x, y))).count();
    yellow as f64 / f64::from((x1 - x0) * (y1 - y0))
}

/// Under the full map, the collapsed bar of the open bar's `layout`: `Some(Some(slot))` with
/// the loaded slot lit, `Some(None)` when every cell shows the bar's translucent dark (nothing
/// loaded: the shell was fired), `None` when a cell is too bright to tell (covered, or bright
/// ground through the bar).
pub fn gunner_strip_highlight(view: &FrameView, client_origin: (i32, i32), layout: &BarLayout) -> Option<Option<usize>> {
    let rows = (client_origin.1 + STRIP_ROWS.0, client_origin.1 + STRIP_ROWS.1);
    let mut lit = None;
    for i in 0..layout.shell_slots {
        if amber_share(view, layout, i, rows) >= MIN_SELECTED_SHARE {
            lit = Some(i);
            continue;
        }
        let (x0, x1) = (layout.cell_x(i) + BAR_HEADER_INSET, layout.cell_x(i) + BarLayout::CELL_WIDTH - BAR_HEADER_INSET);
        if !view.covers(x0, rows.0, x1 - x0, rows.1 - rows.0) {
            return None;
        }
        let mut pixels: Vec<[u8; 3]> = (rows.0..rows.1).step_by(2).flat_map(|y| (x0..x1).step_by(2).map(move |x| (x, y))).map(|(x, y)| view.px(x, y)).collect();
        pixels.sort_by_key(|p| p.iter().map(|&c| u32::from(c)).sum::<u32>());
        if pixels[pixels.len() / 2].iter().any(|&c| c > STRIP_DARK_MAX) {
            return None;
        }
    }
    Some(lit)
}

/// Shell type icons of the gunner's bar, from the game's icon library (whose file names are the
/// shell data's types). In a loaded cell the icon fills a
/// square `ICON_SIDE` px wide, `ICON_LEFT` px in from the left end of the yellow header, top at
/// client y `ICON_TOP` (fitted on a crew-loading recording: HE 0.96, proximity fuze 0.87, time
/// fuze 0.77, smoke 0.96 against at most 0.65 for any other icon). The loaded slot's icon and its
/// charge bags tell the shell without the menu, which the gunner does not see while a teammate
/// loads.
pub struct ShellIcons {
    side: usize,
    /// Icon file name, luma premultiplied by alpha (0–255) and alpha (0–1), row-major.
    icons: Vec<(String, Vec<f32>, Vec<f32>)>,
}

const ICON_TOP: i32 = 1219;
const ICON_LEFT: i32 = 3;
/// Where the cell's background is sampled (client y, px right of the header's left end): left of
/// the icon, below the capsule.
const ICON_BACKGROUND_ROWS: (i32, i32) = (1240, 1250);
const ICON_BACKGROUND_COLUMNS: (i32, i32) = (3, 10);
/// A match needs this correlation and this lead over the next icon.
const ICON_MIN_SCORE: f32 = 0.6;
const ICON_MIN_MARGIN: f32 = 0.1;
/// Among a weapon's types, an icon of another type may score at most this much higher (a look-alike).
const ICON_LOOKALIKE: f32 = 0.05;

/// The loaded slot's icon: the shell data's type it stands for, and the match.
#[derive(Clone, Debug, PartialEq)]
pub struct IconMatch {
    pub icon: String,
    /// The `type` of the shells this icon is drawn for ("HE", "TIME FUZE", ...).
    pub kind: &'static str,
    pub score: f32,
    pub margin: f32,
}

/// The shell data's `type` for an icon file of the library.
pub fn icon_kind(icon: &str) -> &'static str {
    match icon {
        "he" => "HE",
        "time-fuze" => "TIME FUZE",
        "proximity-fuze" => "PROXIMITY FUZE",
        "smoke-shell" | "smoke" => "SMOKE",
        "cluster" => "CLUSTER",
        "rocket" => "ROCKET",
        "heat" => "HEAT",
        "heat-fs" => "HEAT-FS",
        "hesh" => "HESH",
        "saphe" => "SAPHE",
        "thermobaric" => "THERMOBARIC",
        "incendiary" => "INCENDIARY",
        "top-attack-shell" => "Top Attack Shell",
        _ => "",
    }
}

impl Default for ShellIcons {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellIcons {
    pub fn new() -> Self {
        let bytes: &[u8] = include_bytes!("../assets/shell_icons.bin");
        assert_eq!(&bytes[..4], b"MTCI", "shell icon asset");
        let count = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
        let side = usize::from(u16::from_le_bytes([bytes[6], bytes[7]]));
        let mut at = 8;
        let icons = (0..count)
            .map(|_| {
                let len = usize::from(bytes[at]);
                let name = String::from_utf8(bytes[at + 1..at + 1 + len].to_vec()).expect("icon name");
                at += 1 + len;
                let premul = bytes[at..at + side * side].iter().map(|&v| f32::from(v)).collect();
                at += side * side;
                let alpha = bytes[at..at + side * side].iter().map(|&v| f32::from(v) / 255.0).collect();
                at += side * side;
                (name, premul, alpha)
            })
            .collect();
        Self { side, icons }
    }

    /// The icon on slot `slot` of the gunner's bar, when the slot is the loaded (highlighted) one.
    pub fn read(&self, view: &FrameView, client_origin: (i32, i32), layout: &BarLayout, slot: usize) -> Option<IconMatch> {
        self.read_among(view, client_origin, layout, slot, None)
    }

    /// `read` among the icons of the shell types in `kinds` (a weapon's shells): look-alike icons
    /// of other types (cluster and incendiary differ only in a small symbol) do not count against
    /// the match, but one of another type scoring clearly higher rejects it (an icon missing from
    /// the library, such as a rocket's own).
    pub fn read_among(&self, view: &FrameView, client_origin: (i32, i32), layout: &BarLayout, slot: usize, kinds: Option<&[&str]>) -> Option<IconMatch> {
        let scores = self.scores(view, client_origin, layout, slot)?;
        let allowed = |name: &str| kinds.is_none_or(|k| k.contains(&icon_kind(name)));
        let overall = scores[0].0;
        let mut among = scores.iter().filter(|(_, n)| allowed(n));
        let &(best, ref name) = among.next()?;
        let margin = best - among.next().map_or(0.0, |s| s.0);
        (best >= ICON_MIN_SCORE && margin >= ICON_MIN_MARGIN && overall - best <= ICON_LOOKALIKE)
            .then(|| IconMatch { icon: name.clone(), kind: icon_kind(name), score: best, margin })
    }

    /// Correlation of the loaded slot's icon with every icon of the library, best first.
    pub fn scores(&self, view: &FrameView, client_origin: (i32, i32), layout: &BarLayout, slot: usize) -> Option<Vec<(f32, String)>> {
        let header_y = client_origin.1 + (BAR_HEADER.0 + BAR_HEADER.1) / 2;
        let cell = layout.cell_x(slot);
        let x0 = (cell - 6..cell + 12).find(|&x| view.covers(x, header_y, 1, 1) && is_selected_fill(view.px(x, header_y)))?;
        let n = self.side as i32;
        let (left, top) = (x0 + ICON_LEFT, client_origin.1 + ICON_TOP);
        if !view.covers(left, top, n, n) {
            return None;
        }
        let luma = |x: i32, y: i32| {
            let [r, g, b] = view.px(x, y);
            0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)
        };
        let mut background: Vec<f32> = (ICON_BACKGROUND_ROWS.0..ICON_BACKGROUND_ROWS.1)
            .flat_map(|y| (ICON_BACKGROUND_COLUMNS.0..ICON_BACKGROUND_COLUMNS.1).map(move |dx| (x0 + dx, client_origin.1 + y)))
            .filter(|&(x, y)| view.covers(x, y, 1, 1))
            .map(|(x, y)| luma(x, y))
            .collect();
        background.sort_by(f32::total_cmp);
        let bg = *background.get(background.len() / 2)?;
        // The middle three fifths of the rows: the charge capsule sits above, the count below.
        let rows = self.side / 5..self.side * 4 / 5;
        let cell_values: Vec<f32> = rows.clone().flat_map(|y| (0..n).map(move |x| (x, y))).map(|(x, y)| luma(left + x, top + y as i32)).collect();
        let mut scores: Vec<(f32, String)> = self
            .icons
            .iter()
            .map(|(name, premul, alpha)| {
                let template: Vec<f32> = rows.clone().flat_map(|y| (0..self.side).map(move |x| y * self.side + x)).map(|i| premul[i] + bg * (1.0 - alpha[i])).collect();
                (correlation(&cell_values, &template), name.clone())
            })
            .collect();
        scores.sort_by(|a, b| b.0.total_cmp(&a.0));
        Some(scores)
    }
}

/// Normalised cross-correlation of two equally long value lists.
fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut ab, mut aa, mut bb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        let (dx, dy) = (x - ma, y - mb);
        ab += dx * dy;
        aa += dx * dx;
        bb += dy * dy;
    }
    if aa <= 0.0 || bb <= 0.0 { 0.0 } else { ab / (aa * bb).sqrt() }
}

/// The loader's simplified bar after loading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoaderBar {
    pub slots: usize,
    /// The loaded slot (0-based), framed yellow.
    pub loaded: Option<usize>,
}

/// The loader's simplified bar, found like the cards: the largest box count whose every box shows
/// its top and bottom frame lines.
pub fn read_loader_bar(view: &FrameView, client_origin: (i32, i32)) -> Option<LoaderBar> {
    let (ox, oy) = client_origin;
    let rows = (oy + LOADER_BAR_ROWS.0, oy + LOADER_BAR_ROWS.1);
    let centre = |i: usize, n: usize| f64::from(ox) + CARD_CENTRE_X + LOADER_BAR_PITCH * (i as f64 - (n as f64 - 1.0) / 2.0);
    // The widest bar's rows (a read-back region holds just the bar).
    let (left, right) = ((centre(0, MAX_SLOTS) - LOADER_BAR_HALF_WIDTH) as i32, (centre(MAX_SLOTS - 1, MAX_SLOTS) + LOADER_BAR_HALF_WIDTH) as i32 + 1);
    if !view.covers(left, rows.0 - 3, right - left, rows.1 - rows.0 + 7) {
        return None;
    }
    let span = |c: f64| ((c - LOADER_BAR_HALF_WIDTH) as i32 + 6, (c + LOADER_BAR_HALF_WIDTH) as i32 - 6);
    let shows = |c: f64| {
        let (x0, x1) = span(c);
        (line_share(view, x0, x1, rows.0, 1) + line_share(view, x0, x1, rows.1, -1)) / 2.0 >= LINE_COVER
    };
    let n = (1..=MAX_SLOTS).rev().find(|&n| (0..n).all(|i| shows(centre(i, n))))?;
    let loaded = (0..n).find(|&i| {
        let (x0, x1) = span(centre(i, n));
        // `inward`: +1 below the top line, −1 above the bottom line.
        let yellow = |y: i32, inward: i32| {
            (y - 1..=y + 1)
                .map(|y| (x0..x1).filter(|&x| yellowness(view.px(x, y)) - yellowness(view.px(x, y + 3 * inward)) >= MIN_FRAME_YELLOWING).count())
                .max()
                .unwrap_or(0)
        };
        (yellow(rows.0, 1) + yellow(rows.1, -1)) as f64 >= MIN_FRAME_YELLOW_SHARE * 2.0 * f64::from(x1 - x0)
    });
    Some(LoaderBar { slots: n, loaded })
}

/// What a card's text says: the shell name (often cut, "M107 High..."; look-alike glyphs may be
/// confused, see `text`) and the muzzle velocity in the VEL row (m/s, at the selected charge).
#[derive(Clone, Debug, PartialEq)]
pub struct CardText {
    pub name: Option<String>,
    pub velocity: Option<u32>,
    /// The name is the candidate string that matched (`read_card_texts_among`), with its score
    /// and the runner-up's; `None` when read glyph by glyph.
    pub matched: Option<(f32, f32)>,
}

/// A card name matched against whole strings needs this mean glyph score (measured on the
/// labelled menus, `examples/card_texts.rs`).
const MATCH_MIN_SCORE: f32 = 0.5;
/// A VEL value read as a number the shells do not fly at is replaced by the best of theirs only
/// when that scores this (a true value scores about 0.9; one wrong digit of three costs about a
/// third) and leads the next by this much.
const VELOCITY_MATCH_MIN_SCORE: f32 = 0.8;
const VELOCITY_MATCH_MIN_LEAD: f32 = 0.05;

fn menu_shift(seat: MenuSeat) -> i32 {
    match seat {
        MenuSeat::Gunner => 0,
        MenuSeat::Loader => LOADER_SHIFT,
        MenuSeat::Dismounted => DISMOUNTED_SHIFT,
    }
}

/// The name and VEL value of every card of `menu` (a few milliseconds per card; read when the
/// cards change rather than every frame).
pub fn read_card_texts(view: &FrameView, client_origin: (i32, i32), menu: &Menu, reader: &TextReader) -> Vec<CardText> {
    let top = client_origin.1 + menu_shift(menu.seat);
    let text_box = |c: f64, (l, r, y0, y1): (f64, f64, i32, i32)| ((c + l) as i32, top + y0, (c + r) as i32, top + y1);
    menu.cards
        .iter()
        .map(|card| CardText {
            name: reader.read(view, TextStyle::Name, text_box(card.centre_x, NAME_BOX), TEXT_MIN_INK),
            velocity: reader.read_number(view, TextStyle::Value, text_box(card.centre_x, VALUE_BOX), TEXT_MIN_INK),
            matched: None,
        })
        .collect()
}

/// As `read_card_texts`, against the data: the VEL value, when the number read is not one the
/// weapons' shells fly at, compared with those (`velocities`, e.g. `mtc_ammo::AmmoData::
/// card_velocities`) and taken when one scores `VELOCITY_MATCH_MIN_SCORE` and leads the next by
/// `VELOCITY_MATCH_MIN_LEAD` ("259" read "239" on the Centurion AVRE's card); the name compared with
/// the whole strings a card can show at that velocity (`candidates`, e.g. `mtc_ammo::AmmoData::
/// card_texts`), the best when it scores `MATCH_MIN_SCORE` or more, else read glyph by glyph.
pub fn read_card_texts_among(
    view: &FrameView,
    client_origin: (i32, i32),
    menu: &Menu,
    reader: &TextReader,
    velocities: &dyn Fn() -> Vec<u32>,
    candidates: &dyn Fn(Option<u32>) -> Vec<String>,
) -> Vec<CardText> {
    let top = client_origin.1 + menu_shift(menu.seat);
    let text_box = |c: f64, (l, r, y0, y1): (f64, f64, i32, i32)| ((c + l) as i32, top + y0, (c + r) as i32, top + y1);
    let known = velocities();
    menu.cards
        .iter()
        .map(|card| {
            let value_box = text_box(card.centre_x, VALUE_BOX);
            let mut velocity = reader.read_number(view, TextStyle::Value, value_box, TEXT_MIN_INK);
            if !known.is_empty() && velocity.is_none_or(|v| !known.contains(&v)) {
                let words: Vec<String> = known.iter().map(u32::to_string).collect();
                let scored = reader.match_leading(view, TextStyle::Value, value_box, TEXT_MIN_INK, &words);
                if let Some(&(i, score)) = scored.first()
                    && score >= VELOCITY_MATCH_MIN_SCORE
                    && scored.get(1).is_none_or(|next| score - next.1 >= VELOCITY_MATCH_MIN_LEAD)
                {
                    velocity = Some(known[i]);
                }
            }
            let names = candidates(velocity);
            let scored = reader.match_line(view, TextStyle::Name, text_box(card.centre_x, NAME_BOX), TEXT_MIN_INK, &names);
            match scored.first() {
                Some(&(i, score)) if score >= MATCH_MIN_SCORE => {
                    CardText { name: Some(names[i].clone()), velocity, matched: Some((score, scored.get(1).map_or(f32::NAN, |s| s.1))) }
                }
                _ => CardText { name: reader.read(view, TextStyle::Name, text_box(card.centre_x, NAME_BOX), TEXT_MIN_INK), velocity, matched: None },
            }
        })
        .collect()
}

/// A label's calibre (the number it starts with, "155 mm M284") read with digit templates only
/// (`number`) where the free reading put a letter into it ("1E5"), the rest kept.
fn digit_calibre(text: String, number: impl FnOnce() -> Option<u32>) -> String {
    let int_len = text.find(|c: char| c == '.' || c == ' ' || c == 'm').unwrap_or(text.len());
    let int_part = &text[..int_len];
    if int_part.is_empty() || int_part.chars().all(|c| c.is_ascii_digit()) {
        return text;
    }
    match number() {
        Some(n) if n.to_string().len() == int_part.chars().count() => format!("{n}{}", &text[int_len..]),
        _ => text,
    }
}

/// The text rect inside the gunner's yellow-framed weapon box above the bar, when it shows.
pub fn weapon_box(view: &FrameView, client_origin: (i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let (ox, oy) = client_origin;
    let (sx0, sx1) = (WEAPON_BOX_SEARCH.0 + ox, WEAPON_BOX_SEARCH.1 + ox);
    // Amber, saturated: resampled from 100-125 % the 1 px line spreads over two rows at about 60 %
    // (red 150-180, red minus blue 120-150), at 150 % on 2560x1334 (226, 174, 70).
    let is_frame = |p: [u8; 3]| p[0] > 150 && i32::from(p[0]) - i32::from(p[2]) > 100;
    // The box's top frame line: the longest yellow run on the row (±1).
    let mut best: Option<(i32, i32, i32)> = None;
    for y in oy + WEAPON_BOX_TOP - WEAPON_BOX_REACH..=oy + WEAPON_BOX_TOP + WEAPON_BOX_REACH {
        if !view.covers(sx0, y, sx1 - sx0, 1) {
            continue;
        }
        let mut start = None;
        for x in sx0..=sx1 {
            match (x < sx1 && is_frame(view.px(x, y)), start) {
                (true, None) => start = Some(x),
                (false, Some(a)) => {
                    if x - a >= MIN_WEAPON_BOX_WIDTH && best.is_none_or(|(_, l, r)| x - a > r - l) {
                        best = Some((y, a, x));
                    }
                    start = None;
                }
                _ => {}
            }
        }
    }
    best.map(|(y, l, r)| (l + 3, y + 3, r - 2, y + WEAPON_BOX_HEIGHT - 2))
}

/// The gunner's weapon box compared whole with each of `candidates` (`TextReader::match_label`):
/// best first, (candidate index, score); empty when no box shows or no candidate fits its ink.
pub fn weapon_box_scores(view: &FrameView, client_origin: (i32, i32), reader: &TextReader, candidates: &[String]) -> Vec<(usize, f32)> {
    weapon_box(view, client_origin).map_or_else(Vec::new, |rect| reader.match_label(view, TextStyle::Weapon, rect, TEXT_MIN_INK, candidates))
}

/// The weapon label: the gunner's yellow-framed box above the bar, or the loader's strip.
pub fn read_weapon_label(view: &FrameView, client_origin: (i32, i32), reader: &TextReader) -> Option<String> {
    read_weapon_label_among(view, client_origin, reader, &[])
}

/// As `read_weapon_label`, the gunner's box first compared whole with the labels the data's weapons
/// show (`candidates`, `mtc_ammo::AmmoData::label_texts`): the best when it scores
/// `WEAPON_MATCH_MIN_SCORE` and leads the next by `WEAPON_MATCH_MIN_LEAD`, else read glyph by
/// glyph. Small text drawn soft (125 %, under 900 px high: the same pixels at five window sizes
/// read "155 mrnM284", "156 mnnFJ284", "l? mmM284" glyph by glyph as the resampling moved) has
/// only the data's names to choose from; a label the data lacks or one a game notice covers fits
/// none and is read glyph by glyph. The loader's strip likewise.
pub fn read_weapon_label_among(view: &FrameView, client_origin: (i32, i32), reader: &TextReader, candidates: &[String]) -> Option<String> {
    let (ox, oy) = client_origin;
    if let Some(rect) = weapon_box(view, client_origin) {
        if !candidates.is_empty() {
            let scored = reader.match_label(view, TextStyle::Weapon, rect, TEXT_MIN_INK, candidates);
            if let Some(&(i, score)) = scored.first()
                && score >= WEAPON_MATCH_MIN_SCORE
                && scored.get(1).is_none_or(|next| score - next.1 >= WEAPON_MATCH_MIN_LEAD)
            {
                return Some(candidates[i].clone());
            }
        }
        if let Some(text) = reader.read(view, TextStyle::Weapon, rect, TEXT_MIN_INK) {
            return Some(digit_calibre(text, || reader.read_number(view, TextStyle::Weapon, rect, TEXT_MIN_INK)));
        }
    }
    let (x0, y0, x1, y1) = LOADER_LABEL;
    [0, DISMOUNTED_SHIFT - LOADER_SHIFT].iter().find_map(|&dy| {
        let rect = (x0 + ox, y0 + oy + dy, x1 + ox, y1 + oy + dy);
        // The loader's strip as the gunner's box: whole first (at 100 %, 1280 x 720 glyph by glyph
        // read ")?.? mm I.I2?4", taken by the HUD for another weapon).
        if !candidates.is_empty() {
            let scored = reader.match_label(view, TextStyle::Weapon, rect, LOADER_LABEL_MIN_INK, candidates);
            if let Some(&(i, score)) = scored.first()
                && score >= WEAPON_MATCH_MIN_SCORE
                && scored.get(1).is_none_or(|next| score - next.1 >= WEAPON_MATCH_MIN_LEAD)
            {
                return Some(candidates[i].clone());
            }
        }
        let text = reader.read_centred(view, TextStyle::Weapon, rect, LOADER_LABEL_MIN_INK)?;
        text.contains("mm").then(|| digit_calibre(text, || reader.read_number_centred(view, TextStyle::Weapon, rect, LOADER_LABEL_MIN_INK)))
    })
}

/// How far the loading countdown has run, 0–1: the share of the bar's columns filled (its fill
/// grows from the left over a pure black bar; the seconds written on it sit inside the fill).
/// Read where `countdown_shown` holds.
pub fn countdown_progress(view: &FrameView, client_origin: (i32, i32)) -> Option<f64> {
    countdown_progress_in(view, client_origin, &COUNTDOWN_BOX)
}

/// As `countdown_progress`, for the bar at `bar`.
pub fn countdown_progress_in(view: &FrameView, client_origin: (i32, i32), bar: &CountdownBox) -> Option<f64> {
    let (x0, y0, x1, y1) = bar.interior;
    let (x0, x1, y0, y1) = (x0 + client_origin.0, x1 + client_origin.0, y0 + client_origin.1, y1 + client_origin.1);
    if !view.covers(x0, y0, x1 - x0, y1 - y0) {
        return None;
    }
    let filled = (x0..x1)
        .filter(|&x| {
            let lit = (y0..y1).filter(|&y| view.px(x, y).iter().copied().max().unwrap_or(0) > COUNTDOWN_BLACK_MAX).count();
            lit as f64 >= 0.5 * f64::from(y1 - y0)
        })
        .count();
    Some(filled as f64 / f64::from(x1 - x0))
}

/// Whether the loading countdown bar is shown.
pub fn countdown_shown(view: &FrameView, client_origin: (i32, i32)) -> bool {
    countdown_shown_in(view, client_origin, &COUNTDOWN_BOX)
}

/// The countdown bar where it shows on this view, searched for (`COUNTDOWN_SEARCH_Y`) and checked
/// as `countdown_shown_in` checks it; `None` when no bar shows.
pub fn find_countdown(view: &FrameView, client_origin: (i32, i32)) -> Option<CountdownBox> {
    let (ox, oy) = client_origin;
    let centre = (COUNTDOWN_EDGES[0].0 + COUNTDOWN_EDGES[1].0) / 2 + ox;
    let reach = COUNTDOWN_SEARCH_WIDTH.1 / 2 + COUNTDOWN_SEARCH_GAP;
    if !view.covers(centre - reach - 1, COUNTDOWN_SEARCH_Y.0 + oy, 2 * reach + 3, COUNTDOWN_SEARCH_Y.1 - COUNTDOWN_SEARCH_Y.0) {
        return None;
    }
    // The run of bar pixels through the middle of row `y` (frame px, both ends in): it crosses a
    // gap only where `COUNTDOWN_SEARCH_SOLID` bar pixels in a row follow (stray neutral pixels of
    // dark ground beside the bar do not extend it).
    let run = |y: i32| -> Option<(i32, i32)> {
        let start = [0, -4, 4, -8, 8].into_iter().map(|d| centre + d).find(|&x| bar_pixel(view.px(x, y)))?;
        let walk = |step: i32| {
            let (mut x, mut end, mut gap, mut solid) = (start, start, 0, 0);
            while (x + step - centre).abs() <= reach {
                x += step;
                let bar = bar_pixel(view.px(x, y));
                solid = if bar { solid + 1 } else { 0 };
                if bar && (gap == 0 || solid >= COUNTDOWN_SEARCH_SOLID) {
                    end = x;
                    gap = 0;
                } else {
                    gap += 1;
                    if gap > COUNTDOWN_SEARCH_GAP {
                        break;
                    }
                }
            }
            end
        };
        let (l, r) = (walk(-1), walk(1));
        ((COUNTDOWN_SEARCH_WIDTH.0..=COUNTDOWN_SEARCH_WIDTH.1).contains(&(r - l + 1)) && (l + r - 2 * centre).abs() <= 4).then_some((l, r))
    };
    let rows: Vec<(i32, i32, i32)> = (COUNTDOWN_SEARCH_Y.0 + oy..COUNTDOWN_SEARCH_Y.1 + oy).filter_map(|y| run(y).map(|(l, r)| (y, l, r))).collect();
    // The ends most rows agree on, and the rows that have them.
    let agree = |a: &(i32, i32, i32), b: &(i32, i32, i32)| (a.1 - b.1).abs() <= 2 && (a.2 - b.2).abs() <= 2;
    let mode = rows.iter().max_by_key(|a| rows.iter().filter(|b| agree(a, b)).count())?;
    let mut matching: Vec<&(i32, i32, i32)> = rows.iter().filter(|b| agree(mode, b)).collect();
    let (y0, y1) = (matching.first()?.0, matching.last()?.0);
    let (count, span) = (matching.len() as i32, y1 - y0 + 1);
    if count < COUNTDOWN_SEARCH_HEIGHT.0 || span > COUNTDOWN_SEARCH_HEIGHT.1 || (count as f64) < MIN_BAR_COLUMN_SHARE * f64::from(span) {
        return None;
    }
    matching.sort_by_key(|m| m.1);
    let l = matching[matching.len() / 2].1;
    matching.sort_by_key(|m| m.2);
    let r = matching[matching.len() / 2].2;
    let (l, r, y0, y1) = (l - ox, r - ox, y0 - oy, y1 - oy);
    let bar = CountdownBox { interior: (l + 1, y0 + 1, r, y1), edges: [(l, l - 5), (r, r + 6)] };
    countdown_shown_in(view, client_origin, &bar).then_some(bar)
}

/// As `countdown_shown`, for the bar at `bar`.
pub fn countdown_shown_in(view: &FrameView, client_origin: (i32, i32), bar: &CountdownBox) -> bool {
    let (x0, y0, x1, y1) = bar.interior;
    let (x0, x1, y0, y1) = (x0 + client_origin.0, x1 + client_origin.0, y0 + client_origin.1, y1 + client_origin.1);
    if !view.covers(x0, y0, x1 - x0, y1 - y0) {
        return false;
    }
    let bar_columns = (x0..x1)
        .filter(|&x| (y0..y1).filter(|&y| bar_pixel(view.px(x, y))).count() as f64 >= MIN_BAR_COLUMN_SHARE * f64::from(y1 - y0))
        .count();
    if (bar_columns as f64) < MIN_BAR_COLUMNS * f64::from(x1 - x0) {
        return false;
    }
    // Its ends stand out from what is around it (a uniform grey screen has no such ends). Inside,
    // the loader's green progress mark may sit on the end column as loading starts, so a column
    // further in counts too; outside, snow clipped to (237, 255, 255) passes for the cyan fill, so
    // a clear step in brightness counts as an end too.
    bar.edges.iter().all(|&(inside, outside)| {
        let deeper = inside + COUNTDOWN_EDGE_DEEPER * (inside - outside).signum();
        let (inside, deeper, outside) = (inside + client_origin.0, deeper + client_origin.0, outside + client_origin.0);
        let sharp = (y0..y1)
            .filter(|&y| {
                let (i, d, o) = (view.px(inside, y), view.px(deeper, y), view.px(outside, y));
                let bar = if bar_pixel(i) { Some(i) } else { bar_pixel(d).then_some(d) };
                bar.is_some_and(|b| !bar_pixel(o) || (mean(b) - mean(o)).abs() >= COUNTDOWN_EDGE_STEP)
            })
            .count();
        sharp as f64 >= MIN_EDGE_SHARE * f64::from(y1 - y0)
    })
}
