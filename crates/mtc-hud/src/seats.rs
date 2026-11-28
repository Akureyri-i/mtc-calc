//! The seat menu in the HUD. Seats change by clicking the menu, there is no key: after a left
//! click on it the menu is read every `CLICK_READ_EVERY_MS` until it shows the new seat (a switch
//! passes through every seat empty first), for at most `CLICK_WINDOW_MS`; otherwise every
//! `READ_EVERY_MS` (other players take and leave seats with no input of ours) and when the seat
//! the frame shows changes. The player's own row is found by the name learnt in the seat the
//! frame shows (gunner: the angle readout is read; loader: the loader's ammo bar is there), as the
//! panel goes by. The name stays in memory only.

use mtc_vision::FrameView;
use mtc_vision::seats::{SeatKind, SeatMenu, SeatReader};

use crate::PanelSeat;

/// How often the menu is read when nothing changes (a read takes about 7 ms).
const READ_EVERY_MS: u64 = 1000;
/// After a click on the menu: reads this often, until the menu changes or the window ends.
const CLICK_READ_EVERY_MS: u64 = 100;
const CLICK_WINDOW_MS: u64 = 3000;

#[derive(Default)]
pub struct SeatWatch {
    reader: SeatReader,
    last_read: Option<u64>,
    /// The panel's seat at the last read (a name is learnt only when it held over two reads).
    last_panel: Option<PanelSeat>,
    /// The menu as last read.
    menu: Option<SeatMenu>,
    /// When the menu was last clicked and what it showed then, until the new seat shows or the
    /// window ends.
    clicked_at: Option<u64>,
    click_line: Option<String>,
    last_line: Option<String>,
    /// The player's own seat as last read, how many times it changed and how many times it
    /// became the driver's (for the shot records).
    own: Option<SeatKind>,
    changes: u32,
    driver_entries: u32,
}

impl SeatWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many times the player's own seat changed, and became the driver's, so far.
    pub fn seat_changes(&self) -> (u32, u32) {
        (self.changes, self.driver_entries)
    }

    /// A left click at `pos` (frame px): on the menu as last read, it may change the seat.
    pub fn clicked(&mut self, pos: (f64, f64), now_ms: u64) {
        if self.menu.as_ref().is_some_and(|m| on_menu(m, pos)) {
            self.clicked_at = Some(now_ms);
            self.click_line = self.last_line.clone();
        }
    }

    /// The frame region to read now, if a read is due.
    pub fn wanted(&self, client: (i32, i32, u32, u32), panel: Option<PanelSeat>, now_ms: u64) -> Option<(i32, i32, u32, u32)> {
        let since = |t: Option<u64>| t.map(|t| now_ms.saturating_sub(t));
        let after_click = since(self.clicked_at).is_some_and(|c| c <= CLICK_WINDOW_MS) && since(self.last_read).is_none_or(|r| r >= CLICK_READ_EVERY_MS);
        let due = since(self.last_read).is_none_or(|r| r >= READ_EVERY_MS) || panel != self.last_panel || after_click;
        due.then(|| SeatReader::region(client))
    }

    /// Reads the menu from a view covering `wanted`'s region.
    pub fn process(&mut self, view: &FrameView, client: (i32, i32, u32, u32), panel: Option<PanelSeat>, now_ms: u64) {
        self.last_read = Some(now_ms);
        let mut menu = self.reader.read(view, client);
        if let Some(m) = menu.as_mut()
            && let Some(seat) = panel.filter(|&p| Some(p) == self.last_panel)
        {
            let kind = match seat {
                PanelSeat::Gunner => SeatKind::Gunner,
                PanelSeat::Loader => SeatKind::Loader,
            };
            if m.mine() != Some(kind) && self.reader.learn(view, m, kind) {
                self.reader.mark_own(view, m);
            }
        }
        self.last_panel = panel;
        if let Some(m) = &menu {
            crate::diag::note(crate::diag::Event::SeatRead(m.own_known && m.mine().is_some()));
            if let Some(kind) = m.mine().filter(|_| m.own_known)
                && self.own != Some(kind)
            {
                self.changes += u32::from(self.own.is_some());
                self.driver_entries += u32::from(kind == SeatKind::Driver);
                self.own = Some(kind);
            }
            let line = describe(m);
            if self.last_line.as_ref() != Some(&line) {
                log::info!("{line}");
                self.last_line = Some(line);
            }
        }
        if let Some(at) = self.clicked_at {
            let ms = now_ms.saturating_sub(at);
            // The new seat shows: another menu than at the click, with the player in it (someone,
            // while the name is not known yet), not the all-empty moment of the switch.
            let settled = menu.as_ref().is_some_and(|m| {
                self.click_line.as_deref() != Some(describe(m).as_str()) && if m.own_known { m.mine().is_some() } else { m.rows.iter().any(|r| r.taken) }
            });
            if settled {
                log::info!("Seats: new seat read {ms} ms after the menu was clicked");
                self.clicked_at = None;
            } else if ms > CLICK_WINDOW_MS {
                log::info!("Seats: menu clicked, no new seat read within {CLICK_WINDOW_MS} ms");
                self.clicked_at = None;
            }
        }
        self.menu = menu;
    }
}

/// Whether `pos` (frame px) is on the menu's rows: icons and names.
fn on_menu(menu: &SeatMenu, pos: (f64, f64)) -> bool {
    let s = f64::from(menu.scale());
    let (Some(first), Some(last)) = (menu.rows.first(), menu.rows.last()) else { return false };
    let left = menu.rows.iter().map(|r| f64::from(r.center.0)).fold(f64::INFINITY, f64::min);
    let (x, y) = pos;
    x >= left - 30.0 * s && x <= left + 260.0 * s && y >= f64::from(first.center.1) - 32.0 * s && y <= f64::from(last.center.1) + 32.0 * s
}

fn kind_name(kind: SeatKind) -> &'static str {
    match kind {
        SeatKind::Driver => "driver",
        SeatKind::Gunner => "gunner",
        SeatKind::Commander => "commander",
        SeatKind::Loader => "loader",
        SeatKind::Passenger => "passenger",
    }
}

/// "Seats: you are the gunner · driver empty · gunner you · commander empty · loader taken" (no
/// names).
fn describe(menu: &SeatMenu) -> String {
    let own = match (menu.own_known, menu.mine()) {
        (false, _) => "own row not known yet (learnt after sitting as gunner or loader)".to_owned(),
        (true, Some(k)) => format!("you are the {}", kind_name(k)),
        (true, None) => "you are not in this vehicle".to_owned(),
    };
    let rows: Vec<String> = menu.rows.iter().map(|r| format!("{} {}", kind_name(r.kind), if r.mine { "you" } else if r.taken { "taken" } else { "empty" })).collect();
    format!("Seats: {own} · {}", rows.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtc_vision::seats::{MenuLayout, SeatRow};

    fn row(kind: SeatKind, y: f32) -> SeatRow {
        SeatRow { kind, center: (141.0, y), taken: false, mine: false, score: 0.9 }
    }

    fn menu() -> SeatMenu {
        SeatMenu { layout: MenuLayout::Usual, rows: vec![row(SeatKind::Driver, 1136.0), row(SeatKind::Gunner, 1197.0), row(SeatKind::Commander, 1257.0), row(SeatKind::Loader, 1317.0)], own_known: true }
    }

    #[test]
    fn clicks_on_the_menu_count() {
        let m = menu();
        // A row's icon, a name, the gaps between rows.
        assert!(on_menu(&m, (141.0, 1317.0)));
        assert!(on_menu(&m, (300.0, 1198.0)));
        assert!(on_menu(&m, (141.0, 1166.0)));
        // Beside it (the TRT/BRC/GUN column), above it, far right.
        assert!(!on_menu(&m, (58.0, 1250.0)));
        assert!(!on_menu(&m, (141.0, 1090.0)));
        assert!(!on_menu(&m, (900.0, 1250.0)));
    }

    #[test]
    fn a_click_on_the_menu_reads_it_often_for_a_while() {
        let client = (0, 34, 2560, 1334);
        let mut w = SeatWatch { menu: Some(menu()), last_read: Some(10_000), ..SeatWatch::default() };
        assert!(w.wanted(client, None, 10_100).is_none());
        w.clicked((900.0, 1250.0), 10_100);
        assert!(w.wanted(client, None, 10_150).is_none(), "a click elsewhere");
        w.clicked((141.0, 1317.0), 10_100);
        assert!(w.wanted(client, None, 10_150).is_some(), "100 ms after the last read");
        w.last_read = Some(10_150);
        assert!(w.wanted(client, None, 10_200).is_none());
        assert!(w.wanted(client, None, 10_250).is_some());
        w.last_read = Some(13_050);
        assert!(w.wanted(client, None, 13_200).is_none(), "the window ended");
    }
}
