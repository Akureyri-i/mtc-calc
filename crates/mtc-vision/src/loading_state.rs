//! Loading state over time: the charge chosen for each slot, the slot being loaded and the
//! loaded configuration.
//!
//! - Each slot keeps the last charge seen on its card. Under the hover detail box the charge bag
//!   capsule (one segment per charge step on every shell with charges) stands in; with neither
//!   visible the slot keeps what it had.
//! - A countdown starts loading. The slot comes from the highlighted slot (the gunner's bar header
//!   or the loader's framed box) and its charge from the slot's memory at that moment.
//! - When the countdown has been gone for `END_FRAMES` frames and the slot still shows
//!   highlighted, it is loaded; the previous loaded configuration is replaced only then.
//! - Seat changes only move the UI, so nothing is reset for them; a different card count
//!   (another weapon) clears the per-slot memory.

use crate::loading::{CardCharge, Charge, LoaderBar, Menu};

/// Frames without the countdown before loading is taken as finished (the bar drops out for a
/// frame while the menu fades over it).
const END_FRAMES: u32 = 2;
/// Countdown readings must span this long (ms) before its end is predicted.
const MIN_PREDICTION_SPAN_MS: u64 = 300;

/// What one frame shows of the loading UI.
#[derive(Clone, Copy, Debug)]
pub struct LoadingFrame<'a> {
    pub t_ms: u64,
    pub menu: Option<&'a Menu>,
    pub countdown: bool,
    /// How far the countdown has run (0–1), when it shows.
    pub progress: Option<f64>,
    /// Highlighted shell slot in the gunner's bar (0-based), when the bar shows one.
    pub gunner_slot: Option<usize>,
    pub loader_bar: Option<&'a LoaderBar>,
}

impl LoadingFrame<'_> {
    fn highlighted(&self) -> Option<usize> {
        self.gunner_slot.or(self.loader_bar.and_then(|b| b.loaded))
    }
}

/// A slot's charge as last seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SlotCharge {
    #[default]
    Unknown,
    /// The shell is loaded without charges.
    None,
    Selected(Charge),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loaded {
    /// 0-based slot.
    pub slot: usize,
    pub charge: SlotCharge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadingEvent {
    /// A countdown started; the slot once it shows.
    Started { slot: Option<usize> },
    Loaded(Loaded),
}

#[derive(Default)]
pub struct LoadingTracker {
    /// Per slot, left to right.
    charges: Vec<SlotCharge>,
    /// Loading in progress: the slot and its charge once known.
    loading: Option<Option<Loaded>>,
    /// Frames since the countdown was last seen while loading.
    countdown_gone: u32,
    loaded: Option<Loaded>,
    /// The current loading's countdown readings (ms, progress).
    progress: Vec<(u64, f64)>,
}

fn from_bags(bags: u32) -> Option<Charge> {
    match bags {
        1 => Some(Charge::Low),
        2 => Some(Charge::Medium),
        3 => Some(Charge::High),
        _ => None,
    }
}

impl LoadingTracker {
    /// Charge chosen for each slot (the menu's selection).
    pub fn charges(&self) -> &[SlotCharge] {
        &self.charges
    }

    /// The slot being loaded and its charge, once the slot is known.
    pub fn loading(&self) -> Option<Option<Loaded>> {
        self.loading
    }

    /// The loaded configuration.
    pub fn loaded(&self) -> Option<Loaded> {
        self.loaded
    }

    /// When the current loading's countdown runs out (ms), by a straight line through its
    /// readings: for a countdown hidden before its end (the full map opened over it). Recorded
    /// countdowns that ran out in view: within ±0.9 s of their last frame (`countdown_eval`).
    pub fn predicted_end_ms(&self) -> Option<u64> {
        self.loading?;
        let pts = &self.progress;
        let (first, last) = (pts.first()?, pts.last()?);
        if pts.len() < 2 || last.0.saturating_sub(first.0) < MIN_PREDICTION_SPAN_MS {
            return None;
        }
        let n = pts.len() as f64;
        let mt = pts.iter().map(|p| p.0 as f64).sum::<f64>() / n;
        let mp = pts.iter().map(|p| p.1).sum::<f64>() / n;
        let slope = pts.iter().map(|p| (p.0 as f64 - mt) * (p.1 - mp)).sum::<f64>() / pts.iter().map(|p| (p.0 as f64 - mt).powi(2)).sum::<f64>();
        (slope > 0.0).then(|| (mt + (1.0 - mp) / slope).max(0.0) as u64)
    }

    pub fn observe(&mut self, frame: &LoadingFrame) -> Vec<LoadingEvent> {
        let mut events = Vec::new();
        if let Some(menu) = frame.menu {
            if menu.cards.len() != self.charges.len() {
                self.charges = vec![SlotCharge::Unknown; menu.cards.len()];
            }
            for (slot, card) in self.charges.iter_mut().zip(&menu.cards) {
                *slot = match card.charge {
                    CardCharge::Selected(c) => SlotCharge::Selected(c),
                    CardCharge::None => SlotCharge::None,
                    CardCharge::Hidden => match (*slot, from_bags(card.bags)) {
                        (SlotCharge::None, _) => SlotCharge::None,
                        (_, Some(c)) => SlotCharge::Selected(c),
                        (kept, None) => kept,
                    },
                };
            }
        }
        if frame.countdown {
            self.countdown_gone = 0;
            if self.loading.is_none() {
                self.loading = Some(None);
                self.progress.clear();
                events.push(LoadingEvent::Started { slot: frame.highlighted() });
            }
            // The run-out "0.0s" bar after loading reads empty: only readings that move forward.
            if let Some(p) = frame.progress
                && self.progress.last().is_none_or(|last| p >= last.1)
            {
                self.progress.push((frame.t_ms, p));
            }
        } else if self.loading.is_some() {
            self.countdown_gone += 1;
        }
        // The slot shows highlighted once loading starts (the menu has faded).
        if let Some(None) = self.loading
            && let Some(slot) = frame.highlighted()
        {
            let charge = self.charges.get(slot).copied().unwrap_or_default();
            self.loading = Some(Some(Loaded { slot, charge }));
        }
        if self.countdown_gone >= END_FRAMES
            && let Some(Some(loading)) = self.loading
            && frame.highlighted() == Some(loading.slot)
        {
            self.loading = None;
            self.countdown_gone = 0;
            self.progress.clear();
            self.loaded = Some(loading);
            events.push(LoadingEvent::Loaded(loading));
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_hidden_countdown_ends_where_its_progress_points() {
        let mut t = LoadingTracker::default();
        let reading = |t_ms, p: f64| LoadingFrame { t_ms, menu: None, countdown: true, progress: Some(p), gunner_slot: None, loader_bar: None };
        for (ms, p) in [(0, 0.0), (1000, 0.1), (2000, 0.2)] {
            t.observe(&reading(ms, p));
        }
        let end = t.predicted_end_ms().expect("predicted");
        assert!(end.abs_diff(10_000) <= 1, "{end}");
        // The empty run-out bar does not pull the line back.
        t.observe(&reading(2100, 0.0));
        assert!(t.predicted_end_ms().expect("predicted").abs_diff(10_000) <= 1);
    }

    use crate::loading::{Card, MenuSeat};

    fn menu(seat: MenuSeat, cards: &[(CardCharge, u32)]) -> Menu {
        Menu { seat, cards: cards.iter().map(|&(charge, bags)| Card { centre_x: 0.0, charge, bags }).collect() }
    }

    fn frame<'a>(t_ms: u64, menu: Option<&'a Menu>, countdown: bool, gunner_slot: Option<usize>, loader_bar: Option<&'a LoaderBar>) -> LoadingFrame<'a> {
        LoadingFrame { t_ms, menu, countdown, progress: None, gunner_slot, loader_bar }
    }

    /// Gunner picks Low on slot 2 and loads it; the loaded configuration appears only after the
    /// countdown, and a new selection does not change it.
    #[test]
    fn gunner_loads_the_selected_charge() {
        let mut t = LoadingTracker::default();
        let high = (CardCharge::Selected(Charge::High), 3);
        let low = (CardCharge::Selected(Charge::Low), 1);
        let m = menu(MenuSeat::Gunner, &[high, low, high, high]);
        assert!(t.observe(&frame(0, Some(&m), false, None, None)).is_empty());
        assert_eq!(t.observe(&frame(100, None, true, None, None)), vec![LoadingEvent::Started { slot: None }]);
        assert!(t.observe(&frame(200, None, true, Some(1), None)).is_empty());
        assert_eq!(t.loaded(), None);
        assert!(t.observe(&frame(300, None, false, Some(1), None)).is_empty());
        let loaded = Loaded { slot: 1, charge: SlotCharge::Selected(Charge::Low) };
        assert_eq!(t.observe(&frame(400, None, false, Some(1), None)), vec![LoadingEvent::Loaded(loaded)]);
        // A new selection is pending only.
        let m2 = menu(MenuSeat::Gunner, &[high, high, high, high]);
        t.observe(&frame(500, Some(&m2), false, None, None));
        assert_eq!(t.loaded(), Some(loaded));
        assert_eq!(t.charges()[1], SlotCharge::Selected(Charge::High));
    }

    /// The loader picks High on slot 2 under the detail box (capsule of three); after moving to the
    /// gunner seat the loaded configuration stays.
    #[test]
    fn hidden_charge_comes_from_the_capsule_and_survives_a_seat_change() {
        let mut t = LoadingTracker::default();
        let low = (CardCharge::Selected(Charge::Low), 1);
        let m = menu(MenuSeat::Loader, &[low, low, low, low]);
        t.observe(&frame(0, Some(&m), false, None, None));
        let hidden = menu(MenuSeat::Loader, &[(CardCharge::Hidden, 1), (CardCharge::Hidden, 3), (CardCharge::Hidden, 1), low]);
        t.observe(&frame(100, Some(&hidden), false, None, None));
        assert_eq!(t.charges()[1], SlotCharge::Selected(Charge::High));
        let bar = LoaderBar { slots: 4, loaded: Some(1) };
        t.observe(&frame(200, None, true, None, Some(&bar)));
        t.observe(&frame(300, None, false, None, Some(&bar)));
        t.observe(&frame(400, None, false, None, Some(&bar)));
        let loaded = Loaded { slot: 1, charge: SlotCharge::Selected(Charge::High) };
        assert_eq!(t.loaded(), Some(loaded));
        // Gunner seat: the bar shows slot 2; nothing changes.
        assert!(t.observe(&frame(500, None, false, Some(1), None)).is_empty());
        assert_eq!(t.loaded(), Some(loaded));
    }

    /// A shell without charges stays without them under the detail box (rockets show a
    /// one-segment capsule).
    #[test]
    fn no_charge_shell_ignores_its_capsule() {
        let mut t = LoadingTracker::default();
        t.observe(&frame(0, Some(&menu(MenuSeat::Gunner, &[(CardCharge::None, 1), (CardCharge::None, 1)])), false, None, None));
        t.observe(&frame(100, Some(&menu(MenuSeat::Gunner, &[(CardCharge::Hidden, 1), (CardCharge::Hidden, 1)])), false, None, None));
        assert_eq!(t.charges(), [SlotCharge::None, SlotCharge::None]);
    }
}
