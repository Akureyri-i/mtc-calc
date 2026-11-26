//! `--latency` (development): when, after the key, the game's full map and the HUD's plan panel
//! show on the composited screen. DXGI desktop duplication hands every new desktop image with its
//! present time; the game's side is the V1 hint row read by the view cues, the HUD's side the 4×4
//! magenta squares the plan panel and the V1 marks window draw at their top left in this mode
//! (`PROBE_MARKER`). Lines per V1 open or close, with the key and the HUD's own detection on the
//! same clock.
//!
//! The drag probe: while the map is dragged on V1 with a marker selected, every desktop image
//! around that marker gives the game's green diamond and the magenta dot the marks window draws on
//! its centre in this mode. Per drag: how far the dot runs behind (or ahead of) the diamond on
//! screen, as a time shift and in px.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mtc_capture::windows_capture::d3d11::StagingTexture;
use mtc_capture::windows_capture::dxgi_duplication_api::{DxgiDuplicationApi, DxgiDuplicationFormat, Error};
use mtc_capture::windows_capture::monitor::Monitor;
use mtc_vision::map::{CueReader, MapView};
use mtc_vision::{FrameView, PixelFormat};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// What the HUD's loop tells the probe.
#[derive(Default)]
pub struct Shared {
    /// The game's client area on screen.
    pub client: Option<(i32, i32)>,
    /// The last M or X press.
    pub key: Option<Instant>,
    /// The HUD's processing last saw V1 open (`true`) or closed, and when.
    pub detected: Option<(bool, Instant)>,
    /// While the map is dragged on V1: the selected marker's diamond centre as last drawn
    /// (screen px).
    pub drag: Option<(i32, i32)>,
    /// While dragging: when each game frame reached the HUD's loop and when its marks were handed
    /// to the overlay.
    pub arrivals: Vec<(Instant, Instant)>,
}

/// Half the side of the square read around the selected marker during a drag (px).
const DRAG_HALF: i32 = 160;

/// One desktop image of a drag: when it was presented, the game's diamond centre and the HUD's dot.
struct DragSample {
    t: Instant,
    game: Option<(f64, f64)>,
    hud: Option<(f64, f64)>,
}

/// The game's marker green (13, 218, 11) on any ground; the HUD's magenta dot composited.
fn is_marker_green(p: &[u8]) -> bool {
    let (b, g, r) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
    g > 150 && g - r.max(b) > 100
}

fn is_probe_magenta(p: &[u8]) -> bool {
    p[0] > 180 && p[2] > 180 && u16::from(p[1]) + 90 < u16::from(p[0].min(p[2]))
}

/// In a BGRA crop `w` px wide: the centre of the HUD's dot, and the centre of the green diamond
/// outline nearest it (the green blob 20–40 px square; the label beside it is wider than tall).
fn drag_sample(px: &[u8], w: usize) -> (Option<(f64, f64)>, Option<(f64, f64)>) {
    let h = px.len() / 4 / w;
    let mut dot = (0.0, 0.0, 0u32);
    let mut green = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = &px[(y * w + x) * 4..(y * w + x) * 4 + 4];
            if is_probe_magenta(p) {
                dot = (dot.0 + x as f64, dot.1 + y as f64, dot.2 + 1);
            }
            green[y * w + x] = is_marker_green(p);
        }
    }
    let hud = (dot.2 >= 4).then(|| (dot.0 / f64::from(dot.2) + 0.5, dot.1 / f64::from(dot.2) + 0.5));
    // Green blobs (8-connected) by bounding box.
    let mut seen = vec![false; w * h];
    let mut best: Option<((f64, f64), f64)> = None;
    for start in 0..w * h {
        if !green[start] || seen[start] {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if green[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        let (bw, bh) = ((x1 - x0 + 1) as f64, (y1 - y0 + 1) as f64);
        if !(20.0..=40.0).contains(&bw) || (bw - bh).abs() > 6.0 {
            continue;
        }
        let c = ((x0 + x1 + 1) as f64 / 2.0, (y0 + y1 + 1) as f64 / 2.0);
        let near = hud.map_or(0.0, |d| (c.0 - d.0).hypot(c.1 - d.1));
        if best.is_none_or(|(_, n)| near < n) {
            best = Some((c, near));
        }
    }
    (best.map(|(c, _)| c), hud)
}

/// A finished drag: the shift of the HUD's dot behind the game's diamond that matches best (ms;
/// positive: behind) and how far apart they are on screen without and with it (px, median).
fn report_drag(samples: &[DragSample], arrivals: &[(Instant, Instant)]) {
    // Every image of the drag for offline study (`scratch/probe/`, not kept in git).
    if let Some(first) = samples.first() {
        use std::io::Write;
        let dir = std::path::Path::new("scratch/probe");
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        if std::fs::create_dir_all(dir).is_ok()
            && let Ok(mut f) = std::fs::File::create(dir.join(format!("drag-{stamp}.csv")))
        {
            let _ = writeln!(f, "t_ms,game_x,game_y,hud_x,hud_y");
            for s in samples {
                let t = s.t.saturating_duration_since(first.t).as_secs_f64() * 1000.0;
                let (gx, gy) = s.game.map_or((String::new(), String::new()), |g| (format!("{:.1}", g.0), format!("{:.1}", g.1)));
                let (hx, hy) = s.hud.map_or((String::new(), String::new()), |h| (format!("{:.1}", h.0), format!("{:.1}", h.1)));
                let _ = writeln!(f, "{t:.3},{gx},{gy},{hx},{hy}");
            }
        }
        // The HUD's side: frame arrivals and hand-overs on the same clock (negative: before the
        // first image).
        if let Ok(mut f) = std::fs::File::create(dir.join(format!("drag-{stamp}-hud.csv"))) {
            let rel = |t: Instant| if t >= first.t { t.duration_since(first.t).as_secs_f64() * 1000.0 } else { -(first.t.duration_since(t).as_secs_f64() * 1000.0) };
            let _ = writeln!(f, "arrival_ms,handed_ms");
            for &(a, h) in arrivals {
                let _ = writeln!(f, "{:.3},{:.3}", rel(a), rel(h));
            }
        }
    }
    let game: Vec<(Instant, (f64, f64))> = samples.iter().filter_map(|s| Some((s.t, s.game?))).collect();
    let both: Vec<(Instant, (f64, f64), (f64, f64))> = samples.iter().filter_map(|s| Some((s.t, s.game?, s.hud?))).collect();
    if game.len() < 10 || both.len() < 10 {
        log::info!("Drag probe: this drag showed the game's diamond on {} frames and both on {}, too few", game.len(), both.len());
        return;
    }
    let t0 = game[0].0;
    let at = |t: Instant| t.saturating_duration_since(t0).as_secs_f64() * 1000.0;
    // The game's diamond at a time, between the images around it.
    let game_at = |ms: f64| -> Option<(f64, f64)> {
        let i = game.iter().position(|(t, _)| at(*t) >= ms)?;
        if i == 0 {
            return None;
        }
        let ((ta, a), (tb, b)) = ((at(game[i - 1].0), game[i - 1].1), (at(game[i].0), game[i].1));
        let f = if tb > ta { (ms - ta) / (tb - ta) } else { 0.0 };
        Some((a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f))
    };
    let miss = |shift: f64| -> f64 {
        let mut d: Vec<f64> = both.iter().filter_map(|(t, _, h)| game_at(at(*t) - shift).map(|g| (h.0 - g.0).hypot(h.1 - g.1))).collect();
        if d.len() < 5 {
            return f64::INFINITY;
        }
        d.sort_by(f64::total_cmp);
        d[d.len() / 2]
    };
    let (best, best_miss) = (-60..=100).step_by(2).map(|s| (s, miss(f64::from(s)))).min_by(|a, b| a.1.total_cmp(&b.1)).expect("shifts");
    let mut apart: Vec<f64> = both.iter().map(|(_, g, h)| (h.0 - g.0).hypot(h.1 - g.1)).collect();
    apart.sort_by(f64::total_cmp);
    let p95 = apart[apart.len() * 95 / 100];
    let path: f64 = game.windows(2).map(|w| (w[1].1.0 - w[0].1.0).hypot(w[1].1.1 - w[0].1.1)).sum();
    let secs = game.last().map_or(0.0, |(t, _)| at(*t)) / 1000.0;
    log::info!(
        "Drag probe: {} frames ({} with the game's diamond), dragged at {:.0} px/s; marks from the game's diamond on screen median {:.1} px, p95 {p95:.1} px; by time the marks fit best {best} ms behind the game (median {best_miss:.1} px)",
        samples.len(),
        game.len(),
        path / secs.max(0.001),
        apart[apart.len() / 2],
    );
    // Steps: in which screen image the game's diamond moves, and in which the marks follow. Each
    // image is one composition (165 Hz on the test display); the marks moving in the same image as
    // the game's diamond is in step, later is the trail the eye sees between the game's frames.
    let moved = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1) > 1.0;
    let mut lags: Vec<f64> = Vec::new();
    let mut counts = [0u32; 4];
    let mut hud_first = 0u32;
    for i in 1..both.len() {
        if !moved(both[i].1, both[i - 1].1) {
            continue;
        }
        // The game stepped in image i: the first image from i on where the marks moved too.
        let follow = (i..both.len().min(i + 8)).find(|&j| moved(both[j].2, both[j - 1].2));
        if let Some(j) = follow {
            counts[(j - i).min(3)] += 1;
            lags.push(both[j].0.saturating_duration_since(both[i].0).as_secs_f64() * 1000.0);
        }
        // The marks moved in the image before without the game: ahead.
        if i >= 2 && moved(both[i - 1].2, both[i - 2].2) && !moved(both[i - 1].1, both[i - 2].1) {
            hud_first += 1;
        }
    }
    if !lags.is_empty() {
        lags.sort_by(f64::total_cmp);
        log::info!(
            "Drag probe, frame changes: the game's diamond moved {} times; the marks moved in the same picture {} times, one picture late {}, two late {}, three or more late {} (late median {:.1} ms); the marks moved first {hud_first} times",
            counts.iter().sum::<u32>(),
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            lags[lags.len() / 2]
        );
    }
}

/// Starts the probe. `plan_offset`, `marks_offset`: the plan panel's and the V1 marks window's top
/// left from the client origin (px).
pub fn spawn(shared: Arc<Mutex<Shared>>, plan_offset: (i32, i32), marks_offset: (i32, i32)) {
    mtc_overlay::PROBE_MARKER.store(true, std::sync::atomic::Ordering::Relaxed);
    std::thread::spawn(move || run(&shared, plan_offset, marks_offset));
}

/// Desktop images are read this long after an M or X press.
const WINDOW: Duration = Duration::from_millis(1500);

/// A V1 open or close seen on screen, waiting for the HUD to follow.
struct Flip {
    open: bool,
    game: Instant,
    key: Option<Instant>,
    detected: Option<Instant>,
}

fn run(shared: &Mutex<Shared>, plan_offset: (i32, i32), marks_offset: (i32, i32)) {
    let (mut freq, mut qpc0) = (0i64, 0i64);
    // SAFETY: plain out-parameter calls.
    unsafe {
        let _ = QueryPerformanceFrequency(&mut freq);
        let _ = QueryPerformanceCounter(&mut qpc0);
    }
    let inst0 = Instant::now();
    let present_at = |qpc: i64| inst0 + Duration::from_secs_f64((qpc - qpc0).max(0) as f64 / freq as f64);
    let ms = |a: Instant, b: Instant| b.saturating_duration_since(a).as_secs_f64() * 1000.0;
    let cues = CueReader::new();
    // 8-bit BGRA only (the crate prefers 16-bit float when it may).
    let open = || Monitor::primary().map_err(|e| e.to_string()).and_then(|m| DxgiDuplicationApi::new_options(m, &[DxgiDuplicationFormat::Bgra8]).map_err(|e| e.to_string()));
    let mut duplication = match open() {
        Ok(d) => d,
        Err(e) => {
            log::warn!("Latency measurement unavailable: {e}");
            return;
        }
    };
    log::info!("Latency measurement: a line each time the full map opens or closes (key, game picture, HUD recognition, HUD picture, one clock)");
    let (mut game, mut hud, mut drawn): (Option<bool>, Option<bool>, Option<bool>) = (None, None, None);
    let (mut pending, mut pending_marks): (Option<Flip>, Option<Flip>) = (None, None);
    // Staging textures made once: the V1 hint row and the plan panel's marker.
    let mut stages: Option<(StagingTexture, StagingTexture, StagingTexture)> = None;
    // The drag probe's square around the selected marker, and the drag's images so far.
    let mut drag_stage: Option<StagingTexture> = None;
    let mut drag_samples: Vec<DragSample> = Vec::new();
    let mut scratch = Vec::new();
    // Diagnostics: frames seen and the last readings, every 5 s; each kind of failure once.
    let mut frames = 0u32;
    let mut failures: Vec<String> = Vec::new();
    let mut fail = |what: String| {
        if !failures.contains(&what) {
            log::warn!("Latency measurement: {what}");
            failures.push(what);
        }
    };
    loop {
        let mut frame = match duplication.acquire_next_frame(100) {
            Ok(f) => f,
            Err(Error::Timeout) => continue,
            Err(Error::AccessLost) => {
                drop(duplication);
                stages = None;
                match open() {
                    Ok(d) => duplication = d,
                    Err(e) => {
                        log::warn!("Latency measurement stopped: {e}");
                        return;
                    }
                }
                continue;
            }
            Err(e) => {
                log::warn!("Latency measurement: {e}");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let present = frame.frame_info().LastPresentTime;
        if present == 0 {
            continue;
        }
        let t = present_at(present);
        let (client, key, detected, drag) = {
            let s = shared.lock().expect("latency lock");
            (s.client, s.key, s.detected, s.drag)
        };
        // A drag on V1: the square around the selected marker in every image.
        match drag {
            Some((cx, cy)) => {
                let (sw, sh) = (frame.texture_desc().Width as i32, frame.texture_desc().Height as i32);
                let side = 2 * DRAG_HALF;
                let (x0, y0) = ((cx - DRAG_HALF).clamp(0, (sw - side).max(0)), (cy - DRAG_HALF).clamp(0, (sh - side).max(0)));
                if drag_stage.is_none() {
                    match StagingTexture::new(frame.device(), side as u32, side as u32, frame.texture_desc().Format) {
                        Ok(st) => drag_stage = Some(st),
                        Err(e) => fail(format!("Drag probe: {e}")),
                    }
                }
                if let Some(stage) = drag_stage.as_mut() {
                    match frame.buffer_crop_with(stage, x0 as u32, y0 as u32, (x0 + side) as u32, (y0 + side) as u32) {
                        Ok(buffer) => {
                            let px = buffer.as_nopadding_buffer(&mut scratch).to_vec();
                            let (game, hud) = drag_sample(&px, side as usize);
                            let offset = |p: Option<(f64, f64)>| p.map(|(x, y)| (x + f64::from(x0), y + f64::from(y0)));
                            drag_samples.push(DragSample { t, game: offset(game), hud: offset(hud) });
                        }
                        Err(e) => fail(format!("Drag probe capture failed: {e}")),
                    }
                }
                continue;
            }
            None if !drag_samples.is_empty() => {
                let arrivals = std::mem::take(&mut shared.lock().expect("latency lock").arrivals);
                report_drag(&std::mem::take(&mut drag_samples), &arrivals);
            }
            None => {}
        }
        // Desktop images are read only from a key until `WINDOW` after it (each read waits on the
        // GPU; at 165 Hz that is over a core): the first image read is the state before the switch.
        if key.is_none_or(|k| k.elapsed() > WINDOW) {
            (game, hud, drawn, pending, pending_marks) = (None, None, None, None, None);
            continue;
        }
        let Some(client) = client else {
            fail("no game window position yet".to_owned());
            continue;
        };
        let (screen_w, screen_h) = (frame.texture_desc().Width as i32, frame.texture_desc().Height as i32);
        let hints = cues.regions(client)[0];
        let mark = (client.0 + plan_offset.0, client.1 + plan_offset.1, 4, 4);
        let marks_mark = (client.0 + marks_offset.0, client.1 + marks_offset.1, 4, 4);
        let inside = |(x, y, w, h): (i32, i32, i32, i32)| x >= 0 && y >= 0 && x + w <= screen_w && y + h <= screen_h;
        if !inside(hints) || !inside(mark) || !inside(marks_mark) {
            fail(format!("region off the screen: screen {screen_w}x{screen_h}, hint row {hints:?}, mark {mark:?} {marks_mark:?}"));
            continue;
        }
        if stages.is_none() {
            let format = frame.texture_desc().Format;
            let make = |w: i32, h: i32| StagingTexture::new(frame.device(), w as u32, h as u32, format);
            match (make(hints.2, hints.3), make(4, 4), make(4, 4)) {
                (Ok(a), Ok(b), Ok(c)) => stages = Some((a, b, c)),
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                    log::warn!("Latency measurement: {e}");
                    return;
                }
            }
        }
        let (hint_stage, mark_stage, marks_stage) = stages.as_mut().expect("stages");
        let format = frame.format();
        let mut crop = |stage: &mut StagingTexture, (x, y, w, h): (i32, i32, i32, i32)| -> Result<Vec<u8>, String> {
            let buffer = frame.buffer_crop_with(stage, x as u32, y as u32, (x + w) as u32, (y + h) as u32).map_err(|e| e.to_string())?;
            Ok(buffer.as_nopadding_buffer(&mut scratch).to_vec())
        };
        let (hint_px, mark_px, marks_px) = match (crop(hint_stage, hints), crop(mark_stage, mark), crop(marks_stage, marks_mark)) {
            (Ok(a), Ok(b), Ok(c)) => (a, b, c),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                fail(format!("capture failed: {e}"));
                continue;
            }
        };
        if hint_px.len() != (hints.2 * hints.3 * 4) as usize {
            fail(format!("not an 8-bit format: {format:?}, hint row {} bytes", hint_px.len()));
            continue;
        }
        drop(frame);
        let view = FrameView { data: &hint_px, format: PixelFormat::Bgra, origin: (hints.0, hints.1), width: hints.2 as u32, height: hints.3 as u32, stride: hints.2 as usize * 4 };
        let game_now = cues.read_views(std::slice::from_ref(&view), client).shows(MapView::Full);
        // BGRA. Measured on screen (248, 81, 246): the panel is composited, so magenta is red and
        // blue high and green well under both.
        let magenta = |px: &[u8]| px.as_chunks::<4>().0.iter().all(|p| p[0] > 180 && p[2] > 180 && u16::from(p[1]) + 90 < u16::from(p[0].min(p[2])));
        let (hud_now, drawn_now) = (magenta(&mark_px), magenta(&marks_px));
        frames += 1;
        if game != Some(game_now) || hud != Some(hud_now) {
            let ping = cues.read_views(std::slice::from_ref(&view), client).ping_score;
            log::info!(
                "Probe: {frames} frames, {:.0} ms after the key, game V1 {game_now} (Ping score {ping:.2}), HUD panel {hud_now} (mark pixels {:?})",
                key.map_or(f64::NAN, |k| ms(k, t)),
                &mark_px[..4]
            );
        }
        if game.is_some_and(|g| g != game_now) {
            // A key within the last second is this flip's.
            let key = key.filter(|&k| k <= t && t.duration_since(k) < Duration::from_secs(1));
            pending = Some(Flip { open: game_now, game: t, key, detected: None });
            pending_marks = Some(Flip { open: game_now, game: t, key, detected: None });
        }
        if drawn.is_some_and(|d| d != drawn_now)
            && let Some(flip) = pending_marks.take_if(|f| f.open == drawn_now)
        {
            let what = if flip.open { "V1 open" } else { "V1 closed" };
            match flip.key {
                Some(k) => log::info!("Marks {what}: key to game picture {:.0} ms, key to marks picture {:.0} ms; marks {:.0} ms behind the game", ms(k, flip.game), ms(k, t), ms(flip.game, t)),
                None => log::info!("Marks {what} (no key): marks {:.0} ms behind the game", ms(flip.game, t)),
            }
        }
        drawn = Some(drawn_now);
        game = Some(game_now);
        if let Some(flip) = pending.as_mut()
            && let Some((open, at)) = detected
            && open == flip.open
            && at + Duration::from_millis(5) >= flip.key.unwrap_or(flip.game)
        {
            flip.detected.get_or_insert(at);
        }
        if hud.is_some_and(|h| h != hud_now)
            && let Some(flip) = pending.take_if(|f| f.open == hud_now)
        {
            let what = if flip.open { "V1 open" } else { "V1 closed" };
            let line = match flip.key {
                Some(k) => format!(
                    "{what}: key to game picture {:.0} ms, key to HUD recognition {}, key to HUD picture {:.0} ms; HUD {:.0} ms behind the game",
                    ms(k, flip.game),
                    flip.detected.map_or("—".to_owned(), |d| format!("{:.0} ms", ms(k, d))),
                    ms(k, t),
                    ms(flip.game, t)
                ),
                None => format!("{what} (no key): HUD picture {:.0} ms behind the game", ms(flip.game, t)),
            };
            log::info!("{line}");
        }
        hud = Some(hud_now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a recorded V1 frame of Rohkstov (A3's marker point at (1097.1, 938.7) in the frame) with
    /// a magenta dot painted where the marks window draws it: the dot and the game's diamond centre
    /// are found where they are.
    #[test]
    fn drag_sample_finds_the_diamond_and_the_dot() {
        let path = mtc_dataset::datasets_dir().join("raw/20261117-160854/frames/000168962.png");
        let Ok(image) = mtc_dataset::image::RgbImage::load_png(&path) else { return };
        let (cx, cy) = (1097, 939);
        let side = 2 * DRAG_HALF;
        let (x0, y0) = (cx - DRAG_HALF, cy - DRAG_HALF);
        let mut px = vec![0u8; (side * side * 4) as usize];
        for y in 0..side {
            for x in 0..side {
                let j = (((y0 + y) as u32 * image.width + (x0 + x) as u32) * 3) as usize;
                let i = ((y * side + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[image.data[j + 2], image.data[j + 1], image.data[j], 255]);
            }
        }
        // The dot on the diamond centre (marker point + (-1.1, -0.7)), 4 px square.
        let dot = (1096.0 - f64::from(x0), 938.0 - f64::from(y0));
        for y in (dot.1 as usize - 2)..(dot.1 as usize + 2) {
            for x in (dot.0 as usize - 2)..(dot.0 as usize + 2) {
                let i = (y * side as usize + x) * 4;
                px[i..i + 4].copy_from_slice(&[255, 0, 255, 255]);
            }
        }
        let (game, hud) = drag_sample(&px, side as usize);
        let hud = hud.expect("dot");
        let game = game.expect("diamond");
        assert!((hud.0 - dot.0).hypot(hud.1 - dot.1) <= 1.0, "{hud:?} {dot:?}");
        assert!((game.0 - dot.0).hypot(game.1 - dot.1) <= 2.0, "{game:?} {dot:?}");
    }
}
