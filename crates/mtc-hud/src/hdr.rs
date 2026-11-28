//! Ordinary content's white in 16-bit captures of an HDR display. It starts at the display's SDR
//! white level and then follows what the frames show: the brightest channel of the regions read
//! back is that white wherever white text or a full-brightness UI colour shows (one channel at
//! 255), and ordinary content goes no brighter. A display can report a white level its captures
//! do not follow, so the measured value wins.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The brightest channel is taken this often.
const SAMPLE_EVERY: Duration = Duration::from_secs(2);
/// The white is lowered only when every sample this long fell short of it (a dark menu has no
/// full-brightness pixel for a while; white text showing again raises it at the next sample).
const LOWER_AFTER: Duration = Duration::from_secs(30);
/// A measured white this much above the one in use replaces it at once: everything above the
/// assumed white comes out clipped.
const RAISE_ABOVE: f32 = 1.03;
/// And this much below it, held for `LOWER_AFTER`, lowers it.
const LOWER_BELOW: f32 = 0.9;
/// No white below this is taken (80 nits is Windows' lowest SDR white level; a quarter of it
/// leaves room for a display that reports wrongly, not for a frame of noise).
const MIN_WHITE: f32 = 0.25;

pub struct HdrWhite {
    /// The display's SDR white level over 80 nits.
    pub system: f32,
    current: f32,
    samples: VecDeque<(Instant, f32)>,
    last: Option<Instant>,
}

impl HdrWhite {
    pub fn new(system: f32) -> Self {
        Self { system, current: system, samples: VecDeque::new(), last: None }
    }

    /// Takes the brightest channel since the last sample (`peak`, 0 when nothing was read) every
    /// `SAMPLE_EVERY`; the new white when it changes.
    pub fn update(&mut self, now: Instant, peak: impl FnOnce() -> f32) -> Option<f32> {
        if self.last.is_some_and(|t| now.duration_since(t) < SAMPLE_EVERY) {
            return None;
        }
        self.last = Some(now);
        let p = peak();
        if !p.is_finite() || p <= 0.0 {
            return None;
        }
        self.samples.push_back((now, p));
        while self.samples.front().is_some_and(|&(t, _)| now.duration_since(t) > LOWER_AFTER) {
            self.samples.pop_front();
        }
        let max = self.samples.iter().map(|s| s.1).fold(0.0f32, f32::max);
        let span = self.samples.front().map_or(Duration::ZERO, |&(t, _)| now.duration_since(t));
        if max > self.current * RAISE_ABOVE || (span + SAMPLE_EVERY >= LOWER_AFTER && max < self.current * LOWER_BELOW) {
            self.current = max.max(MIN_WHITE);
            return Some(self.current);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A display reporting 480 nits whose captures put white at 1.0: lowered once 30 s of samples
    /// agree; one bright sample raises it again at once; a dark spell under 30 s changes nothing.
    #[test]
    fn the_measured_white_wins() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut w = HdrWhite::new(6.0);
        let mut changes = Vec::new();
        for s in (0..=40).step_by(2) {
            if let Some(v) = w.update(at(s), || 1.0) {
                changes.push((s, v));
            }
        }
        assert_eq!(changes, [(28, 1.0)], "lowered after a full window");

        let mut w = HdrWhite::new(3.5);
        assert_eq!(w.update(at(0), || 3.5), None);
        assert_eq!(w.update(at(1), || 9.0), None, "not yet time for a sample");
        assert_eq!(w.update(at(2), || 4.2), Some(4.2), "brighter: raised at once");
        for s in (4..=20).step_by(2) {
            assert_eq!(w.update(at(s), || 0.3), None, "a dark menu for a while");
        }
        assert_eq!(w.current, 4.2);
        assert_eq!(w.update(at(22), || 0.0), None, "nothing read");
    }
}
