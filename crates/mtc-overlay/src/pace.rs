//! Which cursor moment a followed drag's marks show, from what is measured on the player's machine
//! while it runs:
//!
//! - the screen's real scan-out interval: the display's vertical blanks (`IDXGIOutput::
//!   WaitForVBlank`), which on a variable-refresh screen come with the game's frames (measured: a
//!   stand-in game at 60 fps → 120 a second, at 150 fps → 150). The compositor's own frame period
//!   does not tell it: with Roblox in front it stayed at the mode's 6.06 ms while the game drew
//!   65–73 fps;
//! - how long the overlay's content takes from its cursor sample to the time its composition is
//!   aimed at the screen, per commit, from the compositor's statistics (`clock`, Windows 11);
//! - the game's frame interval and the time of its latest frame (the captured frames' clock, set by
//!   the HUD: `GAME_FRAME_US`, `GAME_FRAME_STAMP`).
//!
//! The marks use the cursor of the moment the game read for the frame on screen together with them:
//! the commit's aimed time less the game's own delay. The game's delay cannot be read on a player's
//! machine with ordinary rights: the captured frames reach us later than the game's frames reach the
//! screen, by an amount that changed from run to run (input to processed frame 23–50 ms in testing,
//! against PresentMon's 16–22 ms from input to screen). So it comes from a calibration on one
//! machine — the drag lead tuned by eye (frames 6.0 ms apart: 5 ms ahead; 10.3 ms: 2 ms ahead) with
//! the overlay's delay at that time taken back out — scaled with the game's frame interval; the
//! player's fine-tuning (settings) on top. Without the compositor statistics (before Windows 11)
//! the drag keeps the lead set by eye.
//!
//! Frame cadence: when the screen scans out clearly more often than the game draws (a fixed-refresh
//! screen, or a variable one below its floor scanning each game frame two or three times), the game's
//! map moves only on the scans with a new game frame; marks moved on every scan ran ahead on the
//! others. Then the marks step once per game frame, on the game frames' times as captured.
//!
//! Passes stay one a millisecond while following: compositions do not come on a predictable grid
//! (in testing, passes timed to the compositor's reported period missed their composition 20–38 %
//! of the time), and a commit every millisecond is at most that old whenever one starts.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::clock::{Clock, instant_of, qpc_frequency, qpc_now, qpc_of};

/// The game's delay from reading the cursor to the time a composition shown with its frame is aimed
/// at (ms), at a 6 ms frame interval, and per ms of longer interval: the drag lead set by eye
/// (6.0 ms → 5 ms ahead, 10.3 ms → 2 ms ahead) plus the overlay's delay then (a commit every
/// millisecond, so its sample ~0.5 ms old when a composition starts, aimed ~0.9 refreshes later,
/// the refresh following the game: 6.06 ms at the panel's 165 Hz cap, 10.3 ms).
const GAME_DELAY_AT_6_MS: f64 = -5.0 + 0.5 + 0.9 * 6.06;
const GAME_DELAY_PER_FRAME_MS: f64 = ((-2.0 + 0.5 + 0.9 * 10.3) - GAME_DELAY_AT_6_MS) / 4.3;
/// The frame intervals the game's delay is scaled over (ms): beyond them it is held.
const GAME_FRAME_RANGE_MS: (f64, f64) = (4.0, 34.0);
/// The game draws clearly less often than the screen scans out: its frame interval this many scans
/// or more.
const CADENCE_RATIO: f64 = 1.4;
/// The compositor takes content handed over at least this long before a frame starts (measured
/// 0.1–0.2 ms).
const PICKUP_US: i64 = 150;
/// A commit's sample is this old on average when a composition takes it (a pass a millisecond).
const SAMPLE_AGE_US: i64 = 500;
/// Overlay delays kept for the median; vertical blanks kept for the scan interval.
const DELAYS_KEPT: usize = 60;
const VBLANKS_KEPT: usize = 64;

/// Vertical blanks of the display under the cursor (performance counter ticks), while `VBLANKS_ON`.
static VBLANKS: Mutex<VecDeque<i64>> = Mutex::new(VecDeque::new());
static VBLANKS_ON: AtomicBool = AtomicBool::new(false);

/// The game's delay (µs) for frames `frame_us` apart.
pub fn game_delay_us(frame_us: i64) -> i64 {
    let frame_ms = (frame_us as f64 / 1000.0).clamp(GAME_FRAME_RANGE_MS.0, GAME_FRAME_RANGE_MS.1);
    ((GAME_DELAY_AT_6_MS + GAME_DELAY_PER_FRAME_MS * (frame_ms - 6.0)) * 1000.0).round() as i64
}

/// Waits for the vertical blanks of the display under the cursor while asked (`VBLANKS_ON`), and
/// records them; picks the display again each time it is asked anew.
fn spawn_vblank_watch() {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput};
    crate::spawn_named("overlay-vblank", || {
        loop {
            if !VBLANKS_ON.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            let at = crate::window::cursor();
            // SAFETY: DXGI enumeration; the output is used on this thread only.
            let output: Option<IDXGIOutput> = unsafe {
                CreateDXGIFactory1::<IDXGIFactory1>().ok().and_then(|factory| {
                    (0..).map_while(|a| factory.EnumAdapters1(a).ok()).find_map(|adapter| {
                        (0..).map_while(|o| adapter.EnumOutputs(o).ok()).find(|output| {
                            output.GetDesc().is_ok_and(|d| {
                                let r = d.DesktopCoordinates;
                                d.AttachedToDesktop.as_bool() && r.left <= at.0 && at.0 < r.right && r.top <= at.1 && at.1 < r.bottom
                            })
                        })
                    })
                })
            };
            let Some(output) = output else {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            };
            VBLANKS.lock().expect("vblanks").clear();
            while VBLANKS_ON.load(Ordering::Relaxed) {
                // SAFETY: blocks until the output's next vertical blank.
                if unsafe { output.WaitForVBlank() }.is_err() {
                    break;
                }
                let mut v = VBLANKS.lock().expect("vblanks");
                v.push_back(qpc_now());
                while v.len() > VBLANKS_KEPT {
                    v.pop_front();
                }
            }
        }
    });
}

/// The median interval between the recorded vertical blanks (ticks).
fn scan_interval() -> Option<i64> {
    let v = VBLANKS.lock().expect("vblanks");
    let mut d: Vec<i64> = v.iter().zip(v.iter().skip(1)).map(|(a, b)| b - a).filter(|d| *d > 0).collect();
    drop(v);
    if d.len() < 8 {
        return None;
    }
    d.sort_unstable();
    Some(d[d.len() / 2])
}

/// A commit waiting for the compositor's statistics: its sample and when it was handed over
/// (ticks).
struct Pending {
    sampled: i64,
    done: i64,
}

pub(crate) struct Pace {
    clock: Clock,
    pending: VecDeque<Pending>,
    /// The overlay's delay per commit (µs): sample to the composition's aimed time.
    delays: VecDeque<i64>,
    /// For the 5 s report: whether a drag was followed, and whether the marks stepped.
    followed: bool,
    stepped: bool,
}

/// What the marks are placed for on one pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Moment {
    /// The cursor moment the marks show.
    pub target: Instant,
    /// Whether the marks step with the game's frames.
    pub stepped: bool,
}

/// Inputs to place the marks by, read once per pass (also for tests).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Inputs {
    /// The game's frame interval (µs), 0 unknown.
    pub frame_us: i64,
    /// The latest game frame's time (performance counter ticks), 0 unknown.
    pub frame_at: i64,
    /// The screen's scan-out interval (ticks).
    pub scan: Option<i64>,
    /// The overlay's delay (µs): measured, else estimated.
    pub overlay_us: Option<i64>,
    /// The player's fine-tuning (µs, positive: later).
    pub offset_us: i64,
    pub ticks_per_us: f64,
}

/// The cursor moment for marks handed over at `now` (ticks): the game's frame on screen when they
/// are, less the game's delay. `None` when the overlay's delay is unknown (no compositor clock).
pub(crate) fn moment(now: i64, i: &Inputs) -> Option<(i64, bool)> {
    let overlay = i.overlay_us?;
    let us = |v: i64| (v as f64 * i.ticks_per_us) as i64;
    // When the commit made now reaches the screen, moved by the player's fine-tuning.
    let aimed = now + us(overlay) - us(i.offset_us);
    // The game's frame shown then: its frames' times stepped back from the latest one (or ahead).
    let stepped = i.frame_us > 0 && i.frame_at > 0 && i.scan.is_some_and(|r| i.frame_us as f64 * i.ticks_per_us >= CADENCE_RATIO * r as f64);
    let shown = if stepped {
        let g = us(i.frame_us).max(1);
        i.frame_at + (aimed - i.frame_at).div_euclid(g) * g
    } else {
        aimed
    };
    Some((shown - us(game_delay_us(i.frame_us)), stepped))
}

impl Pace {
    pub fn new() -> Self {
        spawn_vblank_watch();
        Self { clock: Clock::new(), pending: VecDeque::new(), delays: VecDeque::new(), followed: false, stepped: false }
    }

    /// Takes the compositor's newest frames and judges the commits they took; the vertical blanks
    /// are watched while `watch` (V1 on screen).
    pub fn update(&mut self, watch: bool) {
        VBLANKS_ON.store(watch, Ordering::Relaxed);
        if !self.clock.available() {
            return;
        }
        self.clock.update();
        let f = qpc_frequency() / 1e6;
        while let Some(p) = self.pending.front() {
            let Some(frame) = self.clock.frame_taking(p.done, (PICKUP_US as f64 * f) as i64) else {
                // Not in yet; given up on once the clock is well past it.
                if self.clock.newest_start().is_some_and(|s| s - p.done > (200_000.0 * f) as i64) {
                    self.pending.pop_front();
                    continue;
                }
                break;
            };
            let delay = ((frame.target - p.sampled) as f64 / f) as i64;
            if (0..100_000).contains(&delay) {
                self.delays.push_back(delay);
                while self.delays.len() > DELAYS_KEPT {
                    self.delays.pop_front();
                }
            }
            self.pending.pop_front();
        }
    }

    /// The overlay's delay (µs): the median of the commits measured; before any, a sample's age plus
    /// how long after its start a frame is aimed at the screen.
    fn overlay_us(&self) -> Option<i64> {
        if !self.delays.is_empty() {
            let mut d: Vec<i64> = self.delays.iter().copied().collect();
            d.sort_unstable();
            return Some(d[d.len() / 2]);
        }
        let f = qpc_frequency() / 1e6;
        self.clock.target_offset().map(|o| SAMPLE_AGE_US + (o as f64 / f) as i64)
    }

    fn inputs(&self) -> Inputs {
        let ticks_per_us = qpc_frequency() / 1e6;
        // The captured frames' time is in 100 ns units of the same counter.
        let stamp = crate::GAME_FRAME_STAMP.load(Ordering::Relaxed);
        Inputs {
            frame_us: crate::GAME_FRAME_US.load(Ordering::Relaxed),
            frame_at: if stamp > 0 { (stamp as f64 * ticks_per_us / 10.0) as i64 } else { 0 },
            scan: scan_interval(),
            overlay_us: self.overlay_us(),
            offset_us: crate::FOLLOW_OFFSET_US.load(Ordering::Relaxed),
            ticks_per_us,
        }
    }

    /// The cursor moment the marks show when handed over now; the lead set by eye without the
    /// compositor statistics.
    pub fn moment(&mut self, now: Instant) -> Moment {
        self.followed = true;
        let inputs = self.inputs();
        match moment(qpc_of(now), &inputs) {
            Some((target, stepped)) => {
                self.stepped |= stepped;
                Moment { target: instant_of(target), stepped }
            }
            None => {
                let lag = crate::follow_lag_us(inputs.frame_us) + inputs.offset_us;
                let target = if lag >= 0 { now.checked_sub(Duration::from_micros(lag as u64)).unwrap_or(now) } else { now + Duration::from_micros(lag.unsigned_abs()) };
                Moment { target, stepped: false }
            }
        }
    }

    /// A commit was handed over: its cursor sample and when it was done.
    pub fn committed(&mut self, sampled: Instant, done: Instant) {
        if !self.clock.available() {
            return;
        }
        self.pending.push_back(Pending { sampled: qpc_of(sampled), done: qpc_of(done) });
        while self.pending.len() > 2_048 {
            self.pending.pop_front();
        }
    }

    /// The cursor sampler's interval: a quarter of the scan-out interval, 0.5–2 ms.
    pub fn sample_interval(&self) -> Duration {
        let f = qpc_frequency() / 1e6;
        scan_interval().map_or(Duration::from_millis(2), |p| Duration::from_micros(((p as f64 / f) / 4.0).clamp(500.0, 2_000.0) as u64))
    }

    /// The last 5 s for the log, when a drag was followed: scan-out interval, the compositor's
    /// period, game frames, overlay delay, game delay, the cursor moment used and the cadence.
    pub fn take_report(&mut self) -> Option<String> {
        let (followed, stepped) = (std::mem::take(&mut self.followed), std::mem::take(&mut self.stepped));
        if !followed {
            return None;
        }
        let i = self.inputs();
        let f = qpc_frequency() / 1e6;
        let game = i.frame_us as f64 / 1000.0;
        let scan = i.scan.map_or("scan unknown".to_owned(), |r| {
            let r = r as f64 / f / 1000.0;
            let with = if game > 0.0 && (game / r - 1.0).abs() < 0.15 { "follows the game" } else if game >= r * CADENCE_RATIO { "faster than the game" } else { "independent of the game" };
            format!("scan {r:.2} ms ({with})")
        });
        let period = self.clock.period().map_or("compositor statistics unavailable (lead set by eye)".to_owned(), |p| format!("compositor period {:.2} ms", p as f64 / f / 1000.0));
        let lag = i.overlay_us.map(|o| (game_delay_us(i.frame_us) - o + i.offset_us) as f64 / 1000.0);
        Some(format!(
            "[drag timing] {scan} · {period} · game frame {game:.1} ms · overlay to screen {} · game latency {:.1} ms (this machine's calibration) · marks use {} · {}",
            i.overlay_us.map_or("unknown".to_owned(), |o| format!("{:.1} ms ({} samples)", o as f64 / 1000.0, self.delays.len())),
            game_delay_us(i.frame_us) as f64 / 1000.0,
            lag.map_or("the lead set by eye".to_owned(), |l| if l >= 0.0 { format!("the mouse {l:.1} ms back") } else { format!("a {:.1} ms look-ahead", -l) }),
            if stepped { "changed with the game's frames" } else { "changed every scan" },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(frame_us: i64, scan_us: i64, overlay_us: i64) -> Inputs {
        Inputs { frame_us, frame_at: 1_000_000, scan: Some(scan_us), overlay_us: Some(overlay_us), offset_us: 0, ticks_per_us: 1.0 }
    }

    #[test]
    fn the_game_delay_reproduces_the_lead_set_by_eye_with_the_overlay_delay_of_then() {
        // The overlay then: a commit every millisecond (~0.5 ms old at a start), aimed 0.9 refreshes on.
        for (frame_ms, refresh_ms, lead_ms) in [(6.0, 6.06, -5.0), (10.3, 10.3, -2.0)] {
            let overlay = 0.5 + 0.9 * refresh_ms;
            let lag = game_delay_us((frame_ms * 1000.0) as i64) as f64 / 1000.0 - overlay;
            assert!((lag - lead_ms).abs() < 0.05, "{frame_ms} ms frames: {lag}");
        }
        // Slower games read the cursor longer before their frame shows.
        assert!(game_delay_us(16_700) > game_delay_us(10_300));
        assert_eq!(game_delay_us(60_000), game_delay_us(34_000));
    }

    #[test]
    fn marks_follow_every_scan_while_the_screen_follows_the_game() {
        // 70 fps on a variable-refresh panel: one scan a frame.
        let i = inputs(14_300, 14_300, 9_000);
        let (t1, stepped) = moment(2_000_000, &i).expect("known");
        let (t2, _) = moment(2_003_000, &i).expect("known");
        assert!(!stepped);
        assert_eq!(t2 - t1, 3_000);
        assert_eq!(t1, 2_000_000 + 9_000 - game_delay_us(14_300));
    }

    #[test]
    fn marks_step_with_the_game_frames_when_the_screen_scans_each_frame_twice() {
        // 60 fps on a panel scanning each frame twice (8.33 ms scans).
        let i = inputs(16_667, 8_333, 9_000);
        let mut targets = Vec::new();
        for k in 0..8 {
            let (t, stepped) = moment(2_000_000 + k * 4_000, &i).expect("known");
            assert!(stepped);
            targets.push(t);
        }
        // Within a game frame the moment holds; it moves one frame interval at a time.
        let steps: Vec<i64> = targets.windows(2).map(|w| w[1] - w[0]).filter(|d| *d != 0).collect();
        assert!(!steps.is_empty() && steps.iter().all(|d| *d == 16_667), "{steps:?}");
        // The moment shown is a game frame's time less the game's delay.
        assert_eq!((targets[0] + game_delay_us(16_667) - 1_000_000).rem_euclid(16_667), 0);
    }

    #[test]
    fn a_positive_fine_tuning_takes_an_older_cursor() {
        let mut i = inputs(6_000, 6_000, 6_400);
        let (t0, _) = moment(2_000_000, &i).expect("known");
        i.offset_us = 2_000;
        let (t1, _) = moment(2_000_000, &i).expect("known");
        assert_eq!(t0 - t1, 2_000);
    }

    #[test]
    fn without_the_compositor_clock_there_is_no_measured_moment() {
        let i = Inputs { overlay_us: None, ..inputs(6_000, 6_000, 0) };
        assert_eq!(moment(2_000_000, &i), None);
    }
}
