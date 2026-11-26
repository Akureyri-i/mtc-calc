//! Native overlay windows drawn with Direct2D/DirectWrite through DirectComposition.
//!
//! Windows are top-most and never take focus, so the player's keyboard stays with the game. The
//! aim strip, the map marks and the charge marks are click-through; the panel takes clicks
//! (target and arc choice, gun update, lock) and drags, and reports them as `PanelAction`s.
//! Rendering happens on the overlay's own thread and only when the shown state changes. A gate
//! (the game in the foreground) is checked on every pass of that thread: when another window
//! comes to the front everything hides at once, whether or not the game still sends frames, and
//! shows again as it was when the game comes back.

mod aim_strip;
mod charge_marks;
mod clock;
mod map_marks;
mod pace;
mod panel;
mod plan;
mod render;
mod settings;
mod window;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::Duration;

pub use aim_strip::{AimRow, AimStrip, Axis, hud_right};
pub use charge_marks::{ChargeDot, ChargeMarks};
pub use map_marks::{Blast, CHARGE_COLORS, DragMark, Hover, MapMark, MapMarks, NO_GUN, PanFollow, Planning, RangeRing, RingKind, TargetLabel};
pub use panel::{ConsentChoice, DiagAction, PanelAction, PanelArc, PanelLoad, PanelSolution, PanelState, PanelTarget, WIDTH as PANEL_WIDTH};
pub use map_marks::MARGIN as MAP_MARGIN;
pub use settings::{ChargeRule, ConsentState, LabelField, LabelFields, Setting, SettingsState, Stepper, ToleranceMode, WIDTH as SETTINGS_WIDTH};
pub use plan::{PlanArc, PlanChargeBox, PlanDetail, PlanNext, PlanRow, PlanState, PlanTerrain, ProfileArc, TerrainNote, MAX_HEIGHT as PLAN_MAX_HEIGHT, WIDTH as PLAN_WIDTH};

enum Command {
    Aim(Option<AimStrip>),
    Panel(Option<PanelState>),
    Map(Option<MapMarks>),
    Charges(Option<ChargeMarks>),
    Drag(Option<DragMark>),
    Plan(Option<PlanState>),
    Settings(Option<SettingsState>),
    Consent(Option<ConsentState>),
    /// Development: new devices and surfaces, as after a device loss.
    Rebuild,
    Quit,
}

/// Handle to the overlay thread.
pub struct Overlay {
    tx: Sender<Command>,
    actions: Mutex<Receiver<PanelAction>>,
    thread: Option<JoinHandle<()>>,
    /// Map marks window redraws and the microseconds they took, since the last `take_map_draws`.
    map_draws: Arc<(AtomicU64, AtomicU64)>,
}

/// An overlay window's drawing: where it shows on screen and its premultiplied BGRA pixels.
pub struct Layer {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// The aim strip and the panel drawn offscreen, each where it would show on screen (to check
/// the HUD against recorded frames).
pub fn render_offscreen(font_dir: &std::path::Path, strip: Option<&AimStrip>, panel: Option<&PanelState>, map: Option<&MapMarks>) -> windows::core::Result<Vec<Layer>> {
    let graphics = render::Graphics::new(font_dir)?;
    let mut layers = Vec::new();
    if let Some(map) = map {
        layers.push(map_marks::MapWindow::render_offscreen(&graphics, map)?);
    }
    if let Some(panel) = panel {
        layers.push(panel::PanelWindow::render_offscreen(&graphics, panel)?);
    }
    if let Some(strip) = strip {
        layers.push(aim_strip::StripWindow::render_offscreen(&graphics, strip)?);
    }
    Ok(layers)
}

/// Development: reads the compositor clock for `seconds` and reports what it saw: whether its
/// statistics are there, frames read, refresh interval and how long after its start a frame is aimed
/// at the screen (`clock`).
#[doc(hidden)]
pub fn probe_compositor_clock(seconds: f64) -> String {
    let mut clock = clock::Clock::new();
    if !clock.available() {
        return "Compositor statistics unavailable (this Windows has no DCompositionGetStatistics)".to_owned();
    }
    let started = std::time::Instant::now();
    let mut periods = Vec::new();
    let mut offsets = Vec::new();
    while started.elapsed().as_secs_f64() < seconds {
        clock.update();
        if let (Some(p), Some(o)) = (clock.period(), clock.target_offset()) {
            periods.push(p);
            offsets.push(o);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let f = clock::qpc_frequency() / 1000.0;
    let med = |v: &mut Vec<i64>| if v.is_empty() { f64::NAN } else { v.sort_unstable(); v[v.len() / 2] as f64 / f };
    format!("Compositor statistics: refresh {:.2} ms, start to target on screen {:.2} ms ({} readings)", med(&mut periods), med(&mut offsets), periods.len())
}

/// Development: the time to draw `marks` once (see `MapWindow::benchmark_draw`).
#[doc(hidden)]
pub fn benchmark_map_draw(font_dir: &std::path::Path, marks: &MapMarks, n: u32) -> windows::core::Result<std::time::Duration> {
    map_marks::MapWindow::benchmark_draw(&render::Graphics::new(font_dir)?, marks, n)
}

/// Development (`mtc-hud --latency`): the plan panel draws a 4×4 magenta square at its top left,
/// so the moment it shows can be found on the composited screen.
pub static PROBE_MARKER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How far back in the cursor's track a dragged map's marks (and a dragged marker) take the cursor
/// (µs): the game's map shows the cursor of its input moment, and the overlay, handing over only a
/// moved visual about every millisecond, reaches the screen sooner than the game's map after the
/// same input (measured with a stand-in game: 9–12 ms from sample to screen against the game's
/// 16–22 ms from input to screen for Roblox at 60 fps). Negative is ahead (extrapolated). Tuned by
/// eye in the game (`--follow-tune`): 4–6 ms ahead with Roblox at ~165 fps (frames 6 ms apart),
/// 2 ms ahead at ~97 fps (10.3 ms) — so not behind as the stand-in game suggested. The game's own input-to-screen time grows with its
/// frame interval while the overlay's stays, so the lead goes by the game's frame interval
/// (`follow_lag_us`); this is the value while the interval is not known yet.
pub const FOLLOW_LAG_US: i64 = -4_000;
/// The game's frame interval (µs) as the HUD measures it from the captured frames' times (median of
/// the last ones); 0 while unknown.
pub static GAME_FRAME_US: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
/// The latest captured game frame's time (the capture's system-relative time: the performance
/// counter in 100 ns units); 0 while unknown. The drag follow steps the marks on these frames when
/// the screen refreshes clearly more often than the game draws (`pace`).
pub static GAME_FRAME_STAMP: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
/// The cursor sampler's interval (µs), set from the refresh interval (`pace::Pace::sample_interval`).
static SAMPLE_US: AtomicU64 = AtomicU64::new(2_000);
/// Development (`mtc-hud --hidden`): no window ever shows, everything else runs (to record data
/// while playing without the HUD in the way).
pub static HIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The lead at a 6 ms frame interval (ms ahead) and how much less per ms of longer interval: the
/// line through the two settings found by eye (6.0 ms → 5 ms ahead, the middle of 4–6; 10.3 ms →
/// 2 ms ahead). Beyond the intervals measured the line is held at its ends.
const LEAD_AT_6_MS: f64 = 5.0;
const LEAD_PER_FRAME_MS: f64 = 3.0 / 4.3;
const LEAD_FRAME_RANGE_MS: (f64, f64) = (5.0, 12.0);
/// The player's fine-tuning of the drag follow on top of `follow_lag_us` (µs, positive: further
/// back); `mtc-hud --follow-tune` sets it with [ and ].
pub static FOLLOW_OFFSET_US: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// How far back in the cursor's track (µs, negative: ahead) a drag's marks take the cursor for a
/// game drawing a frame every `frame_us` (0: not known yet, `FOLLOW_LAG_US`).
pub fn follow_lag_us(frame_us: i64) -> i64 {
    if frame_us <= 0 {
        return FOLLOW_LAG_US;
    }
    let frame_ms = (frame_us as f64 / 1000.0).clamp(LEAD_FRAME_RANGE_MS.0, LEAD_FRAME_RANGE_MS.1);
    let ahead_ms = LEAD_AT_6_MS - LEAD_PER_FRAME_MS * (frame_ms - 6.0);
    -(ahead_ms * 1000.0).round() as i64
}
/// Time between passes of the overlay thread while it follows a drag: each hands over the newest
/// cursor, so what the compositor takes is at most this old.
const FOLLOW_PASS: Duration = Duration::from_millis(1);

/// The cursor's recent track (screen px), sampled by its own thread (a quarter of the refresh
/// interval, `SAMPLE_US`) while a drag is followed (`SAMPLING`), and on every pass of the overlay
/// thread: the follow looks the cursor up at a moment in the past.
static CURSOR_TRACK: std::sync::Mutex<std::collections::VecDeque<(std::time::Instant, (f64, f64))>> = std::sync::Mutex::new(std::collections::VecDeque::new());
static SAMPLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Starts a named thread (the name shows in the HUD's per-thread CPU line).
pub(crate) fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().name(name.into()).spawn(f).expect("overlay helper thread");
}

fn spawn_cursor_sampler() {
    spawn_named("overlay-cursor", || {
        raise_thread_priority();
        loop {
            if !SAMPLING.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let now = std::time::Instant::now();
            let c = window::cursor();
            {
                let mut track = CURSOR_TRACK.lock().expect("cursor track");
                track.push_back((now, (f64::from(c.0), f64::from(c.1))));
                while track.front().is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_millis(400)) {
                    track.pop_front();
                }
            }
            std::thread::sleep(Duration::from_micros(SAMPLE_US.load(Ordering::Relaxed)));
        }
    });
}

/// Above normal: a pass that misses a composition shows the marks a refresh late (drags lagged
/// visibly with the computer busy).
fn raise_thread_priority() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};
    // SAFETY: the calling thread's own pseudo-handle.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
}

/// The cursor's velocity (px/s) over its last 30 ms of samples, by least squares.
fn cursor_velocity(track: &std::collections::VecDeque<(std::time::Instant, (f64, f64))>, now: std::time::Instant) -> Option<(f64, f64)> {
    let recent: Vec<(f64, (f64, f64))> = track.iter().filter(|(t, _)| now.saturating_duration_since(*t) <= Duration::from_millis(30)).map(|(t, p)| (-(now.saturating_duration_since(*t).as_secs_f64()), *p)).collect();
    if recent.len() < 3 {
        return None;
    }
    let n = recent.len() as f64;
    let mt = recent.iter().map(|(t, _)| t).sum::<f64>() / n;
    let (mx, my) = (recent.iter().map(|(_, p)| p.0).sum::<f64>() / n, recent.iter().map(|(_, p)| p.1).sum::<f64>() / n);
    let stt: f64 = recent.iter().map(|(t, _)| (t - mt).powi(2)).sum();
    if stt <= 0.0 {
        return None;
    }
    let vx = recent.iter().map(|(t, p)| (t - mt) * (p.0 - mx)).sum::<f64>() / stt;
    let vy = recent.iter().map(|(t, p)| (t - mt) * (p.1 - my)).sum::<f64>() / stt;
    Some((vx, vy))
}

/// While a map or a marker is dragged: the marks moved to where the game shows the cursor, for the
/// cursor moment `pace` gives (the game's frame on screen with them, less the game's delay).
fn follow_drags(map_window: &mut map_marks::MapWindow, drag_window: &mut map_marks::DragWindow, draws: &(AtomicU64, AtomicU64), pace: &mut pace::Pace) {
    let following = map_window.following() || drag_window.active();
    // Sampled all the while V1 shows, so the track holds the press of a drag (the map waits for
    // the cursor to pass a threshold from it).
    SAMPLING.store(following || map_window.on_v1(), Ordering::Relaxed);
    if !following {
        return;
    }
    let now = std::time::Instant::now();
    {
        // A sample now, so the newest one is fresh whatever the sampler's phase.
        let c = window::cursor();
        let mut track = CURSOR_TRACK.lock().expect("cursor track");
        track.push_back((now, (f64::from(c.0), f64::from(c.1))));
    }
    let track = CURSOR_TRACK.lock().expect("cursor track").clone();
    let target = pace.moment(now).target;
    let at = |t: std::time::Instant| cursor_at_or_ahead(&track, t, now);
    let commits = map_window.commits() + drag_window.commits();
    if let Some((press, pressed_at)) = map_window.press() {
        let start = drag_start(&track, press, pressed_at, map_marks::DRAG_THRESHOLD_PX);
        // The map's place by the game's rule: still until the cursor passes the threshold, then
        // the cursor less where the map started to follow (`start`); one origin for every moment.
        let map_at = |t: std::time::Instant| match start {
            Some((crossed, from)) if t >= crossed => at(t).map(|c| (c.0 - from.0 + press.0, c.1 - from.1 + press.1)),
            _ => Some(press),
        };
        let started = std::time::Instant::now();
        match map_window.follow(&map_at, target) {
            Ok(true) => {
                draws.0.fetch_add(1, Ordering::Relaxed);
                draws.1.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
            }
            Ok(false) => {}
            Err(error) => eprintln!("overlay: {error}"),
        }
    }
    if drag_window.active()
        && let Some(c) = at(target)
        && let Some((press, pressed_at)) = drag_window.press()
    {
        // The game moves the marker once the cursor passed its threshold from the press (as of
        // `target`: a start the game had not shown yet does not count).
        let start = drag_start(&track, press, pressed_at, map_marks::MARKER_DRAG_THRESHOLD_PX).filter(|(t, _)| *t <= target).map(|(_, from)| from);
        if let Err(error) = drag_window.follow(c, start) {
            eprintln!("overlay: {error}");
        }
    }
    if map_window.commits() + drag_window.commits() != commits {
        pace.committed(now, std::time::Instant::now());
    }
}

/// When and where the game starts moving what a drag pressed at `press` at `pressed_at` holds (the
/// map, a marker): the first sample after the press farther than `threshold` px from it, and the
/// point at that distance towards it (what is dragged trails the cursor by it after).
fn drag_start(track: &std::collections::VecDeque<(std::time::Instant, (f64, f64))>, press: (f64, f64), pressed_at: std::time::Instant, threshold: f64) -> Option<(std::time::Instant, (f64, f64))> {
    track.iter().filter(|(t, _)| *t >= pressed_at).find_map(|&(t, c)| {
        let (dx, dy) = (c.0 - press.0, c.1 - press.1);
        let d = dx.hypot(dy);
        (d > threshold).then(|| (t, (press.0 + dx / d * threshold, press.1 + dy / d * threshold)))
    })
}

/// The cursor at `t`: from the track up to `now`, extrapolated with its velocity after it.
fn cursor_at_or_ahead(track: &std::collections::VecDeque<(std::time::Instant, (f64, f64))>, t: std::time::Instant, now: std::time::Instant) -> Option<(f64, f64)> {
    if t <= now {
        return cursor_at(track, t);
    }
    let p = cursor_at(track, now)?;
    let (vx, vy) = cursor_velocity(track, now).unwrap_or((0.0, 0.0));
    let dt = (t - now).as_secs_f64();
    Some((p.0 + vx * dt, p.1 + vy * dt))
}

/// The cursor at `t` from its sampled track, between the samples around it; the newest sample for
/// a moment after it (the HUD's marks may be placed for a moment the track has not sampled yet:
/// with nothing returned there the marks stood still through a drag).
fn cursor_at(track: &std::collections::VecDeque<(std::time::Instant, (f64, f64))>, t: std::time::Instant) -> Option<(f64, f64)> {
    let first = track.front()?;
    if t <= first.0 {
        return Some(first.1);
    }
    let Some(i) = track.iter().position(|(at, _)| *at >= t) else { return track.back().map(|(_, p)| *p) };
    let ((ta, a), (tb, b)) = (track[i - 1], track[i]);
    let f = (t - ta).as_secs_f64() / (tb - ta).as_secs_f64().max(1e-9);
    Some((a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f))
}

/// The V1 plan panel drawn offscreen where it would show.
pub fn render_plan(font_dir: &std::path::Path, state: &PlanState) -> windows::core::Result<Layer> {
    plan::PlanWindow::render_offscreen(&render::Graphics::new(font_dir)?, state)
}

/// The startup question about sending statistics drawn offscreen where it would show.
pub fn render_consent(font_dir: &std::path::Path, state: &ConsentState, hover: Option<(i32, i32)>) -> windows::core::Result<Layer> {
    settings::SettingsWindow::render_consent_offscreen(&render::Graphics::new(font_dir)?, state, hover)
}

/// The settings window drawn offscreen where it would show, `hover` (window px) highlighted.
pub fn render_settings(font_dir: &std::path::Path, state: &SettingsState, hover: Option<(i32, i32)>) -> windows::core::Result<Layer> {
    settings::SettingsWindow::render_offscreen(&render::Graphics::new(font_dir)?, state, hover)
}

/// The loading menu's charge marks drawn offscreen where they would show.
pub fn render_charge_marks(font_dir: &std::path::Path, marks: &ChargeMarks) -> windows::core::Result<Layer> {
    charge_marks::ChargeWindow::render_offscreen(&render::Graphics::new(font_dir)?, marks)
}

/// `text` drawn offscreen as the aim strip draws its lines — Montserrat of `size` px (Medium or
/// Regular), white with the strip's shadow, centred in a `width` × `height` box — as premultiplied
/// BGRA: for matching the strip's type against the game's readout.
pub fn render_text_sample(font_dir: &std::path::Path, text: &str, size: f32, medium: bool, width: u32, height: u32) -> windows::core::Result<Vec<u8>> {
    use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER};
    let surface = render::Surface::offscreen(&render::Graphics::new(font_dir)?, width, height)?;
    let weight = if medium { DWRITE_FONT_WEIGHT_MEDIUM } else { DWRITE_FONT_WEIGHT_NORMAL };
    let format = surface.text_format(size, weight, DWRITE_TEXT_ALIGNMENT_CENTER)?;
    surface.begin();
    let rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F { left: 0.0, top: 0.0, right: width as f32, bottom: height as f32 };
    surface.text(text, &format, rect, render::rgba(255, 255, 255, 1.0));
    surface.end()?;
    surface.pixels()
}

/// One string to draw with `render_text_plain`: Montserrat of `size` px and DirectWrite `weight`
/// (400 Regular, 500 Medium, 600 SemiBold, 700 Bold), its layout box's left edge at `x` (sub-pixel)
/// and top at 0 of a `width` × `height` canvas, in the readout values' rendering (`mode` 0), the
/// labels' (1) or without grid fitting (2).
pub struct PlainText<'a> {
    pub text: &'a str,
    pub size: f32,
    pub weight: i32,
    pub mode: u8,
    pub x: f32,
    pub width: u32,
    pub height: u32,
}

/// Development: each string white on black without a shadow, as premultiplied BGRA — for fitting
/// how the game draws its UI text at other display configurations.
pub fn render_text_plain(font_dir: &std::path::Path, texts: &[PlainText]) -> windows::core::Result<Vec<Vec<u8>>> {
    use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_WEIGHT, DWRITE_TEXT_ALIGNMENT_LEADING};
    let graphics = render::Graphics::new(font_dir)?;
    texts
        .iter()
        .map(|t| {
            let surface = render::Surface::offscreen(&graphics, t.width, t.height)?;
            let format = surface.text_format(t.size, DWRITE_FONT_WEIGHT(t.weight), DWRITE_TEXT_ALIGNMENT_LEADING)?;
            surface.begin();
            let rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F { left: t.x, top: 0.0, right: t.width as f32, bottom: t.height as f32 };
            surface.plain_text(t.text, &format, rect, render::rgba(255, 255, 255, 1.0), t.mode);
            surface.end()?;
            surface.pixels()
        })
        .collect()
}

/// One glyph template from `render_ui_glyphs`: grey levels 0-255 row by row, its first row
/// relative to the baseline (negative: above). The HUD hands these to the text reader
/// (`mtc_vision::text::GlyphImage`).
#[derive(Clone, Debug)]
pub struct UiGlyph {
    pub ch: char,
    pub width: usize,
    pub height: usize,
    pub top: i32,
    pub pixels: Vec<u8>,
}

/// One text style's templates: the space's width (px) and the glyphs.
#[derive(Clone, Debug, Default)]
pub struct UiGlyphStyle {
    pub space: f64,
    pub glyphs: Vec<UiGlyph>,
}

/// The characters the loading UI's text reader has templates for.
pub const UI_CHARS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz./-,()";

/// Glyph templates for the loading UI's text reader at another display configuration: each of
/// `chars` in Montserrat of DirectWrite weight and font size (px on screen) per
/// style (weapon label, card name, card value), drawn without grid fitting as the game draws its
/// small UI text, at four quarter-pixel offsets, then resampled by `upscale` (1 / the HUD scale)
/// into the reference geometry the loading UI is read in.
pub fn render_ui_glyphs(font_dir: &std::path::Path, styles: [(i32, f32); 3], upscale: f32, chars: &str) -> windows::core::Result<[UiGlyphStyle; 3]> {
    use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_WEIGHT, DWRITE_TEXT_ALIGNMENT_LEADING};
    let graphics = render::Graphics::new(font_dir)?;
    let mut out: [UiGlyphStyle; 3] = Default::default();
    for (slot, &(weight, size)) in styles.iter().enumerate() {
        let (w, h) = ((size * 3.0).ceil() as u32 + 16, (size * 2.4).ceil() as u32 + 8);
        let surface = render::Surface::offscreen(&graphics, w, h)?;
        let format = surface.text_format(size, DWRITE_FONT_WEIGHT(weight), DWRITE_TEXT_ALIGNMENT_LEADING)?;
        let draw = |text: &str, x: f32| -> windows::core::Result<Vec<f32>> {
            surface.begin();
            let rect = windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F { left: x, top: 4.0, right: w as f32, bottom: h as f32 };
            surface.plain_text(text, &format, rect, render::rgba(255, 255, 255, 1.0), 2);
            surface.end()?;
            Ok(surface.pixels()?.chunks(4).map(|p| f32::from(p[0]) / 255.0).collect())
        };
        // The baseline: the row under the bottom edge of an "H".
        let probe = draw("H", 4.0)?;
        let row_ink = |y: u32| (0..w).map(|x| probe[(y * w + x) as usize]).fold(0.0f32, f32::max);
        let baseline = (0..h).rev().find(|&y| row_ink(y) > 0.5).map_or(h as f32, |y| y as f32 + 1.0);
        // Resampled by `upscale`: each target pixel's centre mapped back, bilinear.
        let (tw, th) = ((w as f32 * upscale).round() as usize, (h as f32 * upscale).round() as usize);
        let resample = |src: &[f32]| -> Vec<f32> {
            let at = |x: i32, y: i32| if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 { 0.0 } else { src[(y as u32 * w + x as u32) as usize] };
            (0..th)
                .flat_map(|j| (0..tw).map(move |i| (i, j)))
                .map(|(i, j)| {
                    let (sx, sy) = ((i as f32 + 0.5) / upscale - 0.5, (j as f32 + 0.5) / upscale - 0.5);
                    let (x0, y0) = (sx.floor() as i32, sy.floor() as i32);
                    let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
                    at(x0, y0) * (1.0 - fx) * (1.0 - fy) + at(x0 + 1, y0) * fx * (1.0 - fy) + at(x0, y0 + 1) * (1.0 - fx) * fy + at(x0 + 1, y0 + 1) * fx * fy
                })
                .collect()
        };
        let base = (baseline * upscale).round() as i32;
        for ch in chars.chars() {
            for off in [0.0, 0.25, 0.5, 0.75] {
                let a = resample(&draw(&ch.to_string(), 4.0 + off)?);
                let ink = |x: usize, y: usize| a[y * tw + x] > 40.0 / 255.0;
                let cols: Vec<usize> = (0..tw).filter(|&x| (0..th).any(|y| ink(x, y))).collect();
                let rows: Vec<usize> = (0..th).filter(|&y| (0..tw).any(|x| ink(x, y))).collect();
                let (Some(&x0), Some(&x1), Some(&y0), Some(&y1)) = (cols.first(), cols.last(), rows.first(), rows.last()) else { continue };
                let pixels = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (y, x))).map(|(y, x)| (a[y * tw + x] * 255.0).round().clamp(0.0, 255.0) as u8).collect();
                out[slot].glyphs.push(UiGlyph { ch, width: x1 - x0 + 1, height: y1 - y0 + 1, top: y0 as i32 - base, pixels });
            }
        }
        out[slot].space = f64::from((surface.text_width("x x", &format)? - surface.text_width("xx", &format)?) * upscale);
    }
    Ok(out)
}

/// Gap between the panel and the loader's cards.
const LOADER_MENU_GAP: i32 = 32;

/// Bottom-right corner of the panel in the loader seat (screen px), where no angle readout
/// shows: `LOADER_MENU_GAP` left of the card menu's left edge `menu_left`, bottom on the bottom of
/// the loader's weapon strip `strip_bottom` — the same place with the menu open or closed.
pub fn loader_panel_anchor(menu_left: i32, strip_bottom: i32) -> (i32, i32) {
    (menu_left - LOADER_MENU_GAP, strip_bottom)
}

/// Bottom-right corner of the panel (screen px): right edge `right` shared with the aim strip
/// below (`hud_right`), bottom on the top edge of the game's ammo bar, 40 px above the readout's
/// elevation label line `label_y` — the panel sits in the band of the weapon label, the strip in
/// the band of the readout. `hud` is the game HUD's scale (the bar's height above the label).
pub fn panel_anchor(right: i32, label_y: i32, hud: f32) -> (i32, i32) {
    (right, label_y - (40.0 * hud).round() as i32)
}

impl Overlay {
    /// Starts the overlay thread. `font_dir` holds Montserrat (the game's UI font).
    pub fn spawn(font_dir: std::path::PathBuf) -> Self {
        Self::spawn_gated(font_dir, || true)
    }

    /// `spawn`, the windows shown only while `gate` says so (checked every few milliseconds).
    pub fn spawn_gated(font_dir: std::path::PathBuf, gate: impl Fn() -> bool + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let (action_tx, actions) = mpsc::channel();
        let map_draws = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
        let draws = map_draws.clone();
        let thread = std::thread::Builder::new().name("overlay".into()).spawn(move || run(rx, action_tx, font_dir, &draws, &gate)).expect("overlay thread");
        Self {
            tx,
            actions: Mutex::new(actions),
            thread: Some(thread),
            map_draws,
        }
    }

    /// Map marks window redraws since the last call and their total time (for the stats).
    pub fn take_map_draws(&self) -> (u64, std::time::Duration) {
        let n = self.map_draws.0.swap(0, Ordering::Relaxed);
        let micros = self.map_draws.1.swap(0, Ordering::Relaxed);
        (n, std::time::Duration::from_micros(micros))
    }

    /// Shows the aim strip, or hides it with `None`.
    pub fn show_aim(&self, strip: Option<AimStrip>) {
        let _ = self.tx.send(Command::Aim(strip));
    }

    /// Shows the panel, or hides it with `None`.
    pub fn show_panel(&self, state: Option<PanelState>) {
        let _ = self.tx.send(Command::Panel(state));
    }

    /// Shows the target marks over the map on screen, or hides them with `None`.
    pub fn show_map(&self, marks: Option<MapMarks>) {
        let _ = self.tx.send(Command::Map(marks));
    }

    /// Shows the charge marks on the loading menu, or hides them with `None`.
    pub fn show_charges(&self, marks: Option<ChargeMarks>) {
        let _ = self.tx.send(Command::Charges(marks));
    }

    /// Shows the V1 plan panel, or hides it with `None`.
    pub fn show_plan(&self, state: Option<PlanState>) {
        let _ = self.tx.send(Command::Plan(state));
    }

    /// Shows the settings window, or hides it with `None`.
    pub fn show_settings(&self, state: Option<SettingsState>) {
        let _ = self.tx.send(Command::Settings(state));
    }

    /// Shows the startup question about sending statistics, or hides it with `None`.
    pub fn show_consent(&self, state: Option<ConsentState>) {
        let _ = self.tx.send(Command::Consent(state));
    }

    /// Shows a marker being dragged at the cursor, or ends it with `None`.
    pub fn show_drag(&self, drag: Option<DragMark>) {
        let _ = self.tx.send(Command::Drag(drag));
    }

    /// Development: makes the graphics devices and every window surface again, as after a device
    /// loss, and redraws what was shown.
    #[doc(hidden)]
    pub fn rebuild_graphics(&self) {
        let _ = self.tx.send(Command::Rebuild);
    }

    /// Clicks on the panel since the last call.
    pub fn take_actions(&self) -> Vec<PanelAction> {
        self.actions.lock().expect("actions lock").try_iter().collect()
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The newest state asked for each window.
#[derive(Default)]
struct Latest {
    aim: Option<AimStrip>,
    panel: Option<PanelState>,
    map: Option<MapMarks>,
    charges: Option<ChargeMarks>,
    drag: Option<DragMark>,
    plan: Option<PlanState>,
    settings: Option<SettingsState>,
    consent: Option<ConsentState>,
}

fn run(rx: Receiver<Command>, actions: Sender<PanelAction>, font_dir: std::path::PathBuf, map_draws: &(AtomicU64, AtomicU64), gate: &dyn Fn() -> bool) {
    // One set of devices and fonts for all five windows.
    let graphics = render::Graphics::new(&font_dir).expect("create overlay graphics");
    let mut strip_window = aim_strip::StripWindow::new(&graphics).expect("create aim strip window");
    let mut panel_window = panel::PanelWindow::new(&graphics).expect("create panel window");
    let mut map_window = map_marks::MapWindow::new(&graphics).expect("create map marks window");
    let mut charge_window = charge_marks::ChargeWindow::new(&graphics).expect("create charge marks window");
    let mut drag_window = map_marks::DragWindow::new(&graphics).expect("create drag window");
    let mut plan_window = plan::PlanWindow::new(&graphics).expect("create plan window");
    let mut settings_window = settings::SettingsWindow::new(&graphics).expect("create settings window");
    let mut consent_window = settings::SettingsWindow::new(&graphics).expect("create consent window");
    let mut pace = pace::Pace::new();
    // Placements seen: the settings window stays above the other windows, which each placement
    // puts on top (otherwise V1's marks open over it).
    let mut placed_seen = 0u64;
    let mut keep_settings_on_top = |settings: &settings::SettingsWindow| {
        let placed = window::PLACED.load(Ordering::Relaxed);
        if placed != placed_seen {
            placed_seen = placed;
            if settings.visible() {
                window::raise(settings.hwnd());
            }
        }
    };
    // The device was lost (or a rebuild was asked for) and the windows are not all made again yet.
    let mut stale = false;
    let mut retry_at = std::time::Instant::now();
    let mut latest = Latest::default();
    let mut timing = DrawTiming::new();
    // The map marks' drawings by why, printed every 5 s when there were any.
    let mut redraws_logged = std::time::Instant::now();
    // The gate was open on the last pass.
    let mut open = true;
    raise_thread_priority();
    spawn_cursor_sampler();
    loop {
        window::pump_messages();
        let allowed = gate();
        if allowed != open {
            open = allowed;
            let result = if open {
                // Back as it was asked for meanwhile.
                strip_window
                    .show(latest.aim.as_ref())
                    .and_then(|()| panel_window.show(latest.panel.as_ref()))
                    .and_then(|()| map_window.show(latest.map.as_ref()).map(|_| ()))
                    .and_then(|()| charge_window.show(latest.charges.as_ref()))
                    .and_then(|()| drag_window.show(latest.drag.as_ref(), window::cursor()))
                    .and_then(|()| plan_window.show(latest.plan.as_ref()))
                    .and_then(|()| settings_window.show(latest.settings.as_ref()))
                    .and_then(|()| consent_window.show_consent(latest.consent.as_ref()))
            } else {
                strip_window
                    .show(None)
                    .and_then(|()| panel_window.show(None))
                    .and_then(|()| map_window.show(None).map(|_| ()))
                    .and_then(|()| charge_window.show(None))
                    .and_then(|()| drag_window.show(None, window::cursor()))
                    .and_then(|()| plan_window.show(None))
                    .and_then(|()| settings_window.show(None))
                    .and_then(|()| consent_window.show_consent(None))
            };
            if let Err(error) = result {
                eprintln!("overlay: {error}");
            }
        }
        // A driver update or reset loses the device all five windows draw with: new devices, every
        // surface made again and redrawn; retried each second until it works.
        if (stale || graphics.lost()) && std::time::Instant::now() >= retry_at {
            if !stale {
                eprintln!("overlay: graphics device lost");
            }
            stale = true;
            let rebuilt = graphics.recover().and_then(|()| {
                strip_window.rebuild()?;
                panel_window.rebuild()?;
                map_window.rebuild()?;
                charge_window.rebuild()?;
                drag_window.rebuild(window::cursor())?;
                plan_window.rebuild()?;
                settings_window.rebuild()?;
                consent_window.rebuild()
            });
            match rebuilt {
                Ok(()) => {
                    stale = false;
                    eprintln!("overlay: windows rebuilt on new graphics devices");
                }
                Err(error) => {
                    retry_at = std::time::Instant::now() + Duration::from_secs(1);
                    eprintln!("overlay: rebuilding the windows: {error}");
                }
            }
        }
        // A dragged map or marker keeps up with the cursor between the HUD's frames, on every pass.
        if open {
            if map_window.following() || drag_window.active() || map_window.on_v1() {
                pace.update(true);
                SAMPLE_US.store(pace.sample_interval().as_micros() as u64, Ordering::Relaxed);
            } else {
                pace.update(false);
            }
            follow_drags(&mut map_window, &mut drag_window, map_draws, &mut pace);
            keep_settings_on_top(&settings_window);
            keep_settings_on_top(&consent_window);
        }
        if redraws_logged.elapsed() >= Duration::from_secs(5) {
            redraws_logged = std::time::Instant::now();
            if let Some(line) = pace.take_report() {
                println!("{line}");
            }
            let redraws = map_window.take_redraws();
            if !redraws.is_empty() {
                let parts: Vec<String> = redraws.iter().rev().map(|(why, n)| format!("{} {n}", why.name())).collect();
                println!("[overlay] map marks redrawn (5 s): {}", parts.join(" · "));
            }
        }
        for event in window::take_mouse() {
            let result = match event {
                window::Mouse::Move(hwnd, x, y) if hwnd == panel_window.hwnd() => {
                    if panel_window.drag((x, y)) { Ok(()) } else { panel_window.hover(Some((x, y))) }
                }
                window::Mouse::Leave(hwnd) if hwnd == panel_window.hwnd() => panel_window.hover(None),
                window::Mouse::Down(hwnd, x, y) if hwnd == panel_window.hwnd() => {
                    panel_window.press((x, y));
                    Ok(())
                }
                window::Mouse::Up(hwnd, _, _) if hwnd == panel_window.hwnd() => {
                    if let Some(action) = panel_window.release() {
                        let _ = actions.send(action);
                    }
                    Ok(())
                }
                window::Mouse::Move(hwnd, x, y) if hwnd == plan_window.hwnd() => plan_window.hover(Some((x, y))).map(|a| {
                    if let Some(a) = a {
                        let _ = actions.send(a);
                    }
                }),
                window::Mouse::Leave(hwnd) if hwnd == plan_window.hwnd() => plan_window.hover(None).map(|a| {
                    if let Some(a) = a {
                        let _ = actions.send(a);
                    }
                }),
                window::Mouse::Down(hwnd, x, y) if hwnd == plan_window.hwnd() => {
                    plan_window.press((x, y));
                    Ok(())
                }
                window::Mouse::Up(hwnd, _, _) if hwnd == plan_window.hwnd() => {
                    if let Some(action) = plan_window.release() {
                        let _ = actions.send(action);
                    }
                    Ok(())
                }
                window::Mouse::Move(hwnd, x, y) if hwnd == settings_window.hwnd() => settings_window.hover(Some((x, y))),
                window::Mouse::Leave(hwnd) if hwnd == settings_window.hwnd() => settings_window.hover(None),
                window::Mouse::Down(hwnd, x, y) if hwnd == settings_window.hwnd() => {
                    settings_window.press((x, y));
                    Ok(())
                }
                window::Mouse::Up(hwnd, _, _) if hwnd == settings_window.hwnd() => {
                    if let Some(action) = settings_window.release() {
                        let _ = actions.send(action);
                    }
                    Ok(())
                }
                window::Mouse::Move(hwnd, x, y) if hwnd == consent_window.hwnd() => consent_window.hover(Some((x, y))),
                window::Mouse::Leave(hwnd) if hwnd == consent_window.hwnd() => consent_window.hover(None),
                window::Mouse::Down(hwnd, x, y) if hwnd == consent_window.hwnd() => {
                    consent_window.press((x, y));
                    Ok(())
                }
                window::Mouse::Up(hwnd, _, _) if hwnd == consent_window.hwnd() => {
                    if let Some(action) = consent_window.release() {
                        let _ = actions.send(action);
                    }
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                eprintln!("overlay: {error}");
            }
        }
        let first = if open && (map_window.following() || drag_window.active() || panel_window.dragging()) {
            // A pass about every millisecond while a drag is followed, whatever the compositor's
            // phase: waiting for its pass (`DwmFlush`) returned just after it took its content, so
            // what was handed over then waited most of a frame; passes timed to its reported period
            // missed it 20–38 % of the time in testing.
            std::thread::sleep(FOLLOW_PASS);
            match rx.try_recv() {
                Ok(command) => command,
                Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => continue,
            }
        } else {
            match rx.recv_timeout(Duration::from_millis(15)) {
                Ok(command) => command,
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => continue,
            }
        };
        // Only the newest state of each window matters.
        let (mut aim, mut panel, mut map, mut charges, mut drag, mut quit) = (None, None, None, None, None, false);
        let mut plan = None;
        let mut settings = None;
        let mut consent = None;
        let mut rebuild = false;
        for command in std::iter::once(first).chain(rx.try_iter()) {
            match command {
                Command::Aim(s) => aim = Some(s),
                Command::Panel(s) => panel = Some(s),
                Command::Map(s) => map = Some(s),
                Command::Charges(s) => charges = Some(s),
                Command::Drag(s) => drag = Some(s),
                Command::Plan(s) => plan = Some(s),
                Command::Settings(s) => settings = Some(s),
                Command::Consent(s) => consent = Some(s),
                Command::Rebuild => rebuild = true,
                Command::Quit => quit = true,
            }
        }
        if quit {
            return;
        }
        stale |= rebuild;
        if let Some(s) = &aim {
            latest.aim = *s;
        }
        if let Some(s) = &panel {
            latest.panel.clone_from(s);
        }
        if let Some(s) = &map {
            latest.map.clone_from(s);
        }
        if let Some(s) = &charges {
            latest.charges.clone_from(s);
        }
        if let Some(s) = &drag {
            latest.drag.clone_from(s);
        }
        if let Some(s) = &plan {
            latest.plan.clone_from(s);
        }
        if let Some(s) = &settings {
            latest.settings.clone_from(s);
        }
        if let Some(s) = &consent {
            latest.consent.clone_from(s);
        }
        if !open {
            continue;
        }
        let t = std::time::Instant::now();
        if let Some(state) = aim {
            timing.strip(t);
            if let Err(error) = strip_window.show(state.as_ref()) {
                eprintln!("overlay: {error}");
            }
        }
        let t = timing.lap(0, t);
        if let Some(state) = panel
            && let Err(error) = panel_window.show(state.as_ref())
        {
            eprintln!("overlay: {error}");
        }
        let t = timing.lap(1, t);
        // The plan panel before the map's marks: a V1 switch shows its panel first.
        if let Some(state) = plan
            && let Err(error) = plan_window.show(state.as_ref())
        {
            eprintln!("overlay: {error}");
        }
        if let Some(state) = settings
            && let Err(error) = settings_window.show(state.as_ref())
        {
            eprintln!("overlay: {error}");
        }
        if let Some(state) = consent
            && let Err(error) = consent_window.show_consent(state.as_ref())
        {
            eprintln!("overlay: {error}");
        }
        let t = timing.lap(2, t);
        if let Some(state) = map {
            let started = std::time::Instant::now();
            match map_window.show(state.as_ref()) {
                Ok(true) => {
                    map_draws.0.fetch_add(1, Ordering::Relaxed);
                    map_draws.1.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
                }
                Ok(false) => {}
                Err(error) => eprintln!("overlay: {error}"),
            }
            // New marks while following a drag are folded in at once.
            follow_drags(&mut map_window, &mut drag_window, map_draws, &mut pace);
        }
        let t = timing.lap(3, t);
        if let Some(state) = charges
            && let Err(error) = charge_window.show(state.as_ref())
        {
            eprintln!("overlay: {error}");
        }
        let t = timing.lap(4, t);
        if let Some(state) = drag
            && let Err(error) = drag_window.show(state.as_ref(), window::cursor())
        {
            eprintln!("overlay: {error}");
        }
        timing.lap(5, t);
        keep_settings_on_top(&settings_window);
        timing.report();
    }
}

/// How the overlay keeps up, over 5 s windows: how often the strip gets a new state, the longest
/// gap between two, and the slowest single window draw. Printed only when the strip stalled
/// (gap over `STALL`) or a draw was slow (over `SLOW_DRAW`).
struct DrawTiming {
    since: std::time::Instant,
    strips: u32,
    last_strip: Option<std::time::Instant>,
    strip_gap: Duration,
    slowest: [Duration; 6],
}

impl DrawTiming {
    const NAMES: [&'static str; 6] = ["aim strip", "panel", "plan panel", "map marks", "charge marks", "dragged marker"];
    /// The strip's states come every 37–40 ms at the 33 ms frame interval (measured).
    const STALL: Duration = Duration::from_millis(150);
    const SLOW_DRAW: Duration = Duration::from_millis(20);

    fn new() -> Self {
        Self { since: std::time::Instant::now(), strips: 0, last_strip: None, strip_gap: Duration::ZERO, slowest: [Duration::ZERO; 6] }
    }

    fn strip(&mut self, at: std::time::Instant) {
        if let Some(last) = self.last_strip {
            self.strip_gap = self.strip_gap.max(at - last);
        }
        self.last_strip = Some(at);
        self.strips += 1;
    }

    /// The time since `t` goes to window `i`; returns now.
    fn lap(&mut self, i: usize, t: std::time::Instant) -> std::time::Instant {
        let now = std::time::Instant::now();
        self.slowest[i] = self.slowest[i].max(now - t);
        now
    }

    fn report(&mut self) {
        let elapsed = self.since.elapsed();
        if elapsed < Duration::from_secs(5) {
            return;
        }
        let (i, worst) = self.slowest.iter().enumerate().max_by_key(|(_, d)| **d).map(|(i, d)| (i, *d)).unwrap_or((0, Duration::ZERO));
        if self.strip_gap <= Self::STALL && worst <= Self::SLOW_DRAW {
            *self = Self { last_strip: self.last_strip, ..Self::new() };
            return;
        }
        println!(
            "[overlay] aim strip updates {:.0}/s, longest gap {:.0} ms; slowest draw: {} {:.1} ms",
            f64::from(self.strips) / elapsed.as_secs_f64(),
            self.strip_gap.as_secs_f64() * 1000.0,
            Self::NAMES[i],
            worst.as_secs_f64() * 1000.0
        );
        *self = Self { last_strip: self.last_strip, ..Self::new() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn the_drag_lead_follows_the_game_frame_interval() {
        // The two settings found by eye: 6.0 ms frames → 5 ms ahead, 10.3 ms → 2 ms ahead.
        assert_eq!(follow_lag_us(6_000), -5_000);
        assert!((follow_lag_us(10_300) + 2_000).abs() <= 50, "{}", follow_lag_us(10_300));
        // Longer frames, less lead; beyond the intervals measured the line holds at its ends.
        assert!(follow_lag_us(8_000) > follow_lag_us(6_000) && follow_lag_us(8_000) < follow_lag_us(10_300));
        assert_eq!(follow_lag_us(16_700), follow_lag_us(12_000));
        assert_eq!(follow_lag_us(3_000), follow_lag_us(5_000));
        // Not known yet: the default.
        assert_eq!(follow_lag_us(0), FOLLOW_LAG_US);
    }

    #[test]
    fn a_drag_moves_the_map_past_the_threshold() {
        let t0 = Instant::now();
        let press = (1000.0, 500.0);
        let track: std::collections::VecDeque<(Instant, (f64, f64))> =
            [(0, (998.0, 500.0)), (2, (1000.0, 500.0)), (4, (1003.0, 500.0)), (6, (1006.0, 500.0)), (8, (1010.0, 500.0)), (10, (1015.0, 500.0))]
                .into_iter()
                .map(|(ms, p)| (t0 + Duration::from_millis(ms), p))
                .collect();
        // Pressed at 2 ms: the first sample past 6.7 px is the one at 8 ms; the map starts from
        // 6.7 px towards it.
        let (at, from) = drag_start(&track, press, t0 + Duration::from_millis(2), map_marks::DRAG_THRESHOLD_PX).expect("started");
        assert_eq!(at, t0 + Duration::from_millis(8));
        assert!((from.0 - (1000.0 + map_marks::DRAG_THRESHOLD_PX)).abs() < 1e-9 && (from.1 - 500.0).abs() < 1e-9);
        // Still within it: no start.
        assert_eq!(drag_start(&track.iter().take(4).copied().collect(), press, t0, map_marks::DRAG_THRESHOLD_PX), None);
    }
}
