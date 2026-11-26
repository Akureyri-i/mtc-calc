//! The minimap's and the full map's containers, found on screen once per display configuration.
//! The rules (`UiLayout::map_container`) are the estimate; a view not yet found is looked for
//! (`mtc_vision::locate`, a few thousand pixels read back in thin strips) at most every
//! `TRY_INTERVAL` until `AGREE_COUNT` results agree, and that box replaces the rule's, its
//! difference from the rule logged. A view whose edges never show (too dark or busy a scene round
//! it) keeps the rule's box. The reference configuration keeps its own.

use std::time::{Duration, Instant};

use mtc_vision::FrameView;
use mtc_vision::layout::{Located, UiLayout};
use mtc_vision::locate;
use mtc_vision::map::geometry::MapView;

const TRY_INTERVAL: Duration = Duration::from_millis(250);
/// Results within `AGREE_PX` of their median on left, top and side needed to take one: in testing
/// a lone wrong box (14 px off) came up once among 661 found.
const AGREE_COUNT: usize = 3;
const AGREE_PX: f64 = 1.0;
/// Results kept per view while waiting for agreement.
const KEEP: usize = 9;

const VIEWS: [(MapView, &str); 2] = [(MapView::Mini, "minimap"), (MapView::Full, "full map")];

type Container = (f64, f64, f64);

pub struct Locator {
    layout: UiLayout,
    located: Located,
    seen: [Vec<Container>; 2],
    last_try: Option<Instant>,
    /// What was found at configurations seen before (a window going back to a size).
    found: Vec<(UiLayout, Located)>,
}

impl Locator {
    pub fn new() -> Self {
        Self { layout: UiLayout::REFERENCE, located: Located::NONE, seen: [Vec::new(), Vec::new()], last_try: None, found: Vec::new() }
    }

    fn switch_to(&mut self, layout: UiLayout) {
        if layout == self.layout {
            return;
        }
        if !self.layout.is_reference() {
            let (layout, located) = (self.layout, self.located);
            self.found.retain(|(l, _)| *l != layout);
            self.found.push((layout, located));
        }
        self.layout = layout;
        self.located = self.found.iter().find(|(l, _)| *l == layout).map_or(Located::NONE, |e| e.1);
        self.seen = [Vec::new(), Vec::new()];
        self.last_try = None;
    }

    /// The containers found at `layout` so far.
    pub fn located(&mut self, layout: UiLayout) -> Located {
        self.switch_to(layout);
        self.located
    }

    /// The client rectangles to read back this frame for the views not found yet (none at the
    /// reference, once all are found, or within `TRY_INTERVAL` of the last try).
    pub fn wanted(&mut self, layout: UiLayout) -> Vec<(i32, i32, i32, i32)> {
        self.switch_to(layout);
        if layout.is_reference() || self.last_try.is_some_and(|t| t.elapsed() < TRY_INTERVAL) {
            return Vec::new();
        }
        let missing: Vec<MapView> = VIEWS.iter().filter(|(v, _)| self.located.get(*v).is_none()).map(|(v, _)| *v).collect();
        if missing.is_empty() {
            return Vec::new();
        }
        self.last_try = Some(Instant::now());
        missing.into_iter().flat_map(|v| locate::probes(&layout, v)).collect()
    }

    /// Looks for the views not found yet on `views` (the read-back strips, or a whole frame) at
    /// `origin`; true when a container was taken.
    pub fn take(&mut self, views: &[FrameView], origin: (i32, i32)) -> bool {
        let mut changed = false;
        for (i, (view, name)) in VIEWS.iter().enumerate() {
            if self.located.get(*view).is_some() {
                continue;
            }
            let Some(found) = locate::container(views, origin, &self.layout, *view) else { continue };
            let seen = &mut self.seen[i];
            seen.push(found);
            if seen.len() > KEEP {
                seen.remove(0);
            }
            let Some(agreed) = agreed(seen) else { continue };
            let rule = self.layout.map_container(*view);
            println!(
                "Located: {name} on screen at ({:.1}, {:.1}), side {:.1}; the rule gives ({:.1}, {:.1}), {:.1}; off by ({:+.1}, {:+.1}), {:+.1} px",
                agreed.0, agreed.1, agreed.2, rule.0, rule.1, rule.2, agreed.0 - rule.0, agreed.1 - rule.1, agreed.2 - rule.2
            );
            let round = |x: f64| (x * 10.0).round() / 10.0;
            crate::diag::note(crate::diag::Event::Container(*view == MapView::Full, [round(agreed.0 - rule.0), round(agreed.1 - rule.1), round(agreed.2 - rule.2)]));
            match view {
                MapView::Mini => self.located.mini = Some(agreed),
                _ => self.located.full = Some(agreed),
            }
            changed = true;
        }
        changed
    }
}

/// The mean of the results within `AGREE_PX` of the median on every coordinate, when there are
/// `AGREE_COUNT` of them.
fn agreed(seen: &[Container]) -> Option<Container> {
    if seen.len() < AGREE_COUNT {
        return None;
    }
    let median = |f: fn(&Container) -> f64| {
        let mut v: Vec<f64> = seen.iter().map(f).collect();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let m = (median(|c| c.0), median(|c| c.1), median(|c| c.2));
    let near: Vec<&Container> = seen.iter().filter(|c| (c.0 - m.0).abs() <= AGREE_PX && (c.1 - m.1).abs() <= AGREE_PX && (c.2 - m.2).abs() <= AGREE_PX).collect();
    if near.len() < AGREE_COUNT {
        return None;
    }
    let n = near.len() as f64;
    Some((near.iter().map(|c| c.0).sum::<f64>() / n, near.iter().map(|c| c.1).sum::<f64>() / n, near.iter().map(|c| c.2).sum::<f64>() / n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_wrong_box_is_not_taken() {
        assert_eq!(agreed(&[(10.0, 20.0, 300.0), (10.2, 20.1, 300.0)]), None);
        assert_eq!(agreed(&[(10.0, 20.0, 300.0), (24.0, 20.0, 300.0), (10.4, 20.0, 299.6)]), None);
        let a = agreed(&[(10.0, 20.0, 300.0), (24.0, 20.0, 300.0), (10.4, 20.0, 299.6), (10.2, 20.2, 300.2)]).expect("three agree");
        assert!((a.0 - 10.2).abs() < 1e-9 && (a.2 - 299.933).abs() < 1e-3);
    }
}
