//! MTC HUD: live angle readout, map, player position and targets, loading state, firing
//! solutions and the aim strip.
//!
//! Captures the Roblox window with Windows Graphics Capture and reads back only small regions:
//! the angle readout box and the map-view cues every processed frame, the container of the map
//! view on screen at that view's rate, and the loading UI region at its rate. Every 5 s it prints
//! throughput, per-stage cost, capture-to-result latency and this process's CPU and memory use.
// Release builds have no console window: the output goes to the log file.
#![cfg_attr(feature = "gui", windows_subsystem = "windows")]

mod diag;
mod fire;
mod game_ref;
mod latency;
mod loading;
mod locator;
mod logfile;
mod maps;
mod marks;
mod plan;
mod prefs;
mod retry;
mod seats;
mod shots;
mod stats;
mod tray;
mod ui_text;
mod worker;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mtc_capture::readback::{Readback, Region};
use mtc_capture::window::{WindowGeometry, cursor_position, enable_per_monitor_dpi_awareness, find_roblox, foreground_handle, is_foreground};
use mtc_capture::windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use mtc_capture::windows_capture::frame::Frame;
use mtc_capture::windows_capture::graphics_capture_api::InternalCaptureControl;
use mtc_capture::windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use mtc_capture::windows_capture::window::Window;
use mtc_vision::aim::{AimReader, AimReading, AimResult};
use mtc_vision::layout::{Placement, UiLayout, picture_shift, shift_probes};
use mtc_vision::map::CueReader;
use mtc_overlay::{
    AimRow, AimStrip, Axis, ChargeDot, ChargeMarks, DragMark, Overlay, PANEL_WIDTH, PanelAction, PanelArc, PanelLoad, PanelSolution, PanelState, PanelTarget,
    PlanState, hud_right, loader_panel_anchor, panel_anchor,
};
use mtc_vision::{FrameView, PixelFormat};
use stats::Stats;

#[global_allocator]
static GLOBAL: stats::CountingAlloc = stats::CountingAlloc;

/// Frames closer together than this are not processed (the readout changes at human speed),
/// except while V1 moves or a marker on it is dragged: then every captured frame, which come at
/// most every `CAPTURE_INTERVAL`.
const PROCESS_INTERVAL: Duration = Duration::from_millis(33);
/// The plan panel's distance right of the full map (px): clear of the map marks window, which
/// extends 60 px past the map for marker text.
const PLAN_GAP: i32 = 64;
/// The plan panel keeps this far (px) inside the game window's right and bottom edges.
const PLAN_MARGIN: i32 = 8;
/// Frames are delivered at most this often: about every frame the game draws (it redrew its map
/// every ~14 ms while V1 was dragged; at 16 ms the HUD saw 50 of them a second), so the arrivals
/// tell the game's frame interval. Processing keeps its own pace.
const CAPTURE_INTERVAL: Duration = Duration::from_millis(4);
/// While V1 pans (the overlay moves the marks with the cursor between frames) frames are
/// processed this often at most; a marker drag or a view key is followed every frame.
const PAN_PROCESS_INTERVAL: Duration = Duration::from_millis(12);
/// With no angle readout and no map view on screen (menus, other games) frames are processed
/// this often, and the wide search for an absent readout runs at this rate too.
const IDLE_INTERVAL: Duration = Duration::from_millis(200);

/// A charge mark sits this far (px) inside a card's right border, on the row's middle.
const CHARGE_DOT_INSET: f64 = 14.0;
/// A dragged panel keeps at least this much of itself (px) inside the game window.
const PANEL_KEEP_IN: i32 = 60;

/// Which seat's panel is on screen (each keeps its own dragged position).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PanelSeat {
    Gunner,
    Loader,
}

/// Development snapshots (`--snapshots`) at most this often, and while V1 zooms or pans.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(2);
const SNAPSHOT_MOVING: Duration = Duration::from_millis(200);
/// Snapshots in one run at most (a frame is 3–6 MB).
const SNAPSHOT_MAX: u32 = 1_500;

/// Running hidden (`--hidden`): the overlay shows nothing; snapshots keep their slow rate.
fn hidden() -> bool {
    mtc_overlay::HIDDEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// Saves the primary monitor as composited (game plus overlay windows).
fn snapshot(path: &std::path::Path) {
    use mtc_capture::windows_capture::dxgi_duplication_api::DxgiDuplicationApi;
    use mtc_capture::windows_capture::encoder::ImageFormat;
    use mtc_capture::windows_capture::monitor::Monitor;
    let Ok(monitor) = Monitor::primary() else { return };
    let Ok(mut duplication) = DxgiDuplicationApi::new(monitor) else { return };
    for _ in 0..3 {
        if let Ok(mut frame) = duplication.acquire_next_frame(200)
            && let Ok(mut buffer) = frame.buffer()
            && buffer.save_as_image(path, ImageFormat::Png).is_ok()
        {
            log::info!("Snapshot {}", path.display());
            return;
        }
    }
}

/// Keys read (virtual-key codes): M (map), X (sight), F (fire), 1–6 (loading). Mouse buttons and
/// wheel are read too. Used only while Roblox has focus; nothing is kept.
const INPUT_KEYS: [u16; 9] = [VK_M, VK_X, VK_F, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36];
/// `--follow-tune` (development): [ and ] take the cursor for a dragged map's marks 2 ms later
/// (ahead) or earlier (further back), to set it by eye.
const VK_OEM_4: u16 = 0xDB;
const VK_OEM_6: u16 = 0xDD;
const FOLLOW_TUNE_STEP_US: i64 = 2_000;
const VK_M: u16 = 0x4D;
const VK_X: u16 = 0x58;
/// F, the game's only fire key: its press starts the shot check.
const VK_F: u16 = 0x46;
/// After F, frames are processed without waiting this long, for the screen to confirm the shot.
const FIRE_KEY_WINDOW: Duration = Duration::from_millis(1500);
/// After M (map) or X (sight) goes down, every captured frame is processed this long, so a view
/// change is seen on the first frame that has it (input says when to look; the screen says what
/// the view is).
const VIEW_KEY_WINDOW: Duration = Duration::from_millis(600);
/// From M to the game's frame with the new view before any press was measured (ms; measured
/// in play: processed 44–74 ms after the key, the frame arriving at once).
const VIEW_SWITCH_DEFAULT_MS: f64 = 40.0;
/// The menu marks switch this much (ms) before the game's frame is due: a composition later they
/// are on screen with it.
const VIEW_SWITCH_LEAD_MS: f64 = 5.0;
/// A V1 click: press and release this close (px) and this quick; on a marker point within
/// `CLICK_MARKER_PX` (its ~30 px diamond), on an objective icon within `CLICK_OBJECTIVE_PX`.
const CLICK_MAX_MOVE: i32 = 4;
const CLICK_MAX_TIME: Duration = Duration::from_millis(400);
const CLICK_MARKER_PX: f32 = 16.0;
/// With the game's "Remove marker" hint showing, the marker pressed is the nearest within this
/// (px): its hit area takes in the label beside the diamond.
const CLICK_MARKER_HINTED_PX: f32 = 48.0;
/// A "Remove marker" deletion counts within this long after a right click.
const RIGHT_CLICK_WINDOW: Duration = Duration::from_millis(1500);
const CLICK_OBJECTIVE_PX: f32 = 20.0;

struct Flags {
    window: Window,
    stop: Arc<AtomicBool>,
    overlay: Arc<Overlay>,
    /// Elevation and azimuth given on the command line, used while there is no firing solution.
    manual: Option<(f64, f64)>,
    /// `--mirror`: the strip shows the readout's own text, to compare the renderings.
    mirror: bool,
    /// `--follow-tune`: [ and ] set the marks' follow delay.
    follow_tune: bool,
    /// `--snapshots <dir>` (development): the composited screen every `SNAPSHOT_INTERVAL` while
    /// Roblox has focus and a map view or the sight is shown, to check the HUD in play.
    snapshots: Option<std::path::PathBuf>,
    /// `--v1-log <file>` (development): every V1 measurement and map input as CSV, to fit the
    /// game's zoom and pan.
    v1_log: Option<std::path::PathBuf>,
    /// `--arrow-log <file>` (development): every sighting of the player's arrow as CSV (time, view,
    /// zoom, map position, heading, in a vehicle), to measure how steady and how true it is.
    arrow_log: Option<std::path::PathBuf>,
    /// `--latency` (development): what the screen probe needs from the loop (`latency.rs`).
    latency: Option<Arc<std::sync::Mutex<latency::Shared>>>,
}

/// The V1 model log: one line per input event and per processed V1 frame, written unbuffered
/// (the HUD is often stopped by killing it). Times are ms since the HUD started; positions are
/// frame px.
struct V1Log {
    file: std::fs::File,
    logged: Option<Instant>,
}

impl V1Log {
    fn create(path: &std::path::Path) -> std::io::Result<Self> {
        use std::io::Write;
        let mut file = std::fs::File::create(path)?;
        writeln!(file, "kind,t_ms,x,y,left,notches,source,pairs,zoom,u0,v0,drawn_zoom,drawn_u0,drawn_v0,canvas_zoom,canvas_u0,canvas_v0,map_left,map_top")?;
        Ok(Self { file, logged: None })
    }

    fn line(&mut self, text: std::fmt::Arguments) {
        use std::io::Write;
        let _ = writeln!(self.file, "{text}");
    }
}

struct Hud {
    /// The capture library calls back on its own thread: named on the first frame.
    capture_thread_named: bool,
    flags: Flags,
    readback: Readback,
    /// Aim readout box, then the three cue regions.
    regions: Vec<Region>,
    /// The readout's whole search area, read back from the same frame when its last place fails.
    aim_area: [Region; 1],
    /// The two strips `picture_shift` reads, the shift they last gave, and whether to read them
    /// again (with each geometry query).
    shift_regions: Vec<Region>,
    shift: (i32, i32),
    shift_due: bool,
    /// The configuration the readout reader is set for.
    aim_layout: UiLayout,
    /// The last frame's placement (input handled before this frame's is known).
    placement: Placement,
    /// Screen minus frame coordinates this frame: where the geometry puts the client on screen,
    /// less where the placement found it in the frame (the frame's own offset unless the picture
    /// is shifted).
    screen_offset: (i32, i32),
    /// The aim strip's and the panel's scale for this frame's configuration (`overlay_scales`).
    overlay_scales: (f32, f32),
    map_region: Region,
    loading_region: Region,
    seat_region: Region,
    seats: seats::SeatWatch,
    aim: AimReader,
    cues: CueReader,
    maps: maps::MapPipeline,
    loading: loading::LoadingPipeline,
    /// The loading UI's text reader for the configuration on screen.
    ui_text: ui_text::UiText,
    /// The minimap's and the full map's containers found on screen per configuration.
    locator: locator::Locator,
    fire: fire::FireControl,
    /// Keyboard and mouse events, handled only while Roblox has focus.
    input: Option<mtc_input::Input>,
    /// Events taken from the input thread on frame arrival, handled with the next processed frame.
    pending_input: Vec<mtc_input::InputEvent>,
    /// The last M or X press (virtual-key code, when), until the view changes or the window ends.
    view_key: Option<(u16, Instant)>,
    /// When F last went down (Roblox in front).
    fire_key_at: Option<Instant>,
    /// From M to the game's frame showing the new view (ms), the latest presses: leaving V3, and
    /// back to it (about 10 ms longer).
    view_switch_ms: [std::collections::VecDeque<f64>; 2],
    /// The M press the loading menu's marks already switched for.
    view_predicted: Option<Instant>,
    /// What the view cues showed on the last processed frame (map view, scoped).
    last_view: Option<(Option<mtc_vision::map::MapView>, bool)>,
    /// Left button held: where and when it went down (click selection, drags).
    press: Option<((i32, i32), Instant)>,
    /// The last frame's capture time (ms, system-relative), and the last intervals between frames
    /// (ms): the game's frame interval.
    last_arrival: Option<f64>,
    game_intervals: std::collections::VecDeque<f64>,
    /// The target under the pointer on V1's map, and on the plan panel's list: its row
    /// and its label light up together, the map shows its firing data.
    map_hover: Option<String>,
    panel_hover: Option<String>,
    /// Each shot, its predicted landing and its X.
    shots: shots::ShotLog,
    /// Last right button press (a marker deletion needs one).
    last_right_click: Option<Instant>,
    /// The latest right click on V1 away from any marker (placing one) and where (frame px).
    last_place: Option<(Instant, (f64, f64))>,
    last_snapshot: Option<Instant>,
    snapshots_taken: u32,
    v1_log: Option<V1Log>,
    arrow_log: Option<(std::fs::File, Option<Instant>)>,
    /// The lowest and highest elevation the readout has shown for the current weapon: on a slope or
    /// on built-up ground the gun reaches past its flat-ground range.
    elevation_seen: Option<(String, f64, f64)>,
    /// When the map step totals were last printed (`[map parts]`).
    stats_since: Instant,
    /// When the height layers' pages were last taken out of the working set.
    height_pages_released: Instant,
    /// The last elevation and azimuth read.
    reading: Option<(f64, f64)>,
    prefs: prefs::Prefs,
    /// The settings window is open.
    settings_open: bool,
    /// Diagnostic data: the run's records, sent when the player agreed.
    diag: diag::Diag,
    /// The startup question about sending statistics is up this run, and its "Don't ask again".
    consent_open: bool,
    consent_dont_ask: bool,
    /// A problem report was asked for (made on the next frame, with its screenshots), and what
    /// the last one did.
    report_pending: bool,
    report_note: Option<String>,
    /// The seat whose panel was shown last (where a drop belongs), the place it sits by default
    /// there (screen px) and its scale (a drop is kept as the offset from that place, by it).
    panel_seat: std::cell::Cell<Option<PanelSeat>>,
    panel_default: std::cell::Cell<Option<((i32, i32), f32)>>,
    /// A place saved in client px by older versions, taken as this offset (saved after the frame).
    panel_migrated: std::cell::Cell<Option<(PanelSeat, (f32, f32))>>,
    started: Instant,
    geometry: Option<WindowGeometry>,
    geometry_checked: Instant,
    last_processed: Instant,
    /// Last time the angle readout was searched for or read.
    last_aim: Instant,
    aim_present: bool,
    idle: bool,
    last_text: Option<(String, String)>,
    stats: Stats,
}

impl GraphicsCaptureApiHandler for Hud {
    type Flags = Flags;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let v1_log = ctx.flags.v1_log.as_deref().map(|p| V1Log::create(p).expect("V1 log file"));
        let arrow_log = ctx.flags.arrow_log.as_deref().map(|p| {
            use std::io::Write;
            let mut file = std::fs::File::create(p).expect("arrow log file");
            writeln!(file, "t_ms,view,zoom,u,v,heading_deg,mounted,shape").expect("arrow log header");
            (file, None)
        });
        let keys: Vec<u16> = INPUT_KEYS.iter().copied().chain(ctx.flags.follow_tune.then_some([VK_OEM_4, VK_OEM_6]).into_iter().flatten()).collect();
        let input = mtc_input::Input::spawn(keys).map_err(|e| log::warn!("Input reading unavailable: {e}; shots are recorded from the screen only")).ok();
        let mut loading = loading::LoadingPipeline::new();
        if input.is_some() {
            loading.keys_live();
        }
        Ok(Self {
            capture_thread_named: false,
            geometry: WindowGeometry::query(&ctx.flags.window).ok(),
            flags: ctx.flags,
            readback: Readback::new(),
            regions: Vec::new(),
            aim_area: [Region::default()],
            shift_regions: Vec::new(),
            shift: (0, 0),
            shift_due: true,
            aim_layout: UiLayout::REFERENCE,
            placement: Placement::new(Placement::VIRTUAL_ORIGIN, UiLayout::REFERENCE),
            screen_offset: (0, 0),
            overlay_scales: (1.0, 1.0),
            map_region: Region::default(),
            loading_region: Region::default(),
            seat_region: Region::default(),
            seats: seats::SeatWatch::new(),
            aim: AimReader::new(),
            cues: CueReader::new(),
            maps: {
                let mut maps = forced_map(maps::MapPipeline::new(true));
                // Development (`--measure`): the X is looked for under every objective
                // icon on every map read, so the cost of that search shows outside artillery too.
                if std::env::args().any(|a| a == "--measure") {
                    maps.look_under_every_icon();
                }
                maps
            },
            loading,
            ui_text: ui_text::UiText::new(roblox_font_dir()),
            locator: locator::Locator::new(),
            fire: fire::FireControl::new(true),
            input,
            pending_input: Vec::new(),
            view_key: None,
            fire_key_at: None,
            view_switch_ms: Default::default(),
            view_predicted: None,
            last_view: None,
            press: None,
            last_arrival: None,
            game_intervals: std::collections::VecDeque::new(),
            map_hover: None,
            panel_hover: None,
            shots: shots::ShotLog::default(),
            last_right_click: None,
            last_place: None,
            last_snapshot: None,
            snapshots_taken: 0,
            v1_log,
            arrow_log,
            elevation_seen: None,
            stats_since: Instant::now(),
            height_pages_released: Instant::now(),
            reading: None,
            diag: diag::Diag::new(prefs::Prefs::load().sharing()),
            consent_open: prefs::Prefs::load().ask_consent(),
            consent_dont_ask: false,
            report_pending: false,
            report_note: None,
            prefs: prefs::Prefs::load(),
            settings_open: false,
            panel_seat: std::cell::Cell::new(None),
            panel_default: std::cell::Cell::new(None),
            panel_migrated: std::cell::Cell::new(None),
            started: Instant::now(),
            geometry_checked: Instant::now(),
            last_processed: Instant::now() - PROCESS_INTERVAL,
            last_aim: Instant::now() - IDLE_INTERVAL,
            aim_present: false,
            idle: false,
            last_text: None,
            stats: Stats::new(),
        })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, control: InternalCaptureControl) -> Result<(), Self::Error> {
        let handler_cycles = stats::HandlerCycles::start();
        if self.flags.stop.load(Ordering::SeqCst) {
            control.stop();
            return Ok(());
        }
        if !self.capture_thread_named {
            stats::name_this_thread("capture");
            self.capture_thread_named = true;
        }
        let arrived = Instant::now();
        self.stats.frames_arrived += 1;
        // The game's frame interval from the frames' capture times (one frame per frame it draws,
        // at most one per composition), not from when they reach this callback (which a busy frame
        // loop delays). The drag follow's lead goes by it (`mtc_overlay::follow_lag_us`).
        if let Ok(stamp) = frame.timestamp() {
            mtc_overlay::GAME_FRAME_STAMP.store(stamp.Duration, Ordering::Relaxed);
        }
        if let Ok(stamp) = frame.timestamp()
            && let Some(last) = self.last_arrival.replace(stamp.Duration as f64 / 1e4)
        {
            let gap = stamp.Duration as f64 / 1e4 - last;
            if gap > 0.0 && gap < 200.0 {
                self.stats.game_intervals.push(gap);
                self.game_intervals.push_back(gap);
                while self.game_intervals.len() > 120 {
                    self.game_intervals.pop_front();
                }
                if self.stats.frames_arrived % 30 == 0 {
                    let mut gaps: Vec<f64> = self.game_intervals.iter().copied().collect();
                    gaps.sort_by(f64::total_cmp);
                    let median = gaps[gaps.len() / 2];
                    mtc_overlay::GAME_FRAME_US.store((median * 1000.0).round() as i64, Ordering::Relaxed);
                }
            }
        }
        // Input is taken on every arriving frame: M or X lifts the processing interval at once.
        if let Some(input) = &self.input {
            for event in input.drain() {
                if let mtc_input::InputEvent::Key { vk: vk @ (VK_OEM_4 | VK_OEM_6), down: true, .. } = event
                    && self.flags.follow_tune
                    && is_foreground(&self.flags.window)
                {
                    use std::sync::atomic::Ordering::Relaxed;
                    let step = if vk == VK_OEM_4 { -FOLLOW_TUNE_STEP_US } else { FOLLOW_TUNE_STEP_US };
                    let now = mtc_overlay::FOLLOW_OFFSET_US.load(Relaxed);
                    let next = (now + step).clamp(-60_000, 60_000);
                    mtc_overlay::FOLLOW_OFFSET_US.store(next, Relaxed);
                    // The automatic part is the overlay's (`[drag timing]` lines every 5 s while dragging).
                    log::info!("Drag follow fine-tuning {:+.0} ms ([ earlier, ] later, 2 ms a step; the automatic part in [drag timing])", next as f64 / 1000.0);
                }
                if let mtc_input::InputEvent::Key { vk: VK_F, down: true, at } = event
                    && is_foreground(&self.flags.window)
                {
                    self.loading.fire_key(at.saturating_duration_since(self.started).as_millis() as u64);
                    self.fire_key_at = Some(at);
                }
                // Seats change by clicking the seat menu: read it soon after.
                if let mtc_input::InputEvent::Button { button: mtc_input::MouseButton::Left, down: true, pos, at } = event
                    && is_foreground(&self.flags.window)
                    && self.geometry.is_some()
                {
                    let (fx, fy) = (pos.0 - self.screen_offset.0, pos.1 - self.screen_offset.1);
                    let at_menu = self.placement.frame_to_seats((f64::from(fx) + 0.5, f64::from(fy) + 0.5));
                    self.seats.clicked(at_menu, at.saturating_duration_since(self.started).as_millis() as u64);
                }
                if let mtc_input::InputEvent::Key { vk: vk @ (VK_M | VK_X), down: true, at } = event
                    && is_foreground(&self.flags.window)
                {
                    self.view_key = Some((vk, at));
                    if let Some(l) = &self.flags.latency {
                        l.lock().expect("latency lock").key = Some(at);
                    }
                }
                self.pending_input.push(event);
            }
        }
        let looking = self.view_key.is_some_and(|(_, at)| at.elapsed() < VIEW_KEY_WINDOW) || self.fire_key_at.is_some_and(|at| at.elapsed() < FIRE_KEY_WINDOW);
        let interval = if looking || self.maps.marker_dragging() {
            Duration::ZERO
        } else if self.maps.v1_moving() {
            PAN_PROCESS_INTERVAL
        } else if self.idle {
            IDLE_INTERVAL
        } else {
            PROCESS_INTERVAL
        };
        if self.last_processed.elapsed() < interval {
            self.stats.skipped_work += arrived.elapsed();
            self.stats.part(stats::Part::Skipped, handler_cycles.started());
            return Ok(());
        }
        self.last_processed = Instant::now();
        // How old the picture is on arrival (from the frame's capture time).
        let frame_age = frame.timestamp().ok().map(|t| stats::frame_age_ms(t.Duration));
        if let Some(age) = frame_age {
            self.stats.frame_ages.push(age);
        }
        if self.geometry_checked.elapsed() > Duration::from_secs(1) {
            self.geometry = WindowGeometry::query(&self.flags.window).ok().or(self.geometry);
            self.geometry_checked = Instant::now();
            self.shift_due = true;
        }
        let Some(geometry) = self.geometry else {
            return Ok(());
        };
        let frame_size = (frame.width(), frame.height());
        // A restored window's picture can sit up-left of its geometry: told by the black it
        // leaves at the frame's bottom and right, with each geometry query.
        if std::mem::take(&mut self.shift_due) && frame_size.0 as i32 > geometry.client.width {
            self.shift_regions.resize_with(2, Region::default);
            self.readback.read_regions(frame, &shift_probes(frame_size), &mut self.shift_regions)?;
            if let Some(shift) = picture_shift(&region_view(&self.shift_regions[0]), &region_view(&self.shift_regions[1]))
                && shift != self.shift
            {
                log::info!("The picture is ({}, {}) px up-left of the window geometry", shift.0, shift.1);
                self.shift = shift;
            }
        }
        let mut placement = placement_of(&geometry, frame_size.0 as i32, self.shift);
        placement.located = self.locator.located(placement.layout);
        let origin = placement.origin;
        self.screen_offset = (geometry.client.x - origin.0, geometry.client.y - origin.1);
        self.placement = placement;
        self.overlay_scales = placement.layout.overlay_scales();
        if placement.layout != self.aim_layout {
            self.aim_layout = placement.layout;
            self.aim.set_scale(self.aim_layout.readout_text());
            self.cues = CueReader::for_placement(&placement);
            self.maps.set_placement(&placement);
            let l = self.aim_layout;
            let mapped = if placement.is_reference() { "the reference configuration" } else { "read mapped to the reference geometry, the HUD placed for it" };
            log::info!("Display configuration: {}x{}, {}% ({mapped})", l.width, l.height, l.dpi * 100 / 96);
        }
        let started = Instant::now();
        let cyc = self.stats.part(stats::Part::Top, handler_cycles.started());
        if let Some(l) = &self.flags.latency {
            l.lock().expect("latency lock").client = Some((geometry.client.x, geometry.client.y));
        }
        // The readout is searched for (a wide strip) at the idle rate while absent; once found, only
        // the columns around it are read back.
        let read_aim = self.aim_present || self.last_aim.elapsed() >= IDLE_INTERVAL;
        let near = read_aim && self.aim.knows_place();
        // The readout's region as the reader sees it (at the placement's `bottom_origin`) and the
        // frame rectangle read for it.
        let bottom = placement.bottom_origin();
        let aim_rect = if read_aim { self.aim.wanted_region(bottom) } else { (0, 0, 0, 0) };
        let (x, y, w, h) = if read_aim { placement.bottom_source(aim_rect, frame_size) } else { (0, 0, 0, 0) };
        let mut rects = vec![(x, y, w as u32, h as u32)];
        // The cues where this configuration puts them (`CueReader::for_placement`).
        rects.extend(self.cues.regions(origin).iter().map(|&(x, y, w, h)| (x, y, w as u32, h as u32)));
        // Strips round the map containers not found on screen yet (`locator`).
        let probes_from = rects.len();
        rects.extend(self.locator.wanted(placement.layout).into_iter().map(|(x, y, w, h)| (x + origin.0, y + origin.1, w as u32, h as u32)));
        self.regions.resize_with(rects.len(), Region::default);
        self.readback.read_regions(frame, &rects, &mut self.regions)?;
        let read_back = started.elapsed();
        let mut cyc = self.stats.part(stats::Part::Readback, cyc);

        let views: Vec<FrameView> = self.regions.iter().map(region_view).collect();
        let decode_start = Instant::now();
        let mut area_bytes = 0;
        let result = if read_aim {
            self.last_aim = Instant::now();
            let remapped = (!placement.is_reference()).then(|| placement.bottom_remap(&views[0], aim_rect));
            cyc = self.stats.part(stats::Part::Remap, cyc);
            let aim_view = remapped.as_ref().map_or(views[0], |r| r.view());
            let r = match near.then(|| self.aim.read_near(&aim_view, bottom)).flatten() {
                Some(r) => r,
                None if near => {
                    // Not at its last place: the whole area, from the same frame.
                    let area = self.aim.wanted_region(bottom);
                    let (x, y, w, h) = placement.bottom_source(area, frame_size);
                    self.readback.read_regions(frame, &[(x, y, w as u32, h as u32)], &mut self.aim_area)?;
                    area_bytes = self.aim_area[0].data.len();
                    let remapped = (!placement.is_reference()).then(|| placement.bottom_remap(&region_view(&self.aim_area[0]), area));
                    self.aim.read(&remapped.as_ref().map_or(region_view(&self.aim_area[0]), |r| r.view()), bottom)
                }
                None => self.aim.read(&aim_view, bottom),
            };
            self.aim_present = !matches!(r, AimResult::Absent);
            r
        } else {
            AimResult::Absent
        };
        let cues = self.cues.read_views(&views[1..probes_from], origin);
        // A container found: the cues and the map views from it on.
        if views.len() > probes_from && self.locator.take(&views[probes_from..], origin) {
            placement.located = self.locator.located(placement.layout);
            self.cues = CueReader::for_placement(&placement);
            self.maps.set_placement(&placement);
        }
        let cyc = self.stats.part(stats::Part::Decode, cyc);
        self.idle = !self.aim_present && cues.primary().is_none();
        let view = (cues.primary(), cues.scoped());
        let full = |v: (Option<mtc_vision::map::MapView>, bool)| v.0 == Some(mtc_vision::map::MapView::Full);
        let mini = |v: (Option<mtc_vision::map::MapView>, bool)| v.0 == Some(mtc_vision::map::MapView::Mini);
        // Back on V3 from V1 or V2 (not from nothing: a respawn or a loading screen), or left it.
        let v1_or_v2 = |v: (Option<mtc_vision::map::MapView>, bool)| matches!(v.0, Some(mtc_vision::map::MapView::Full | mtc_vision::map::MapView::Scope));
        let back_on_v3 = mini(view) && self.last_view.is_some_and(v1_or_v2);
        let left_v3 = v1_or_v2(view) && self.last_view.is_some_and(mini);
        if let (Some(l), Some(last)) = (&self.flags.latency, self.last_view)
            && full(last) != full(view)
        {
            l.lock().expect("latency lock").detected = Some((full(view), started));
        }
        if let Some(last) = self.last_view.filter(|&v| v != view)
            && let Some((vk, at)) = self.view_key.take()
        {
            let key = if vk == VK_M { "M" } else { "X" };
            let processed = started.saturating_duration_since(at).as_secs_f64() * 1000.0;
            // When the game's frame with the new view was composed, from the key.
            let shown = frame_age.map(|age| processed - age).filter(|&ms| ms > 0.0 && ms < VIEW_KEY_WINDOW.as_secs_f64() * 1000.0);
            if let Some(ms) = shown.filter(|_| vk == VK_M) {
                let times = &mut self.view_switch_ms[usize::from(mini(view))];
                times.push_back(ms);
                while times.len() > 20 {
                    times.pop_front();
                }
            }
            let shown = shown.map_or(String::new(), |ms| format!(", the picture changed {ms:.0} ms after the key"));
            log::info!("View {} -> {}: handled {processed:.0} ms after {key}{shown}", view_name(last), view_name(view));
        }
        if let Some((vk, _)) = self.view_key.filter(|&(_, at)| at.elapsed() >= VIEW_KEY_WINDOW) {
            log::info!("No view change within {} ms of {} ({})", if vk == VK_M { "M" } else { "X" }, VIEW_KEY_WINDOW.as_millis(), view_name(view));
            self.view_key = None;
        }
        self.last_view = Some(view);
        // M pressed and the view not changed yet (a change takes the key above): the loading
        // menu's marks go with the view to come. Off V3 at once (the menu went 49–65 ms after the
        // key, with the view; marks removed a little early look in step, removed late they lag);
        // back to V3 when the game's frame shows it, measured on earlier presses. The screen
        // decides once it changes, or 600 ms on, unchanged.
        let m_pressed = self.view_key.filter(|&(vk, at)| vk == VK_M && at.elapsed() < VIEW_KEY_WINDOW).map(|(_, at)| at);
        let due_ms = if mini(view) { 0.0 } else { self.view_switch_due_ms(true) };
        let switch_due = m_pressed.filter(|at| at.elapsed().as_secs_f64() * 1000.0 >= due_ms);
        let menu_view = if switch_due.is_some() { v1_or_v2(view) } else { mini(view) };
        let decoded = decode_start.elapsed();
        let bytes = self.regions.iter().map(|r| r.data.len()).sum::<usize>() + area_bytes;
        self.stats.record(read_back, decoded, bytes, started.elapsed().as_secs_f64() * 1000.0, &result);
        if let (AimResult::Read(r), Some(weapon)) = (&result, self.loading.weapon()) {
            self.elevation_seen = Some(match self.elevation_seen.take() {
                Some((w, lo, hi)) if w == weapon => (w, lo.min(r.elevation), hi.max(r.elevation)),
                _ => (weapon.to_owned(), r.elevation, r.elevation),
            });
        }
        // The readout the landing prediction is made from: kept through an unreadable
        // frame (a muzzle flash over the digits).
        self.reading = match &result {
            AimResult::Read(r) => Some((r.elevation, r.azimuth)),
            AimResult::Unreadable { .. } => self.reading,
            AimResult::Absent => None,
        };
        self.fire.predict(self.reading);
        self.fire.set_rule(self.prefs.charge_rule, self.reading.map(|r| r.0));

        let now_ms = self.started.elapsed().as_millis() as u64;
        if let Some(at) = switch_due.filter(|&at| self.view_predicted != Some(at)) {
            self.view_predicted = Some(at);
            let marks_before = self.loading.menu_cards(now_ms).is_some();
            let key_ms = at.saturating_duration_since(self.started).as_millis() as u64;
            if mini(view) {
                self.loading.left_v3(now_ms);
                if marks_before {
                    self.loading.m_on_v3(key_ms);
                }
            } else if v1_or_v2(view) {
                self.loading.back_on_v3(now_ms);
            }
            let elapsed = at.elapsed().as_secs_f64() * 1000.0;
            if marks_before && mini(view) {
                log::info!("Loading menu marks: removed {elapsed:.0} ms after M");
            } else if self.loading.menu_cards(now_ms).is_some() {
                let due = self.view_switch_due_ms(true) + VIEW_SWITCH_LEAD_MS;
                log::info!("Loading menu marks: drawn {elapsed:.0} ms after M (the picture is back on V3 after about {due:.0} ms, as measured)");
            }
        }
        if left_v3 {
            self.loading.left_v3(now_ms);
        }
        if back_on_v3 {
            self.loading.back_on_v3(now_ms);
        }
        self.maps.see(&cues, now_ms);
        let v1 = cues.shows(mtc_vision::map::MapView::Full);
        self.handle_input(&placement, cues.remove_hint(), v1);
        self.map_hover = if v1 { self.target_under_pointer(&placement) } else { None };
        if !v1 {
            self.panel_hover = None;
        }
        // The panels follow the view before the map work below (a V1 registration takes 25–50
        // ms): measured on screen, the plan panel showed 72 ms after V1 was seen when it waited
        // (`--latency`). The panels show the solutions as of the last frame meanwhile.
        let cyc = self.stats.part(stats::Part::Logic, cyc);
        self.update_overlay(&geometry, &placement, &result, now_ms, cues.scoped(), v1);
        if let Some((seat, offset)) = self.panel_migrated.take() {
            self.set_panel_offset(seat, offset);
        }
        // One settings window, centred on the game, at the screen's display scaling like
        // an ordinary program (at most its 150 % size); the startup question likewise.
        let centre = (geometry.client.x + geometry.client.width / 2, geometry.client.y + geometry.client.height / 2);
        let window_scale = (placement.layout.dpi as f32 / 144.0).min(1.0);
        self.flags.overlay.show_settings(self.settings_open.then(|| self.prefs.settings_state(centre, window_scale, self.report_note.clone())));
        self.flags.overlay.show_consent(self.consent_open.then_some(mtc_overlay::ConsentState { centre, dont_ask: self.consent_dont_ask, scale: window_scale }));
        if self.report_pending {
            self.report_pending = false;
            self.report_note = Some(match self.problem_report(frame, &placement, frame_size) {
                Ok(dir) => {
                    let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                    "Saved; the folder is open. Attach its files to your report.".to_owned()
                }
                Err(e) => format!("Could not save the report: {e}"),
            });
        }
        // The marks likewise: on the view now on screen with the last registration (a reopened V1
        // keeps its canvas), drawn again below if the map work moves them. While V1 moves (the
        // canvas follows the input) they are drawn here only, before the map work (2–4 ms), so
        // they reach the screen with the game's step; again below only if the work rebased it.
        let rebases_before = self.maps.canvas_rebases();
        self.show_marks(&placement);
        let cyc = self.stats.part(stats::Part::Panels, cyc);
        // Off the reference a view is read where its container is here and resampled onto the
        // reference container.
        let map_origin = placement.bottom_origin();
        let map_due = self.maps.wanted(&cues, map_origin, now_ms);
        self.stats.part(stats::Part::MapCheck, cyc);
        if let Some((view, rect)) = map_due {
            let map_started = Instant::now();
            let c = stats::cycles();
            let (mx, my, mw, mh) = placement.map_source(view, rect, frame_size);
            self.readback.read_region(frame, mx, my, mw as u32, mh as u32, &mut self.map_region)?;
            let c = self.stats.part(stats::Part::MapReadback, c);
            let cursor = cursor_position().map(|(cx, cy)| {
                let (fx, fy) = (cx - self.screen_offset.0, cy - self.screen_offset.1);
                placement.frame_to_map(view, (f64::from(fx) + 0.5, f64::from(fy) + 0.5))
            });
            let input = maps::MapInput {
                dragging: self.press.is_some(),
                right_click: self.input.as_ref().map(|_| self.last_right_click.is_some_and(|t| t.elapsed() <= RIGHT_CLICK_WINDOW)),
                placed: self.last_place.map(|(at, pos)| (at.saturating_duration_since(self.started).as_millis() as u64, pos)),
            };
            let remapped = (!placement.is_reference()).then(|| placement.map_remap(view, &region_view(&self.map_region), rect));
            let c = self.stats.part(stats::Part::Remap, c);
            let map_view = remapped.as_ref().map_or_else(|| region_view(&self.map_region), |r| r.view());
            self.maps.process(&map_view, view, &cues, map_origin, cursor, input, now_ms);
            self.stats.part(stats::Part::Map, c);
            self.stats.map_reads += 1;
            self.stats.map_work += map_started.elapsed();
            if view == mtc_vision::map::MapView::Full {
                self.log_v1_frame(cursor);
            }
            self.log_arrow();
        }
        // Not with no game view on screen (lobby, menus): no loading UI or seat menu there. At
        // another configuration the region is read where the bottom group puts it and resampled
        // to the reference geometry, once its text reader is in.
        if let Some(reader) = self.ui_text.poll(placement.layout) {
            self.loading.set_reader(reader, !placement.is_reference());
        }
        if !self.idle
            && self.ui_text.ready_for(placement.layout)
            && let Some(rect) = self.loading.wanted(bottom, now_ms)
        {
            let t = Instant::now();
            let c = stats::cycles();
            let (lx, ly, lw, lh) = placement.bottom_source(rect, frame_size);
            self.readback.read_region(frame, lx, ly, lw as u32, lh as u32, &mut self.loading_region)?;
            let c = self.stats.part(stats::Part::LoadingReadback, c);
            let remapped = (!placement.is_reference()).then(|| placement.bottom_remap(&region_view(&self.loading_region), rect));
            let c = self.stats.part(stats::Part::Remap, c);
            let view = remapped.as_ref().map_or_else(|| region_view(&self.loading_region), |r| r.view());
            self.loading.process(&view, bottom, label_x(&result), cues.shows(mtc_vision::map::MapView::Full), now_ms);
            let [texts, weapon, decide] = std::mem::take(&mut self.loading.cost);
            self.stats.part_less(stats::Part::Loading, c, texts.0 + weapon.0 + decide.0);
            self.stats.add(stats::Part::LoadingTexts, texts);
            self.stats.add(stats::Part::LoadingWeapon, weapon);
            self.stats.add(stats::Part::LoadingDecide, decide);
            self.stats.loading.0 += 1;
            self.stats.loading.1 += t.elapsed();
        }
        // The seat menu, with the seat this frame shows (as the panel decides it). Off the
        // reference its region is read where the bottom-left group puts it and resampled to the
        // reference geometry.
        let c = geometry.client_in_frame();
        let client = placement.seats_client((origin.0, origin.1, c.width as u32, c.height as u32));
        let seat = frame_seat(&result, &self.loading, now_ms);
        if !self.idle
            && let Some((rx, ry, rw, rh)) = self.seats.wanted(client, seat, now_ms)
        {
            let t = Instant::now();
            let c = stats::cycles();
            let rect = (rx, ry, rw as i32, rh as i32);
            let (sx, sy, sw, sh) = placement.seats_source(rect, frame_size);
            self.readback.read_region(frame, sx, sy, sw as u32, sh as u32, &mut self.seat_region)?;
            let c = self.stats.part(stats::Part::SeatReadback, c);
            let remapped = (!placement.is_reference()).then(|| placement.seats_remap(&region_view(&self.seat_region), rect));
            let c = self.stats.part(stats::Part::Remap, c);
            let seat_view = remapped.as_ref().map_or_else(|| region_view(&self.seat_region), |r| r.view());
            self.seats.process(&seat_view, client, seat, now_ms);
            self.stats.part(stats::Part::Seats, c);
            self.stats.seats.0 += 1;
            self.stats.seats.1 += t.elapsed();
        }
        let c = stats::cycles();
        for action in self.flags.overlay.take_actions() {
            match action {
                PanelAction::Select(label) => self.fire.select(&label),
                PanelAction::UpdateGun => self.fire.update_gun(&self.maps),
                PanelAction::Arc(high) => self.fire.choose_arc(high),
                PanelAction::Hover(label) => self.panel_hover = label,
                PanelAction::Terrain(open) => {
                    self.prefs.plan_terrain_open = open;
                    self.prefs.save();
                }
                PanelAction::Lock(locked) => {
                    log::info!("Gunner panel: {}", if locked { "locked" } else { "unlocked" });
                    self.prefs.panel_locked = locked;
                    self.prefs.save();
                }
                PanelAction::Settings(open) => self.settings_open = open,
                PanelAction::Targets(open) => {
                    self.prefs.plan_targets_open = open;
                    self.prefs.save();
                }
                PanelAction::Diag(mtc_overlay::DiagAction::ViewData) => {
                    if let Some(path) = self.diag.view_file(self.prefs.diag_settings()) {
                        let _ = std::process::Command::new("notepad").arg(path).spawn();
                    }
                }
                PanelAction::Diag(mtc_overlay::DiagAction::OpenLogs) => {
                    if let Some(dir) = logfile::dir() {
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::process::Command::new("explorer").arg(dir).spawn();
                    }
                }
                PanelAction::Diag(mtc_overlay::DiagAction::CreateReport) => self.report_pending = true,
                PanelAction::Consent(choice) => match choice {
                    mtc_overlay::ConsentChoice::DontAsk(on) => self.consent_dont_ask = on,
                    mtc_overlay::ConsentChoice::Send => {
                        self.prefs.apply(mtc_overlay::Setting::Share(true));
                        self.prefs.consent_never = false;
                        self.prefs.save();
                        self.diag.set_sharing(true);
                        self.consent_open = false;
                        log::info!("Statistics: the player agreed to send them");
                    }
                    mtc_overlay::ConsentChoice::NotNow => {
                        if self.consent_dont_ask {
                            self.prefs.consent_never = true;
                            self.prefs.save();
                        }
                        self.consent_open = false;
                        log::info!("Statistics: the player chose not now{}", if self.consent_dont_ask { ", don't ask again" } else { "" });
                    }
                },
                PanelAction::Set(setting) => {
                    self.prefs.apply(setting);
                    self.prefs.save();
                    if let mtc_overlay::Setting::Share(_) = setting {
                        self.diag.set_sharing(self.prefs.sharing());
                    }
                    if let mtc_overlay::Setting::Step(mtc_overlay::Stepper::DragOffset, _) = setting {
                        mtc_overlay::FOLLOW_OFFSET_US.store(i64::from(self.prefs.drag_offset_ms) * 1000, Ordering::Relaxed);
                    }
                    log::info!("Setting: {setting:?}");
                }
                PanelAction::Moved((x, y)) => {
                    if let Some(((dx, dy), scale)) = self.panel_default.get() {
                        let offset = ((x - dx) as f32 / scale, (y - dy) as f32 / scale);
                        self.set_panel_offset(self.panel_seat.get().unwrap_or(PanelSeat::Gunner), offset);
                    }
                }
            }
        }
        // A dragged marker moves every frame: its solutions wait for the drop. Nor while V1 pans or
        // zooms: solving takes 47–70 ms in the frame loop with nine targets, which stalled the
        // marks mid-drag, and nothing to solve for changes meanwhile.
        let c = self.stats.part(stats::Part::Actions, c);
        let solved = if !self.maps.marker_dragging() && !self.maps.v1_moving() { self.fire.update(&self.maps, self.loading.shell()) } else { None };
        if let Some(took) = solved {
            self.stats.solves.0 += 1;
            self.stats.solves.1 += took;
        }
        let c = self.stats.part_n(stats::Part::Solve, c, u32::from(solved.is_some()));
        self.shots.update(now_ms, self.reading, &mut self.loading, &mut self.maps, &self.fire);
        for record in self.shots.take_records() {
            self.diag.add_shot(record);
        }
        self.diag.config((placement.layout.width, placement.layout.height), placement.layout.dpi, None);
        self.diag.take_events();
        self.diag.maybe_send(|| self.prefs.diag_settings(), false);
        let c = self.stats.part(stats::Part::Shots, c);
        // The game shows the loading menu only on V3 (3019 recorded frames): its charge marks go
        // with V1 or V2 as the game's frame shows it, not with the next read of the menu.
        let charges = (is_foreground(&self.flags.window) && menu_view).then(|| self.charge_marks(&placement, now_ms)).flatten();
        self.flags.overlay.show_charges(charges);
        self.stats.part(stats::Part::Charges, c);

        // A sight counts with a map view beside it: a loading or vote screen is all black and
        // reads as scoped too (30 of 42 snapshots in one test session).
        if let Some(dir) = &self.flags.snapshots
            && (cues.shows(mtc_vision::map::MapView::Full)
                || cues.shows(mtc_vision::map::MapView::Scope)
                || (cues.scoped() && cues.primary().is_some()))
            && self.last_snapshot.is_none_or(|t| t.elapsed() >= if self.maps.v1_moving() && !hidden() { SNAPSHOT_MOVING } else { SNAPSHOT_INTERVAL })
            && is_foreground(&self.flags.window)
            && self.snapshots_taken < SNAPSHOT_MAX
        {
            self.last_snapshot = Some(Instant::now());
            self.snapshots_taken += 1;
            if self.snapshots_taken == SNAPSHOT_MAX {
                log::info!("{SNAPSHOT_MAX} snapshots taken, no more");
            }
            let path = dir.join(format!("{:07}.png", now_ms));
            std::thread::spawn(move || snapshot(&path));
        }
        let c = stats::cycles();
        if !self.maps.v1_active() || self.maps.canvas_rebases() != rebases_before {
            self.show_marks(&placement);
        }
        self.stats.part(stats::Part::Marks, c);
        match result {
            AimResult::Read(r) => {
                let text = (r.elevation_text.clone(), r.azimuth_text.clone());
                if self.last_text.as_ref() != Some(&text) {
                    let landing = self.fire.prediction().map(|p| format!("  -> lands {:.0} m out, {:.1} s", p.distance_m, p.time_s)).unwrap_or_default();
                    log::info!("elevation {:>7}°  azimuth {}°{landing}   (cost {}, area {}x{}) @{now_ms}", r.elevation_text, r.azimuth_text, r.cost, w, h);
                    self.last_text = Some(text);
                }
            }
            AimResult::Absent if self.last_text.is_some() => {
                log::info!("(angle readout not on screen)");
                self.last_text = None;
            }
            _ => {}
        }
        self.stats.frame_work += started.elapsed();
        let (n, t) = self.flags.overlay.take_map_draws();
        self.stats.map_draws.0 += n;
        self.stats.map_draws.1 += t;
        let c = stats::cycles();
        let since = self.stats_since;
        if let Some(report) = self.stats.maybe_report() {
            self.diag.add_report(report);
            // Every map step's share (not only those of the frames over 30 ms): cores, and calls a
            // second × ms a call.
            let seconds = since.elapsed().as_secs_f64().max(1.0);
            let laps = self.maps.take_laps();
            if !laps.is_empty() {
                let parts: Vec<String> = laps.iter().map(|(view, step, ms, n)| format!("{view} {step} {:.3} ({:.1}/s × {:.1} ms)", ms / 1000.0 / seconds, f64::from(*n) / seconds, ms / f64::from(*n))).collect();
                log::info!("[map parts] {}", parts.join(" · "));
            }
            // What the working set holds beyond the private bytes and the heap: the height layers'
            // mapped pages read, and the readback's staging textures (driver memory).
            let heights: Vec<String> = self.fire.height_layers(|b| format!("{:.1}/{:.0}", stats::resident_mb(b), b.len() as f64 / 1e6));
            let (textures, staging) = self.readback.staging_bytes();
            log::info!(
                "[memory parts] heap {:.1} MB · heightmap resident/mapped {} MB · readback staging textures {textures}, {:.1} MB · identification thumbnails {}",
                stats::heap_mb(),
                if heights.is_empty() { "-".to_owned() } else { heights.join(" + ") },
                staging as f64 / 1e6,
                thumbnails_note(self.maps.thumbnail_state())
            );
            if self.height_pages_released.elapsed() >= HEIGHT_PAGES_RELEASE {
                self.fire.release_height_pages();
                self.height_pages_released = Instant::now();
            }
            self.stats_since = Instant::now();
        }
        self.stats.part(stats::Part::Report, c);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.flags.overlay.show_aim(None);
        Ok(())
    }
}

/// Where the angle readout's label is, when it is on screen.
fn label_x(result: &AimResult) -> Option<i32> {
    match result {
        AimResult::Read(r) => Some(r.label_x),
        AimResult::Unreadable { label_x, .. } => Some(*label_x),
        AimResult::Absent => None,
    }
}

/// A view as the log names it: V1 full map, V2 scope map, V3 minimap.
fn view_name((map, scoped): (Option<mtc_vision::map::MapView>, bool)) -> String {
    use mtc_vision::map::MapView;
    let map = match map {
        Some(MapView::Full) => "V1",
        Some(MapView::Scope) => "V2",
        Some(MapView::Mini) => "V3",
        None => "no map",
    };
    if scoped { format!("{map} (scoped)") } else { map.to_owned() }
}

/// The seat a frame shows: the gunner's when the angle readout is read, the loader's when the
/// loader's ammo bar is there (the panel's rule).
fn frame_seat(aim: &AimResult, loading: &loading::LoadingPipeline, now_ms: u64) -> Option<PanelSeat> {
    match aim {
        AimResult::Read(_) => Some(PanelSeat::Gunner),
        _ => loading.loader_layout(now_ms).map(|_| PanelSeat::Loader),
    }
}

fn region_view(r: &Region) -> FrameView<'_> {
    FrameView {
        data: &r.data,
        format: PixelFormat::Bgra,
        origin: (r.x, r.y),
        width: r.width,
        height: r.height,
        stride: r.width as usize * 4,
    }
}

/// At exit: the last statistics sent, when the player agreed.
impl Drop for Hud {
    fn drop(&mut self) {
        let settings = self.prefs.diag_settings();
        self.diag.maybe_send(|| settings, true);
    }
}

/// A path as shown to the player: the user's folders written as their variables.
fn short_path(path: &std::path::Path) -> String {
    scrub_paths(&path.display().to_string())
}

/// Text with the user's own folders replaced by their variables (`%LOCALAPPDATA%`, `%APPDATA%`,
/// `%USERPROFILE%`), so a problem report carries no Windows user name.
fn scrub_paths(text: &str) -> String {
    let mut out = text.to_owned();
    for var in ["LOCALAPPDATA", "APPDATA", "USERPROFILE"] {
        if let Ok(value) = std::env::var(var)
            && !value.is_empty()
        {
            out = out.replace(&value, &format!("%{var}%"));
            out = out.replace(&value.replace('\\', "/"), &format!("%{var}%"));
        }
    }
    out
}

impl Hud {
    /// A problem report: a folder with this run's records, the last minutes of the log (folders as
    /// their variables), the settings and the places found on screen; with "Screenshots" ticked,
    /// what the HUD reads on this frame: the readout box, the loading area, the map view and the
    /// seat menu with the names blacked out.
    fn problem_report(&mut self, frame: &mut Frame, placement: &Placement, frame_size: (u32, u32)) -> Result<std::path::PathBuf, String> {
        let dir = diag::dir().ok_or("no folder")?.join("reports").join(format!("report-{}", logfile::day_time_stamp()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let l = placement.layout;
        let summary = format!(
            "MTC Calculator {} problem report, {}\nWindow client {}x{}, display scaling {}%, gun {}\nSettings: {:?}\n\nDescribe what went wrong and when (the log below has the seconds since start).\n",
            env!("CARGO_PKG_VERSION"),
            self.diag.day(),
            l.width,
            l.height,
            l.dpi * 100 / 96,
            self.loading.weapon().unwrap_or("none recognised"),
            self.prefs.diag_settings()
        );
        std::fs::write(dir.join("report.txt"), summary).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("records.jsonl"), self.diag.run_lines(self.prefs.diag_settings()).join("\n") + "\n").map_err(|e| e.to_string())?;
        if let Some(log) = logfile::get() {
            let lines: Vec<String> = log.recent(logfile::RECENT).iter().map(|l| scrub_paths(l)).collect();
            std::fs::write(dir.join("log.txt"), lines.join("\n") + "\n").map_err(|e| e.to_string())?;
        }
        if self.prefs.report_screenshots {
            let bottom = placement.bottom_origin();
            let (lx, ly, lw, lh) = mtc_vision::loading::LOADING_REGION;
            let mut shots = vec![
                ("readout", placement.bottom_source(self.aim.wanted_region(bottom), frame_size), None),
                ("loading", placement.bottom_source((lx + bottom.0, ly + bottom.1, lw, lh), frame_size), None),
            ];
            if let Some(view) = self.maps.shown_transform(placement.bottom_origin()).map(|t| t.view) {
                let rect = view.frame_rect(placement.bottom_origin());
                shots.push(("map", placement.map_source(view, rect, frame_size), None));
            }
            let c = self.geometry.as_ref().map(|g| g.client_in_frame());
            if let Some(c) = c {
                let client = placement.seats_client((placement.origin.0, placement.origin.1, c.width as u32, c.height as u32));
                let (rx, ry, rw, rh) = mtc_vision::seats::SeatReader::region(client);
                // The names start right of the seat circles (12 + 86 px at the HUD scale).
                let names_from = (110.0 * placement.layout.hud_scale()).round() as u32;
                shots.push(("seats", placement.seats_source((rx, ry, rw as i32, rh as i32), frame_size), Some(names_from)));
            }
            for (name, (x, y, w, h), mask_from) in shots {
                if w <= 0 || h <= 0 {
                    continue;
                }
                let mut region = Region::default();
                self.readback.read_region(frame, x, y, w as u32, h as u32, &mut region).map_err(|e| e.to_string())?;
                let mut image = mtc_dataset::image::RgbImage::new(region.width, region.height);
                for py in 0..region.height as usize {
                    for px in 0..region.width as usize {
                        let i = (py * region.width as usize + px) * 4;
                        let o = (py * region.width as usize + px) * 3;
                        if mask_from.is_some_and(|m| px as u32 >= m) {
                            continue;
                        }
                        image.data[o] = region.data[i + 2];
                        image.data[o + 1] = region.data[i + 1];
                        image.data[o + 2] = region.data[i];
                    }
                }
                image.save_png(&dir.join(format!("{name}.png"))).map_err(|e| e.to_string())?;
            }
        }
        log::info!("Problem report: {}", short_path(&dir));
        Ok(dir)
    }

    /// Input since the last frame, while Roblox has focus: a left click on a marker or a selected
    /// objective in V1 selects it (a press and release less than `CLICK_MAX_MOVE` px apart within
    /// `CLICK_MAX_TIME`); drags (moving a marker, panning) are the game's.
    /// `hovering_marker`: the game shows "Remove marker" (the cursor is in a marker's hit area,
    /// larger than the diamond: its label too), so a press drags that marker, not the map.
    fn handle_input(&mut self, placement: &Placement, hovering_marker: bool, v1: bool) {
        if self.input.is_none() {
            return;
        }
        let events = std::mem::take(&mut self.pending_input);
        if !is_foreground(&self.flags.window) {
            self.press = None;
            self.maps.end_pan();
            self.maps.end_marker_drag();
            return;
        }
        // Screen points where V1's recognizers see them (on the view mapped to the reference
        // container off the reference).
        let to_frame = |(x, y): (i32, i32)| {
            let (fx, fy) = (x - self.screen_offset.0, y - self.screen_offset.1);
            placement.frame_to_map(mtc_vision::map::MapView::Full, (f64::from(fx) + 0.5, f64::from(fy) + 0.5))
        };
        if self.press.is_some()
            && let Some(cursor) = cursor_position()
        {
            self.maps.pan_to(to_frame(cursor));
        }
        for event in events {
            if let Some(log) = self.v1_log.as_mut() {
                let ms = |at: Instant| at.saturating_duration_since(self.started).as_secs_f64() * 1000.0;
                match event {
                    mtc_input::InputEvent::Wheel { notches, pos, at } => {
                        let (x, y) = to_frame(pos);
                        log.line(format_args!("wheel,{:.1},{x:.1},{y:.1},,{notches},,,,,,,,,,,,,", ms(at)));
                    }
                    mtc_input::InputEvent::Button { button, down, pos, at } => {
                        let (x, y) = to_frame(pos);
                        let kind = match (button, down) {
                            (mtc_input::MouseButton::Left, true) => "left_down",
                            (mtc_input::MouseButton::Left, false) => "left_up",
                            (_, true) => "other_down",
                            (_, false) => "other_up",
                        };
                        log.line(format_args!("{kind},{:.1},{x:.1},{y:.1},,,,,,,,,,,,,,,", ms(at)));
                    }
                    _ => {}
                }
            }
            if v1 && matches!(event, mtc_input::InputEvent::Button { .. } | mtc_input::InputEvent::Wheel { .. }) {
                self.maps.note_input(self.started.elapsed().as_millis() as u64);
            }
            if let mtc_input::InputEvent::Button { button: mtc_input::MouseButton::Right, down: true, at, pos } = event {
                self.last_right_click = Some(at);
                // Away from any marker the game adds one there ("Add marker"): it tells the
                // player's letter.
                if v1 && !hovering_marker {
                    self.last_place = Some((at, to_frame(pos)));
                }
            }
            // The cursor as it was at the notch (the game zooms about it), not at processing. With
            // the left button held (dragging the map) the game does not zoom.
            if let mtc_input::InputEvent::Wheel { notches, pos, at } = event
                && self.press.is_none()
            {
                self.maps.wheel(notches, to_frame(pos), at);
            }
            let mtc_input::InputEvent::Button { button: mtc_input::MouseButton::Left, down, pos, at } = event else { continue };
            if down {
                self.press = Some((pos, at));
                // Pressed on a marker: the game drags the marker; elsewhere on V1 it pans the map.
                let (x, y) = (pos.0 as f32, pos.1 as f32);
                // The game's hover hint decides whether the press is on a marker; which marker is
                // the nearest one (its hit area reaches over the label, up and to the right).
                let marks = marks::map_marks(&self.maps, &self.fire, placement, self.screen_offset, None);
                // The hit areas at the marks' size (the game's markers follow its HUD scale).
                let k = placement.map_marks_scale(mtc_vision::map::MapView::Full);
                let reach = k * if hovering_marker { CLICK_MARKER_HINTED_PX } else { CLICK_MARKER_PX };
                let on_marker = marks.and_then(|m| {
                    m.marks
                        .into_iter()
                        .filter(|k| !k.objective && (k.x - x).hypot(k.y - y) <= reach)
                        .min_by(|a, b| (a.x - x).hypot(a.y - y).total_cmp(&(b.x - x).hypot(b.y - y)))
                        .map(|k| k.label)
                });
                if hovering_marker && on_marker.is_none() {
                    // A marker the HUD does not know: the map does not move either.
                    continue;
                }
                match on_marker {
                    Some(label) => self.maps.begin_marker_drag(&label, to_frame(pos), at),
                    None => self.maps.begin_pan(to_frame(pos), at),
                }
                continue;
            }
            // The last stretch of a pan, up to where the button came up.
            self.maps.pan_to(to_frame(pos));
            self.maps.end_pan();
            self.maps.release_marker_drag(pos);
            let Some((start, pressed)) = self.press.take() else { continue };
            let still = (pos.0 - start.0).abs() <= CLICK_MAX_MOVE && (pos.1 - start.1).abs() <= CLICK_MAX_MOVE;
            if !still || at.duration_since(pressed) > CLICK_MAX_TIME {
                continue;
            }
            let Some(marks) = marks::map_marks(&self.maps, &self.fire, placement, self.screen_offset, None).filter(|m| m.planning.is_some()) else { continue };
            let k = marks.scale;
            let (x, y) = (pos.0 as f32, pos.1 as f32);
            // Markers first: an objective counts here only when selected, else after them below.
            let hit = |m: &&mtc_overlay::MapMark| (!m.objective || m.selected) && (m.x - x).hypot(m.y - y) <= k * if m.objective { CLICK_OBJECTIVE_PX } else { CLICK_MARKER_PX };
            let label = marks.marks.iter().filter(hit).min_by(|a, b| (a.x - x).hypot(a.y - y).total_cmp(&(b.x - x).hypot(b.y - y))).map(|m| m.label.clone());
            let label = label.or_else(|| self.objective_at(placement, self.screen_offset, (x, y)));
            if let Some(label) = label {
                log::info!("Selected target {label} (V1 click)");
                self.fire.select(&label);
            }
        }
    }

    /// The hovered target: on the plan panel's list, else on the map.
    fn hovered(&self) -> Option<&str> {
        self.panel_hover.as_deref().or(self.map_hover.as_deref())
    }

    /// The target under the pointer on V1's map, as a click would pick it (markers before
    /// objectives); none while the button is held (a drag or a pan).
    fn target_under_pointer(&self, placement: &Placement) -> Option<String> {
        if self.press.is_some() || !is_foreground(&self.flags.window) {
            return None;
        }
        let (x, y) = cursor_position()?;
        let (x, y) = (x as f32, y as f32);
        let marks = marks::map_marks(&self.maps, &self.fire, placement, self.screen_offset, None).filter(|m| m.planning.is_some())?;
        let k = marks.scale;
        let (x0, y0) = (marks.left as f32, marks.top as f32);
        if x < x0 || y < y0 || x >= x0 + marks.size as f32 || y >= y0 + marks.size as f32 {
            return None;
        }
        let near = |objective: bool| {
            marks
                .marks
                .iter()
                .filter(|m| m.objective == objective && (m.x - x).hypot(m.y - y) <= k * if objective { CLICK_OBJECTIVE_PX } else { CLICK_MARKER_PX })
                .min_by(|a, b| (a.x - x).hypot(a.y - y).total_cmp(&(b.x - x).hypot(b.y - y)))
                .map(|m| m.label.clone())
        };
        near(false).or_else(|| near(true))
    }

    /// Whether the planning layer (plan panel, map marks) may show: not while the weapon on screen
    /// is a known non-artillery one (a tank or helicopter seat; the HUD is for artillery, as for
    /// the loader's panel). On foot, before any weapon was read, it shows.
    fn planning_allowed(&self) -> bool {
        self.loading.weapon().is_none_or(mtc_ammo::is_artillery_weapon)
    }

    /// The map marks and a dragged marker's window, for the view on screen.
    fn show_marks(&self, placement: &Placement) {
        let mut marks = (is_foreground(&self.flags.window) && self.planning_allowed()).then(|| marks::map_marks(&self.maps, &self.fire, placement, self.screen_offset, self.hovered())).flatten();
        // The pipeline's V1 points and lengths (on the view mapped to the reference container off
        // the reference) on screen.
        let full = mtc_vision::map::MapView::Full;
        let frame = (f64::from(self.screen_offset.0), f64::from(self.screen_offset.1));
        let to_screen = |p: (f64, f64)| {
            let (x, y) = placement.map_to_frame(full, p);
            (x + frame.0, y + frame.1)
        };
        let length = |px: f64| placement.map_length_to_frame(full, px);
        // Dragging V1: the overlay moves the marks with the cursor between these drawings.
        if let Some(m) = marks.as_mut().filter(|m| m.planning.is_some() && self.press.is_some()) {
            m.follow = self.maps.pan_follow().map(|(at, limits, (press, pressed_at))| {
                let l = |v: f32| length(f64::from(v)) as f32;
                mtc_overlay::PanFollow { at, limits: (l(limits.0), l(limits.1), l(limits.2), l(limits.3)), press: to_screen(press), pressed_at }
            });
        }
        // A dragged marker leaves the map drawing for the overlay's drag window, which keeps it at
        // the cursor (its blast waits for the drop).
        let drag = self.maps.drag().and_then(|(label, grab, released_at, (pressed, pressed_at))| {
            let marks = marks.as_mut()?;
            let i = marks.marks.iter().position(|m| !m.objective && m.label == label)?;
            let mark = marks.marks.remove(i);
            if mark.selected {
                marks.blast = None;
            }
            // `grab` is from the cursor's pixel centre; the drag window counts from the pixel.
            let snap = self.maps.drag_snap().map(|(a, step)| (to_screen(a), length(step)));
            let grab = (length(grab.0), length(grab.1));
            Some(DragMark { mark, coarse_m: marks.coarse_m, grab: ((grab.0 + 0.5).round() as i32, (grab.1 + 0.5).round() as i32), released_at, snap, press: to_screen(pressed), pressed_at, scale: marks.scale })
        });
        // The drag probe (`--latency`): the selected marker while the map is dragged on V1.
        if let Some(l) = &self.flags.latency {
            let probe = marks.as_ref().filter(|m| m.planning.is_some() && self.press.is_some() && self.maps.v1_moving()).and_then(|m| m.marks.iter().find(|k| k.selected && !k.objective)).map(|k| (k.x.round() as i32, k.y.round() as i32));
            let mut shared = l.lock().expect("latency lock");
            if probe.is_some() {
                shared.arrivals.push((self.last_processed, Instant::now()));
            }
            shared.drag = probe;
        }
        if let Some(m) = marks.as_mut() {
            if !self.prefs.show_pred {
                m.pred = None;
            }
            if let Some(p) = m.planning.as_mut() {
                p.fields = self.prefs.map_labels;
            }
        }
        self.flags.overlay.show_map(marks);
        self.flags.overlay.show_drag(drag);
    }

    /// A new arrow sighting for the arrow log.
    fn log_arrow(&mut self) {
        use std::io::Write;
        let (Some((file, logged)), Some((arrow, view, zoom, at))) = (self.arrow_log.as_mut(), self.maps.last_arrow()) else { return };
        if logged.is_some_and(|l| *at <= l) {
            return;
        }
        *logged = Some(*at);
        let view = match view {
            mtc_vision::map::MapView::Full => "V1",
            mtc_vision::map::MapView::Scope => "V2",
            mtc_vision::map::MapView::Mini => "V3",
        };
        let t = at.saturating_duration_since(self.started).as_secs_f64() * 1000.0;
        let _ = writeln!(file, "{t:.1},{view},{zoom:.2},{:.6},{:.6},{:.1},{},{:.3}", arrow.map.0, arrow.map.1, arrow.heading_deg, u8::from(arrow.mounted), arrow.shape);
    }

    /// One processed V1 frame for the model log: the cursor, the button, the measurement made in
    /// this frame (if any) and the input prediction.
    fn log_v1_frame(&mut self, cursor: Option<(f64, f64)>) {
        let Some(log) = self.v1_log.as_mut() else { return };
        let t = self.started.elapsed().as_secs_f64() * 1000.0;
        let (x, y) = cursor.unwrap_or((f64::NAN, f64::NAN));
        let fresh = self.maps.last_measure().filter(|m| log.logged.is_none_or(|at| m.at > at));
        let measured = fresh.map_or(",,,,".to_owned(), |m| {
            format!("{},{},{:.5},{:.6},{:.6}", m.source, m.pairs, m.transform.zoom, m.transform.origin.0, m.transform.origin.1)
        });
        let container = self.maps.last_measure().map_or(",".to_owned(), |m| format!("{:.1},{:.1}", m.transform.left, m.transform.top));
        if let Some(m) = fresh {
            log.logged = Some(m.at);
        }
        let predicted = self.maps.drawn().map_or(",,".to_owned(), |p| format!("{:.5},{:.6},{:.6}", p.zoom, p.origin.0, p.origin.1));
        let canvas = self.maps.canvas().map_or(",,".to_owned(), |c| format!("{:.5},{:.6},{:.6}", c.zoom, c.origin.0, c.origin.1));
        log.line(format_args!("frame,{t:.1},{x:.1},{y:.1},{},,{measured},{predicted},{canvas},{container}", u8::from(self.press.is_some())));
        // A dragged marker: cursor in x, y; the game's diamond in source, pairs; the transform it
        // was read with in zoom, u0, v0 (columns reused).
        if let Some((c, s, tr)) = self.maps.take_marker_sample() {
            log.line(format_args!("marker,{t:.1},{:.1},{:.1},,,{:.2},{:.2},{:.5},{:.6},{:.6},,,,,,,{:.1},{:.1}", c.0, c.1, s.0, s.1, tr.zoom, tr.origin.0, tr.origin.1, tr.left, tr.top));
        }
    }

    /// The objective whose icon is under a screen point on the map on screen.
    fn objective_at(&self, placement: &Placement, offset: (i32, i32), (x, y): (f32, f32)) -> Option<String> {
        let t = placement.map_transform_to_frame(self.maps.shown_transform(placement.bottom_origin())?);
        let map = self.maps.map()?;
        let reach = CLICK_OBJECTIVE_PX * placement.map_marks_scale(t.view);
        mtc_maps::objectives(&map.id).iter().find_map(|o| {
            let (ox, oy) = t.to_frame(o.u, o.v);
            let d = (ox as f32 + offset.0 as f32 - x).hypot(oy as f32 + offset.1 as f32 - y);
            (d <= reach).then(|| o.letter.clone())
        })
    }

    /// While the game has focus: the aim strip and the panel beside the angle readout (gunner
    /// seat; an unreadable readout keeps what is shown), or the panel beside the loader's menu.
    fn update_overlay(&self, geometry: &WindowGeometry, placement: &Placement, result: &AimResult, now_ms: u64, scoped: bool, v1: bool) {
        let overlay = &self.flags.overlay;
        let foreground = is_foreground(&self.flags.window);
        // V1 is for planning: the plan panel docks beside the big map instead of the gunner's
        // panel; the strip stays by the readout below it. Not in a tank or helicopter.
        if v1 && self.planning_allowed() {
            overlay.show_panel(None);
            overlay.show_plan(Some(self.plan_state(placement)));
            match result {
                AimResult::Read(r) if foreground => overlay.show_aim(self.aim_strip(geometry, placement, r)),
                AimResult::Unreadable { .. } if foreground => {}
                _ => overlay.show_aim(None),
            }
            return;
        }
        overlay.show_plan(None);
        let (fx, fy) = self.screen_offset;
        let r = match result {
            AimResult::Read(r) if foreground => r,
            AimResult::Unreadable { .. } if foreground => return,
            _ => {
                overlay.show_aim(None);
                match self.loading.loader_layout(now_ms).filter(|_| foreground) {
                    Some(n) => {
                        let b = placement.bottom_origin();
                        let (left, bottom) = placement.bottom_to_frame((f64::from(b.0) + mtc_vision::loading::menu_left(n), f64::from(b.1 + mtc_vision::loading::LOADER_STRIP_BOTTOM)));
                        let beside_cards = loader_panel_anchor(fx + left as i32, fy + bottom as i32);
                        let anchor = self.panel_anchor(geometry, placement, PanelSeat::Loader, beside_cards, placement.layout.overlay_scales().1);
                        self.panel_seat.set(Some(PanelSeat::Loader));
                        overlay.show_panel(Some(self.panel_state(anchor, false)));
                    }
                    None => overlay.show_panel(None),
                }
                return;
            }
        };
        let (x, [(el, _), _]) = placement.readout_rows(r.anchor_x);
        let (sx, sy) = screen_point(geometry, placement, (x, el));
        let (strip, hud) = placement.layout.overlay_scales();
        let right = hud_right(sx, geometry.client.x + geometry.client.width / 2, strip, hud);
        // Third person and scoped share the gunner's position (the scoped line follows it).
        let anchor = self.panel_anchor(geometry, placement, PanelSeat::Gunner, panel_anchor(right, sy, hud), hud);
        self.panel_seat.set(Some(PanelSeat::Gunner));
        overlay.show_panel(Some(self.panel_state(anchor, scoped)));
        overlay.show_aim(self.aim_strip(geometry, placement, r));
    }

    /// The aim strip beside the angle readout `r`: to the selected target's chosen arc.
    fn aim_strip(&self, geometry: &WindowGeometry, placement: &Placement, r: &AimReading) -> Option<AimStrip> {
        let (x, rows) = placement.readout_rows(r.anchor_x);
        let screen_y = |y: i32| screen_point(geometry, placement, (x, y)).1;
        let [(el, ev), (al, av)] = rows.map(|(l, v)| (screen_y(l), screen_y(v)));
        let (scale, hud) = placement.layout.overlay_scales();
        let right = hud_right(screen_point(geometry, placement, (x, 0)).0, geometry.client.x + geometry.client.width / 2, scale, hud);
        let mirror = self.flags.mirror.then_some((r.elevation, r.azimuth));
        let (elevation, azimuth) = mirror.or(self.fire.aim()).or(self.flags.manual)?;
        let tolerance = self.aim_tolerance();
        Some(AimStrip {
            right,
            rows: [
                AimRow {
                    axis: Axis::Elevation,
                    current: r.elevation,
                    target: elevation,
                    tolerance: tolerance.0,
                    label_y: el,
                    value_y: ev,
                    beyond_reach: self.beyond_reach(elevation),
                },
                AimRow { axis: Axis::Azimuth, current: r.azimuth, target: azimuth, tolerance: tolerance.1, label_y: al, value_y: av, beyond_reach: false },
            ],
            mirror: self.flags.mirror,
            scale,
        })
    }

    /// The elevation and azimuth tolerance (°) by the settings: the angles set, or the miss
    /// set on the ground turned into angles on the trajectory to the selected target — along the
    /// line of fire by how far the landing moves per degree of elevation, across it by the distance.
    /// Without a selected target's trajectory, the angles.
    fn aim_tolerance(&self) -> (f64, f64) {
        let angle = self.prefs.tolerance_angle;
        if self.prefs.tolerance_mode == mtc_overlay::ToleranceMode::Angle {
            return angle;
        }
        let distance = self.fire.selected().map(|(_, s)| s.distance_m);
        prefs::tolerance_from_impact(self.prefs.tolerance_impact, self.fire.aim_slope(), distance, angle)
    }

    /// Whether `elevation` is outside what the current gun reaches (`plan::beyond_reach`).
    fn beyond_reach(&self, elevation: f64) -> bool {
        plan::beyond_reach(&self.loading, self.elevation_seen.as_ref(), elevation)
    }

    /// A dragged panel's place for a seat, kept as its offset from the default place (`Prefs`).
    fn set_panel_offset(&mut self, seat: PanelSeat, offset: (f32, f32)) {
        match seat {
            PanelSeat::Loader => (self.prefs.panel_loader_offset, self.prefs.panel_loader) = (Some(offset), None),
            PanelSeat::Gunner => (self.prefs.panel_gunner_offset, self.prefs.panel_gunner) = (Some(offset), None),
        }
        self.prefs.save();
    }

    /// The panel's anchor for a seat: where it sits by default (`default`, screen px), moved by the
    /// dragged offset at the panel's `scale`, kept on the client.
    fn panel_anchor(&self, geometry: &WindowGeometry, placement: &Placement, seat: PanelSeat, default: (i32, i32), scale: f32) -> (i32, i32) {
        self.panel_default.set(Some((default, scale)));
        let c = &geometry.client;
        let (offset, legacy) = match seat {
            PanelSeat::Gunner => (self.prefs.panel_gunner_offset, self.prefs.panel_gunner),
            PanelSeat::Loader => (self.prefs.panel_loader_offset, self.prefs.panel_loader),
        };
        let offset = offset.or_else(|| {
            let (x, y) = legacy.filter(|_| placement.is_reference())?;
            let offset = ((c.x + x - default.0) as f32 / scale, (c.y + y - default.1) as f32 / scale);
            self.panel_migrated.set(Some((seat, offset)));
            Some(offset)
        });
        let Some((ox, oy)) = offset else { return default };
        let (x, y) = (default.0 + (ox * scale).round() as i32 - c.x, default.1 + (oy * scale).round() as i32 - c.y);
        let width = (PANEL_WIDTH as f32 * self.overlay_scales.1).ceil() as i32;
        let x = x.clamp(PANEL_KEEP_IN, c.width + width - PANEL_KEEP_IN);
        let y = y.clamp(PANEL_KEEP_IN, c.height);
        (c.x + x, c.y + y)
    }

    /// When (ms after M) the loading menu's marks switch, leaving V3 or back to it: the median of
    /// the measured presses (from the key to the game's frame with the new view), a composition
    /// early.
    fn view_switch_due_ms(&self, to_v3: bool) -> f64 {
        let mut ms: Vec<f64> = self.view_switch_ms[usize::from(to_v3)].iter().copied().collect();
        ms.sort_by(f64::total_cmp);
        ms.get(ms.len() / 2).copied().unwrap_or(VIEW_SWITCH_DEFAULT_MS) - VIEW_SWITCH_LEAD_MS
    }

    /// Green and red dots beside each charge of each card of the loading menu on screen: whether
    /// that shell at that charge reaches the selected target.
    fn charge_marks(&mut self, placement: &Placement, now_ms: u64) -> Option<ChargeMarks> {
        use mtc_ammo::ChargeLevel;
        use mtc_vision::loading::{CARD_HALF, Charge, charge_row_centre};
        let (seat, weapon, cards) = self.loading.menu_cards(now_ms)?;
        self.fire.selected()?;
        let (fx, fy) = (f64::from(self.screen_offset.0), f64::from(self.screen_offset.1));
        let mut dots = Vec::new();
        for card in cards.iter().filter(|c| c.rows_shown) {
            // Each charge's mark where every kind the card can be agrees (still tracing: none yet).
            let mut by_level: Vec<(ChargeLevel, Vec<Option<bool>>)> = Vec::new();
            for family in &card.families {
                let reach = self.fire.reach(&weapon, family);
                for (shell, reaches) in family.iter().zip(reach) {
                    let Some(level) = shell.charge() else { continue };
                    match by_level.iter_mut().find(|(l, _)| *l == level) {
                        Some((_, all)) => all.push(reaches),
                        None => by_level.push((level, vec![reaches])),
                    }
                }
            }
            for (level, all) in by_level {
                let Some(reaches) = all.iter().copied().collect::<Option<Vec<bool>>>().filter(|r| r.windows(2).all(|w| w[0] == w[1])).and_then(|r| r.first().copied()) else {
                    continue;
                };
                let charge = match level {
                    ChargeLevel::Low => Charge::Low,
                    ChargeLevel::Medium => Charge::Medium,
                    ChargeLevel::High => Charge::High,
                };
                // Read at the bottom origin; on screen where the bottom group puts it.
                let (x, y) = placement.bottom_to_frame((card.centre_x + CARD_HALF - CHARGE_DOT_INSET, f64::from(placement.bottom_origin().1) + charge_row_centre(seat, charge)));
                dots.push(ChargeDot { x: (fx + x) as f32, y: (fy + y) as f32, reaches });
            }
        }
        let fold = |f: fn(f32, f32) -> f32, start: f32, pick: fn(&ChargeDot) -> f32| dots.iter().map(pick).fold(start, f);
        let (x0, x1) = (fold(f32::min, f32::MAX, |d| d.x), fold(f32::max, f32::MIN, |d| d.x));
        let (y0, y1) = (fold(f32::min, f32::MAX, |d| d.y), fold(f32::max, f32::MIN, |d| d.y));
        let scale = placement.layout.overlay_scales().1;
        (!dots.is_empty()).then(|| {
            let (left, top) = (x0.floor() as i32 - 10, y0.floor() as i32 - 10);
            ChargeMarks { left, top, width: x1.ceil() as i32 + 10 - left, height: y1.ceil() as i32 + 10 - top, dots, scale }
        })
    }

    /// The V1 plan panel: docked right of the full map, top on its top, clear of the map marks
    /// window's text margin; a row per target with the charge to fire it with.
    fn plan_state(&self, placement: &Placement) -> PlanState {
        let (at, scale) = plan_place(placement, self.screen_offset);
        let sources = plan::PlanSources { maps: &self.maps, fire: &self.fire, loading: &self.loading, elevation_seen: self.elevation_seen.as_ref(), hover: self.hovered(), terrain_open: self.prefs.plan_terrain_open };
        PlanState { settings_open: self.settings_open, targets_open: self.prefs.plan_targets_open, scale, ..plan::plan_state(at, &sources) }
    }

    /// What the panel shows: targets, the selected target's solution, the shell, and the first
    /// thing missing for a firing solution (only while it is missing).
    fn panel_state(&self, anchor: (i32, i32), compact: bool) -> PanelState {
        let fire = &self.fire;
        let shell = fire.shell();
        let aim = fire.aim();
        let selected = fire.selected();
        let arc = |a: Option<mtc_ballistics::Arc>| {
            a.map(|a| PanelArc {
                elevation: a.elevation,
                time_of_flight: a.time_of_flight,
                active: aim.is_some_and(|(e, _)| e == a.elevation),
                beyond_reach: self.beyond_reach(a.elevation),
            })
        };
        let solution = selected.map(|(_, s)| PanelSolution { distance_m: s.distance_m, height_m: s.height_m, azimuth: s.azimuth, low: arc(s.low), high: arc(s.high) });
        let targets = fire
            .aimpoints()
            .iter()
            .map(|a| {
                let row = fire.rows.iter().find(|(r, _)| r.label == a.label);
                let reachable = shell.is_none() || row.is_none_or(|(_, s)| s.low.is_some() || s.high.is_some());
                PanelTarget { label: a.label.clone(), objective: a.objective, reachable }
            })
            .collect();
        let notice = if fire.map().is_none() {
            Some("Open the map to identify it")
        } else if !fire.gun_set() {
            Some("Gun position unknown: show the map")
        } else if !fire.heights_settled() {
            Some("Loading heightmap…")
        } else if fire.selected_label().is_none() {
            None
        } else if shell.is_none() {
            Some("Load a shell for elevations")
        } else if selected.is_some_and(|(_, s)| s.low.is_none() && s.high.is_none()) {
            Some("Out of range for this shell")
        } else {
            None
        };
        let short = |c: mtc_ammo::ChargeLevel| c.word().trim_end_matches(" Charge").to_owned();
        PanelState {
            anchor,
            targets,
            selected: fire.selected_label().map(str::to_owned),
            solution,
            shell: shell.map(|p| p.shell.base_name()),
            load: if self.loading.is_loading() {
                PanelLoad::Loading
            } else if self.loading.is_loaded() {
                PanelLoad::Loaded
            } else if self.loading.is_fired() {
                PanelLoad::Fired
            } else {
                PanelLoad::Empty
            },
            charge: shell.and_then(|p| p.shell.charge()).map(short),
            recommended: fire.recommendation.as_ref().map(|r| short(r.charge)),
            gun_set: fire.gun_set(),
            notice: notice.map(str::to_owned),
            precision: self.maps.map().zip(fire.selected_label()).and_then(|(map, label)| {
                let target = self.maps.targets().iter().find(|t| t.label == label)?;
                let metres = marks::precision_m(map.size_m(), target.scale);
                Some((metres as f32, metres > marks::COARSE_PRECISION_M))
            }),
            compact,
            locked: self.prefs.panel_locked,
            settings_open: self.settings_open,
            scale: self.overlay_scales.1,
        }
    }
}

fn arg_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// The height layers' pages read are taken out of the working set this often: each line a shot
/// is checked along brings in its tiles, and over a match the aimed sectors add up (replaying a
/// recorded match: 11 MB after 19 minutes with tiles, all 33 MB of both layers in rows).
const HEIGHT_PAGES_RELEASE: Duration = Duration::from_secs(30);

/// The identification thumbnails' state for the memory lines.
fn thumbnails_note(state: Option<(bool, u32)>) -> String {
    match state {
        Some((dropped, reloads)) => format!("{} (read back {reloads} times)", if dropped { "dropped" } else { "in memory" }),
        None => "-".to_owned(),
    }
}

/// `--map <id>` (development): the map given, not identified (`debug` for the debug map).
fn forced_map(mut pipeline: maps::MapPipeline) -> maps::MapPipeline {
    if let Some(id) = arg_value("--map") {
        let info = mtc_maps::by_id_or_debug(&id).unwrap_or_else(|| panic!("--map: no map with id {id:?} (ids are in data/maps.json, or debug)"));
        pipeline.force_map(info);
    }
    pipeline
}

/// `--target <elevation>,<azimuth>`.
fn parse_manual() -> Option<(f64, f64)> {
    let target = arg_value("--target")?;
    let (e, a) = target.split_once(',')?;
    Some((e.trim().parse().ok()?, a.trim().parse().ok()?))
}

fn roblox_font_dir() -> std::path::PathBuf {
    let versions = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("Roblox/Versions");
    std::fs::read_dir(versions)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path().join("content/fonts"))
        .filter(|p| p.join("Montserrat-Medium.ttf").is_file())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .unwrap_or_default()
}

/// The V1 plan panel's top-left (screen px; `offset` turns frame into screen px) and scale: docked
/// right of the full map at its top; its own size where it fits there and inside the game window,
/// else smaller to fit (a smaller window).
fn plan_place(placement: &Placement, offset: (i32, i32)) -> ((i32, i32), f32) {
    let (left, top, side) = placement.map_container(mtc_vision::map::MapView::Full);
    let (sx, sy) = (offset.0 + placement.origin.0, offset.1 + placement.origin.1);
    let at = (sx + (left + side).round() as i32 + PLAN_GAP, sy + top.round() as i32);
    let room = (sx + placement.layout.width as i32 - PLAN_MARGIN - at.0, sy + placement.layout.height as i32 - PLAN_MARGIN - at.1);
    let scale = 1.0f32.min(room.0 as f32 / mtc_overlay::PLAN_WIDTH as f32).min(room.1 as f32 / mtc_overlay::PLAN_MAX_HEIGHT as f32).max(0.3);
    (at, scale)
}

/// Where a frame shows the client and the configuration it is laid out for: a restored window's
/// picture can sit `shift` up-left of its geometry (`picture_shift`).
fn placement_of(geometry: &WindowGeometry, frame_width: i32, shift: (i32, i32)) -> Placement {
    let c = geometry.client_in_frame();
    let shift = if frame_width > c.width { shift } else { (0, 0) };
    Placement::new((c.x - shift.0, c.y - shift.1), UiLayout { width: c.width as u32, height: c.height as u32, dpi: geometry.dpi })
}

/// A frame point on screen: the client sits on screen where the geometry says, in the frame where
/// the placement found it.
fn screen_point(geometry: &WindowGeometry, placement: &Placement, (x, y): (i32, i32)) -> (i32, i32) {
    (geometry.client.x + x - placement.origin.0, geometry.client.y + y - placement.origin.1)
}

/// The overlay layers blended onto a frame (premultiplied BGRA over RGB).
fn composite(image: &mut mtc_dataset::image::RgbImage, layers: &[mtc_overlay::Layer]) {
    for layer in layers {
        for y in 0..layer.height as i32 {
            for x in 0..layer.width as i32 {
                let (fx, fy) = (layer.x + x, layer.y + y);
                if fx < 0 || fy < 0 || fx >= image.width as i32 || fy >= image.height as i32 {
                    continue;
                }
                let i = (y as usize * layer.width as usize + x as usize) * 4;
                let keep = 255 - u32::from(layer.bgra[i + 3]);
                let j = (fy as usize * image.width as usize + fx as usize) * 3;
                for (k, src) in [layer.bgra[i + 2], layer.bgra[i + 1], layer.bgra[i]].into_iter().enumerate() {
                    image.data[j + k] = (u32::from(src) + (u32::from(image.data[j + k]) * keep + 127) / 255).min(255) as u8;
                }
            }
        }
    }
}

/// Replay mode: runs the map and loading pipelines over a recorded session (frame timestamps as
/// the clock) instead of capturing. `--render-marks <dir>` saves the map views with the target
/// marks drawn on them (at most one image every 2 s); `--select <label>` selects that target
/// whenever it is on the map. `--render-aim <dir>` saves the aim strip (comparison mode) where the
/// HUD would put it beside the readout, one image per configuration every 10 s; `--aim-frames`
/// prints every frame's readout. Off the reference the readout and the loading UI are read too,
/// the loading UI with templates drawn for the configuration.
fn replay(id: &str) {
    logfile::console_only();
    let render_dir = arg_value("--render-marks").map(std::path::PathBuf::from);
    // How often the map marks are rendered (ms of recording; 0: every frame).
    let render_every: u64 = arg_value("--render-every").map_or(2000, |v| v.parse().expect("--render-every <ms>"));
    let select = arg_value("--select");
    let font_dir = roblox_font_dir();
    let mut last_render: Option<u64> = None;
    let session = mtc_dataset::Session::open(mtc_dataset::datasets_dir().join("raw").join(id)).expect("session");
    let mut cues = CueReader::new();
    // The configuration the cue reader and the map pipeline are set for, and the containers found.
    let mut map_layout = UiLayout::REFERENCE;
    let mut locator = locator::Locator::new();
    let mut pipeline = forced_map(maps::MapPipeline::new(false));
    if std::env::args().any(|a| a == "--under-all") {
        pipeline.look_under_every_icon();
    }
    let mut aim = AimReader::new();
    // Off the reference the readout is read on its area resampled to the reference geometry.
    let aim_area = AimReader::new().wanted_region(Placement::VIRTUAL_ORIGIN);
    let mut aim_layout = UiLayout::REFERENCE;
    let mut shift = (0, 0);
    let mut aim_counts: std::collections::BTreeMap<(u32, u32, u32), (u32, u32)> = Default::default();
    let render_aim = arg_value("--render-aim").map(std::path::PathBuf::from);
    let aim_frames = std::env::args().any(|a| a == "--aim-frames");
    let mut aim_rendered: std::collections::HashMap<(u32, u32, u32), u64> = Default::default();
    let mut loading = loading::LoadingPipeline::new();
    let mut fire = fire::FireControl::new(false);
    let mut seat_watch = seats::SeatWatch::new();
    let mut loading_work = Duration::ZERO;
    let mut loading_remap = Duration::ZERO;
    let mut loading_reads = 0u32;
    let mut frames: Vec<_> = session.frames.iter().collect();
    frames.sort_by_key(|m| m.t_ms);
    let mut work = Duration::ZERO;
    let mut processed = 0u32;
    let mut reading: Option<(f64, f64)> = None;
    let mut last_prediction: Option<String> = None;
    let mut last_kinds: Option<String> = None;
    let mut shot_log = shots::ShotLog::default();
    // F presses logged by the recorder, fed as they would arrive live.
    if session.dir.join("keys.jsonl").exists() {
        loading.keys_live();
    }
    let mut fire_keys = session.keys().into_iter().filter(|(_, k)| k.vk == VK_F && k.down).map(|(t, _)| t).peekable();
    let mut last_view: Option<mtc_vision::map::MapView> = None;
    // The configuration the loading pipeline's text reader is for.
    let mut text_layout: Option<UiLayout> = None;
    let mut memory_at: Option<u64> = None;
    let mut pages_released_at = 0u64;
    for meta in frames {
        // Memory as the session goes (what the program's own heap holds, against the working set).
        if memory_at.is_none_or(|t| meta.t_ms.saturating_sub(t) >= 60_000) {
            memory_at = Some(meta.t_ms);
            let (ws, private) = stats::memory_mb();
            let heights = fire.height_layers(stats::resident_mb).iter().fold(0.0, |a, b| a + b);
            log::info!(
                "[memory] {:.0} s: working set {ws:.0} MB · private {private:.0} MB · heap {:.1} MB · heightmap resident {heights:.1} MB · identification thumbnails {}",
                meta.t_ms as f64 / 1000.0,
                stats::heap_mb(),
                thumbnails_note(pipeline.thumbnail_state())
            );
            pages_released_at = 0;
        }
        // As live, every 30 s; the memory line shows the 30 s before a release.
        if meta.t_ms.saturating_sub(pages_released_at) >= HEIGHT_PAGES_RELEASE.as_millis() as u64 {
            fire.release_height_pages();
            pages_released_at = meta.t_ms;
        }
        while let Some(t) = fire_keys.next_if(|&t| t <= meta.t_ms) {
            loading.fire_key(t);
        }
        let image = mtc_dataset::image::RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        if image.width as i32 > o.width {
            let [(bx, by, bw, bh), (rx, ry, rw, rh)] = shift_probes((image.width, image.height));
            shift = picture_shift(&view.sub(bx, by, bw, bh), &view.sub(rx, ry, rw, rh)).unwrap_or(shift);
        }
        let mut placement = placement_of(&meta.geometry, image.width as i32, shift);
        placement.located = locator.located(placement.layout);
        let origin = placement.origin;
        let started = Instant::now();
        // The cues where this configuration puts them; the map views mapped to the reference
        // containers.
        if placement.layout != map_layout {
            map_layout = placement.layout;
            cues = CueReader::for_placement(&placement);
            pipeline.set_placement(&placement);
        }
        if !locator.wanted(placement.layout).is_empty() && locator.take(&[view], origin) {
            placement.located = locator.located(placement.layout);
            cues = CueReader::for_placement(&placement);
            pipeline.set_placement(&placement);
        }
        let c = cues.read(&view, origin);
        // V1 or V2 opened from V3, or V3 again, as live (a press on the map waits for V3).
        {
            use mtc_vision::map::MapView;
            let (now, before) = (c.primary(), last_view);
            let away = |v: Option<MapView>| matches!(v, Some(MapView::Full | MapView::Scope));
            if away(now) && before == Some(MapView::Mini) {
                loading.left_v3(meta.t_ms);
            } else if now == Some(MapView::Mini) && away(before) {
                loading.back_on_v3(meta.t_ms);
            }
            if now.is_some() {
                last_view = now;
            }
        }
        let map_origin = placement.bottom_origin();
        if let Some((map_view, rect)) = pipeline.wanted(&c, map_origin, meta.t_ms) {
            let cursor = meta.cursor.map(|(x, y)| placement.frame_to_map(map_view, (f64::from(x) + 0.5, f64::from(y) + 0.5)));
            log::info!("-- {:.1}s", meta.t_ms as f64 / 1000.0);
            if placement.is_reference() {
                pipeline.process(&view, map_view, &c, origin, cursor, maps::MapInput::default(), meta.t_ms);
            } else {
                // From the frame rectangle the HUD reads back, as live.
                let (x, y, w, h) = placement.map_source(map_view, rect, (image.width, image.height));
                let remapped = placement.map_remap(map_view, &view.sub(x, y, w as u32, h as u32), rect);
                pipeline.process(&remapped.view(), map_view, &c, map_origin, cursor, maps::MapInput::default(), meta.t_ms);
            }
            processed += 1;
        }
        work += started.elapsed();
        if placement.layout != aim_layout {
            aim_layout = placement.layout;
            aim.set_scale(aim_layout.readout_text());
        }
        let result = if placement.is_reference() {
            aim.read(&view, origin)
        } else {
            let remapped = placement.bottom_remap(&view, aim_area);
            aim.read(&remapped.view(), Placement::VIRTUAL_ORIGIN)
        };
        let key = (aim_layout.dpi, aim_layout.width, aim_layout.height);
        let count = aim_counts.entry(key).or_default();
        count.1 += 1;
        if aim_frames {
            let shown = match &result {
                AimResult::Read(r) => format!("{} / {}", r.elevation_text, r.azimuth_text),
                AimResult::Unreadable { .. } => "unreadable".to_owned(),
                AimResult::Absent => "none".to_owned(),
            };
            log::info!("Readout frame {} {}% {}x{} {shown}", meta.file, key.0 * 100 / 96, key.1, key.2);
        }
        if let AimResult::Read(r) = &result {
            count.0 += 1;
            // The strip in comparison mode (the readout's own words) where the HUD would put it.
            if let Some(dir) = &render_aim
                && aim_rendered.get(&key).is_none_or(|&t| meta.t_ms.saturating_sub(t) >= 10_000)
            {
                aim_rendered.insert(key, meta.t_ms);
                let (x, [(el, ev), (al, av)]) = placement.readout_rows(r.anchor_x);
                let row = |axis, current, label_y, value_y| AimRow { axis, current, target: current, tolerance: 0.05, label_y, value_y, beyond_reach: false };
                let (scale, hud) = aim_layout.overlay_scales();
                let strip = AimStrip { right: hud_right(x, origin.0 + o.width / 2, scale, hud), rows: [row(Axis::Elevation, r.elevation, el, ev), row(Axis::Azimuth, r.azimuth, al, av)], mirror: true, scale };
                let layers = mtc_overlay::render_offscreen(&font_dir, Some(&strip), None, None).expect("render aim");
                let mut shown = image.clone();
                composite(&mut shown, &layers);
                let x0 = (strip.right - 240).max(0) as u32;
                let y0 = (el - 50).max(0) as u32;
                let (w, h) = ((x + 140 - x0 as i32).max(1) as u32, (av + 40 - y0 as i32).max(1) as u32);
                let (w, h) = (w.min(shown.width - x0), h.min(shown.height - y0));
                shown.crop(x0, y0, w, h).save_png(&dir.join(format!("{}-{}x{}_{:07}.png", key.0, key.1, key.2, meta.t_ms))).expect("save");
            }
        }
        if text_layout != Some(placement.layout) {
            text_layout = Some(placement.layout);
            let reader = if placement.is_reference() { mtc_vision::text::TextReader::new() } else { ui_text::reader_for(&font_dir, placement.layout).expect("UI glyph templates") };
            loading.set_reader(reader, !placement.is_reference());
        }
        let bottom = placement.bottom_origin();
        if let Some(rect) = loading.wanted(bottom, meta.t_ms) {
            let started = Instant::now();
            // From the frame rectangle the HUD reads back, as live.
            let remapped = (!placement.is_reference()).then(|| {
                let (x, y, w, h) = placement.bottom_source(rect, (image.width, image.height));
                placement.bottom_remap(&view.sub(x, y, w as u32, h as u32), rect)
            });
            loading_remap += started.elapsed();
            let loading_view = remapped.as_ref().map_or(view, |r| r.view());
            loading.process(&loading_view, bottom, label_x(&result), c.shows(mtc_vision::map::MapView::Full), meta.t_ms);
            loading_work += started.elapsed();
            loading_reads += 1;
            // What each card of the menu can be (the charge marks' shells), when it changes.
            if let Some((_, _, cards)) = loading.menu_cards(meta.t_ms) {
                let kinds: Vec<String> = cards
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.rows_shown)
                    .map(|(i, c)| {
                        let names: Vec<String> = c.families.iter().filter_map(|f| f.first()).map(|s| s.base_name()).collect();
                        format!("#{} {}", i + 1, if names.is_empty() { "?".to_owned() } else { names.join(" or ") })
                    })
                    .collect();
                let line = kinds.join(" · ");
                if !line.is_empty() && last_kinds.as_ref() != Some(&line) {
                    log::info!("Charge marks' shells: {line}");
                    last_kinds = Some(line);
                }
            }
        }
        let client = placement.seats_client((origin.0, origin.1, o.width as u32, o.height as u32));
        let seat = frame_seat(&result, &loading, meta.t_ms);
        if let Some((rx, ry, rw, rh)) = seat_watch.wanted(client, seat, meta.t_ms) {
            if placement.is_reference() {
                seat_watch.process(&view, client, seat, meta.t_ms);
            } else {
                let rect = (rx, ry, rw as i32, rh as i32);
                let (x, y, w, h) = placement.seats_source(rect, (image.width, image.height));
                let remapped = placement.seats_remap(&view.sub(x, y, w as u32, h as u32), rect);
                seat_watch.process(&remapped.view(), client, seat, meta.t_ms);
            }
        }
        if let Some(label) = &select {
            fire.select(label);
        }
        fire.update(&pipeline, loading.shell());
        reading = match &result {
            AimResult::Read(r) => Some((r.elevation, r.azimuth)),
            AimResult::Unreadable { .. } => reading,
            AimResult::Absent => None,
        };
        fire.predict(reading);
        shot_log.update(meta.t_ms, reading, &mut loading, &mut pipeline, &fire);
        if let (AimResult::Read(r), Some(p)) = (&result, fire.prediction()) {
            let text = format!("elevation {:>7}°  azimuth {}°  -> lands {:.0} m out, {:.1} s", r.elevation_text, r.azimuth_text, p.distance_m, p.time_s);
            if last_prediction.as_ref() != Some(&text) {
                log::info!("{text}");
                last_prediction = Some(text);
            }
        }
        if let Some(dir) = &render_dir
            && last_render.is_none_or(|t| meta.t_ms.saturating_sub(t) >= render_every)
            && let Some(marks) = marks::map_marks(&pipeline, &fire, &placement, (0, 0), None).filter(|m| !m.marks.is_empty())
        {
            last_render = Some(meta.t_ms);
            let mut layers = mtc_overlay::render_offscreen(&font_dir, None, None, Some(&marks)).expect("render marks");
            // V1: the plan panel beside the map, on a canvas widened to take it.
            let mut right = 0;
            if marks.planning.is_some() {
                let (at, scale) = plan_place(&placement, (0, 0));
                let sources = plan::PlanSources { maps: &pipeline, fire: &fire, loading: &loading, elevation_seen: None, hover: None, terrain_open: true };
                let layer = mtc_overlay::render_plan(&font_dir, &PlanState { scale, ..plan::plan_state(at, &sources) }).expect("render plan");
                right = layer.x + layer.width as i32 + 16;
                layers.push(layer);
            }
            let mut image = if right as u32 > image.width {
                let mut wide = mtc_dataset::image::RgbImage::new(right as u32, image.height);
                wide.data.fill(40);
                for y in 0..image.height as usize {
                    let (w, ow) = (image.width as usize * 3, wide.width as usize * 3);
                    wide.data[y * ow..y * ow + w].copy_from_slice(&image.data[y * w..(y + 1) * w]);
                }
                wide
            } else {
                image.clone()
            };
            composite(&mut image, &layers);
            let margin = 60;
            let (x, y) = ((marks.left - margin).max(0) as u32, (marks.top - margin).max(0) as u32);
            let side = (marks.size + 2 * margin) as u32;
            let (w, h) = ((side.max((right - x as i32).max(0) as u32)).min(image.width - x), side.min(image.height - y));
            let view = if marks.small { "V3" } else if marks.planning.is_some() { "V1" } else { "V2" };
            image.crop(x, y, w, h).save_png(&dir.join(format!("{:07}_{view}.png", meta.t_ms))).expect("save");
        }
    }
    shot_log.flush(&fire);
    log::info!("Replay done: {processed} map views processed, {:.1} ms on average", work.as_secs_f64() * 1000.0 / f64::from(processed.max(1)));
    let per_read = |d: Duration| d.as_secs_f64() * 1000.0 / f64::from(loading_reads.max(1));
    log::info!("Loading UI: read {loading_reads} times, {:.2} ms on average (of which {:.2} ms mapping another configuration to the reference geometry)", per_read(loading_work), per_read(loading_remap));
    let counts: Vec<String> = aim_counts.iter().map(|((dpi, w, h), (read, n))| format!("{}% {w}×{h} {read}/{n}", dpi * 100 / 96)).collect();
    log::info!("Angle readout (by display configuration): {}", counts.join(" · "));
}

fn main() {
    if let Some(i) = std::env::args().position(|a| a == "--replay") {
        replay(&std::env::args().nth(i + 1).expect("session id after --replay"));
        return;
    }
    enable_per_monitor_dpi_awareness();
    stats::name_this_thread("main");
    // The log also goes to a file per run, set up before anything is printed.
    diag::count_panics();
    // Development builds print the messages as well; the release build has no console.
    if let Some(log) = logfile::start(Instant::now(), !cfg!(feature = "gui")) {
        log::info!("Log file: {}", log.path().display());
    }
    if already_running() {
        log::info!("Another copy is already running; this one ends");
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst)).expect("Ctrl+C handler");
    }
    let tray = tray::spawn(stop.clone());
    let manual = parse_manual();
    let mirror = std::env::args().any(|a| a == "--mirror");
    let follow_tune = std::env::args().any(|a| a == "--follow-tune");
    if std::env::args().any(|a| a == "--hidden") {
        mtc_overlay::HIDDEN.store(true, std::sync::atomic::Ordering::Relaxed);
        log::info!("Hidden mode: no windows, recognition and logging only");
    }
    // The drag follow's fine-tuning from the settings; `--follow-offset <ms>` (development) to start
    // from another (positive: later), not saved.
    let prefs = prefs::Prefs::load();
    let offset_ms = arg_value("--follow-offset").and_then(|v| v.parse::<f64>().ok()).unwrap_or(f64::from(prefs.drag_offset_ms));
    mtc_overlay::FOLLOW_OFFSET_US.store((offset_ms * 1000.0) as i64, std::sync::atomic::Ordering::Relaxed);
    if follow_tune {
        log::info!("Drag follow tuning: while dragging the full map press [ to move the marks {} ms earlier, ] {} ms later, until they look stuck to it", FOLLOW_TUNE_STEP_US / 1000, FOLLOW_TUNE_STEP_US / 1000);
    }
    let snapshots = arg_value("--snapshots").map(std::path::PathBuf::from);
    let v1_log = arg_value("--v1-log").map(std::path::PathBuf::from);
    let arrow_log = arg_value("--arrow-log").map(std::path::PathBuf::from);
    let latency = std::env::args().any(|a| a == "--latency").then(|| {
        let shared = Arc::new(std::sync::Mutex::new(latency::Shared::default()));
        let c = mtc_vision::map::MapView::Full.container();
        let plan = ((c.x + c.size).round() as i32 + PLAN_GAP, c.y.round() as i32);
        let marks = (c.x.round() as i32 - mtc_overlay::MAP_MARGIN, c.y.round() as i32 - mtc_overlay::MAP_MARGIN);
        latency::spawn(shared.clone(), plan, marks);
        shared
    });
    if let Some(dir) = &snapshots {
        std::fs::create_dir_all(dir).expect("snapshot folder");
    }
    // The HUD shows only over the game: hidden the moment another window is in front, even if the
    // game, now covered, sends no more frames to process.
    let roblox_handle = Arc::new(std::sync::atomic::AtomicIsize::new(0));
    let gate_handle = roblox_handle.clone();
    let overlay = Arc::new(Overlay::spawn_gated(roblox_font_dir(), move || {
        let front = foreground_handle();
        front != 0 && front == gate_handle.load(Ordering::Relaxed)
    }));
    log::info!("MTC Calculator {} · quit from the notification area icon (Ctrl+C in a console)", env!("CARGO_PKG_VERSION"));
    if let Some((e, a)) = manual {
        log::info!("Manual target (used with no firing solution): elevation {e:+.2}°  azimuth {a:06.2}°");
    }
    match prefs.tolerance_mode {
        mtc_overlay::ToleranceMode::Angle => log::info!("Aim tolerance (settings): elevation ±{:.2}°, azimuth ±{:.2}°", prefs.tolerance_angle.0, prefs.tolerance_angle.1),
        mtc_overlay::ToleranceMode::Impact => log::info!("Aim tolerance (settings): impact along the line of fire ±{:.0} m, across ±{:.0} m", prefs.tolerance_impact.0, prefs.tolerance_impact.1),
    }
    if mirror {
        log::info!("Mirror mode: the aim strip shows the same text as the game's readout");
    }
    let mut waiting_said = false;
    while !stop.load(Ordering::SeqCst) {
        let Some(window) = find_roblox() else {
            if !waiting_said {
                log::info!("Waiting for the Roblox window...");
                waiting_said = true;
            }
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        waiting_said = false;
        roblox_handle.store(window.as_raw_hwnd() as isize, Ordering::Relaxed);
        let settings = Settings::new(
            window,
            CursorCaptureSettings::WithoutCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Custom(CAPTURE_INTERVAL),
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            Flags {
                window,
                stop: stop.clone(),
                overlay: overlay.clone(),
                manual,
                mirror,
                follow_tune,
                snapshots: snapshots.clone(),
                v1_log: v1_log.clone(),
                arrow_log: arrow_log.clone(),
                latency: latency.clone(),
            },
        );
        match Hud::start_free_threaded(settings) {
            Ok(capture) => {
                while !capture.is_finished() && !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = capture.stop();
            }
            Err(error) => {
                log::warn!("Capture failed to start: {error}");
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
    tray.remove();
}

/// Whether another copy of the calculator is running (two would draw over each other); that one
/// is pointed to.
fn already_running() -> bool {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};
    // Held until the process ends.
    let Ok(_mutex) = (unsafe { CreateMutexW(None, true, windows::core::w!(r"Local\MTCCalculator")) }) else { return false };
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        return false;
    }
    unsafe {
        MessageBoxW(
            None,
            windows::core::w!("MTC Calculator is already running. Its icon is in the notification area at the bottom right of the screen."),
            windows::core::w!("MTC Calculator"),
            MB_OK | MB_ICONINFORMATION,
        )
    };
    true
}

#[cfg(test)]
mod report_tests {
    /// A problem report's log carries the user's folders as their variables.
    #[test]
    fn report_paths_carry_no_user_name() {
        let Ok(local) = std::env::var("LOCALAPPDATA") else { return };
        let line = format!("Log file: {local}\\MTC Calculator\\logs\\hud.log");
        let scrubbed = super::scrub_paths(&line);
        assert_eq!(scrubbed, "Log file: %LOCALAPPDATA%\\MTC Calculator\\logs\\hud.log");
    }
}
