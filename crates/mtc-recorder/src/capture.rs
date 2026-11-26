//! Capture callback: decides per frame whether it is needed and reads back only those frames.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::Sender;
use mtc_capture::readback::Readback;
use mtc_capture::window::{WindowGeometry, cursor_position, is_foreground};
use mtc_capture::windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use mtc_capture::windows_capture::frame::Frame;
use mtc_capture::windows_capture::graphics_capture_api::InternalCaptureControl;
use mtc_capture::windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use mtc_capture::windows_capture::window::Window;
use mtc_dataset::SaveReason;
use mtc_vision::map::{CueReader, MapView};
use mtc_vision::view::{FrameView, PixelFormat};

use crate::writer::{Captured, Packet};
use crate::{Args, Control};

pub struct Flags {
    window: Window,
    control: Arc<Control>,
    sender: Sender<Packet>,
    args: Args,
    started_at: Instant,
}

struct Handler {
    flags: Flags,
    readback: Readback,
    geometry: Option<WindowGeometry>,
    interval_ms: u64,
    next_rate_t: u64,
    next_periodic_t: u64,
    next_map_t: u64,
    cues: CueReader,
    follow_until_t: u64,
    mark_index: u32,
    clock: Option<(i64, u64)>,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = Flags;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let interval_ms = (1000.0 / ctx.flags.args.rate).round() as u64;
        Ok(Self {
            geometry: WindowGeometry::query(&ctx.flags.window).ok(),
            flags: ctx.flags,
            readback: Readback::new(),
            interval_ms,
            next_rate_t: 0,
            next_periodic_t: 0,
            next_map_t: 0,
            cues: CueReader::new(),
            follow_until_t: 0,
            mark_index: 0,
            clock: None,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let control = &self.flags.control;
        if control.stop.load(Ordering::SeqCst) {
            capture_control.stop();
            return Ok(());
        }
        control.captured.fetch_add(1, Ordering::Relaxed);

        // Session time from the frame's own timestamp (100 ns units).
        let stamp = frame.timestamp()?.Duration;
        let (stamp0, base_ms) = *self
            .clock
            .get_or_insert((stamp, self.flags.started_at.elapsed().as_millis() as u64));
        let t_ms = base_ms + ((stamp - stamp0).max(0) / 10_000) as u64;

        let args = &self.flags.args;
        if control.mark.swap(false, Ordering::SeqCst) {
            self.mark_index += 1;
            self.follow_until_t = t_ms + (args.follow * 1000.0) as u64;
            self.flags.sender.send(Packet::Mark { index: self.mark_index })?;
            println!("mark #{} @ {:.1}s", self.mark_index, t_ms as f64 / 1000.0);
        }

        let mut save = None;
        let mut mark = None;
        let mut history = false;
        let rate_tick = t_ms >= self.next_rate_t;
        if rate_tick {
            self.next_rate_t = (self.next_rate_t + self.interval_ms).max(t_ms);
            history = args.history > 0.0;
            if self.mark_index > 0 && t_ms <= self.follow_until_t {
                save = Some(SaveReason::MarkFollow);
                mark = Some(self.mark_index);
            } else if control.continuous.load(Ordering::Relaxed) {
                save = Some(SaveReason::Continuous);
            }
        }
        if args.periodic > 0.0 && t_ms >= self.next_periodic_t {
            self.next_periodic_t = t_ms + (args.periodic * 1000.0) as u64;
            save.get_or_insert(SaveReason::Periodic);
        }
        if control.snapshot.swap(false, Ordering::SeqCst) {
            save = Some(SaveReason::Snapshot);
        }
        // Map views are checked at `--rate` on the frames read back for the history anyway.
        let map_check = args.map_rate > 0.0 && rate_tick && t_ms >= self.next_map_t;
        if !history && save.is_none() && !map_check {
            return Ok(());
        }

        if let Ok(geometry) = WindowGeometry::query(&self.flags.window) {
            self.geometry = Some(geometry);
        }
        let Some(geometry) = self.geometry else {
            return Ok(());
        };

        let started = Instant::now();
        let mut pixels = Vec::new();
        self.readback.read_full(frame, &mut pixels)?;
        control
            .readback_us
            .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        control.readbacks.fetch_add(1, Ordering::Relaxed);

        if map_check && save.is_none() {
            let view = FrameView {
                data: &pixels,
                format: PixelFormat::Bgra,
                origin: (0, 0),
                width: frame.width(),
                height: frame.height(),
                stride: frame.width() as usize * 4,
            };
            let client = (geometry.client.x - geometry.frame.x, geometry.client.y - geometry.frame.y);
            if matches!(self.cues.read(&view, client).primary(), Some(MapView::Full | MapView::Scope)) {
                save = Some(SaveReason::Map);
                self.next_map_t = t_ms + (1000.0 / args.map_rate) as u64;
                control.map_frames.fetch_add(1, Ordering::Relaxed);
            }
        }
        if !history && save.is_none() {
            return Ok(());
        }
        let cursor = cursor_position().map(|(x, y)| geometry.screen_to_frame(x, y));
        self.flags.sender.send(Packet::Frame(Captured {
            t_ms,
            unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or_default(),
            width: frame.width(),
            height: frame.height(),
            pixels,
            geometry,
            cursor,
            foreground: is_foreground(&self.flags.window),
            history,
            save,
            mark,
        }))?;
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Captures `window` until it closes or the user stops the recorder.
pub fn run(
    window: Window,
    args: &Args,
    control: Arc<Control>,
    sender: Sender<Packet>,
    started_at: Instant,
) {
    let settings = Settings::new(
        window,
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Custom(Duration::from_secs_f64(0.5 / args.rate)),
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        Flags {
            window,
            control: control.clone(),
            sender,
            args: args.clone(),
            started_at,
        },
    );
    let capture = match Handler::start_free_threaded(settings) {
        Ok(capture) => capture,
        Err(error) => {
            eprintln!("Capture failed to start: {error}");
            std::thread::sleep(Duration::from_secs(2));
            return;
        }
    };

    let mut last_report = Instant::now();
    let mut last_captured = 0;
    while !capture.is_finished() {
        if control.stop.load(Ordering::SeqCst) {
            let _ = capture.stop();
            return;
        }
        if last_report.elapsed() >= Duration::from_secs(15) {
            let captured = control.captured.load(Ordering::Relaxed);
            let readbacks = control.readbacks.load(Ordering::Relaxed).max(1);
            println!(
                "capture {:.0} fps · saved {} frames (map {}) {:.0} MB · history in memory {:.0} MB · readback {:.1} ms/frame{}",
                (captured - last_captured) as f64 / last_report.elapsed().as_secs_f64(),
                control.saved.load(Ordering::Relaxed),
                control.map_frames.load(Ordering::Relaxed),
                control.saved_bytes.load(Ordering::Relaxed) as f64 / 1e6,
                control.history_bytes.load(Ordering::Relaxed) as f64 / 1e6,
                control.readback_us.load(Ordering::Relaxed) as f64 / readbacks as f64 / 1000.0,
                if control.continuous.load(Ordering::Relaxed) { " · recording" } else { "" },
            );
            last_captured = captured;
            last_report = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if let Err(error) = capture.wait() {
        eprintln!("Capture ended: {error}");
    }
}
