//! Map pipeline: which map, where the player is, which targets are marked.
//!
//! Per processed frame the view cues decide which container (if any) to read back; each view
//! has its own rate. The map is identified on V3/V2 (V1 never overrides a decided map), V1 is
//! registered only when its content changed, and markers feed the target book. The player's arrow
//! is reported as the gun position candidate; the firing solutions come from `fire`.
//!
//! Where the marks are drawn: V2/V3 by their fixed containers and V1 by the canvas
//! (`mtc_vision::map::canvas`), each nudged so the remembered markers sit on the game's diamonds
//! in the latest read. The canvas follows the wheel and drags by the game's
//! measured rules and is drawn as the newest input left it; while V1 is dragged the overlay moves
//! the marks with the cursor between frames (`PanFollow`). Recognition stays the authority: at rest every
//! registration corrects the canvas when it is off by more than `CANVAS_TOLERANCE_PX`; while V1
//! moves it is read every `MOVING_CHECK_INTERVAL_MS`, and the canvas is rebased on the diamonds
//! when no delay lines it up with them within `CANVAS_REBASE_PX` twice in a row (a fast flick the game
//! moved short, a notch it did not take). Each motion's comparison is summarised in the log.
//!
//! A marker dragged on V1 (the left button went down on it) sits at the cursor plus where the
//! cursor grabbed it; the overlay draws it there itself between frames (a captured frame shows
//! the game tens of milliseconds late). V1 is read every frame (diamonds only) so the target book
//! follows; after the button comes up the captured frames still show the diamond moving for that
//! delay, so the drag lasts until its diamond stands still (`DRAG_SETTLE_FRAMES`). The move is
//! reported once at the end, with how far the captured diamond trailed the cursor.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mtc_maps::MapInfo;
use mtc_vision::FrameView;
use mtc_vision::map::arrow::{ArrowSighting, ClippedBlob, fit_clipped_blob_near, player_arrow_candidates_at};
use mtc_vision::map::canvas::{V1Canvas, difference_px};
use mtc_vision::map::impacts::{Impact, ImpactSighting, ImpactTracker, read_impact_under, read_impacts_among};
use mtc_vision::map::markers::{MarkerReader, MarkerSighting, minimap_spot_clear};
use mtc_vision::map::targets::{MapFrame, Target, TargetBook, TargetEvent};
use mtc_vision::map::zoom::{Method, Registration, UNZOOMED_HINT, V1Registrar};

use crate::game_ref::GameReference;

use crate::worker::{OwnedFrame, Worker};
use mtc_vision::map::{MapIdentifier, MapMatch, MapReference, MapTransform, MapView, MapVote, Objective, Thumbnail, ViewCues};

/// A reference-only V1 registration scoring below this (the registrar's own bar for an unzoomed
/// or grid-checked match) at another zoom than the canvas is a false match and is not used: on a
/// reopened 1× Zone 11, whose reference correlates only 0.19–0.30 with the game, the whole-canvas
/// search found 5× at 0.16–0.18 and two such reads moved the canvas 711 px; correct
/// reference-only reads at zoom scored 0.77 (Fulvia Gap).
const WEAK_ZOOM_MIN_SCORE: f32 = 0.30;
/// Read-back intervals per view (ms).
const FULL_INTERVAL_MS: u64 = 100;
/// V1 standing still with no click, wheel or drag for `FULL_INPUT_RECENT_MS`, no landing awaited
/// and not just opened: read this often. What changes on it then (the arrow while driving, an X)
/// keeps for seconds, and a V1 read costs 32–39 ms in play, which made the CPU peaks of
/// 0.57–0.60 core.
const FULL_IDLE_INTERVAL_MS: u64 = 300;
const FULL_INPUT_RECENT_MS: u64 = 1500;
/// V1 gone from the screen this long (ms) was closed, not a cue missed for a frame: the game opens
/// it again at 1×.
const REOPEN_GAP_MS: u64 = 80;
const SCOPE_INTERVAL_MS: u64 = 200;
const MINI_INTERVAL_MS: u64 = 500;
/// While a shot's landing is expected (`expect_impacts`), the minimap and the scope map are read
/// this often: the X shows for ~2 s, and beside an objective only the tips of its arms show.
const WATCH_INTERVAL_MS: u64 = 100;
/// An X whose centre is this near an objective icon's (as |dx| + |dy|, frame px) is under it: the
/// icon's 41 px diamond hides most of the 30 px X (`read_impact_under` reads it from its tips).
const UNDER_ICON_PX: f64 = 24.5;

/// Where and when the shot log expects a shell to land: its X is looked for more often
/// then, and under the objective icons near that point.
#[derive(Clone, Copy, Debug)]
pub struct ExpectedImpact {
    /// The landing window (ms, the frames' clock), the X's ~2 s on screen included.
    pub from_ms: u64,
    pub until_ms: u64,
    /// The predicted landing (normalised map coordinates) and how far from it the X may be (m).
    pub map: (f64, f64),
    pub radius_m: f64,
}
/// A decided map is re-checked this often on V3/V2 (ms).
const RECHECK_INTERVAL_MS: u64 = 15_000;
/// While no map is decided, identification runs at most this often (ms): a thumbnail pass over
/// all references costs ~70 ms.
const UNDECIDED_INTERVAL_MS: u64 = 1_000;
/// V1 signature: luma sampled every this many px; re-register when the mean change exceeds this.
const SIGNATURE_STEP: usize = 24;
const SIGNATURE_CHANGE: f32 = 3.0;
/// V1 counts as moving this long after the last input motion (wheel notch, cursor during a pan):
/// it is followed by its diamonds every frame, then registered again.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(400);
/// A diamond is matched to a remembered marker this close to where the transform puts it (frame
/// px): V3, V1/V2 at rest, V1 under a fit from labelled diamonds, V1 where the canvas puts it.
const MATCH_MINI_PX: f64 = 4.0;
const MATCH_PX: f64 = 6.0;
const MATCH_FITTED_PX: f64 = 12.0;
const MATCH_CANVAS_PX: f64 = 40.0;
/// A frame read while V1 moves shows the canvas as it was this long before (ms) until moving
/// frames have measured it: the delay whose canvas fits a frame's diamonds best was 50 ms in the
/// two longest drags measured (70 and 113 frames; an assumed 33 ms caused 13–23 wrong rebases in
/// each).
const FRAME_DELAY_MS: f64 = 50.0;
/// The last this many moving frames that told delays apart vote for the frame delay (median).
const FRAME_DELAY_VOTES: usize = 41;
/// A moving frame tells delays apart when the best and worst delay miss by this much more (px).
const FRAME_DELAY_SPREAD_PX: f64 = 3.0;
/// While V1 moves it is checked this often (ms); a marker drag is followed every frame.
const MOVING_CHECK_INTERVAL_MS: u64 = 50;
/// A moving check whose diamonds the canvas misses by more than this (px, median) this many times
/// in a row rebases the canvas on them.
const CANVAS_REBASE_PX: f64 = 6.0;
const CANVAS_REBASE_CHECKS: u32 = 2;
/// Once the wheel stopped this long ago and no drag is on, V1 stands still while still `SETTLE`ing:
/// its diamonds then rebase the canvas when it misses them by more than `CANVAS_STILL_REBASE_PX`.
/// After a fast zoom 1× → 5× the canvas was off by 1.5–5 px in no fixed direction and stayed so
/// until the next registration at rest (`--v1-log`; the red frame visibly sat off the diamond
/// after zooming).
const STILL_AFTER: std::time::Duration = std::time::Duration::from_millis(60);
const CANVAS_STILL_REBASE_PX: f64 = 1.0;
/// A diamond this far (px) from where the fit of the others puts it is left out of the fit.
const FIT_OUTLIER_PX: f64 = 3.0;
/// Two diamonds fit a scale only this far apart (frame px); closer ones fit the offset alone.
const MIN_FIT_SPREAD_PX: f64 = 40.0;
/// A released marker has settled once its diamond moved less than `DRAG_STILL_PX` over this many
/// reads in a row, or after `DRAG_SETTLE_MAX`.
const DRAG_SETTLE_FRAMES: u32 = 3;
const DRAG_STILL_PX: f64 = 0.5;
const DRAG_SETTLE_MAX: std::time::Duration = std::time::Duration::from_millis(1500);
/// The canvas is compared with each moving measurement as it was this long before it (ms), for
/// the summary: the delay that fits best is the game's.
const CANVAS_DELAYS_MS: [u64; 22] = [0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 84];
/// The game puts a dragged marker on a lattice of this spacing on the ground (m), counted from the
/// map's centre, at the lattice point nearest the cursor. Lattice fits: 431.7–432.0 cells to the
/// side on Snowy Fields (6461 recorded frames) and 430.0–430.6 on Normandy Bocage (`--v1-log`),
/// against 431.9 and 430.3 for 5 m; the phase as from the centre (Normandy 0.12–0.20 of a cell,
/// 0.14 predicted).
const MARKER_STEP_M: f64 = 5.0;
/// At rest, a registration this far (px) from the canvas corrects it.
const CANVAS_TOLERANCE_PX: f64 = 2.0;
/// Weight of the newest read in the per-view nudge.
const OFFSET_SMOOTHING: f64 = 0.3;

/// The gun position line is reprinted when the arrow moved this far (m).
const GUN_REPORT_M: f64 = 3.0;

/// Where the player's arrow was seen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GunSighting {
    /// Normalised map coordinates.
    pub map: (f64, f64),
    /// In a vehicle (the arrow on a vehicle icon).
    pub mounted: bool,
    /// Frame px per map unit of the view it was seen in.
    pub scale: f64,
    /// Fitted from an arrow cut by the map's edge (less sure).
    pub clipped: bool,
    /// The view it was seen in.
    pub view: MapView,
}

/// What input says about the map; `Default` is "no input" (replay).
#[derive(Clone, Copy, Debug, Default)]
pub struct MapInput {
    /// The left button is held: a marker may be dragged.
    pub dragging: bool,
    /// Whether a right click came recently (a deletion needs one); `None` without input.
    pub right_click: Option<bool>,
    /// The latest right click on V1 away from any marker (placing one): when (ms since start)
    /// and where (frame px); the player's letter is learned from it.
    pub placed: Option<(u64, (f64, f64))>,
}

struct Library {
    maps: Vec<&'static MapInfo>,
    identifier: MapIdentifier,
    /// Each map's reference file hash, the key its saved thumbnail is read back with.
    keys: Vec<u64>,
    /// The thumbnails (7.7 MB for 25 maps) are dropped while a map is decided and read back from
    /// the cache when the objective layout cannot tell the map.
    thumbnails_dropped: bool,
    /// Times they were read back.
    thumbnail_reloads: u32,
}

impl Library {
    fn ensure_thumbnails(&mut self) {
        if !self.thumbnails_dropped {
            return;
        }
        for (index, (info, key)) in self.maps.iter().zip(&self.keys).enumerate() {
            if let Some(t) = std::fs::read(mtc_maps::thumbnail_path(info)).ok().and_then(|bytes| Thumbnail::from_bytes(&bytes, *key)) {
                self.identifier.set_thumbnail(index, t);
            }
        }
        self.thumbnails_dropped = false;
        self.thumbnail_reloads += 1;
    }

    fn drop_thumbnails(&mut self) {
        if !self.thumbnails_dropped {
            self.identifier.drop_thumbnails();
            self.thumbnails_dropped = true;
        }
    }
}

struct CurrentMap {
    info: &'static MapInfo,
}

pub struct MapPipeline {
    /// V1's marker reader, and V2's: at another display configuration each view mapped to the
    /// reference container shows the markers at its own scale (`set_layout`).
    markers: MarkerReader,
    markers_scope: MarkerReader,
    /// The configuration's map icon scale per view, `MapView::ALL` order, and
    /// whether the views come from another configuration than the reference.
    icon_scales: [f64; 3],
    elsewhere: bool,
    library: Arc<Mutex<Option<Library>>>,
    vote: MapVote,
    current: Option<CurrentMap>,
    book: TargetBook,
    last_read: [Option<u64>; 3],
    last_check: Option<u64>,
    last_attempt: Option<u64>,
    signature: Vec<f32>,
    full_transform: Option<MapTransform>,
    /// When `full_transform` was last confirmed by a read.
    full_at: Option<Instant>,
    /// Per view: the nudge (frame px) that puts the remembered markers on the game's diamonds,
    /// for the transform it was measured on.
    offsets: [Option<Offset>; 3],
    track_stats: TrackStats,
    /// The arrow as last seen, the view and its scale (frame px per map unit).
    gun: Option<(ArrowSighting, MapView, f64)>,
    reported_gun: Option<(f64, f64)>,
    /// The map view on screen in the latest frame.
    on_screen: Option<MapView>,
    /// When input last moved V1 (a wheel notch, the cursor during a pan).
    last_motion: Option<Instant>,
    /// Moving checks in a row the canvas missed the diamonds by more than `CANVAS_REBASE_PX`.
    rebase_streak: u32,
    /// Rebases since start.
    rebases_total: u64,
    /// The frame delay (ms) and the moving frames' votes for it.
    frame_delay_ms: f64,
    delay_votes: std::collections::VecDeque<f64>,
    marker_drag: Option<MarkerDrag>,
    last_measure: Option<V1Measure>,
    /// While a marker is dragged: the cursor and the game's diamond in the newest frame (frame px)
    /// and the transform it was read with, for the `--v1-log`.
    marker_sample: Option<((f64, f64), (f64, f64), MapTransform)>,
    /// The latest arrow sighting with its view, that view's zoom and when it was made (for the
    /// arrow log, `--arrow-log`).
    last_arrow: Option<(ArrowSighting, MapView, f64, Instant)>,
    canvas: V1Canvas,
    canvas_stats: CanvasStats,
    /// A registration by reference correlation alone (no grid read) that puts V1 at another zoom
    /// than the canvas, held until the next fresh registration agrees: once at 5× such reads went
    /// 5.0 → 4.6 → 4.4 → 5.0 with no wheel turned, and each would have moved the canvas and the
    /// marker reading.
    pending_zoom: Option<MapTransform>,
    /// When another view (or none) replaced V1 on screen (ms), while it stays away.
    full_gone_since: Option<u64>,
    /// Last click, drag or wheel over V1, or its opening (monotonic ms): `FULL_INPUT_RECENT_MS`.
    last_full_input: Option<u64>,
    /// Map work by view and step since the last report: (view, step, ms, calls).
    lap_totals: Vec<(&'static str, &'static str, f64, u32)>,
    /// V1 openings so far (the game opens it at 1×).
    opening: u32,
    /// The game-made reference of the current map: collected, or in use (`game_ref.rs`).
    game_ref: Option<GameReference>,
    /// Bumped when the current map's reference changed; the registrar is made again.
    reference_generation: u32,
    /// The last registration's unzoomed-hypothesis score.
    last_unzoomed: Option<f32>,
    /// Arrows cut by the container's edge, fitted off the frame loop (`worker.rs`) from the
    /// clipped blobs and the last sighting to search from (map position, heading). A fit takes
    /// 150–200 ms while the arrow turns, which in the loop stalled the readout and the strip at
    /// every minimap read.
    arrow_fit: Worker<(Vec<ClippedBlob>, Option<((f64, f64), f64)>), Option<ArrowSighting>>,
    /// The view, scale and zoom the fit job was read on.
    arrow_fit_on: Option<(MapView, f64, f64)>,
    /// Map identification off the frame loop (about 33 ms a check).
    ident: Worker<IdentJob, IdentResult>,
    /// V1 registration off the frame loop (25–50 ms; the registrar lives on the worker).
    register: Worker<RegisterJob, RegisterResult>,
    /// The last registration came back for the picture now held in `signature` and is not used yet.
    fresh_registration: bool,
    /// How the last successful registration was made.
    last_method: Option<Method>,
    /// The last registration's score.
    last_score: f32,
    /// The map was given on the command line (`--map`): not identified, never changed.
    forced: bool,
    /// The current map has no reference picture (the debug map): V1 is placed by the input alone.
    no_reference: bool,
    /// The player's shells' landings, from the yellow X.
    impacts: ImpactTracker,
    /// Landings first seen since `take_new_impacts`.
    new_impacts: Vec<Impact>,
    /// The shot log's expected landings.
    expected: Vec<ExpectedImpact>,
    /// Development (`--replay … --under-all`): the X under every objective icon is looked for on
    /// every read, expected or not, to count what the reader makes of frames with no X there.
    under_all: bool,
    /// The minimap's transform when last read.
    last_mini: Option<MapTransform>,
}

struct IdentJob {
    frame: OwnedFrame,
    view: MapView,
    origin: (i32, i32),
    /// The configuration's icon scale per view (`MapPipeline::set_placement`), and whether it is
    /// another configuration than the reference.
    icon_scales: [f64; 3],
    elsewhere: bool,
    now_ms: u64,
    /// A map is decided: the thumbnails are dropped after the job.
    decided: bool,
}

enum IdentOutcome {
    Decided(MapMatch, &'static MapInfo),
    Scores(Vec<f32>),
    Nothing,
}

struct IdentResult {
    view: MapView,
    now_ms: u64,
    outcome: IdentOutcome,
}

struct RegisterJob {
    frame: OwnedFrame,
    origin: (i32, i32),
    cursor: Option<(f64, f64)>,
    map: &'static MapInfo,
    signature: Vec<f32>,
    /// Bumped when the map's reference changed (a game-made one saved or dropped).
    generation: u32,
    /// The canvas the wheel and drags led to: the registration checks around it first.
    prior: Option<MapTransform>,
}

struct RegisterResult {
    map: &'static MapInfo,
    signature: Vec<f32>,
    registration: Option<Registration>,
    /// The frame's unzoomed-hypothesis score.
    unzoomed: Option<f32>,
}

/// Identifies the map on a copied container: by the objective layout, else thumbnail scores for
/// the vote (V2, V3); a confident match only on V1 (`identify_full`).
fn identify_job(library: &Mutex<Option<Library>>, job: &IdentJob) -> IdentOutcome {
    let mut guard = library.lock().expect("library lock");
    let Some(library) = guard.as_mut() else { return IdentOutcome::Nothing };
    let view = job.frame.view();
    let t = MapTransform::fixed(job.view, job.origin);
    library.identifier.set_icon_scales(job.icon_scales, job.elsewhere);
    // The objective layout first; the thumbnails (read back if dropped) only when it cannot tell.
    let by_layout = library.identifier.identify_by_layout(&view, &t);
    let outcome = match by_layout {
        Some(m) if job.view != MapView::Full || m.confident_on(MapView::Full) => IdentOutcome::Decided(m, library.maps[m.index]),
        Some(_) => IdentOutcome::Nothing,
        None => {
            library.ensure_thumbnails();
            match library.identifier.scores(&view, &t) {
                Some(s) if job.view == MapView::Full => {
                    let m = MapMatch::best(&s);
                    if m.confident_on(MapView::Full) { IdentOutcome::Decided(m, library.maps[m.index]) } else { IdentOutcome::Nothing }
                }
                Some(s) => IdentOutcome::Scores(s),
                None => IdentOutcome::Nothing,
            }
        }
    };
    if job.decided {
        library.drop_thumbnails();
    }
    outcome
}

/// The V1 registrar for the job's map (made on the worker when the map changes) and its
/// registration of the job's frame.
fn register_job(current: &mut Option<(&'static MapInfo, u32, V1Registrar)>, job: RegisterJob) -> RegisterResult {
    if current.as_ref().is_none_or(|(m, g, _)| m.id != job.map.id || *g != job.generation) {
        let started = Instant::now();
        let (size, cell) = (job.map.size_m(), job.map.grid_cell_m);
        // The game-made reference when there is one (`game_ref.rs`), its screen-size icons masked
        // for the zoomed searches; else the downloaded one.
        let (r, which) = match mtc_maps::load_game_reference(job.map) {
            Some(g) => {
                let masked = crate::game_ref::zoom_masked(job.map, g.width, g.height, &g.data);
                let full = MapReference::from_rgba(&job.map.id, size, g.width, g.height, &g.data);
                let zoomed = MapReference::from_rgba(&job.map.id, size, g.width, g.height, &masked);
                (V1Registrar::with_zoom_reference(&full, &zoomed, cell), "in-game reference")
            }
            None => {
                let img = mtc_maps::load_reference(job.map).expect("cached reference");
                (V1Registrar::new(&MapReference::from_rgba(&job.map.id, size, img.width, img.height, &img.data), cell), "downloaded reference")
            }
        };
        println!("V1 registration ready ({which}, {} ms)", started.elapsed().as_millis());
        *current = Some((job.map, job.generation, r));
    }
    let (_, _, registrar) = current.as_mut().expect("registrar");
    let registration = registrar.register_with_prior(&job.frame.view(), job.origin, job.cursor, job.prior);
    RegisterResult { map: job.map, signature: job.signature, registration, unzoomed: registrar.unzoomed_score() }
}

/// Canvas against recognition since the last summary.
#[derive(Default)]
struct CanvasStats {
    /// While moving, per frame with 2+ diamonds: how far (px) the canvas puts the markers from
    /// their diamonds (median over them), at each delay.
    moving: Vec<[f64; CANVAS_DELAYS_MS.len()]>,
    /// At rest, per registration: the difference (px) before any correction.
    rest: Vec<f64>,
    corrections: u32,
    largest_correction: f64,
    /// Reference-only registrations held back for disagreeing with the canvas zoom.
    held: u32,
    /// Reference-only registrations too weak to change the canvas zoom (`WEAK_ZOOM_MIN_SCORE`).
    ignored: u32,
}

/// A measured V1 transform (for the model log, `--v1-log`): when, how, from how many diamonds.
#[derive(Clone, Debug)]
pub struct V1Measure {
    pub at: Instant,
    pub transform: MapTransform,
    /// "diamonds" (followed while moving) or the registration method.
    pub source: String,
    pub pairs: usize,
}

/// A marker being dragged on V1.
struct MarkerDrag {
    label: String,
    /// Its marker point relative to the cursor when the button went down (frame px).
    grab: (f64, f64),
    /// Where and when the button went down (frame px).
    pressed: (f64, f64),
    pressed_at: Instant,
    /// Where its diamond was read in the latest frame (frame px).
    seen: Option<(f64, f64)>,
    /// The book saw it move.
    moved: bool,
    /// When and where (screen px) the button came up, and reads in a row its diamond has stood
    /// still since.
    released: Option<Instant>,
    released_at: Option<(i32, i32)>,
    still: u32,
    /// Per read: how far the diamond was from the cursor plus the grab offset (px).
    trail_px: Vec<f64>,
}

#[derive(Clone, Copy, Debug)]
struct Offset {
    dx: f64,
    dy: f64,
    /// The transform it was measured on (left, top, zoom, origin).
    on: (f64, f64, f64, f64, f64),
}

fn transform_key(t: &MapTransform) -> (f64, f64, f64, f64, f64) {
    (t.left, t.top, t.zoom, t.origin.0, t.origin.1)
}

/// One motion's checks, printed when V1 settles.
#[derive(Default)]
struct TrackStats {
    frames: u32,
    fitted: u32,
    micros: u128,
    rebases: u32,
}

/// Loads the library into `slot`: every reference the cache lacks is downloaded from the map
/// service, and each map keeps only its identification thumbnail and objective table. A
/// thumbnail is read from its saved copy when that was made from the same reference file; the
/// library is published with those, and the missing ones (first run, a reference that changed)
/// are made afterwards and filled in. Making all 25 takes 2.6 s of one core (the HUD's start-up
/// CPU peak): with `pace` each is followed by a rest of
/// `PACE_REST` times its time, so the thread stays under `1 / (1 + PACE_REST)` of a core. Maps
/// whose thumbnail is not made yet are found by their objective layout alone.
fn load_library(slot: &Mutex<Option<Library>>, pace: bool) -> Vec<&'static MapInfo> {
    let started = Instant::now();
    let mut absent = Vec::new();
    let mut maps = Vec::new();
    let mut identifier = MapIdentifier::new(&[]);
    let missing = mtc_maps::all().iter().filter(|m| !mtc_maps::reference_path(m).is_file()).count();
    if missing > 0 {
        println!("Map pictures: {missing} missing from the cache, downloading from the map service");
    }
    let mut fetched = 0;
    let mut to_make: Vec<(usize, &'static MapInfo, u64)> = Vec::new();
    let mut keys = Vec::new();
    for info in mtc_maps::all() {
        match mtc_maps::ensure_reference(info) {
            Ok(true) => {
                fetched += 1;
                println!("  downloaded {} ({fetched}/{missing})", info.id);
            }
            Ok(false) => {}
            Err(e) => eprintln!("  {} could not be downloaded: {e}", info.id),
        }
        let key = match std::fs::read(mtc_maps::reference_path(info)) {
            Ok(bytes) => mtc_maps::content_hash(&bytes),
            Err(e) => {
                eprintln!("Map picture missing {}: {e}", info.id);
                absent.push(info);
                continue;
            }
        };
        let saved = std::fs::read(mtc_maps::thumbnail_path(info)).ok().and_then(|bytes| Thumbnail::from_bytes(&bytes, key));
        if saved.is_none() {
            to_make.push((maps.len(), info, key));
        }
        let layout: Vec<Objective> = mtc_maps::objectives(&info.id).iter().map(|o| Objective { letter: o.letter.chars().next().expect("letter"), u: o.u, v: o.v }).collect();
        identifier.push_thumbnail(saved, &layout);
        maps.push(info);
        keys.push(key);
    }
    let count = maps.len();
    *slot.lock().expect("library lock") = Some(Library { maps, identifier, keys, thumbnails_dropped: false, thumbnail_reloads: 0 });
    println!("Map library ready: {count} maps ({} thumbnails from the cache, {:.2} s)", count - to_make.len(), started.elapsed().as_secs_f64());
    if to_make.is_empty() {
        return absent;
    }
    let making = Instant::now();
    for (index, info, key) in &to_make {
        let t = Instant::now();
        let thumbnail = match mtc_maps::load_reference(info) {
            Ok(img) => Thumbnail::of(&MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data)),
            Err(e) => {
                eprintln!("Map picture unreadable {}: {e}", info.id);
                continue;
            }
        };
        if let Err(e) = std::fs::write(mtc_maps::thumbnail_path(info), thumbnail.to_bytes(*key)) {
            eprintln!("Thumbnail could not be saved {}: {e}", info.id);
        }
        if let Some(library) = slot.lock().expect("library lock").as_mut() {
            library.identifier.set_thumbnail(*index, thumbnail);
        }
        if pace {
            std::thread::sleep(t.elapsed().mul_f64(PACE_REST));
        }
    }
    println!("Made {} missing thumbnails ({:.1} s), saved to the cache", to_make.len(), making.elapsed().as_secs_f64());
    absent
}

/// The map pictures that could not be downloaded, tried again (`retry::delay`) until all are in;
/// the library is made again whenever one arrives.
fn retry_references(slot: &Mutex<Option<Library>>, absent: Vec<&'static MapInfo>) {
    retry_until_in(absent, crate::retry::delay, |info| mtc_maps::ensure_reference(info).is_ok() && mtc_maps::reference_path(info).is_file(), || load_library(slot, true));
}

/// Tries to `fetch` what is `absent` after each `delay` until nothing is; when anything arrived,
/// `rebuild` makes the library again and says what is still absent.
fn retry_until_in<T: Copy>(mut absent: Vec<T>, delay: impl Fn(usize) -> Duration, fetch: impl Fn(T) -> bool, mut rebuild: impl FnMut() -> Vec<T>) {
    let mut attempt = 0;
    while !absent.is_empty() {
        std::thread::sleep(delay(attempt));
        attempt += 1;
        let arrived = absent.iter().filter(|&&item| fetch(item)).count();
        if arrived > 0 {
            println!("Map pictures: retry {attempt} downloaded {arrived}, rebuilding the map library");
            absent = rebuild();
        } else {
            println!("Map pictures: retry {attempt}, {} still not downloaded, next try in {} s", absent.len(), delay(attempt).as_secs());
        }
    }
}

/// The last arrow sighting a clipped arrow's fit searches from, at most this old.
const ARROW_SEED_MAX_AGE: Duration = Duration::from_secs(3);

/// After making a thumbnail, rest this many times as long (flattens the start-up CPU peak).
const PACE_REST: f64 = 1.5;

fn view_index(view: MapView) -> usize {
    match view {
        MapView::Full => 0,
        MapView::Scope => 1,
        MapView::Mini => 2,
    }
}

fn view_name(view: MapView) -> &'static str {
    match view {
        MapView::Full => "V1",
        MapView::Scope => "V2",
        MapView::Mini => "V3",
    }
}

impl MapPipeline {
    /// The display configuration the views come mapped from: each view's map
    /// icons at `Placement::map_icon_scale` (1 at the reference; from the containers found on
    /// screen once they are).
    pub fn set_placement(&mut self, placement: &mtc_vision::layout::Placement) {
        self.elsewhere = !placement.is_reference();
        let scales = MapView::ALL.map(|v| placement.map_icon_scale(v));
        if scales != self.icon_scales {
            self.icon_scales = scales;
            self.markers = MarkerReader::with_icon_scale(scales[0]);
            self.markers_scope = MarkerReader::with_icon_scale(scales[1]);
        }
    }

    fn icon_scale(&self, view: MapView) -> f64 {
        self.icon_scales[MapView::ALL.iter().position(|&v| v == view).unwrap_or(0)]
    }

    /// `background`: load the reference library on a thread (live) or right away (replay).
    pub fn new(background: bool) -> Self {
        let library = Arc::new(Mutex::new(None));
        let load = {
            let library = library.clone();
            move || {
                let absent = load_library(&library, background);
                // Live, the pictures not downloaded are tried again in the background (from
                // mainland China a quarter of China Unicom's attempts timed out).
                if background {
                    retry_references(&library, absent);
                }
            }
        };
        if background {
            std::thread::Builder::new().name("map-library".into()).spawn(load).expect("map library thread");
        } else {
            load();
        }
        let library_for_ident = library.clone();
        Self {
            markers: MarkerReader::new(),
            markers_scope: MarkerReader::new(),
            icon_scales: [1.0; 3],
            elsewhere: false,
            library,
            vote: MapVote::default(),
            current: None,
            book: TargetBook::default(),
            last_read: [None; 3],
            last_check: None,
            last_attempt: None,
            signature: Vec::new(),
            full_transform: None,
            full_at: None,
            offsets: [None; 3],
            track_stats: TrackStats::default(),
            gun: None,
            reported_gun: None,
            on_screen: None,
            last_motion: None,
            rebase_streak: 0,
            rebases_total: 0,
            frame_delay_ms: FRAME_DELAY_MS,
            delay_votes: std::collections::VecDeque::new(),
            marker_drag: None,
            last_measure: None,
            marker_sample: None,
            last_arrow: None,
            canvas: V1Canvas::default(),
            canvas_stats: CanvasStats::default(),
            pending_zoom: None,
            full_gone_since: None,
            last_full_input: None,
            lap_totals: Vec::new(),
            opening: 0,
            game_ref: None,
            reference_generation: 0,
            last_unzoomed: None,
            arrow_fit: Worker::new(background, "arrow-fit", |(blobs, seed): (Vec<ClippedBlob>, Option<((f64, f64), f64)>)| {
                blobs.iter().filter_map(|b| fit_clipped_blob_near(b, seed)).max_by(|a, b| a.shape.total_cmp(&b.shape))
            }),
            arrow_fit_on: None,
            ident: {
                let library = library_for_ident;
                Worker::new(background, "map-ident", move |job: IdentJob| IdentResult { view: job.view, now_ms: job.now_ms, outcome: identify_job(&library, &job) })
            },
            register: {
                let mut current = None;
                Worker::new(background, "v1-register", move |job: RegisterJob| register_job(&mut current, job))
            },
            fresh_registration: false,
            last_method: None,
            last_score: 0.0,
            forced: false,
            no_reference: false,
            impacts: ImpactTracker::default(),
            new_impacts: Vec::new(),
            expected: Vec::new(),
            under_all: false,
            last_mini: None,
        }
    }

    /// The shot log's landings to come: until the next call, the views showing the
    /// landing are read every `WATCH_INTERVAL_MS`, and an X is looked for under the objective
    /// icons near each landing.
    pub fn expect_impacts(&mut self, expected: Vec<ExpectedImpact>) {
        self.expected = expected;
    }

    /// Development: look for the X under every objective icon on every read.
    pub fn look_under_every_icon(&mut self) {
        self.under_all = true;
    }

    /// The objective whose icon on the minimap, as last read, hides an X landing at `map`
    /// (normalised map coordinates).
    pub fn objective_over(&self, map: (f64, f64)) -> Option<char> {
        let (t, info) = (self.last_mini?, self.current.as_ref()?.info);
        let (x, y) = t.to_frame(map.0, map.1);
        mtc_maps::objectives(&info.id).iter().find_map(|o| {
            let (ox, oy) = t.to_frame(o.u, o.v);
            ((x - ox).abs() + (y - oy).abs() <= UNDER_ICON_PX).then(|| o.letter.chars().next()).flatten()
        })
    }

    /// A landing expected at `now_ms`.
    fn watching(&self, now_ms: u64) -> bool {
        self.expected.iter().any(|e| (e.from_ms..=e.until_ms).contains(&now_ms))
    }

    /// The landings whose X may still show.
    pub fn impacts(&self) -> &[Impact] {
        self.impacts.active()
    }

    /// Landings first seen since the last call (for the shot log).
    pub fn take_new_impacts(&mut self) -> Vec<Impact> {
        std::mem::take(&mut self.new_impacts)
    }

    /// Development (`--map <id>`): `info` is the map, without identifying it (the debug map, which
    /// is in no library, or a library map to skip the wait). A map outside the configuration has no
    /// reference picture: V1 is then placed by the input alone, from 1× when first seen.
    pub fn force_map(&mut self, info: &'static MapInfo) {
        self.set_current(info);
        self.forced = true;
        self.no_reference = mtc_maps::by_id(&info.id).is_none();
        println!("Map: {} (given on the command line{})", info.name, if self.no_reference { "; no reference picture: the full map is tracked from the input and must be at 1x when the HUD opens" } else { "" });
    }

    /// The V1 canvas (for the model log).
    pub fn canvas(&self) -> Option<MapTransform> {
        self.canvas.transform()
    }

    /// The canvas against the diamonds of a moving frame: where it puts each paired marker
    /// against where its diamond is (what the marks would be drawn off by).
    /// Returns the delay that fits best (index into `CANVAS_DELAYS_MS`) and its miss (px).
    fn compare_canvas_moving(&mut self, pairs: &[((f64, f64), (f64, f64))], at: Instant) -> Option<(usize, f64)> {
        if self.canvas.transform().is_none() || pairs.is_empty() {
            return None;
        }
        let diffs = CANVAS_DELAYS_MS.map(|ms| {
            let then = at.checked_sub(std::time::Duration::from_millis(ms)).unwrap_or(at);
            self.canvas.at(then).map_or(f64::NAN, |c| {
                let mut misses: Vec<f64> = pairs
                    .iter()
                    .map(|(m, f)| {
                        let (x, y) = c.to_frame(m.0, m.1);
                        (x - f.0).hypot(y - f.1)
                    })
                    .collect();
                median(&mut misses)
            })
        });
        self.canvas_stats.moving.push(diffs);
        // A frame of a drag fast enough to tell the delays apart votes for its best one.
        let finite: Vec<(usize, f64)> = diffs.iter().copied().enumerate().filter(|(_, d)| d.is_finite()).collect();
        let best = finite.iter().min_by(|a, b| a.1.total_cmp(&b.1));
        let worst = finite.iter().map(|&(_, d)| d).fold(f64::NAN, f64::max);
        let fit = best.copied();
        if let Some(&(k, d)) = best
            && worst - d >= FRAME_DELAY_SPREAD_PX
        {
            // Between the grid's steps: the vertex of the parabola through the best and its
            // neighbours.
            let step = (CANVAS_DELAYS_MS[1] - CANVAS_DELAYS_MS[0]) as f64;
            let mut vote = CANVAS_DELAYS_MS[k] as f64;
            if k > 0 && k + 1 < diffs.len() && diffs[k - 1].is_finite() && diffs[k + 1].is_finite() {
                let (a, b, c) = (diffs[k - 1], d, diffs[k + 1]);
                let curve = a - 2.0 * b + c;
                if curve > 1e-9 {
                    vote += step * ((a - c) / (2.0 * curve)).clamp(-0.5, 0.5);
                }
            }
            self.delay_votes.push_back(vote);
            while self.delay_votes.len() > FRAME_DELAY_VOTES {
                self.delay_votes.pop_front();
            }
            let mut votes: Vec<f64> = self.delay_votes.iter().copied().collect();
            votes.sort_by(f64::total_cmp);
            self.frame_delay_ms = votes[votes.len() / 2];
        }
        fit
    }

    /// How long before a moving frame's arrival the canvas was as the frame shows it.
    fn frame_delay(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.frame_delay_ms / 1000.0)
    }

    /// A registration against the canvas at rest: a clear disagreement (or no canvas yet) puts
    /// the canvas where recognition says, except that a reference-only read (`weak`) at another
    /// zoom waits for the next fresh registration to agree (`pending_zoom`, the frame left
    /// unread meanwhile).
    fn compare_canvas(&mut self, measured: &MapTransform, weak: bool, at: Instant) {
        let Some(now) = self.canvas.transform() else {
            self.canvas.correct(*measured, at);
            self.pending_zoom = None;
            return;
        };
        let d = difference_px(measured, &now);
        self.canvas_stats.rest.push(d);
        if d <= CANVAS_TOLERANCE_PX || self.canvas.panning() {
            self.pending_zoom = None;
            return;
        }
        if weak && (measured.zoom - now.zoom).abs() > 1e-6 {
            let confirmed = self.pending_zoom.is_some_and(|p| (p.zoom - measured.zoom).abs() < 1e-6 && difference_px(&p, measured) <= CANVAS_TOLERANCE_PX);
            if !confirmed {
                println!("V1 registration {:.1}x disagrees with the canvas {:.1}x: this frame is skipped until the next registration confirms", measured.zoom, now.zoom);
                self.pending_zoom = Some(*measured);
                self.canvas_stats.held += 1;
                return;
            }
        }
        self.pending_zoom = None;
        self.canvas.correct(*measured, at);
        self.canvas_stats.corrections += 1;
        self.canvas_stats.largest_correction = self.canvas_stats.largest_correction.max(d);
    }

    fn report_canvas(&mut self) {
        let s = std::mem::take(&mut self.canvas_stats);
        if s.moving.is_empty() && s.rest.is_empty() {
            return;
        }
        let mut line = String::from("Canvas check: ");
        if !s.moving.is_empty() {
            let column = |k: usize| {
                let mut v: Vec<f64> = s.moving.iter().map(|d| d[k]).filter(|d| d.is_finite()).collect();
                let max = v.iter().fold(0.0f64, |a, &b| a.max(b));
                (if v.is_empty() { f64::NAN } else { median(&mut v) }, max)
            };
            let (best, (med, max)) = (0..CANVAS_DELAYS_MS.len()).map(|k| (k, column(k))).min_by(|a, b| a.1.0.total_cmp(&b.1.0)).expect("delays");
            let at_zero = column(0).0;
            line += &format!(
                "moving {} frames, at {} ms latency off by median {med:.1} px, max {max:.0} px (without latency median {at_zero:.1} px); ",
                s.moving.len(),
                CANVAS_DELAYS_MS[best]
            );
        }
        if !s.rest.is_empty() {
            let mut rest = s.rest.clone();
            let max = rest.iter().fold(0.0f64, |a, &b| a.max(b));
            line += &format!("still {} times off by median {:.1} px, max {max:.0} px; ", rest.len(), median(&mut rest));
        }
        line += &format!("the picture follows the input of {:.0} ms before; {} corrections", self.frame_delay_ms, s.corrections);
        if s.ignored > 0 {
            line += &format!(", {} untrusted reference registrations (read by the canvas)", s.ignored);
        }
        if s.held > 0 {
            line += &format!(", {} unconfirmed zooms held back", s.held);
        }
        if s.corrections > 0 {
            line += &format!(" (max {:.0} px)", s.largest_correction);
        }
        println!("{line}");
    }

    /// The latest arrow sighting: the arrow, its view, the view's zoom and when.
    pub fn last_arrow(&self) -> Option<&(ArrowSighting, MapView, f64, Instant)> {
        self.last_arrow.as_ref()
    }

    /// The latest V1 measurement (registration or diamonds).
    pub fn last_measure(&self) -> Option<&V1Measure> {
        self.last_measure.as_ref()
    }

    /// The newest dragged-marker sample not taken yet (cursor, diamond, transform).
    pub fn take_marker_sample(&mut self) -> Option<((f64, f64), (f64, f64), MapTransform)> {
        self.marker_sample.take()
    }

    /// How many times the canvas was rebased on diamonds so far (a change since a drawing means
    /// the marks moved).
    pub fn canvas_rebases(&self) -> u64 {
        self.rebases_total
    }

    /// The moment the canvas drawn now shows: when its newest input was read. The overlay moves a
    /// dragged map's marks by the cursor's travel since this moment (`mtc_overlay::PanFollow`);
    /// stamped later (when the marks were made, a few ms on) the marks fell behind by the cursor's
    /// travel in between.
    pub fn drawn_at(&self) -> Instant {
        self.canvas.latest_at().unwrap_or_else(Instant::now)
    }

    /// The V1 canvas as drawn now (at `drawn_at`).
    pub fn drawn(&self) -> Option<MapTransform> {
        self.canvas.at(self.drawn_at()).or(self.canvas.transform())
    }

    /// While V1 is dragged: the moment the marks drawn now are placed for, how far (px: left, up,
    /// right, down) the map can still follow the cursor from there before it stops at its edge
    /// (the canvas origin stays within 0..1 − 1/zoom; a cursor move of d px moves it −d/scale),
    /// and where and when the button went down (frame px; the map waits for the cursor to pass
    /// `DRAG_THRESHOLD_PX` from there).
    #[allow(clippy::type_complexity)]
    pub fn pan_follow(&self) -> Option<(Instant, (f32, f32, f32, f32), ((f64, f64), Instant))> {
        if self.on_screen != Some(MapView::Full) || !self.canvas.panning() {
            return None;
        }
        let at = self.drawn_at();
        let t = self.canvas.at(at)?;
        let max = (1.0 - 1.0 / t.zoom).max(0.0);
        let s = t.scale();
        let limits = (((t.origin.0 - max) * s) as f32, ((t.origin.1 - max) * s) as f32, (t.origin.0 * s) as f32, (t.origin.1 * s) as f32);
        Some((at, limits, self.canvas.pressed()?))
    }

    /// The left button went down on marker `label` with the cursor at `cursor` (frame px).
    pub fn begin_marker_drag(&mut self, label: &str, cursor: (f64, f64), at: Instant) {
        let Some(t) = self.v1_transform().filter(|_| self.on_screen == Some(MapView::Full)) else { return };
        let Some(target) = self.book.targets().iter().find(|t| t.label == label) else { return };
        let (x, y) = t.to_frame(target.map.0, target.map.1);
        self.marker_drag = Some(MarkerDrag {
            label: label.to_owned(),
            grab: (x - cursor.0, y - cursor.1),
            pressed: cursor,
            pressed_at: at,
            seen: None,
            moved: false,
            released: None,
            released_at: None,
            still: 0,
            trail_px: Vec::new(),
        });
    }

    /// While a marker is dragged: the lattice the game puts it on (`MARKER_STEP_M`), as a lattice
    /// point (the map's centre, frame px) and the step (px).
    pub fn drag_snap(&self) -> Option<((f64, f64), f64)> {
        self.marker_drag.as_ref()?;
        let (t, map) = (self.v1_transform()?, self.map()?);
        let cells = map.size_studs * mtc_maps::METRES_PER_STUD / MARKER_STEP_M;
        Some((t.to_frame(0.5, 0.5), t.size * t.zoom / cells))
    }

    /// The button came up at `at` (screen px): the drag goes on until the marker settles.
    pub fn release_marker_drag(&mut self, at: (i32, i32)) {
        if let Some(drag) = self.marker_drag.as_mut().filter(|d| d.released.is_none()) {
            drag.released = Some(Instant::now());
            drag.released_at = Some(at);
        }
    }

    /// Ends a drag now (settled, or the game lost focus).
    pub fn end_marker_drag(&mut self) {
        let Some(mut drag) = self.marker_drag.take() else { return };
        if !drag.moved {
            return;
        }
        let trail = if drag.trail_px.is_empty() {
            String::new()
        } else {
            let max = drag.trail_px.iter().fold(0.0f64, |a, &b| a.max(b));
            format!(" ({} frames, the diamond on screen behind the cursor by median {:.0} px, max {max:.0} px)", drag.trail_px.len(), median(&mut drag.trail_px))
        };
        println!("Target {} moved{trail}", drag.label);
    }

    pub fn marker_dragging(&self) -> bool {
        self.marker_drag.is_some()
    }

    /// V1 is moving or a marker on it is being dragged: read every frame.
    pub fn v1_active(&self) -> bool {
        self.v1_moving() || self.marker_drag.is_some()
    }

    /// The marker being dragged: its label, the grab offset (px from the cursor to its marker
    /// point) and where the button came up (screen px).
    #[allow(clippy::type_complexity)]
    pub fn drag(&self) -> Option<(&str, (f64, f64), Option<(i32, i32)>, ((f64, f64), Instant))> {
        let d = self.marker_drag.as_ref()?;
        Some((d.label.as_str(), d.grab, d.released_at, (d.pressed, d.pressed_at)))
    }

    /// The V1 transform to draw with: the canvas as drawn now, nudged onto the diamonds (before
    /// the first registration placed the canvas: none).
    fn v1_transform(&self) -> Option<MapTransform> {
        self.drawn().or(self.full_transform).map(|t| self.nudged(MapView::Full, t))
    }

    /// `t` moved by the view's nudge measured on it.
    fn nudged(&self, view: MapView, t: MapTransform) -> MapTransform {
        match self.offsets[view_index(view)] {
            Some(o) if o.on == transform_key(&t) => MapTransform { left: t.left + o.dx, top: t.top + o.dy, ..t },
            _ => t,
        }
    }

    /// Whether the map scale on screen is known (the blast circles wait for it): V1's comes from
    /// the canvas, whose zoom follows the notches exactly.
    pub fn scale_known(&self) -> bool {
        self.on_screen != Some(MapView::Full) || self.canvas.transform().is_some()
    }

    /// V1 is moving by input (a pan, or a notch less than `SETTLE` ago).
    pub fn v1_moving(&self) -> bool {
        self.canvas.panning() || self.last_motion.is_some_and(|at| at.elapsed() < SETTLE)
    }

    /// Adds `ms` to step `step` of view `view` in the per-report totals (`take_laps`).
    fn add_lap(&mut self, view: &'static str, step: &'static str, ms: f64) {
        match self.lap_totals.iter_mut().find(|l| l.0 == view && l.1 == step) {
            Some(l) => {
                l.2 += ms;
                l.3 += 1;
            }
            None => self.lap_totals.push((view, step, ms, 1)),
        }
    }

    /// Whether the identification thumbnails are dropped and how often they were read back
    /// (`None` while the library is loading or busy).
    pub fn thumbnail_state(&self) -> Option<(bool, u32)> {
        self.library.try_lock().ok()?.as_ref().map(|l| (l.thumbnails_dropped, l.thumbnail_reloads))
    }

    /// The map work since the last call by view and step: (view, step, ms in all, calls), for the
    /// stats report (every map step's share, not only those of slow frames).
    pub fn take_laps(&mut self) -> Vec<(&'static str, &'static str, f64, u32)> {
        std::mem::take(&mut self.lap_totals)
    }

    /// A click, a drag or the wheel over V1 at `now_ms` (monotonic): V1 is read at the full rate
    /// for a while (a marker placed or deleted shows at once).
    pub fn note_input(&mut self, now_ms: u64) {
        self.last_full_input = Some(now_ms);
    }

    /// Wheel notches with the cursor at `cursor` (frame px) over V1: the game zooms 0.2 per notch
    /// between 1 and 5 at once (no animation) and keeps the map point under the cursor in place.
    pub fn wheel(&mut self, notches: f32, cursor: (f64, f64), at: Instant) {
        if self.on_screen == Some(MapView::Full) {
            self.canvas.wheel(notches, cursor, at);
            self.last_motion = Some(Instant::now());
        }
    }

    /// The left button went down at `cursor` over V1, not on a marker: the map follows the cursor.
    pub fn begin_pan(&mut self, cursor: (f64, f64), at: Instant) {
        if self.on_screen == Some(MapView::Full) {
            self.canvas.press(cursor, false, at);
            self.last_motion = Some(Instant::now());
        }
    }

    /// The cursor during a pan (frame px).
    pub fn pan_to(&mut self, cursor: (f64, f64)) {
        self.canvas.moved(cursor, Instant::now());
        if self.canvas.panning() {
            self.last_motion = Some(Instant::now());
        }
    }

    pub fn end_pan(&mut self) {
        self.canvas.release();
    }

    /// The view on screen and where its map sits: fixed for V2/V3, the latest registration or
    /// following for V1 (none until it registered); nudged onto the game's diamonds.
    pub fn shown_transform(&self, origin: (i32, i32)) -> Option<MapTransform> {
        match self.on_screen? {
            MapView::Full => self.v1_transform(),
            view => Some(self.nudged(view, MapTransform::fixed(view, origin))),
        }
    }

    /// The identified map.
    pub fn map(&self) -> Option<&'static MapInfo> {
        self.current.as_ref().map(|c| c.info)
    }

    /// The player's arrow as last seen.
    pub fn arrow(&self) -> Option<GunSighting> {
        self.gun.map(|(g, view, scale)| GunSighting { map: g.map, mounted: g.mounted, scale, clipped: g.clipped, view })
    }

    /// The remembered markers.
    pub fn targets(&self) -> &[Target] {
        self.book.targets()
    }

    /// Takes the map view on screen from the cues (at `now_ms`). V1 opened again shows the whole
    /// map: the game resets its zoom to 1× on closing, so the canvas does too, rather than drawing
    /// the marks at the old zoom until two registrations agree (a 1× read against a zoomed canvas
    /// is held for confirmation, `compare_canvas`).
    pub fn see(&mut self, cues: &ViewCues, now_ms: u64) {
        let view = cues.primary();
        if view == Some(MapView::Full) {
            let reopened = self.full_gone_since.take().is_some_and(|t| now_ms.saturating_sub(t) >= REOPEN_GAP_MS);
            if reopened || self.on_screen != Some(MapView::Full) {
                // Just opened: read at the full rate for a while.
                self.last_full_input = Some(now_ms);
            }
            if reopened {
                self.opening += 1;
            }
            if reopened && let Some(t) = self.canvas.transform().filter(|t| t.zoom > 1.0 + 1e-6 || t.origin != (0.0, 0.0)) {
                println!("V1 reopened: canvas reset from {:.1}x to 1.0x", t.zoom);
                self.canvas.correct(MapTransform { zoom: 1.0, origin: (0.0, 0.0), ..t }, Instant::now());
                self.pending_zoom = None;
            }
        } else if self.full_gone_since.is_none() {
            self.full_gone_since = Some(now_ms);
        }
        self.on_screen = view;
    }

    /// The container to read back at `now_ms` (monotonic), if a map view is shown and due.
    pub fn wanted(&mut self, cues: &ViewCues, origin: (i32, i32), now_ms: u64) -> Option<(MapView, (i32, i32, i32, i32))> {
        self.see(cues, now_ms);
        let view = self.on_screen?;
        if !self.v1_moving() && self.track_stats.frames > 0 {
            self.report_tracking();
        }
        let interval = match view {
            MapView::Full if self.marker_drag.is_some() => 0,
            MapView::Full if self.v1_moving() => MOVING_CHECK_INTERVAL_MS,
            MapView::Full if self.watching(now_ms) || self.last_full_input.is_some_and(|t| now_ms.saturating_sub(t) < FULL_INPUT_RECENT_MS) => FULL_INTERVAL_MS,
            MapView::Full => FULL_IDLE_INTERVAL_MS,
            MapView::Scope | MapView::Mini if self.watching(now_ms) => WATCH_INTERVAL_MS,
            MapView::Scope => SCOPE_INTERVAL_MS,
            MapView::Mini => MINI_INTERVAL_MS,
        };
        let slot = &mut self.last_read[view_index(view)];
        if slot.is_some_and(|t| now_ms.saturating_sub(t) < interval) {
            return None;
        }
        *slot = Some(now_ms);
        Some((view, view.frame_rect(origin)))
    }

    /// Processes the read-back container of `view`.
    pub fn process(&mut self, frame: &FrameView, view: MapView, cues: &ViewCues, origin: (i32, i32), cursor: Option<(f64, f64)>, input: MapInput, now_ms: u64) {
        let started = Instant::now();
        let mut laps: Vec<(&str, f64)> = Vec::new();
        let mut lap = |name: &'static str, from: &mut Instant| {
            laps.push((name, from.elapsed().as_secs_f64() * 1000.0));
            *from = Instant::now();
        };
        let mut t = Instant::now();
        self.collect_arrow_fit();
        if !self.forced {
            self.collect_ident();
            if view != MapView::Full {
                self.identify(frame, view, origin, now_ms);
            } else if self.current.is_none() {
                self.identify_full(frame, origin, now_ms);
            }
        }
        let Some(info) = self.current.as_ref().map(|c| c.info) else { return };
        if view == MapView::Full && self.v1_moving() {
            let t0 = Instant::now();
            self.track_v1(frame, origin);
            self.add_lap("V1", "following motion", t0.elapsed().as_secs_f64() * 1000.0);
            return;
        }
        let transform = match view {
            MapView::Full => match self.register_full(frame, origin, cursor) {
                Some((t, fresh)) => {
                    self.full_at = Some(Instant::now());
                    let method = self.last_method;
                    self.last_measure = Some(V1Measure { at: Instant::now(), transform: t, source: method.map(|m| format!("{m:?}")).unwrap_or_default(), pairs: 0 });
                    // Too weak to move the canvas to another zoom: the frame is read on the canvas.
                    if method == Some(Method::Reference)
                        && self.last_score < WEAK_ZOOM_MIN_SCORE
                        && let Some(c) = self.canvas.transform().filter(|c| (c.zoom - t.zoom).abs() > 1e-6)
                    {
                        if fresh {
                            self.canvas_stats.ignored += 1;
                        }
                        self.pending_zoom = None;
                        c
                    } else {
                        if fresh {
                            self.compare_canvas(&t, method == Some(Method::Reference), Instant::now());
                        }
                        // A registration held for confirmation leaves the frame unread: the canvas
                        // still has the old zoom, and reading the arrow and markers with it put the
                        // gun hundreds of metres off (in one recording four held frames each gave
                        // a wrong gun position, three of them V1 reopened at 1× after closing zoomed).
                        if self.pending_zoom.is_some() {
                            return;
                        }
                        t
                    }
                }
                None => return,
            },
            v => MapTransform::fixed(v, origin),
        };
        if view == MapView::Full {
            self.offer_game_reference(frame, origin);
        }
        // Following a dragged marker: the cheaper pass, every frame; the view stands still, so its
        // nudge stays (a moving marker would only drag it along).
        lap("identify and register", &mut t);
        let candidates = player_arrow_candidates_at(frame, &transform, self.icon_scale(view));
        match candidates.complete {
            Some(arrow) => self.take_arrow(arrow, view, transform.scale(), transform.zoom),
            // An arrow cut by the edge is fitted on the worker; one fit at a time.
            None if !candidates.clipped.is_empty() && !self.arrow_fit.busy() => {
                // From the last sighting when it is recent (the global search costs 167 ms a fit
                // and runs back to back while the arrow stays cut by the edge).
                let seed = self.last_arrow.filter(|(.., at)| at.elapsed() <= ARROW_SEED_MAX_AGE).map(|(a, ..)| (a.map, a.heading_deg));
                if self.arrow_fit.send((candidates.clipped, seed)) {
                    self.arrow_fit_on = Some((view, transform.scale(), transform.zoom));
                }
                // Inline (replay): back at once.
                self.collect_arrow_fit();
            }
            None => {}
        }
        lap("arrow", &mut t);
        let markers = if view == MapView::Scope { &self.markers_scope } else { &self.markers };
        let mut found = if self.marker_drag.is_some() { markers.read_diamonds(frame, &transform) } else { markers.read(frame, &transform) };
        // A label on the player's own vehicle icon is a piece of it (a squad's icons are green).
        if let Some(icon) = candidates.complete.map(|a| a.frame) {
            found.retain(|s| !markers.label_on_icon(s, icon));
        }
        lap("markers", &mut t);
        let mut settled = false;
        match &mut self.marker_drag {
            Some(drag) => {
                let seen = found.iter().find(|s| s.complete && s.label.as_deref() == Some(drag.label.as_str())).map(|s| s.frame);
                if let (Some(s), Some(c)) = (seen, cursor) {
                    self.marker_sample = Some((c, s, transform));
                }
                if let Some(released) = drag.released {
                    let still = matches!((seen, drag.seen), (Some(a), Some(b)) if (a.0 - b.0).hypot(a.1 - b.1) < DRAG_STILL_PX);
                    drag.still = if still { drag.still + 1 } else { 0 };
                    settled = drag.still >= DRAG_SETTLE_FRAMES || released.elapsed() >= DRAG_SETTLE_MAX;
                } else if let (Some(seen), Some(c)) = (seen, cursor) {
                    drag.trail_px.push((seen.0 - c.0 - drag.grab.0).hypot(seen.1 - c.1 - drag.grab.1));
                }
                drag.seen = seen;
            }
            // On V1 against the canvas as drawn (the nudge applies to the transform it was
            // measured on: measured on the registration, it never reached a canvas within the
            // correction tolerance of it).
            None => {
                let on = if view == MapView::Full { self.drawn().unwrap_or(transform) } else { transform };
                self.update_offset(view, &on, &found);
            }
        }
        let spot = |m: (f64, f64)| minimap_spot_clear(frame, &transform, m);
        let events = self.book.observe(&MapFrame {
            t_ms: now_ms,
            transform,
            markers: &found,
            remove_hint: cues.remove_hint(),
            dragging: (input.dragging || self.marker_drag.is_some()) && view == MapView::Full,
            // "Remove marker" also shows while a marker is dragged: a deletion needs a right click.
            right_click: input.right_click,
            cursor,
            client_origin: origin,
            spot_clear: (view == MapView::Mini).then_some(&spot as &dyn Fn((f64, f64)) -> bool),
            placed: if view == MapView::Full { input.placed } else { None },
        });
        lap("target book", &mut t);
        // The objective icons are drawn over the X: an X beside one (often on the minimap) is
        // read from the part in view.
        let objectives = mtc_maps::objectives(&info.id);
        let icons: Vec<(f64, f64)> = objectives.iter().map(|o| transform.to_frame(o.u, o.v)).collect();
        let mut sightings = read_impacts_among(frame, &transform, &icons);
        // An X mostly under an icon, read from its tips: only near a landing expected now (a
        // landing's X shows ~2 s), and where no X beside the icon was read.
        let px_per_m = transform.scale() / info.size_m();
        let mut under: Vec<(char, ImpactSighting)> = Vec::new();
        for (o, &icon) in objectives.iter().zip(&icons) {
            let expected = self.under_all
                || self.expected.iter().any(|e| {
                    let (x, y) = transform.to_frame(e.map.0, e.map.1);
                    (e.from_ms..=e.until_ms).contains(&now_ms) && (x - icon.0).hypot(y - icon.1) <= e.radius_m * px_per_m + UNDER_ICON_PX
                });
            if !expected || sightings.iter().any(|s| (s.frame.0 - icon.0).abs() + (s.frame.1 - icon.1).abs() <= UNDER_ICON_PX + 15.0) {
                continue;
            }
            if let Some(s) = read_impact_under(frame, &transform, icon, &icons) {
                under.push((o.letter.chars().next().unwrap_or('?'), s));
            }
        }
        sightings.extend(under.iter().map(|&(_, s)| s));
        if view == MapView::Mini {
            self.last_mini = Some(transform);
        }
        for impact in self.impacts.observe(now_ms, &transform, &sightings) {
            let (x, y) = transform.to_frame(impact.map.0, impact.map.1);
            let note = under.iter().find(|(_, s)| (s.frame.0 - x).hypot(s.frame.1 - y) < 1.0).map(|(letter, s)| format!(" (under objective {letter}'s icon, {:.0}% of the arms seen)", s.arms * 100.0));
            self.report_impact(&impact, view, info, note.as_deref().unwrap_or(""));
            self.new_impacts.push(impact);
        }
        lap("impacts", &mut t);
        let view_key = view_name(view);
        for &(name, ms) in &laps {
            self.add_lap(view_key, name, ms);
        }
        if started.elapsed() > std::time::Duration::from_millis(30) {
            let parts: Vec<String> = laps.iter().map(|(n, ms)| format!("{n} {ms:.1}")).collect();
            println!("Slow map step ({}, {:.0} ms): {}", view_name(view), started.elapsed().as_secs_f64() * 1000.0, parts.join(" · "));
        }
        let gun_moved = match (self.gun, self.reported_gun) {
            (Some((g, _, _)), Some(r)) => dist_m(g.map, r, info) > GUN_REPORT_M,
            (Some(_), None) => true,
            _ => false,
        };
        for e in &events {
            match e {
                TargetEvent::Added(l) => println!("Target +{l}"),
                TargetEvent::Removed(l) => println!("Target -{l}"),
                TargetEvent::Moved(l) => match &mut self.marker_drag {
                    Some(drag) if &drag.label == l => drag.moved = true,
                    _ => println!("Target {l} moved"),
                },
                TargetEvent::Cleared => println!("Targets all cleared"),
                TargetEvent::Series(letter) => println!("Own markers are the {letter} series (seen when placing one); markers taken as own before are dropped"),
            }
        }
        // A dragged marker moves every read: the position line waits for other news.
        if gun_moved || events.iter().any(|e| !matches!(e, TargetEvent::Moved(_))) {
            self.report(info);
        }
        if settled {
            self.end_marker_drag();
        }
    }

    fn take_arrow(&mut self, arrow: ArrowSighting, view: MapView, scale: f64, zoom: f64) {
        self.gun = Some((arrow, view, scale));
        self.last_arrow = Some((arrow, view, zoom, Instant::now()));
    }

    /// A clipped arrow's fit that has come back from the worker, taken as a sighting of the view
    /// and scale it was read on.
    fn collect_arrow_fit(&mut self) {
        let Some(arrow) = self.arrow_fit.take() else { return };
        if let (Some(arrow), Some((view, scale, zoom))) = (arrow, self.arrow_fit_on.take()) {
            self.take_arrow(arrow, view, scale, zoom);
        }
    }

    /// An identification that has come back: adopted when decided (a thumbnail score set goes to
    /// the vote first).
    fn collect_ident(&mut self) {
        let Some(result) = self.ident.take() else { return };
        let decided = match result.outcome {
            IdentOutcome::Decided(m, info) => Some((m, info)),
            IdentOutcome::Scores(scores) => self.vote.push(scores).and_then(|m| {
                let guard = self.library.lock().expect("library lock");
                guard.as_ref().map(|l| (m, l.maps[m.index]))
            }),
            IdentOutcome::Nothing => None,
        };
        if let Some((m, info)) = decided {
            self.last_check = Some(result.now_ms);
            self.adopt(m, info, result.view);
        }
    }

    /// A registration that has come back, for the current map: the picture it was made on and its
    /// transform become the ones to compare and draw with.
    fn collect_registration(&mut self) {
        let Some(result) = self.register.take() else { return };
        if self.current.as_ref().is_none_or(|c| c.info.id != result.map.id) {
            return;
        }
        if let Some(r) = result.registration {
            if self.full_transform.is_none_or(|t| (t.zoom - r.transform.zoom).abs() > 1e-6) {
                println!("V1 zoom {:.1}x ({:?})", r.transform.zoom, r.method);
            }
            self.last_method = Some(r.method);
            self.last_score = r.score;
        }
        self.last_unzoomed = result.unzoomed;
        self.signature = result.signature;
        self.full_transform = result.registration.map(|r| r.transform);
        self.fresh_registration = true;
    }

    /// A new landing: where, and how far and in which direction from the gun's last sighting.
    fn report_impact(&self, impact: &Impact, view: MapView, info: &MapInfo, note: &str) {
        let w = info.size_m();
        let from_gun = self.gun.map(|(g, _, _)| {
            let (dx, dy) = ((impact.map.0 - g.map.0) * w, (impact.map.1 - g.map.1) * w);
            format!(", {:.1} m from the gun at azimuth {:.2}°", dx.hypot(dy), (dx.atan2(-dy).to_degrees() + 360.0) % 360.0)
        });
        println!(
            "Impact #{} ({}){note}: x {:.1} m  y {:.1} m{}; strength {:.2}, landed about {:.1} s ago",
            impact.id,
            view_name(view),
            impact.map.0 * w,
            impact.map.1 * w,
            from_gun.unwrap_or_default(),
            impact.first_alpha,
            (impact.first_ms - impact.landed_ms) as f64 / 1000.0
        );
    }

    fn report(&mut self, info: &MapInfo) {
        let Some((gun, view, _)) = self.gun else {
            println!(" (player arrow not found)");
            return;
        };
        self.reported_gun = Some(gun.map);
        let w = info.size_m();
        println!(
            "Gun ({} arrow{}): x {:.1} m  y {:.1} m",
            view_name(view),
            if gun.mounted { "" } else { ", not in a vehicle" },
            gun.map.0 * w,
            gun.map.1 * w
        );
    }

    /// While no map is decided, V1 decides it too (the HUD started with the full map open): by the
    /// objective layout, or one frame above the V1 thresholds only — a zoomed V1 can look like
    /// another map unzoomed. Labelled V1 frames: unzoomed 244/244 decided right, zoomed 140 none.
    fn identify_full(&mut self, frame: &FrameView, origin: (i32, i32), now_ms: u64) {
        if self.last_attempt.is_some_and(|t| now_ms.saturating_sub(t) < UNDECIDED_INTERVAL_MS) {
            return;
        }
        if self.ident.busy() {
            return;
        }
        self.last_attempt = Some(now_ms);
        self.ident.send(IdentJob { frame: OwnedFrame::copy(frame), view: MapView::Full, origin, icon_scales: self.icon_scales, elsewhere: self.elsewhere, now_ms, decided: self.current.is_some() });
        self.collect_ident();
    }

    fn identify(&mut self, frame: &FrameView, view: MapView, origin: (i32, i32), now_ms: u64) {
        if self.current.is_some() && self.last_check.is_some_and(|t| now_ms.saturating_sub(t) < RECHECK_INTERVAL_MS) {
            return;
        }
        if self.last_attempt.is_some_and(|t| now_ms.saturating_sub(t) < UNDECIDED_INTERVAL_MS) {
            return;
        }
        if self.ident.busy() {
            return;
        }
        self.last_attempt = Some(now_ms);
        // The objective layout decides at once; the thumbnail vote is the fallback.
        self.ident.send(IdentJob { frame: OwnedFrame::copy(frame), view, origin, icon_scales: self.icon_scales, elsewhere: self.elsewhere, now_ms, decided: self.current.is_some() });
        self.collect_ident();
    }

    /// Takes `info` as the map on screen (decided on `view`), unless it already is.
    fn adopt(&mut self, decided: MapMatch, info: &'static MapInfo, view: MapView) {
        if self.current.as_ref().is_some_and(|c| c.info.id == info.id) {
            return;
        }
        println!(
            "Map: {} ({} {}, score {:.2}, lead {:.2})",
            info.name,
            view_name(view),
            if decided.by_layout { "by objective layout" } else { "by thumbnail" },
            decided.score,
            decided.margin
        );
        crate::diag::note(crate::diag::Event::Map(info.id.clone(), if decided.by_layout { "layout" } else { "thumbnail" }));
        self.set_current(info);
    }

    /// A V1 frame at rest to the game-made reference (`game_ref.rs`): taken while the canvas has the
    /// whole map (1×) and the frame looks unzoomed to the registrar; checked while one is in use.
    fn offer_game_reference(&mut self, frame: &FrameView, origin: (i32, i32)) {
        let (Some(info), Some(score)) = (self.current.as_ref().map(|c| c.info), self.last_unzoomed) else { return };
        let whole = self.canvas.transform().is_some_and(|c| (c.zoom - 1.0).abs() < 1e-6 && c.origin == (0.0, 0.0));
        let Some(g) = self.game_ref.as_mut().filter(|_| whole && score >= UNZOOMED_HINT) else { return };
        if g.check(self.opening, score) {
            println!("In-game reference ({}): several full-map openings did not match, collecting again", info.name);
            let _ = mtc_maps::remove_game_reference(info);
            self.reference_generation += 1;
            return;
        }
        if let Some(path) = g.offer(info, frame, &MapTransform::fixed(MapView::Full, origin), self.opening, Instant::now()) {
            println!("In-game reference ({}): composed and saved to {}; the full map registers against it now", info.name, path.display());
            self.reference_generation += 1;
        }
    }

    /// `info` becomes the map: what was known about the last one goes.
    fn set_current(&mut self, info: &'static MapInfo) {
        self.current = Some(CurrentMap { info });
        self.game_ref = Some(GameReference::for_map(info));
        self.last_unzoomed = None;
        self.fresh_registration = false;
        self.last_method = None;
        self.book.reset();
        self.book.set_objectives(mtc_maps::objectives(&info.id).iter().map(|o| (o.u, o.v)).collect());
        self.gun = None;
        self.reported_gun = None;
        self.signature.clear();
        self.full_transform = None;
        self.full_at = None;
        self.offsets = [None; 3];
        self.canvas = V1Canvas::default();
        self.pending_zoom = None;
        self.vote.clear();
        self.impacts = ImpactTracker::default();
        self.new_impacts.clear();
    }

    /// The nudge that puts the remembered markers on the diamonds just read on `t`: the median of
    /// their differences, smoothed while the transform stays the same.
    fn update_offset(&mut self, view: MapView, t: &MapTransform, found: &[MarkerSighting]) {
        let radius = if view == MapView::Mini { MATCH_MINI_PX } else { MATCH_PX };
        let pairs = match_sightings(self.book.targets(), found, t, radius);
        if pairs.is_empty() {
            return;
        }
        let mut dx: Vec<f64> = pairs.iter().map(|(m, f)| f.0 - t.to_frame(m.0, m.1).0).collect();
        let mut dy: Vec<f64> = pairs.iter().map(|(m, f)| f.1 - t.to_frame(m.0, m.1).1).collect();
        let (dx, dy) = (median(&mut dx), median(&mut dy));
        let on = transform_key(t);
        let slot = &mut self.offsets[view_index(view)];
        *slot = Some(match *slot {
            Some(o) if o.on == on => Offset { dx: o.dx + OFFSET_SMOOTHING * (dx - o.dx), dy: o.dy + OFFSET_SMOOTHING * (dy - o.dy), on },
            _ => Offset { dx, dy, on },
        });
    }

    /// Checks a moving V1 against its diamonds: matches them to the remembered markers (by label,
    /// then near where the canvas puts them), fits the view to them, and rebases the canvas when it
    /// keeps missing them.
    fn track_v1(&mut self, frame: &FrameView, origin: (i32, i32)) {
        let started = Instant::now();
        let now = Instant::now();
        let then = now.checked_sub(self.frame_delay()).unwrap_or(now);
        // The canvas as this frame shows it, on the container as read back (no nudge).
        let container = MapTransform::fixed(MapView::Full, origin);
        let Some(seed) = self.canvas.at(then).map(|t| MapTransform { left: container.left, top: container.top, size: container.size, ..t }) else { return };
        let found = self.markers.read_diamonds(frame, &seed);
        let targets = self.book.targets();
        let labelled: Vec<MarkerSighting> = found.iter().filter(|s| s.complete && s.label.is_some()).cloned().collect();
        let mut pairs = match_sightings(targets, &labelled, &seed, f64::INFINITY);
        let (guide, radius) = match fit_transform(&pairs, &seed, false) {
            Some((t, _)) => (t, MATCH_FITTED_PX),
            None => (seed, MATCH_CANVAS_PX),
        };
        let unlabelled: Vec<MarkerSighting> = found.iter().filter(|s| s.complete && s.label.is_none()).cloned().collect();
        let free: Vec<Target> = targets.iter().filter(|t| !pairs.iter().any(|(m, _)| *m == t.map)).cloned().collect();
        pairs.extend(match_sightings(&free, &unlabelled, &guide, radius));
        self.track_stats.frames += 1;
        // A marker being dragged moves alone: those frames say nothing about the view.
        if self.marker_drag.is_none()
            && let Some((transform, _, used)) = fit_robust(&pairs, &seed, false)
        {
            self.track_stats.fitted += 1;
            self.last_measure = Some(V1Measure { at: now, transform, source: "diamonds".to_owned(), pairs: used });
            // The canvas is off when no delay lines it up with the diamonds (a clamp at the edge
            // the game draws elsewhere, a zoom off): a mere timing difference has a delay that
            // fits. A fast drag missed by speed × the frame delay's uncertainty (22–54 ms) when
            // compared at one delay (16–33 wrong rebases a drag); a wide allowance instead let a
            // 30 px offset ride through a whole drag (seen in a screen recording).
            let fit = self.compare_canvas_moving(&pairs, now);
            let still = !self.canvas.panning() && self.last_motion.is_some_and(|at| at.elapsed() >= STILL_AFTER);
            let limit = if still { CANVAS_STILL_REBASE_PX } else { CANVAS_REBASE_PX };
            self.rebase_streak = if fit.is_some_and(|(_, miss)| miss > limit) { self.rebase_streak + 1 } else { 0 };
            if self.rebase_streak >= CANVAS_REBASE_CHECKS
                && let Some((k, _)) = fit
            {
                let then = now.checked_sub(std::time::Duration::from_millis(CANVAS_DELAYS_MS[k])).unwrap_or(now);
                let measured = MapTransform { left: seed.left, top: seed.top, size: seed.size, ..transform };
                if self.canvas.rebase(measured, then, now) {
                    self.track_stats.rebases += 1;
                    self.rebases_total += 1;
                    self.rebase_streak = 0;
                }
            }
        }
        self.track_stats.micros += started.elapsed().as_micros();
    }

    /// V1 of a map without a reference picture (the debug map: a dark translucent band whose only
    /// icons, objectives A and B, sit on one spot, so nothing in the picture gives the zoom): the
    /// canvas as the wheel and drags left it, from 1× when first seen. Moving checks still rebase it
    /// on remembered markers' diamonds.
    fn input_registration(&mut self, origin: (i32, i32)) -> (MapTransform, bool) {
        let fresh = match self.canvas.transform() {
            Some(t) => {
                self.full_transform = Some(t);
                return (t, false);
            }
            None => MapTransform::fixed(MapView::Full, origin),
        };
        self.canvas.correct(fresh, Instant::now());
        self.full_transform = Some(fresh);
        println!("V1 zoom 1.0x (no reference picture, tracked from the input)");
        (fresh, true)
    }

    fn report_tracking(&mut self) {
        self.report_canvas();
        let s = std::mem::take(&mut self.track_stats);
        self.rebase_streak = 0;
        println!(
            "V1 checks while moving: {} ({} matched a diamond), {:.1} ms each; canvas rebased on a diamond {} times",
            s.frames,
            s.fitted,
            s.micros as f64 / 1000.0 / f64::from(s.frames.max(1)),
            s.rebases
        );
    }

    /// The V1 registration of `frame`, and whether it was made afresh (not the last one again for an
    /// unchanged picture). A held zoom (`pending_zoom`) asks for a fresh one every time.
    fn register_full(&mut self, frame: &FrameView, origin: (i32, i32), cursor: Option<(f64, f64)>) -> Option<(MapTransform, bool)> {
        if self.no_reference {
            return Some(self.input_registration(origin));
        }
        self.collect_registration();
        let base = MapTransform::fixed(MapView::Full, origin);
        let signature = signature(frame, &base);
        let same = signature.len() == self.signature.len()
            && signature.iter().zip(&self.signature).map(|(a, b)| (a - b).abs()).sum::<f32>() / signature.len() as f32 <= SIGNATURE_CHANGE;
        if let Some(t) = self.full_transform
            && same
        {
            if std::mem::take(&mut self.fresh_registration) {
                return Some((t, true));
            }
            if self.pending_zoom.is_none() {
                return Some((t, false));
            }
        }
        // A changed picture, or a held zoom asking for a fresh registration: on the worker. This
        // read waits for it (markers and arrow come with the next read on the same picture).
        let map = self.current.as_ref()?.info;
        let prior = self.canvas.transform();
        if self.register.send(RegisterJob { frame: OwnedFrame::copy(frame), origin, cursor, map, signature, generation: self.reference_generation, prior }) {
            self.fresh_registration = false;
            // Inline (replay): back at once.
            self.collect_registration();
            if std::mem::take(&mut self.fresh_registration) {
                return self.full_transform.map(|t| (t, true));
            }
        }
        None
    }
}

fn dist_m(a: (f64, f64), b: (f64, f64), info: &MapInfo) -> f64 {
    ((a.0 - b.0).hypot(a.1 - b.1)) * info.size_m()
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 { values[n / 2] } else { (values[n / 2 - 1] + values[n / 2]) / 2.0 }
}

/// Pairs (marker map position, diamond frame position) of whole diamonds read on `t`: a labelled
/// diamond goes with the marker of its label, an unlabelled one with the nearest marker within
/// `radius` px that is clearly nearer than the next (half the distance or less).
fn match_sightings(targets: &[Target], found: &[MarkerSighting], t: &MapTransform, radius: f64) -> Vec<((f64, f64), (f64, f64))> {
    let mut used = vec![false; targets.len()];
    let mut pairs = Vec::new();
    for s in found.iter().filter(|s| s.complete && !s.from_label) {
        if let Some(label) = &s.label {
            if let Some(i) = targets.iter().position(|t| &t.label == label)
                && !used[i]
            {
                used[i] = true;
                pairs.push((targets[i].map, s.frame));
            }
            continue;
        }
        let mut near: Vec<(f64, usize)> = targets
            .iter()
            .enumerate()
            .filter(|(i, _)| !used[*i])
            .map(|(i, target)| {
                let p = t.to_frame(target.map.0, target.map.1);
                ((p.0 - s.frame.0).hypot(p.1 - s.frame.1), i)
            })
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        if let Some(&(d, i)) = near.first()
            && d <= radius
            && near.get(1).is_none_or(|&(d2, _)| d2 >= 2.0 * d)
        {
            used[i] = true;
            pairs.push((targets[i].map, s.frame));
        }
    }
    pairs
}

/// `fit_transform` with the worst pair dropped while it misses its diamond by more than
/// `FIT_OUTLIER_PX` and three or more pairs remain: a marker being dragged (or mismatched) does
/// not pull the view along.
fn fit_robust(pairs: &[((f64, f64), (f64, f64))], seed: &MapTransform, zooming: bool) -> Option<(MapTransform, bool, usize)> {
    let mut pairs = pairs.to_vec();
    loop {
        let (t, scale_known) = fit_transform(&pairs, seed, zooming)?;
        let miss = |p: &((f64, f64), (f64, f64))| {
            let (x, y) = t.to_frame(p.0.0, p.0.1);
            (x - p.1.0).hypot(y - p.1.1)
        };
        let worst = (0..pairs.len()).max_by(|&a, &b| miss(&pairs[a]).total_cmp(&miss(&pairs[b])))?;
        if pairs.len() < 3 || miss(&pairs[worst]) <= FIT_OUTLIER_PX {
            return Some((t, scale_known, pairs.len()));
        }
        pairs.remove(worst);
    }
}

/// The V1 transform that puts the paired markers on their diamonds: frame = c + scale * map with
/// the container of `seed`. Two diamonds or more far enough apart give the scale too; otherwise
/// it is `seed`'s, known unless the zoom may be changing. A fit outside the zoom range is a
/// mismatch.
fn fit_transform(pairs: &[((f64, f64), (f64, f64))], seed: &MapTransform, zooming: bool) -> Option<(MapTransform, bool)> {
    if pairs.is_empty() {
        return None;
    }
    let n = pairs.len() as f64;
    let (mx, my) = (pairs.iter().map(|p| p.0.0).sum::<f64>() / n, pairs.iter().map(|p| p.0.1).sum::<f64>() / n);
    let (fx, fy) = (pairs.iter().map(|p| p.1.0).sum::<f64>() / n, pairs.iter().map(|p| p.1.1).sum::<f64>() / n);
    let spread = pairs.iter().map(|p| (p.1.0 - fx).hypot(p.1.1 - fy)).fold(0.0f64, f64::max);
    let (scale, scale_known) = if pairs.len() >= 2 && spread >= MIN_FIT_SPREAD_PX / 2.0 {
        let num: f64 = pairs.iter().map(|p| (p.0.0 - mx) * (p.1.0 - fx) + (p.0.1 - my) * (p.1.1 - fy)).sum();
        let den: f64 = pairs.iter().map(|p| (p.0.0 - mx).powi(2) + (p.0.1 - my).powi(2)).sum();
        (num / den, true)
    } else {
        (seed.scale(), !zooming)
    };
    let zoom = scale / seed.size;
    if !(0.95..=5.25).contains(&zoom) {
        return None;
    }
    let (cx, cy) = (fx - scale * mx, fy - scale * my);
    Some((MapTransform { zoom, origin: ((seed.left - cx) / scale, (seed.top - cy) / scale), ..*seed }, scale_known))
}

/// Coarse luma samples of the container, to tell whether the V1 view moved.
fn signature(frame: &FrameView, t: &MapTransform) -> Vec<f32> {
    let (x0, y0, n) = (t.left as i32, t.top as i32, t.size as usize);
    let mut out = Vec::new();
    for y in (SIGNATURE_STEP / 2..n).step_by(SIGNATURE_STEP) {
        for x in (SIGNATURE_STEP / 2..n).step_by(SIGNATURE_STEP) {
            let (fx, fy) = (x0 + x as i32, y0 + y as i32);
            if frame.covers(fx, fy, 1, 1) {
                out.push(frame.luma(fx, fy));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The debug map given with `--map debug` (no reference picture, V1 at 1× from the input):
    /// the gun read on V1 at the 1000 m and 2000 m distance signs (a calibration recording) is
    /// 1000 m apart, as the signs are.
    #[test]
    fn the_debug_map_puts_the_distance_signs_1000_m_apart() {
        let session = mtc_dataset::datasets_dir().join("raw/20261117-135421/frames");
        if !session.is_dir() {
            return;
        }
        let cues = mtc_vision::map::CueReader::new();
        let gun_at = |file: &str| {
            let image = mtc_dataset::image::RgbImage::load_png(&session.join(file)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let mut pipeline = MapPipeline::new(false);
            pipeline.force_map(mtc_maps::debug_map());
            let c = cues.read(&view, (0, 34));
            assert_eq!(c.primary(), Some(MapView::Full), "{file}");
            pipeline.process(&view, MapView::Full, &c, (0, 34), None, MapInput::default(), 0);
            let gun = pipeline.arrow().expect("arrow").map;
            (gun.0 * 4788.0, gun.1 * 4788.0)
        };
        let (a, b) = (gun_at("000168697.png"), gun_at("000269989.png"));
        assert!((b.0 - a.0 - 1000.0).abs() < 3.0 && (b.1 - a.1).abs() < 3.0, "{a:?} {b:?}");
    }

    /// V1 closed and opened again shows the whole map (the game resets the zoom);
    /// a cue missed for a frame changes nothing.
    #[test]
    fn a_reopened_v1_is_back_at_1x() {
        let cue = |v1: bool| ViewCues { ping_score: if v1 { 1.0 } else { 0.0 }, m_key_score: if v1 { 0.0 } else { 1.0 }, scope_black: 0.0, scope_lit: 0.0, remove_hint_lead: 0.0, m_key_min: 0.5 };
        let mut pipeline = MapPipeline::new(false);
        pipeline.canvas.correct(MapTransform::zoomed(MapView::Full, (0, 34), 2.4, (0.3, 0.3)), Instant::now());
        pipeline.see(&cue(true), 0);
        pipeline.see(&cue(false), 100);
        pipeline.see(&cue(true), 140);
        assert!((pipeline.canvas.transform().expect("canvas").zoom - 2.4).abs() < 1e-9, "a 40 ms gap is a missed cue");
        pipeline.see(&cue(false), 200);
        pipeline.see(&cue(true), 600);
        let t = pipeline.canvas.transform().expect("canvas");
        assert_eq!((t.zoom, t.origin), (1.0, (0.0, 0.0)));
        assert_eq!((t.left, t.top, t.size), { let f = MapTransform::fixed(MapView::Full, (0, 34)); (f.left, f.top, f.size) });
    }

    #[test]
    fn diamonds_give_the_v1_transform() {
        let truth = MapTransform::zoomed(MapView::Full, (0, 34), 2.37, (0.31, 0.42));
        let marks = [(0.40, 0.50), (0.55, 0.61), (0.35, 0.70)];
        let pairs: Vec<_> = marks.iter().map(|&m| (m, truth.to_frame(m.0, m.1))).collect();
        // Mid-zoom from an unzoomed seed: two diamonds or more give scale and offset.
        let (t, scale_known) = fit_transform(&pairs, &MapTransform::fixed(MapView::Full, (0, 34)), true).expect("fit");
        assert!(scale_known);
        assert!((t.zoom - 2.37).abs() < 1e-9 && (t.origin.0 - 0.31).abs() < 1e-9 && (t.origin.1 - 0.42).abs() < 1e-9);
        // One diamond: the offset at the seed's scale, which is not known while the zoom changes.
        let seed = MapTransform::zoomed(MapView::Full, (0, 34), 2.37, (0.2, 0.2));
        let (t, scale_known) = fit_transform(&pairs[..1], &seed, true).expect("fit");
        assert!(!scale_known);
        let p = t.to_frame(marks[0].0, marks[0].1);
        assert!((p.0 - pairs[0].1.0).abs() < 1e-9 && (p.1 - pairs[0].1.1).abs() < 1e-9);
        assert!(fit_transform(&pairs[..1], &seed, false).expect("fit").1);
        // A pairing that implies a zoom outside 1–5 (two diamonds swapped) is a mismatch.
        let wrong = [(marks[0], pairs[1].1), (marks[1], pairs[0].1)];
        assert!(fit_transform(&wrong, &seed, true).is_none());
    }
}

#[cfg(test)]
mod retry_tests {
    use super::retry_until_in;
    use std::cell::{Cell, RefCell};
    use std::time::Duration;

    /// Map 1 comes on the first retry, map 2 on the third; the library is made again after each.
    #[test]
    fn pictures_are_retried_until_all_are_in() {
        let attempt = Cell::new(0usize);
        let have = RefCell::new(Vec::new());
        let rebuilds = Cell::new(0);
        retry_until_in(
            vec![1u32, 2],
            |a| {
                attempt.set(a + 1);
                Duration::ZERO
            },
            |m| {
                let arrives = (m == 1 && attempt.get() >= 1) || (m == 2 && attempt.get() >= 3);
                if arrives {
                    have.borrow_mut().push(m);
                }
                arrives
            },
            || {
                rebuilds.set(rebuilds.get() + 1);
                [1u32, 2].into_iter().filter(|m| !have.borrow().contains(m)).collect()
            },
        );
        assert_eq!(rebuilds.get(), 2);
        assert_eq!(attempt.get(), 3);
    }
}
