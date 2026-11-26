//! V1 as a canvas driven by input: the zoom and origin of the big map follow the wheel and drags
//! by the game's own rules, and recognition corrects the state when it
//! disagrees (screen recognition stays the authority).
//!
//! - A wheel notch changes the zoom by 0.2 within 1–5 at once (no animation), keeping the map
//!   point at the cursor in place (`ZOOM_FIXED_POINT` from the cursor's pixel centre).
//! - The left button down on V1 away from markers drags the map 1:1 with the cursor once the cursor
//!   is `DRAG_THRESHOLD_PX` from where the button went down; the map then trails the cursor by
//!   that much for the rest of the drag. The origin stays within 1 − 1/zoom of the canvas (each
//!   move applied on the clamped origin).
//! - States are kept with their times, so a measurement from a frame can be compared with the
//!   canvas as it was when the game drew that frame.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::geometry::MapTransform;

/// Zoom step per wheel notch and the zoom range.
pub const ZOOM_STEP: f64 = 0.2;
pub const ZOOM_MIN: f64 = 1.0;
pub const ZOOM_MAX: f64 = 5.0;
/// Where the game's zoom keeps the map still, from the cursor's pixel centre (frame px): 40 single
/// notches put it at a median (−0.4, −0.8). Using the cursor itself drew the marks up and to the
/// left of the game's by (1.3–2.0, 1.5–3.6) px after zooming 1× → 5× (measured from the diamonds'
/// registration after each burst), four times the offset as the zoom grows fivefold.
pub const ZOOM_FIXED_POINT: (f64, f64) = (-0.4, -0.8);
/// How far the cursor goes from where the left button went down before the game drags the map
/// (frame px): over 34 drags on V1 (registration before and after each) the map moved less than
/// the cursor by a median 6.7 px (quartiles 5.4–7.4) along the drag's first direction (1 px across
/// it), whatever the drag's length (a 7 px drag left the map where it was). Measured at 150 %
/// display scaling; whether the game counts it in screen or interface pixels is not known.
pub const DRAG_THRESHOLD_PX: f64 = 6.7;
/// States older than this are dropped from the history.
const HISTORY: Duration = Duration::from_millis(500);
/// Between two states at most this far apart a pan is interpolated (frames come every ~20 ms
/// while V1 moves; further apart, the cursor stood still in between).
const INTERPOLATE_GAP: Duration = Duration::from_millis(50);

/// The canvas origin stays within the map: the view covers 1/zoom of it.
pub fn clamp_origin(origin: (f64, f64), zoom: f64) -> (f64, f64) {
    let max = (1.0 - 1.0 / zoom).max(0.0);
    (origin.0.clamp(0.0, max), origin.1.clamp(0.0, max))
}

/// A pan in progress: where and when the button went down (frame px), and the cursor at the last
/// applied move once the map moves (past `DRAG_THRESHOLD_PX`).
#[derive(Clone, Copy, Debug)]
struct Pan {
    pressed: (f64, f64),
    pressed_at: Instant,
    last: Option<(f64, f64)>,
}

/// Where the map starts to follow a drag pressed at `pressed` whose cursor is now at `cursor`
/// (frame px): `DRAG_THRESHOLD_PX` from the press towards the cursor; `None` while the cursor is
/// within it (the map stands still).
pub fn drag_start(pressed: (f64, f64), cursor: (f64, f64)) -> Option<(f64, f64)> {
    let (dx, dy) = (cursor.0 - pressed.0, cursor.1 - pressed.1);
    let d = dx.hypot(dy);
    (d > DRAG_THRESHOLD_PX).then(|| (pressed.0 + dx / d * DRAG_THRESHOLD_PX, pressed.1 + dy / d * DRAG_THRESHOLD_PX))
}

#[derive(Clone, Debug, Default)]
pub struct V1Canvas {
    /// Current state (none until recognition first placed V1).
    now: Option<MapTransform>,
    /// A pan in progress.
    pan: Option<Pan>,
    /// Recent states with the time each began.
    history: VecDeque<(Instant, MapTransform)>,
}

impl V1Canvas {
    pub fn transform(&self) -> Option<MapTransform> {
        self.now
    }

    /// The state the canvas had at `at`: during a pan, between the two cursor samples around it
    /// (a pan moves the map continuously; taking the last sample alone put the map one or two
    /// frames behind by turns).
    pub fn at(&self, at: Instant) -> Option<MapTransform> {
        let Some(i) = self.history.iter().rposition(|(t, _)| *t <= at) else { return self.history.front().map(|(_, s)| *s) };
        let (t0, s0) = self.history[i];
        match self.history.get(i + 1) {
            Some(&(t1, s1)) if t1 > t0 && t1 - t0 <= INTERPOLATE_GAP && s1.zoom == s0.zoom && (s1.left, s1.top, s1.size) == (s0.left, s0.top, s0.size) => {
                let f = (at - t0).as_secs_f64() / (t1 - t0).as_secs_f64();
                let origin = (s0.origin.0 + (s1.origin.0 - s0.origin.0) * f, s0.origin.1 + (s1.origin.1 - s0.origin.1) * f);
                Some(MapTransform { origin, ..s0 })
            }
            _ => Some(s0),
        }
    }

    /// When the newest state began (for a pan: when its cursor was read).
    pub fn latest_at(&self) -> Option<Instant> {
        self.history.back().map(|(t, _)| *t)
    }

    pub fn panning(&self) -> bool {
        self.pan.is_some()
    }

    /// Where and when the button of the pan in progress went down (frame px).
    pub fn pressed(&self) -> Option<((f64, f64), Instant)> {
        self.pan.map(|p| (p.pressed, p.pressed_at))
    }

    fn set(&mut self, state: MapTransform, at: Instant) {
        self.now = Some(state);
        self.history.push_back((at, state));
        while self.history.front().is_some_and(|(t, _)| at.saturating_duration_since(*t) > HISTORY) && self.history.len() > 1 {
            self.history.pop_front();
        }
    }

    /// Recognition says where V1 is (a registration at rest): the canvas takes it.
    pub fn correct(&mut self, measured: MapTransform, at: Instant) {
        self.set(measured, at);
    }

    /// Recognition says where V1 was at `then` (a frame read while it moves): at the same zoom,
    /// the difference moves every state since `then`, so input applied after it is kept. A
    /// different zoom (the frame caught a notch the game had not shown yet) changes nothing;
    /// returns whether it applied.
    pub fn rebase(&mut self, measured: MapTransform, then: Instant, now: Instant) -> bool {
        let Some(before) = self.at(then).filter(|s| (s.zoom - measured.zoom).abs() < 0.01) else { return false };
        let shift = (measured.origin.0 - before.origin.0, measured.origin.1 - before.origin.1);
        let moved = |s: MapTransform| MapTransform { origin: clamp_origin((s.origin.0 + shift.0, s.origin.1 + shift.1), s.zoom), ..s };
        for (t, s) in self.history.iter_mut() {
            if *t >= then {
                *s = moved(*s);
            }
        }
        if let Some(current) = self.now {
            self.set(moved(current), now);
        }
        true
    }

    /// Wheel notches (positive when rolled forward) with the cursor at `cursor` (frame px).
    pub fn wheel(&mut self, notches: f32, cursor: (f64, f64), at: Instant) {
        let Some(t) = self.now.filter(|t| t.contains_frame(cursor.0, cursor.1)) else { return };
        let zoom = (((t.zoom + ZOOM_STEP * f64::from(notches)) / ZOOM_STEP).round() * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
        if (zoom - t.zoom).abs() < 1e-9 {
            return;
        }
        let fixed = (cursor.0 + ZOOM_FIXED_POINT.0, cursor.1 + ZOOM_FIXED_POINT.1);
        let point = t.to_map(fixed.0, fixed.1);
        let s = t.size * zoom;
        let origin = clamp_origin((point.0 - (fixed.0 - t.left) / s, point.1 - (fixed.1 - t.top) / s), zoom);
        self.set(MapTransform { zoom, origin, ..t }, at);
    }

    /// The left button went down at `cursor` at `at`; `on_marker`: on a marker (the game drags the
    /// marker).
    pub fn press(&mut self, cursor: (f64, f64), on_marker: bool, at: Instant) {
        if !on_marker && self.now.is_some_and(|t| t.contains_frame(cursor.0, cursor.1)) {
            self.pan = Some(Pan { pressed: cursor, pressed_at: at, last: None });
        }
    }

    /// The cursor during a pan: the map follows it 1:1 once past the threshold. Every sample is
    /// kept, moved or not, so `at` interpolates between neighbouring frames only.
    pub fn moved(&mut self, cursor: (f64, f64), at: Instant) {
        let (Some(mut pan), Some(t)) = (self.pan, self.now) else { return };
        let last = match pan.last.or_else(|| drag_start(pan.pressed, cursor)) {
            Some(last) => last,
            None => {
                // Within the threshold: the map stands still.
                self.set(t, at);
                return;
            }
        };
        pan.last = Some(cursor);
        self.pan = Some(pan);
        let s = t.scale();
        let origin = clamp_origin((t.origin.0 - (cursor.0 - last.0) / s, t.origin.1 - (cursor.1 - last.1) / s), t.zoom);
        self.set(MapTransform { origin, ..t }, at);
    }

    pub fn release(&mut self) {
        self.pan = None;
    }
}

/// How far apart two V1 transforms draw the visible map (frame px): the largest distance between
/// where each puts the corners and the middle of the view `a` shows.
pub fn difference_px(a: &MapTransform, b: &MapTransform) -> f64 {
    let (x0, y0, x1, y1) = (a.left, a.top, a.left + a.size, a.top + a.size);
    [(x0, y0), (x1, y0), (x0, y1), (x1, y1), ((x0 + x1) / 2.0, (y0 + y1) / 2.0)]
        .into_iter()
        .map(|(x, y)| {
            let m = a.to_map(x, y);
            let (bx, by) = b.to_frame(m.0, m.1);
            (bx - x).hypot(by - y)
        })
        .fold(0.0, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MapView;

    fn start() -> (V1Canvas, Instant) {
        let mut c = V1Canvas::default();
        let t0 = Instant::now();
        c.correct(MapTransform::fixed(MapView::Full, (0, 34)), t0);
        (c, t0)
    }

    #[test]
    fn a_notch_zooms_about_the_cursor() {
        let (mut c, t0) = start();
        let before = c.transform().expect("state");
        let cursor = (1300.0, 700.0);
        let at = (cursor.0 + ZOOM_FIXED_POINT.0, cursor.1 + ZOOM_FIXED_POINT.1);
        let fixed = before.to_map(at.0, at.1);
        c.wheel(1.0, cursor, t0 + Duration::from_millis(10));
        let after = c.transform().expect("state");
        assert!((after.zoom - 1.2).abs() < 1e-9);
        let (x, y) = after.to_frame(fixed.0, fixed.1);
        assert!((x - at.0).abs() < 1e-9 && (y - at.1).abs() < 1e-9);
        // Out of range notches do nothing; the history keeps the earlier state.
        for k in 0..30 {
            c.wheel(1.0, cursor, t0 + Duration::from_millis(20 + k));
        }
        assert!((c.transform().expect("state").zoom - ZOOM_MAX).abs() < 1e-9);
        assert_eq!(c.at(t0 + Duration::from_millis(5)), Some(before));
    }

    #[test]
    fn a_pan_follows_the_cursor_and_stops_at_the_edge() {
        let (mut c, t0) = start();
        c.wheel(10.0, (1280.0, 700.0), t0);
        let z = c.transform().expect("state").zoom;
        c.press((1200.0, 700.0), false, t0);
        let o = c.transform().expect("state").origin;
        c.moved((1100.0, 650.0), t0 + Duration::from_millis(16));
        let s = c.transform().expect("state").scale();
        let moved = c.transform().expect("state").origin;
        // The map trails the cursor by the threshold along the drag.
        let k = 1.0 - DRAG_THRESHOLD_PX / 100.0f64.hypot(50.0);
        assert!((moved.0 - (o.0 + 100.0 * k / s)).abs() < 1e-9 && (moved.1 - (o.1 + 50.0 * k / s)).abs() < 1e-9);
        // Far past the left edge of the view, then back: the map turns at once at the edge.
        c.moved((-20_000.0, 650.0), t0 + Duration::from_millis(32));
        assert!((c.transform().expect("state").origin.0 - (1.0 - 1.0 / z)).abs() < 1e-9);
        c.moved((-19_900.0, 650.0), t0 + Duration::from_millis(48));
        assert!(c.transform().expect("state").origin.0 < 1.0 - 1.0 / z);
        c.release();
        // On a marker the map does not move.
        c.press((1200.0, 700.0), true, t0);
        let still = c.transform();
        c.moved((1000.0, 700.0), t0 + Duration::from_millis(64));
        assert_eq!(c.transform(), still);
    }

    #[test]
    fn a_frame_read_late_rebases_what_came_after() {
        let (mut c, t0) = start();
        c.wheel(5.0, (1280.0, 700.0), t0);
        let then = t0 + Duration::from_millis(100);
        c.press((1200.0, 700.0), false, t0);
        c.moved((1150.0, 700.0), then);
        let at_then = c.transform().expect("state");
        // Later input, then a frame from `then` showing the map 4 px further left.
        c.moved((1100.0, 700.0), then + Duration::from_millis(30));
        let s = at_then.scale();
        let measured = MapTransform { origin: (at_then.origin.0 + 4.0 / s, at_then.origin.1), ..at_then };
        assert!(c.rebase(measured, then, then + Duration::from_millis(40)));
        let now = c.transform().expect("state");
        assert!((now.origin.0 - (at_then.origin.0 + 50.0 / s + 4.0 / s)).abs() < 1e-9);
        // A frame at another zoom is ignored.
        let other = MapTransform { zoom: now.zoom - 0.2, ..now };
        assert!(!c.rebase(other, then + Duration::from_millis(40), then + Duration::from_millis(50)));
    }

    #[test]
    fn a_pan_is_interpolated_between_frames() {
        let (mut c, t0) = start();
        c.wheel(10.0, (1280.0, 700.0), t0);
        c.press((1200.0, 700.0), false, t0);
        c.moved((1200.0, 700.0), t0 + Duration::from_millis(100));
        let a = c.transform().expect("state");
        c.moved((1160.0, 700.0), t0 + Duration::from_millis(120));
        let b = c.transform().expect("state");
        let half = c.at(t0 + Duration::from_millis(110)).expect("state");
        assert!((half.origin.0 - (a.origin.0 + b.origin.0) / 2.0).abs() < 1e-12);
        // Still for longer than a frame gap, then moved: no slow drift across the pause.
        c.moved((1100.0, 700.0), t0 + Duration::from_millis(300));
        assert_eq!(c.at(t0 + Duration::from_millis(200)), Some(b));
        // A notch in between is a step, not a blend of two zooms.
        let before = c.transform().expect("state");
        c.wheel(-1.0, (1280.0, 700.0), t0 + Duration::from_millis(310));
        assert_eq!(c.at(t0 + Duration::from_millis(305)), Some(before));
    }

    #[test]
    fn the_map_waits_for_the_drag_threshold() {
        let (mut c, t0) = start();
        c.wheel(10.0, (1280.0, 700.0), t0);
        let before = c.transform().expect("state");
        c.press((1200.0, 700.0), false, t0);
        // 5 px: within the threshold, the map stays.
        c.moved((1205.0, 700.0), t0 + Duration::from_millis(16));
        assert_eq!(c.transform(), Some(before));
        // 20 px: it moved 20 less the threshold, and from then on 1:1.
        c.moved((1220.0, 700.0), t0 + Duration::from_millis(32));
        let s = before.scale();
        assert!((c.transform().expect("state").origin.0 - (before.origin.0 - (20.0 - DRAG_THRESHOLD_PX) / s)).abs() < 1e-9);
        c.moved((1230.0, 700.0), t0 + Duration::from_millis(48));
        assert!((c.transform().expect("state").origin.0 - (before.origin.0 - (30.0 - DRAG_THRESHOLD_PX) / s)).abs() < 1e-9);
        assert_eq!(c.pressed().map(|p| p.0), Some((1200.0, 700.0)));
    }

    #[test]
    fn difference_is_zero_for_the_same_view() {
        let a = MapTransform::zoomed(MapView::Full, (0, 34), 2.4, (0.3, 0.2));
        assert!(difference_px(&a, &a) < 1e-9);
        let b = MapTransform { origin: (0.3 + 3.0 / a.scale(), 0.2), ..a };
        assert!((difference_px(&a, &b) - 3.0).abs() < 1e-9);
    }
}
