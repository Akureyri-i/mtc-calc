//! The game-made V1 reference. The map service's reference fits some maps' V1 poorly (Zone 11's
//! 1× view correlates 0.19–0.30 with it), so the HUD builds one from the game's own picture. The
//! game opens V1 at 1×, so a frame the canvas puts at 1× shows the whole map at a known place; one
//! frame per opening, at least `APART` after the last, is kept at `SIDE`². With `FRAMES` frames
//! from `OPENINGS` openings their per-pixel median drops what moves (vehicles, teammates, markers,
//! X) and keeps what stays (the objectives and the spawn boxes, which are part of the map); it is
//! saved next to the downloaded reference and used from then on. Should the game redraw the map,
//! the 1× check fails on `RECHECK_MISSES` openings in a row and it is made again.

use std::time::{Duration, Instant};

use mtc_maps::MapInfo;
use mtc_vision::FrameView;
use mtc_vision::map::MapTransform;

/// Side of the kept frames and of the reference made from them.
pub const SIDE: usize = 512;
const FRAMES: usize = 12;
const OPENINGS: usize = 4;
const APART: Duration = Duration::from_secs(20);
/// The unzoomed check's own bar (`mtc_vision::map::zoom` `FIXED_MIN_SCORE`): a game-made reference
/// scoring below it at 1× on this many openings in a row is no longer the game's picture.
const RECHECK_BAR: f32 = 0.3;
const RECHECK_MISSES: u32 = 5;
/// Masked out of the reference for zoomed searches: the objective icons (drawn at screen size, a
/// 40 px diamond on the 882 px 1× container, so they do not scale with the zoom) and the
/// container's bottom band, where the collapsed ammo bar and the key hints stay on screen.
const ICON_RADIUS: f64 = 26.0 / 882.0;
const BOTTOM_BAND: f64 = 0.88;

pub struct GameReference {
    frames: Vec<Vec<u8>>,
    openings: Vec<u32>,
    last_take: Option<Instant>,
    /// A game-made reference exists and the registrar uses it.
    pub in_use: bool,
    misses: u32,
    checked: Option<u32>,
}

impl GameReference {
    pub fn for_map(info: &MapInfo) -> Self {
        Self { frames: Vec::new(), openings: Vec::new(), last_take: None, in_use: mtc_maps::game_reference_path(info).exists(), misses: 0, checked: None }
    }

    /// A V1 frame the canvas puts at 1× (`base`: the fixed 1× transform), from opening `opening`.
    /// Kept when due; when enough are kept, the reference is made and saved: its path.
    pub fn offer(&mut self, info: &MapInfo, frame: &FrameView, base: &MapTransform, opening: u32, now: Instant) -> Option<std::path::PathBuf> {
        if self.in_use || self.openings.contains(&opening) || self.last_take.is_some_and(|t| now.duration_since(t) < APART) {
            return None;
        }
        self.frames.push(crop(frame, base)?);
        self.openings.push(opening);
        self.last_take = Some(now);
        log::info!("In-game reference ({}): frame {} (full map opened {} times)", info.name, self.frames.len(), opening);
        if self.frames.len() < FRAMES || self.openings.len() < OPENINGS {
            return None;
        }
        let rgb = median(&self.frames);
        self.frames.clear();
        self.openings.clear();
        match mtc_maps::save_game_reference(info, SIDE as u32, &rgb) {
            Ok(path) => {
                self.in_use = true;
                Some(path)
            }
            Err(e) => {
                log::warn!("In-game reference could not be saved ({e})");
                None
            }
        }
    }

    /// The unzoomed check's score on a 1× frame of opening `opening` while the game-made reference
    /// is in use: whether to drop it (`RECHECK_MISSES` openings in a row below the bar).
    pub fn check(&mut self, opening: u32, score: f32) -> bool {
        if !self.in_use || self.checked == Some(opening) {
            return false;
        }
        self.checked = Some(opening);
        self.misses = if score < RECHECK_BAR { self.misses + 1 } else { 0 };
        if self.misses < RECHECK_MISSES {
            return false;
        }
        self.in_use = false;
        self.misses = 0;
        true
    }
}

/// The 1× container of `frame` area-resampled to `SIDE`² RGB; `None` when the frame does not hold it.
fn crop(frame: &FrameView, base: &MapTransform) -> Option<Vec<u8>> {
    let (left, top, size) = (base.left, base.top, base.size);
    if !frame.covers(left.floor() as i32, top.floor() as i32, size.ceil() as i32, size.ceil() as i32) {
        return None;
    }
    let step = size / SIDE as f64;
    let mut out = vec![0u8; SIDE * SIDE * 3];
    for j in 0..SIDE {
        let (y0, y1) = ((top + j as f64 * step).floor() as i32, ((top + (j + 1) as f64 * step).ceil() as i32 - 1).max((top + j as f64 * step).floor() as i32));
        for i in 0..SIDE {
            let (x0, x1) = ((left + i as f64 * step).floor() as i32, ((left + (i + 1) as f64 * step).ceil() as i32 - 1).max((left + i as f64 * step).floor() as i32));
            let mut sum = [0u32; 3];
            let mut n = 0u32;
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let p = frame.px(x, y);
                    sum[0] += u32::from(p[0]);
                    sum[1] += u32::from(p[1]);
                    sum[2] += u32::from(p[2]);
                    n += 1;
                }
            }
            for c in 0..3 {
                out[(j * SIDE + i) * 3 + c] = (sum[c] / n.max(1)) as u8;
            }
        }
    }
    Some(out)
}

/// The per-pixel, per-channel median of `frames`.
fn median(frames: &[Vec<u8>]) -> Vec<u8> {
    let mut values = vec![0u8; frames.len()];
    (0..SIDE * SIDE * 3)
        .map(|k| {
            for (v, f) in values.iter_mut().zip(frames) {
                *v = f[k];
            }
            values.sort_unstable();
            values[values.len() / 2]
        })
        .collect()
}

/// The game-made reference (RGBA, as loaded) with what does not scale with the zoom masked out
/// (alpha 0): the objective icons of `info` and the container's bottom band.
pub fn zoom_masked(info: &MapInfo, width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let (w, h) = (f64::from(width), f64::from(height));
    let objectives = mtc_maps::objectives(&info.id);
    let mut out = rgba.to_vec();
    for y in 0..height as usize {
        for x in 0..width as usize {
            let (u, v) = ((x as f64 + 0.5) / w, (y as f64 + 0.5) / h);
            let icon = objectives.iter().any(|o| (o.u - u).hypot(o.v - v) <= ICON_RADIUS);
            if icon || v >= BOTTOM_BAND {
                out[(y * width as usize + x) * 4 + 3] = 0;
            }
        }
    }
    out
}
