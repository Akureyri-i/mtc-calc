//! The compositor's clock, read at run time on the player's machine: its frames' period, and how
//! long after it starts the frame it composes is aimed at the screen — what the overlay's delay is
//! measured against (`pace`).
//!
//! Windows 11's compositor statistics (`DCompositionGetFrameId`, `DCompositionGetStatistics`) give
//! each finished composition frame's start, the time it is aimed at and its period. The period is
//! the real refresh interval, also on a variable-refresh screen following the game (measured on a
//! 165 Hz display: 6.67 ms with a stand-in game at 150 fps, 8.33 ms at 60 fps, the panel scanning
//! each frame twice below its 70 Hz floor), unlike `DwmGetCompositionTimingInfo`, which kept
//! reporting the mode's 165 Hz. The aimed time came 0.87–0.96 periods after the start.
//!
//! The functions are looked up at run time: an older Windows without them must still run the HUD
//! (the drag follow then keeps its fixed pace and lead).

use std::collections::VecDeque;

use windows::Win32::Graphics::DirectComposition::{COMPOSITION_FRAME_ID_COMPLETED, COMPOSITION_FRAME_ID_TYPE, COMPOSITION_FRAME_STATS, COMPOSITION_TARGET_ID};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::core::{HRESULT, s, w};

/// Finished frames kept: about a quarter to half a second.
const KEPT: usize = 64;
/// Frames the medians are taken over.
const RECENT: usize = 30;

type GetFrameId = unsafe extern "system" fn(COMPOSITION_FRAME_ID_TYPE, *mut u64) -> HRESULT;
type GetStatistics = unsafe extern "system" fn(u64, *mut COMPOSITION_FRAME_STATS, u32, *mut COMPOSITION_TARGET_ID, *mut u32) -> HRESULT;

/// The performance counter now (ticks).
pub fn qpc_now() -> i64 {
    let mut now = 0i64;
    // SAFETY: plain out-parameter.
    unsafe {
        let _ = QueryPerformanceCounter(&mut now);
    }
    now
}

/// Performance counter ticks per second.
pub fn qpc_frequency() -> f64 {
    let mut f = 0i64;
    // SAFETY: plain out-parameter.
    unsafe {
        let _ = QueryPerformanceFrequency(&mut f);
    }
    f.max(1) as f64
}

/// `t` on the performance counter (ticks), by the counter and the clock read together now.
pub fn qpc_of(t: std::time::Instant) -> i64 {
    let (now, q) = (std::time::Instant::now(), qpc_now());
    let f = qpc_frequency();
    if t <= now { q - ((now - t).as_secs_f64() * f) as i64 } else { q + ((t - now).as_secs_f64() * f) as i64 }
}

/// The moment a performance counter value stands for.
pub fn instant_of(qpc: i64) -> std::time::Instant {
    let (now, q) = (std::time::Instant::now(), qpc_now());
    let secs = (q - qpc) as f64 / qpc_frequency();
    if secs >= 0.0 { now.checked_sub(std::time::Duration::from_secs_f64(secs)).unwrap_or(now) } else { now + std::time::Duration::from_secs_f64(-secs) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub start: i64,
    pub target: i64,
    pub period: i64,
}

pub(crate) struct Clock {
    calls: Option<(GetFrameId, GetStatistics)>,
    last_id: u64,
    frames: VecDeque<Frame>,
}

fn median(mut v: Vec<i64>) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

impl Clock {
    pub fn new() -> Self {
        // SAFETY: looking up two functions by name in the system's dcomp.dll; the signatures are
        // the documented ones (dcomp.h).
        let calls = unsafe {
            LoadLibraryW(w!("dcomp.dll")).ok().and_then(|module| {
                let id = GetProcAddress(module, s!("DCompositionGetFrameId"))?;
                let stats = GetProcAddress(module, s!("DCompositionGetStatistics"))?;
                Some((std::mem::transmute::<_, GetFrameId>(id), std::mem::transmute::<_, GetStatistics>(stats)))
            })
        };
        Self { calls, last_id: 0, frames: VecDeque::new() }
    }

    /// A clock fed by hand (tests).
    #[cfg(test)]
    pub fn from_frames(frames: &[Frame]) -> Self {
        Self { calls: None, last_id: 0, frames: frames.iter().copied().collect() }
    }

    pub fn available(&self) -> bool {
        self.calls.is_some()
    }

    /// Takes the frames finished since the last call.
    pub fn update(&mut self) {
        let Some((get_id, get_stats)) = self.calls else { return };
        let mut newest = 0u64;
        // SAFETY: plain out-parameter.
        if unsafe { get_id(COMPOSITION_FRAME_ID_COMPLETED, &mut newest) }.is_err() || newest == 0 {
            return;
        }
        let first = if self.last_id == 0 { newest.saturating_sub(4) } else { (self.last_id + 1).max(newest.saturating_sub(KEPT as u64)) };
        for id in first..=newest {
            let mut stats = COMPOSITION_FRAME_STATS::default();
            // SAFETY: frame statistics only (no targets asked for).
            if unsafe { get_stats(id, &mut stats, 0, std::ptr::null_mut(), std::ptr::null_mut()) }.is_ok() && stats.startTime > 0 {
                self.frames.push_back(Frame { start: stats.startTime as i64, target: stats.targetTime as i64, period: stats.framePeriod as i64 });
            }
        }
        self.last_id = newest;
        while self.frames.len() > KEPT {
            self.frames.pop_front();
        }
    }

    /// The refresh interval (ticks): the median period of the recent frames.
    pub fn period(&self) -> Option<i64> {
        median(self.frames.iter().rev().take(RECENT).map(|f| f.period).filter(|p| *p > 0).collect())
    }

    /// How long after it starts a frame is aimed at the screen (ticks).
    pub fn target_offset(&self) -> Option<i64> {
        median(self.frames.iter().rev().take(RECENT).map(|f| f.target - f.start).filter(|d| *d > 0).collect())
    }

    /// The finished frame that took content handed over at `t` (ticks): the first to start after
    /// it by `lead` ticks (what the compositor needs to pick it up). `None` until that frame is in.
    pub fn frame_taking(&self, t: i64, lead: i64) -> Option<Frame> {
        self.frames.iter().find(|f| f.start >= t + lead).copied()
    }

    /// The newest finished frame's start (ticks).
    pub fn newest_start(&self) -> Option<i64> {
        self.frames.back().map(|f| f.start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(starts: &[i64], period: i64, offset: i64) -> Vec<Frame> {
        starts.iter().map(|&s| Frame { start: s, target: s + offset, period }).collect()
    }

    #[test]
    fn the_clock_finds_the_frame_that_took_a_commit() {
        let c = Clock::from_frames(&frames(&[1_000, 2_000, 3_000, 4_000], 1_000, 900));
        assert_eq!(c.period(), Some(1_000));
        assert_eq!(c.target_offset(), Some(900));
        // Handed over 50 ticks before a start the compositor needs 15 for: that frame took it.
        assert_eq!(c.frame_taking(2_950, 15).map(|f| f.start), Some(3_000));
        // Handed over too late for it: the next one.
        assert_eq!(c.frame_taking(2_990, 15).map(|f| f.start), Some(4_000));
        assert_eq!(c.frame_taking(4_200, 15), None);
    }

    #[test]
    fn counter_and_clock_convert_both_ways() {
        let t = std::time::Instant::now() - std::time::Duration::from_millis(30);
        let back = instant_of(qpc_of(t));
        let off = if back > t { back - t } else { t - back };
        assert!(off < std::time::Duration::from_micros(500), "{off:?}");
    }
}
