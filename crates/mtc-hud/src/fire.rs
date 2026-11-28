//! Firing solutions.
//!
//! Inputs:
//! - the gun: the player's arrow while in a vehicle. It is taken when first seen on a map and
//!   refined by a sighting at finer map resolution within `REFINE_M`; a position fitted from an
//!   arrow cut by the map's edge is replaced by any whole arrow within `CLIPPED_REFINE_M`. The
//!   player can have it taken again; a vehicle that moved farther is not followed.
//! - the targets: remembered markers and the map's objectives.
//! - the current shell (`loading`).
//! - the ground heights (`mtc_maps` heightmaps, downloaded into the cache when missing).
//!
//! Output per target: distance, height difference, azimuth, and the low and high arcs of the
//! current shell (`mtc_ballistics`, the upstream calculator's model). The selected target also
//! gets a charge recommendation, but none while the loaded charge reaches it: a higher charge's
//! flatter arc is the one buildings and trees block, and the heightmap knows only terrain.
//! Otherwise the settings' rule picks the reaching charge closest to the loaded one, or the one
//! whose arc is closest to where the gun points now.
//!
//! Every target also gets the charge to fire it with for the V1 plan panel: the loaded one when it
//! reaches, else the reaching charge closest to it, with both of that charge's arcs and the one to
//! use: the arc the player chose for this target, else the low one when it reaches (each target
//! keeps its own choice: one behind a hill wants the high arc, another not). The selected target
//! also gets every charge's arcs (the plan panel's charge boxes), and the current shell each
//! charge's range band within the gun's elevation limits (the V1 range rings). Everything is
//! recomputed only when an input changes.
//!
//! Distance and height follow the upstream calculator: horizontal distance on the reference
//! canvas (normalised distance × map size), height difference = ground at the target − ground at
//! the gun, both sampled from the heightmap.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc as Shared, Mutex};

use mtc_ammo::{ChargeLevel, Shell};
use mtc_ballistics::{Arc, Ballistics, LaunchPoint, ReachTable};
use mtc_maps::{Heightmap, MapInfo};
use mtc_overlay::ChargeRule;

use crate::loading::ShellPick;
use crate::maps::{GunSighting, MapPipeline};

/// A finer sighting of the arrow this close to the gun position (m) refines it: the V3 minimap
/// puts the arrow ~6–11 m off the V1 position in the recordings.
const REFINE_M: f64 = 40.0;
/// A gun position fitted from an arrow cut by the map's edge is replaced by a whole arrow within
/// this distance, from any view: the cut arrow's fit is off by up to ~3 px (12 m on the minimap,
/// labelled arrows cut 25–70 %), and before the fit a cut arrow's centroid leaned well into the map.
const CLIPPED_REFINE_M: f64 = 100.0;

/// A marker read again within this (m) of where it was has not moved: positions read off V1
/// scatter by ~0.1 m between reads (in testing a distance alternated between 2363.1 and 2363.2
/// m), and each new set of solutions costs 47–70 ms (3OF61, nine targets).
const AIMPOINT_STILL_M: f64 = 0.3;
/// A predicted landing nearer the gun than this (m) is not shown: the gun is laid into the ground
/// before it (at a depressed elevation the model brings the shell down 23–28 m out), under the
/// player's own arrow.
const PRED_MIN_M: f64 = 60.0;
/// The charge rule "least gun move" goes by the readout's elevation rounded to this (°): finer would
/// solve again (~36 ms) at every small turn of the gun.
const MOVE_STEP_DEG: f64 = 1.0;
/// Half the elevation step the range slope is measured over (°), for the aim tolerance by impact.
const SLOPE_STEP_DEG: f64 = 0.05;
/// Ground this close to the gun (m) is not checked: the gun's own ground height is uncertain by
/// its position's precision (±13 m on a slope is metres of height), which put flat full-charge arcs
/// "into" the slope 25–66 m out in recorded replays; a crest that close the player sees.
const OCCLUSION_NEAR_M: f64 = 100.0;

/// Ground met this far short of the target (m) or more blocks the arc: within it the shell lands
/// on the target's own slope, inside a blast's reach.
fn occlusion_margin(distance_m: f64) -> f64 {
    (0.02 * distance_m).max(20.0)
}

/// A target on the list: a marker or an objective.
#[derive(Clone, Debug, PartialEq)]
pub struct Aimpoint {
    /// "A3", or the objective's letter.
    pub label: String,
    /// Normalised map coordinates.
    pub map: (f64, f64),
    pub objective: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Solution {
    pub distance_m: f64,
    /// Ground at the target minus ground at the gun (m).
    pub height_m: f64,
    /// 0–360°, 0° up on the map, clockwise (the game's Azimuth readout).
    pub azimuth: f64,
    pub low: Option<Arc>,
    pub high: Option<Arc>,
    /// The charge to fire it with and that charge's arc; `None` when no charge of the current
    /// shell reaches it, or no shell is known.
    pub plan: Option<PlanCharge>,
    /// The loaded charge's plan when the terrain stops its arc and `plan` moved to another charge.
    pub stopped: Option<PlanCharge>,
}

/// A target's charge on the plan panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanCharge {
    /// `None` for a shell without charges.
    pub charge: Option<ChargeLevel>,
    /// The arc to fire with (`high` or `low` below).
    pub arc: Arc,
    /// That charge's two arcs.
    pub low: Option<Arc>,
    pub high: Option<Arc>,
    /// `arc` is the high one.
    pub high_chosen: bool,
    /// Chosen automatically (the player has not picked an arc for this target).
    pub auto: bool,
    /// The loaded charge reaches it (no change needed).
    pub loaded: bool,
    /// Where (m from the gun) the ground between stops each arc short of the target.
    pub low_blocked: Option<f64>,
    pub high_blocked: Option<f64>,
    /// Where something narrower than the terrain layer keeps (a tree, a small house, a narrow
    /// bank) may stop each arc before the terrain does (m from the arrow): only a hint, it
    /// changes nothing.
    pub low_hint: Option<f64>,
    pub high_hint: Option<f64>,
}

/// One charge of the current shell for the selected target (the plan panel's charge boxes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChargeOption {
    pub charge: ChargeLevel,
    pub low: Option<Arc>,
    pub high: Option<Arc>,
    pub loaded: bool,
}

/// Ranges one charge of the current shell reaches on level ground within the gun's elevation
/// limits (m): the V1 range rings. `charge` is `None` for a shell without charges (L33A1,
/// rockets: one ring).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChargeBand {
    pub charge: Option<ChargeLevel>,
    pub min_m: f64,
    pub max_m: f64,
}

/// Where the loaded shell comes down if fired now: from the gun, at the elevation and
/// azimuth on the readout, over the heightmap (`mtc_ballistics::impact`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prediction {
    /// Normalised map coordinates.
    pub map: (f64, f64),
    pub distance_m: f64,
    pub time_s: f64,
}

/// The selected target's terrain profile: the ground along the line from the gun and the
/// arcs over it, in metres from the gun and above the gun's ground.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainProfile {
    pub label: String,
    pub distance_m: f64,
    pub target_m: f64,
    /// Every ~1/160 of the way, a little past the target.
    pub ground: Vec<(f64, f64)>,
    pub arcs: Vec<ProfileArc>,
    /// Where the stopped arc meets the ground, on the map (normalised coordinates).
    pub stopped_map: Option<(f64, f64)>,
    /// Where the arc to fire with, clear by the check, still dips under the heightmap's ground
    /// (m from the gun, m): within one step of the heightmap, its error decides it.
    pub skims: Option<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProfileArc {
    pub points: Vec<(f64, f64)>,
    pub high: bool,
    /// Its charge (`None` for a shell without charges).
    pub charge: Option<ChargeLevel>,
    /// The arc the plan fires with.
    pub used: bool,
    /// Where the ground stops it (m from the gun, m), short of the target.
    pub stopped: Option<(f64, f64)>,
}

/// What a prediction was made from: map, gun, shell, heights' state and the readout.
type PredictionKey = (String, (f64, f64), String, u8, (f64, f64));

#[derive(Clone, Debug, PartialEq)]
pub struct Recommendation {
    pub charge: ChargeLevel,
    pub arc: Arc,
}

/// Ground heights of the current map.
enum Heights {
    Loading,
    /// The heightmap (`None`: the map has none, flat as in the upstream calculator) and its
    /// terrain layer when we have one (`mtc_maps::load_terrain`).
    Ready(Option<Heightmap>, Option<Heightmap>),
    Failed(String),
}

impl Heights {
    fn status(&self) -> u8 {
        match self {
            Self::Loading => 0,
            Self::Ready(_, None) => 1,
            // A terrain layer that came later makes the solutions be computed again.
            Self::Ready(_, Some(_)) => 3,
            Self::Failed(_) => 2,
        }
    }
}

/// What the last computation was made from.
#[derive(Clone, Debug, PartialEq)]
struct Inputs {
    map: String,
    gun: (f64, f64),
    aimpoints: Vec<Aimpoint>,
    selected: Option<String>,
    shell: Option<ShellPick>,
    heights: u8,
    arcs_chosen: Vec<(String, bool)>,
    rule: ChargeRule,
    gun_elevation: Option<f64>,
}

pub struct FireControl {
    background: bool,
    /// The arc the player chose per target (`true`: high); a target not in it gets the
    /// low arc when that reaches.
    arcs_chosen: HashMap<String, bool>,
    /// Reach tables of the loading menu's other shells (keyed "weapon/shell"), those being made on
    /// a thread, and where they arrive: whether a shell reaches needs no full trajectories.
    reach_tables: HashMap<String, ReachTable>,
    tracing: HashSet<String>,
    traced: (Sender<(String, ReachTable)>, Receiver<(String, ReachTable)>),
    map: Option<&'static MapInfo>,
    /// Map id and its heights.
    heights: Shared<Mutex<(String, Heights)>>,
    /// Map id and where its picture lies on the reference canvas (u0, v0, u1, v1): portrait and
    /// landscape maps leave transparent margins the range rings stay out of.
    picture: Shared<Mutex<(String, Option<[f64; 4]>)>>,
    gun: Option<GunSighting>,
    /// How many times the gun position was taken or replaced (for the shot records: shots fired
    /// since it last changed).
    gun_changes: u32,
    aimpoints: Vec<Aimpoint>,
    selected: Option<String>,
    /// The player cleared the selection: none is picked for them until they pick one or place a
    /// new marker.
    none_chosen: bool,
    shell: Option<ShellPick>,
    /// Last printed solutions.
    printed: String,
    /// Traced trajectories per "weapon/shell name".
    ballistics: HashMap<String, Shared<Ballistics>>,
    computed: Option<Inputs>,
    /// Per target, with the current shell's arcs.
    pub rows: Vec<(Aimpoint, Solution)>,
    pub recommendation: Option<Recommendation>,
    /// Every charge of the current shell for the selected target, Low to High.
    charge_options: Vec<ChargeOption>,
    /// The current shell's charges' range bands, Low to High.
    bands: Vec<ChargeBand>,
    /// Where the loaded shell lands fired now, and what that was worked out from.
    prediction: Option<Prediction>,
    predicted_for: Option<PredictionKey>,
    /// The selected target's terrain profile.
    terrain: Option<TerrainProfile>,
    /// Which reaching charge to use when the loaded one does not reach.
    rule: ChargeRule,
    /// The readout's elevation rounded to `MOVE_STEP_DEG`, while the rule goes by it.
    gun_elevation: Option<f64>,
    /// The selected target's range change per degree of elevation (m/°) on the loaded shell's low
    /// and high arcs, for the aim tolerance by impact.
    slopes: (Option<f64>, Option<f64>),
    /// The weapon whose launch point was last printed.
    launch_printed: Option<String>,
}

/// Number of a marker label ("A12" → 12).
fn marker_number(label: &str) -> u32 {
    label.trim_start_matches(|c: char| c.is_ascii_alphabetic()).parse().unwrap_or(0)
}

impl FireControl {
    /// `background`: load heightmaps on a thread (live) or right away (replay).
    pub fn new(background: bool) -> Self {
        Self {
            background,
            arcs_chosen: HashMap::new(),
            reach_tables: HashMap::new(),
            tracing: HashSet::new(),
            traced: mpsc::channel(),
            map: None,
            heights: Shared::new(Mutex::new((String::new(), Heights::Loading))),
            picture: Shared::new(Mutex::new((String::new(), None))),
            gun: None,
            gun_changes: 0,
            aimpoints: Vec::new(),
            selected: None,
            none_chosen: false,
            shell: None,
            printed: String::new(),
            ballistics: HashMap::new(),
            computed: None,
            rows: Vec::new(),
            recommendation: None,
            charge_options: Vec::new(),
            bands: Vec::new(),
            prediction: None,
            predicted_for: None,
            launch_printed: None,
            terrain: None,
            rule: ChargeRule::NearestCharge,
            gun_elevation: None,
            slopes: (None, None),
        }
    }

    /// The charge rule from the settings and the readout's elevation (`None` when there is none);
    /// the elevation counts only for the rule that goes by it.
    pub fn set_rule(&mut self, rule: ChargeRule, elevation: Option<f64>) {
        self.rule = rule;
        self.gun_elevation = match rule {
            ChargeRule::LeastMove => elevation.map(|e| (e / MOVE_STEP_DEG).round() * MOVE_STEP_DEG),
            ChargeRule::NearestCharge => None,
        };
    }

    /// How far the landing moves per degree of elevation (m/°, positive) on the arc the aim strip
    /// guides to (`aim`), for the aim tolerance by impact.
    pub fn aim_slope(&self) -> Option<f64> {
        let (a, s) = self.selected()?;
        let high = match s.plan.filter(|p| p.loaded) {
            Some(plan) => plan.high_chosen,
            None if self.arcs_chosen.get(&a.label).copied().unwrap_or(false) => s.high.is_some(),
            None => s.low.is_none(),
        };
        if high { self.slopes.1 } else { self.slopes.0 }
    }

    /// `f` of each mapped height layer of the current map (heightmap, terrain layer), for the
    /// memory report.
    pub fn height_layers<R>(&self, f: impl Fn(&[u8]) -> R) -> Vec<R> {
        match &self.heights.lock().expect("heights lock").1 {
            Heights::Ready(h, t) => h.iter().chain(t).map(|l| f(l.mapped())).collect(),
            _ => Vec::new(),
        }
    }

    /// Takes the height layers' pages read so far out of the working set (`Heightmap::release_pages`).
    pub fn release_height_pages(&self) {
        if let Heights::Ready(h, t) = &self.heights.lock().expect("heights lock").1 {
            h.iter().chain(t).for_each(Heightmap::release_pages);
        }
    }

    /// The selected target's terrain profile, as last worked out.
    pub fn terrain(&self) -> Option<&TerrainProfile> {
        self.terrain.as_ref().filter(|t| self.selected.as_deref() == Some(t.label.as_str()))
    }

    /// Works out where the loaded shell lands fired at `reading` (elevation, azimuth: the readout;
    /// `None` when there is none), when that or the gun, shell or heights changed.
    pub fn predict(&mut self, reading: Option<(f64, f64)>) {
        let key = match (reading, self.map, self.gun.as_ref(), self.shell.as_ref()) {
            (Some(r), Some(map), Some(gun), Some(pick)) => (map.id.clone(), gun.map, pick.shell.name.clone(), self.heights.lock().expect("heights lock").1.status(), r),
            _ => {
                (self.prediction, self.predicted_for) = (None, None);
                return;
            }
        };
        if self.predicted_for.as_ref() == Some(&key) {
            return;
        }
        let Some(gun) = self.gun.map(|g| g.map) else { return };
        self.prediction = self.landing_from(gun, key.4).filter(|p| p.distance_m >= PRED_MIN_M);
        self.predicted_for = Some(key);
    }

    /// Where the loaded shell lands fired from `gun` (normalised map coordinates) at `reading`
    /// (elevation, azimuth), over the heightmap (flat where the map has none); `None` without a
    /// map or a shell, while the heights load, or off the map. The landing prediction fires from
    /// the solution's gun; the shot log from the arrow at the moment of the shot.
    pub fn landing_from(&self, gun: (f64, f64), (elevation, azimuth): (f64, f64)) -> Option<Prediction> {
        // Not with the charge unknown: which charge flies is not known.
        let (map, pick) = (self.map?, self.shell.as_ref().filter(|p| p.charge_known)?);
        let guard = self.heights.lock().expect("heights lock");
        let heights = match &guard.1 {
            Heights::Loading => return None,
            heights => heights,
        };
        // Over the terrain layer when there is one, which leaves out the heightmap's trees (the
        // tops of their crowns; leaves stop nothing) and small houses: test shots on Rohkshort
        // came down within 10 m of the X this way.
        let ground = |p: (f64, f64)| match heights {
            Heights::Ready(Some(h), terrain) => terrain.as_ref().unwrap_or(h).height_m(p.0, p.1),
            _ => 0.0,
        };
        // Azimuth 0° is up the map (−v), 90° to the right (+u).
        let (du, dv) = (azimuth.to_radians().sin(), -azimuth.to_radians().cos());
        let size = map.size_m();
        let at = |x: f64| (gun.0 + du * x / size, gun.1 + dv * x / size);
        let base = ground(gun);
        // Not the ground right in front of the gun, as the terrain check (`OCCLUSION_NEAR_M`): the
        // gun's own ground height is off by its position's precision there.
        let along = |x: f64| if x < OCCLUSION_NEAR_M { f64::NEG_INFINITY } else { ground(at(x)) - base };
        self.launch().impact(&pick.shell.projectile(), elevation, along)
            .map(|(x, time_s)| Prediction { map: at(x), distance_m: x, time_s })
            .filter(|p| (0.0..=1.0).contains(&p.map.0) && (0.0..=1.0).contains(&p.map.1))
    }

    /// Where the loaded shell lands fired now, as last worked out.
    pub fn prediction(&self) -> Option<&Prediction> {
        self.prediction.as_ref()
    }

    /// Takes the newest map, gun, targets and shell from the map and loading pipelines; solves
    /// and prints again when they changed. Returns the time solving took, when it did.
    pub fn update(&mut self, maps: &MapPipeline, shell: Option<&ShellPick>) -> Option<std::time::Duration> {
        let map = maps.map();
        if map.map(|m| &m.id) != self.map.map(|m| &m.id) {
            self.map = map;
            self.gun = None;
            self.selected = None;
            self.aimpoints.clear();
            self.rows.clear();
            self.recommendation = None;
            self.charge_options.clear();
            self.arcs_chosen.clear();
            self.computed = None;
            if let Some(map) = map {
                self.load_heights(map);
            }
        }
        let map = self.map?;
        if let Some(arrow) = maps.arrow().filter(|a| a.mounted) {
            let refines = |g: &GunSighting| {
                let d = (arrow.map.0 - g.map.0).hypot(arrow.map.1 - g.map.1) * map.size_m();
                (arrow.scale > g.scale && d <= REFINE_M) || (g.clipped && !arrow.clipped && d <= CLIPPED_REFINE_M)
            };
            if self.gun.as_ref().is_none_or(refines) {
                self.gun = Some(arrow);
                self.gun_changes += 1;
            }
        }
        self.set_aimpoints(maps, map);
        self.shell = shell.cloned();
        if let Some(p) = &self.shell
            && self.launch_printed.as_deref() != Some(p.weapon.as_str())
        {
            let (point, from) = mtc_ammo::launch_point(&p.weapon);
            let source = from.map_or("measured".to_owned(), |f| format!("not measured, borrowed from {f}"));
            log::info!("Launch point ({}): {:.2} m above ground, {:.1} m ahead of the arrow ({source})", p.weapon, point.height_m, point.ahead_m);
            self.launch_printed = Some(p.weapon.clone());
        }
        let gun = self.gun.map(|g| g.map)?;
        let inputs = Inputs {
            map: map.id.clone(),
            gun,
            aimpoints: self.aimpoints.clone(),
            selected: self.selected.clone(),
            shell: self.shell.clone(),
            heights: self.heights.lock().expect("heights lock").1.status(),
            arcs_chosen: {
                let mut v: Vec<(String, bool)> = self.arcs_chosen.iter().map(|(k, v)| (k.clone(), *v)).collect();
                v.sort();
                v
            },
            rule: self.rule,
            gun_elevation: self.gun_elevation,
        };
        if self.computed.as_ref() == Some(&inputs) || inputs.heights == 0 {
            return None;
        }
        let started = std::time::Instant::now();
        self.compute(map, gun);
        self.computed = Some(inputs);
        let took = started.elapsed();
        self.print(map, gun, took.as_secs_f64() * 1000.0);
        Some(took)
    }

    /// Takes the player's arrow as the gun position.
    pub fn update_gun(&mut self, maps: &MapPipeline) {
        if let Some(arrow) = maps.arrow() {
            self.gun = Some(arrow);
            self.gun_changes += 1;
        }
    }

    /// How many times the gun position was taken or replaced so far.
    pub fn gun_changes(&self) -> u32 {
        self.gun_changes
    }

    /// Selects a target by label; selecting the selected one again clears the selection (having
    /// no target selected is allowed), which holds until the player picks one or places a new
    /// marker.
    pub fn select(&mut self, label: &str) {
        if self.selected.as_deref() == Some(label) {
            self.selected = None;
            self.none_chosen = true;
        } else if self.aimpoints.iter().any(|a| a.label == label) {
            self.selected = Some(label.to_owned());
            self.none_chosen = false;
        }
    }

    /// Selects `label` when it is among the targets (no toggling off); whether it was.
    pub fn select_listed(&mut self, label: &str) -> bool {
        let listed = self.aimpoints.iter().any(|a| a.label == label);
        if listed {
            self.selected = Some(label.to_owned());
            self.none_chosen = false;
        }
        listed
    }

    pub fn map(&self) -> Option<&'static MapInfo> {
        self.map
    }

    /// Where the gun is, once seen.
    pub fn gun(&self) -> Option<&GunSighting> {
        self.gun.as_ref()
    }

    pub fn gun_set(&self) -> bool {
        self.gun.is_some()
    }

    /// Heights are loaded (or known to be unavailable).
    pub fn heights_settled(&self) -> bool {
        self.heights.lock().expect("heights lock").1.status() != 0
    }

    pub fn aimpoints(&self) -> &[Aimpoint] {
        &self.aimpoints
    }

    pub fn selected_label(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn shell(&self) -> Option<&ShellPick> {
        self.shell.as_ref()
    }

    pub fn selected(&self) -> Option<&(Aimpoint, Solution)> {
        let label = self.selected.as_deref()?;
        self.rows.iter().find(|(a, _)| a.label == label)
    }

    /// Elevation and azimuth to aim at with the loaded charge: the selected target's arc the
    /// player chose for it (else the low one), or the other one when that does not reach.
    pub fn aim(&self) -> Option<(f64, f64)> {
        // The plan's arc with the loaded charge: the one the player chose, else the automatic one
        // (low unless the ground stops it); nothing when the loaded charge does not reach.
        let (a, s) = self.selected()?;
        if let Some(plan) = s.plan.filter(|p| p.loaded) {
            return Some((plan.arc.elevation, s.azimuth));
        }
        // The plan is another charge (the loaded one does not reach, or the ground stops it): the
        // loaded charge's arcs still guide the strip, the player's choice first.
        let high = self.arcs_chosen.get(&a.label).copied().unwrap_or(false);
        let (chosen, other) = if high { (s.high, s.low) } else { (s.low, s.high) };
        Some((chosen.or(other)?.elevation, s.azimuth))
    }

    /// The player picks the low or the high arc for the selected target (a click on a panel).
    pub fn choose_arc(&mut self, high: bool) {
        if let Some(label) = &self.selected {
            self.arcs_chosen.insert(label.clone(), high);
        }
    }

    /// Every charge of the current shell for the selected target, Low to High.
    pub fn charge_options(&self) -> &[ChargeOption] {
        &self.charge_options
    }

    /// The current shell's charges' range bands, Low to High.
    pub fn bands(&self) -> &[ChargeBand] {
        &self.bands
    }

    /// Where the current map's picture lies on the reference canvas (u0, v0, u1, v1), once known.
    pub fn picture(&self) -> Option<[f64; 4]> {
        let guard = self.picture.lock().expect("picture lock");
        guard.1.filter(|_| self.map.is_some_and(|m| m.id == guard.0))
    }

    /// For the loading menu: whether each shell of `family` (one card's shell at its
    /// charges) reaches the selected target, by the loaded shell's trajectories, else by a reach
    /// table; `None` while the table is being made or nothing is selected.
    pub fn reach(&mut self, weapon: &str, family: &[Shell]) -> Vec<Option<bool>> {
        for (key, table) in self.traced.1.try_iter() {
            self.tracing.remove(&key);
            self.reach_tables.insert(key, table);
        }
        let launch = mtc_ammo::launch_point(weapon).0;
        let target = self.selected().map(|(_, s)| launch.target(s.distance_m, s.height_m));
        family
            .iter()
            .map(|shell| {
                let (d, h) = target?;
                let key = format!("{weapon}/{}", shell.name);
                if let Some(b) = self.ballistics.get(&key) {
                    return Some(b.reaches(d, h));
                }
                if let Some(t) = self.reach_tables.get(&key) {
                    return Some(t.reaches(d, h));
                }
                // Tracing costs ~35 ms a shell (a menu of four cards would hold the frame loop) and
                // ~2 MB: the table is made on a thread and the trajectories dropped there.
                if self.tracing.insert(key.clone()) {
                    let (tx, projectile) = (self.traced.0.clone(), shell.projectile());
                    std::thread::Builder::new()
                        .name("reach-table".into())
                        .spawn(move || {
                            let _ = tx.send((key, ReachTable::new(&Ballistics::new(projectile))));
                        })
                        .expect("reach table thread");
                }
                None
            })
            .collect()
    }

    /// Markers, then the map's objectives. A newly placed marker becomes the selection; when the
    /// selected marker goes, the newest remaining one takes its place.
    fn set_aimpoints(&mut self, maps: &MapPipeline, map: &MapInfo) {
        let mut list: Vec<Aimpoint> =
            maps.targets().iter().map(|t| Aimpoint { label: t.label.clone(), map: t.map, objective: false }).collect();
        // Markers by series and number (A1, A2, …, then a squad's B1, …), as the player reads them,
        // not in the order they were placed.
        list.sort_by(|a, b| {
            let series = |x: &Aimpoint| x.label.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned();
            (series(a), marker_number(&a.label)).cmp(&(series(b), marker_number(&b.label)))
        });
        list.extend(mtc_maps::objectives(&map.id).iter().map(|o| Aimpoint { label: o.letter.clone(), map: (o.u, o.v), objective: true }));
        let still = |a: &Aimpoint, b: &Aimpoint| {
            a.label == b.label && a.objective == b.objective && (a.map.0 - b.map.0).hypot(a.map.1 - b.map.1) * map.size_m() < AIMPOINT_STILL_M
        };
        if list.len() == self.aimpoints.len() && list.iter().zip(&self.aimpoints).all(|(a, b)| still(a, b)) {
            return;
        }
        // A target placed anew starts on the automatic arc (its label may be an old one's).
        self.arcs_chosen.retain(|label, _| self.aimpoints.iter().any(|a| &a.label == label) && list.iter().any(|a| &a.label == label));
        let newest = |items: &mut dyn Iterator<Item = &Aimpoint>| items.filter(|a| !a.objective).max_by_key(|a| marker_number(&a.label)).map(|a| a.label.clone());
        let added = newest(&mut list.iter().filter(|a| !self.aimpoints.iter().any(|b| b.label == a.label)));
        let kept = self.selected.as_ref().is_some_and(|s| list.iter().any(|a| &a.label == s));
        if added.is_some() {
            self.selected = added;
            self.none_chosen = false;
        } else if !kept && !self.none_chosen {
            self.selected = newest(&mut list.iter());
        }
        self.aimpoints = list;
    }

    fn load_heights(&mut self, map: &'static MapInfo) {
        *self.heights.lock().expect("heights lock") = (map.id.clone(), Heights::Loading);
        *self.picture.lock().expect("picture lock") = (map.id.clone(), None);
        let slot = self.heights.clone();
        let picture_slot = self.picture.clone();
        let background = self.background;
        let load = move || {
            if let Ok(img) = mtc_maps::load_reference(map) {
                let bounds = picture_bounds(&img);
                let mut guard = picture_slot.lock().expect("picture lock");
                if guard.0 == map.id {
                    guard.1 = Some(bounds);
                }
            }
            let attempt_load = || {
                if map.heightmap.is_none() {
                    return Ok(None);
                }
                let cached = mtc_maps::heightmap_path(map).is_file();
                (if cached { Ok(()) } else { mtc_maps::download(map, mtc_maps::HEIGHTMAP_FILE).map(drop) }).and_then(|()| mtc_maps::load_heightmap(map))
            };
            let mut result = attempt_load();
            let mut attempt = 0;
            let heightmap = loop {
                let failed = result.is_err();
                let loaded = result.as_ref().ok().cloned().flatten();
                {
                    let mut guard = slot.lock().expect("heights lock");
                    if guard.0 != map.id {
                        return;
                    }
                    guard.1 = match result {
                        Ok(h) => Heights::Ready(h, None),
                        Err(e) => Heights::Failed(e.to_string()),
                    };
                }
                // Live, a heightmap that could not be had is tried again while the map stays (the
                // solutions take flat ground meanwhile and change once it is in).
                if !failed || !background {
                    break loaded;
                }
                std::thread::sleep(crate::retry::delay(attempt));
                attempt += 1;
                if slot.lock().expect("heights lock").0 != map.id {
                    return;
                }
                result = attempt_load();
                log::info!("Heightmap {}: retry {attempt} {}", map.id, if result.is_ok() { "downloaded" } else { "failed again" });
            };
            // The terrain layer follows, outside the lock (it may be downloaded): the solutions use
            // the heightmap meanwhile, obstructions only warned about, and are computed again once
            // it is in. One that could not be downloaded is tried again the same way.
            let Some(heightmap) = heightmap else { return };
            for attempt in 0.. {
                if attempt > 0 {
                    std::thread::sleep(crate::retry::delay(attempt - 1));
                    if slot.lock().expect("heights lock").0 != map.id {
                        return;
                    }
                }
                let (terrain, retry) = terrain_layer(map, background);
                if attempt > 0 {
                    log::info!("Terrain layer {}: retry {attempt} {}", map.id, if terrain.is_some() { "downloaded" } else { "failed again" });
                }
                if terrain.is_some() {
                    let mut guard = slot.lock().expect("heights lock");
                    if guard.0 == map.id {
                        guard.1 = Heights::Ready(Some(heightmap), terrain);
                    }
                    return;
                }
                if !retry || !background {
                    return;
                }
            }
        };
        if self.background {
            std::thread::Builder::new().name("heights".into()).spawn(load).expect("heights thread");
        } else {
            load();
        }
    }

}

/// The map's terrain layer and whether it is worth trying again. Live, the cached layer is used
/// only once checked against the published one for this heightmap (`mtc_maps::fetch_terrain`);
/// replays and tests read the cache as it is, offline.
fn terrain_layer(map: &MapInfo, live: bool) -> (Option<Heightmap>, bool) {
    let load = || mtc_maps::load_terrain(map).map_err(|e| log::warn!("Terrain layer {} unreadable ({e}): obstructions only warned", map.id)).ok().flatten();
    if !live {
        return (load(), false);
    }
    match mtc_maps::fetch_terrain(map) {
        mtc_maps::TerrainStatus::Ready => (load(), false),
        mtc_maps::TerrainStatus::Unpublished => {
            log::info!("Terrain layer {}: none published for this heightmap; obstructions only warned", map.id);
            (None, false)
        }
        mtc_maps::TerrainStatus::Unreachable(e) => {
            log::warn!("Terrain layer {}: not downloaded ({e}); obstructions only warned until it is", map.id);
            (None, true)
        }
    }
}

impl FireControl {
    /// The loaded weapon's launch point; at the arrow without a shell.
    fn launch(&self) -> LaunchPoint {
        self.shell.as_ref().map_or_else(LaunchPoint::default, |p| mtc_ammo::launch_point(&p.weapon).0)
    }

    fn trajectories(&mut self, weapon: &str, shell: &Shell) -> Shared<Ballistics> {
        self.ballistics
            .entry(format!("{weapon}/{}", shell.name))
            .or_insert_with(|| Shared::new(Ballistics::new(shell.projectile())))
            .clone()
    }

    fn compute(&mut self, map: &MapInfo, gun: (f64, f64)) {
        let current = self.shell.clone().map(|pick| {
            let own = self.trajectories(&pick.weapon, &pick.shell);
            let family: Vec<(ChargeLevel, Shared<Ballistics>)> =
                pick.family.iter().filter_map(|s| Some((s.charge()?, self.trajectories(&pick.weapon, s)))).collect();
            // Only the loaded shell's trajectories are kept (~2 MB a charge); a shell loaded before
            // is traced again if it comes back.
            let keep: HashSet<String> = std::iter::once(&pick.shell).chain(&pick.family).map(|s| format!("{}/{}", pick.weapon, s.name)).collect();
            self.ballistics.retain(|key, _| keep.contains(key));
            (own, family)
        });
        let guard = self.heights.lock().expect("heights lock");
        // The terrain layer is what surely stops a shell (hills, banks, big houses); the heightmap
        // also has trees and small houses, which may or may not, so only the terrain blocks and
        // the rest is a hint. With no terrain layer every obstacle is only a hint.
        let ground = |p: (f64, f64)| match &guard.1 {
            Heights::Ready(Some(h), terrain) => terrain.as_ref().unwrap_or(h).height_m(p.0, p.1),
            _ => 0.0,
        };
        let full = |p: (f64, f64)| match &guard.1 {
            Heights::Ready(Some(h), _) => h.height_m(p.0, p.1),
            _ => 0.0,
        };
        let has_terrain = matches!(&guard.1, Heights::Ready(Some(_), Some(_)));
        let gun_ground = ground(gun);
        // The shell leaves from the gun's launch point; the terrain check lets an arc
        // pass ground within one step of the heightmap (its error).
        let launch = self.launch();
        let clearance = match &guard.1 {
            Heights::Ready(Some(h), _) => h.step_m(),
            _ => 0.0,
        };
        let level = |c: ChargeLevel| ChargeLevel::ALL.iter().position(|&l| l == c).unwrap_or(0) as i32;
        let loaded_charge = self.shell.as_ref().and_then(|p| p.loaded_charge());
        // The loaded charge not known: no arcs of its own, the plan's charge stands for it.
        let charge_known = self.shell.as_ref().is_none_or(|p| p.charge_known);
        let loaded = loaded_charge.map(level);
        let choices = self.arcs_chosen.clone();
        // The reaching charge of the family closest to the loaded one (of two as close, the
        // lower), with its arcs; the cheap reach test first, arcs only for the winner.
        // By the "least gun move" rule, the one with an arc closest to the readout's elevation.
        let (rule, gun_elevation) = (self.rule, self.gun_elevation);
        let closest = |family: &[(ChargeLevel, Shared<Ballistics>)], d: f64, h: f64| {
            let reaching = family.iter().filter(|(_, b)| b.reaches(d, h));
            if let (ChargeRule::LeastMove, Some(el)) = (rule, gun_elevation) {
                return reaching
                    .map(|(c, b)| {
                        let (low, high) = b.arcs(d, h);
                        let off = [low, high].into_iter().flatten().map(|a| (a.elevation - el).abs()).fold(f64::INFINITY, f64::min);
                        (off, *c, b.clone(), low, high)
                    })
                    .min_by(|x, y| x.0.total_cmp(&y.0))
                    .map(|(_, c, b, low, high)| (c, b, low, high));
            }
            let (charge, b) = reaching.min_by_key(|(c, _)| (loaded.map_or(0, |l| (level(*c) - l).abs()), level(*c)))?;
            let (low, high) = b.arcs(d, h);
            Some((*charge, b.clone(), low, high))
        };
        // Where the ground between the gun and `to` (`d` m away) stops `arc` short of it:
        // the arc flown from the launch point over the heightmap along the line (`impact`), the
        // ground taken one heightmap step lower; ground met more than `occlusion_margin` short blocks.
        let size_m = map.size_m();
        let first_hit = |b: &Ballistics, arc: Option<Arc>, to: (f64, f64), d: f64, height: &dyn Fn((f64, f64)) -> f64| -> Option<f64> {
            let arc = arc?;
            let span = (to.0 - gun.0).hypot(to.1 - gun.1);
            if span <= 0.0 {
                return None;
            }
            let unit = ((to.0 - gun.0) / span, (to.1 - gun.1) / span);
            let along = |x: f64| {
                if x < OCCLUSION_NEAR_M {
                    return f64::NEG_INFINITY;
                }
                height((gun.0 + unit.0 * x / size_m, gun.1 + unit.1 * x / size_m)) - gun_ground - clearance
            };
            let (x, _) = launch.impact(b.projectile(), arc.elevation, along)?;
            (x < d - occlusion_margin(d)).then_some(x)
        };
        // Stopped by the terrain layer: the arc is out.
        let blocked = |b: &Ballistics, arc: Option<Arc>, to: (f64, f64), d: f64| if has_terrain { first_hit(b, arc, to, d, &ground) } else { None };
        // Something narrower in the heightmap first: only a hint.
        let hinted = |b: &Ballistics, arc: Option<Arc>, to: (f64, f64), d: f64, stop: Option<f64>| {
            first_hit(b, arc, to, d, &full).filter(|&x| stop.is_none_or(|s| x < s - 1.0))
        };
        // The arc to use of two: the target's chosen one; else the low one, unless the terrain
        // stops it and the high one goes through; else whichever reaches. Narrower things only
        // give a hint.
        let pick = |label: &str, low: Option<Arc>, high: Option<Arc>, low_blocked: bool, high_blocked: bool| -> Option<(Arc, bool, bool)> {
            let choice = choices.get(label).copied();
            let high_first = choice == Some(true) || (choice.is_none() && low_blocked && high.is_some() && !high_blocked);
            let (first, second) = if high_first { ((high, true), (low, false)) } else { ((low, false), (high, true)) };
            first.0.map(|a| (a, first.1, choice.is_none())).or(second.0.map(|a| (a, second.1, choice.is_none())))
        };
        self.rows = self
            .aimpoints
            .iter()
            .map(|a| {
                let distance_m = mtc_ballistics::studs_to_meters(mtc_ballistics::distance(gun.0, gun.1, a.map.0, a.map.1, 0.0, 0.0) * map.size_studs);
                let height_m = ground(a.map) - gun_ground;
                // The target as the launch point sees it, for the arcs and reach tests.
                let (to_d, to_h) = launch.target(distance_m, height_m);
                let (low, high) = current.as_ref().map_or((None, None), |(own, _)| own.arcs(to_d, to_h));
                let azimuth = mtc_ballistics::azimuth(gun.0, gun.1, a.map.0, a.map.1);
                let plan_with = |b: &Ballistics, charge: Option<ChargeLevel>, low: Option<Arc>, high: Option<Arc>, loaded: bool| {
                    let (low_blocked, high_blocked) = (blocked(b, low, a.map, distance_m), blocked(b, high, a.map, distance_m));
                    let (low_hint, high_hint) = (hinted(b, low, a.map, distance_m, low_blocked), hinted(b, high, a.map, distance_m, high_blocked));
                    let (arc, high_chosen, auto) = pick(&a.label, low, high, low_blocked.is_some(), high_blocked.is_some())?;
                    Some(PlanCharge { charge, arc, low, high, high_chosen, auto, loaded, low_blocked, high_blocked, low_hint, high_hint })
                };
                // An arc the terrain stops counts as not reaching when the arc was chosen
                // automatically (a flat full-charge shot is often stopped by the ground): the
                // closest charge whose arc goes through, else the stopped one shown so. Narrower
                // things never count.
                let clear = |p: &PlanCharge| !p.auto || (if p.high_chosen { p.high_blocked } else { p.low_blocked }).is_none();
                let (plan, stopped_plan) = match &current {
                    Some((own, family)) => {
                        let loaded_plan = if charge_known && (low.is_some() || high.is_some()) { plan_with(own, loaded_charge, low, high, true) } else { None };
                        match loaded_plan {
                            Some(p) if clear(&p) => (Some(p), None),
                            stopped => {
                                let mut others: Vec<&(ChargeLevel, Shared<Ballistics>)> =
                                    family.iter().filter(|(c, b)| Some(*c) != loaded_charge && b.reaches(to_d, to_h)).collect();
                                others.sort_by_key(|(c, _)| (loaded.map_or(0, |l| (level(*c) - l).abs()), level(*c)));
                                let plan_of = |(c, b): &&(ChargeLevel, Shared<Ballistics>)| {
                                    let (low, high) = b.arcs(to_d, to_h);
                                    plan_with(b, Some(*c), low, high, false).filter(clear)
                                };
                                let through = match (rule, gun_elevation) {
                                    (ChargeRule::LeastMove, Some(el)) => others.iter().filter_map(plan_of).min_by(|x, y| (x.arc.elevation - el).abs().total_cmp(&(y.arc.elevation - el).abs())),
                                    _ => others.iter().find_map(plan_of),
                                };
                                match through {
                                    Some(p) => (Some(p), stopped),
                                    None => (stopped.or_else(|| closest(family, to_d, to_h).and_then(|(c, b, low, high)| plan_with(&b, Some(c), low, high, false))), None),
                                }
                            }
                        }
                    }
                    None => (None, None),
                };
                let (low, high) = if charge_known { (low, high) } else { plan.as_ref().map_or((None, None), |p| (p.low, p.high)) };
                (a.clone(), Solution { distance_m, height_m, azimuth, low, high, plan, stopped: stopped_plan })
            })
            .collect();
        // The selected target's terrain profile: the ground along the line from the gun,
        // the arc the plan fires with, and any arc the ground stops (the plan's other one, the
        // loaded charge's when the plan moved to another charge).
        self.terrain = self.selected().and_then(|(a, s)| {
            let plan = s.plan?;
            let span = (a.map.0 - gun.0).hypot(a.map.1 - gun.1);
            if span <= 0.0 {
                return None;
            }
            let unit = ((a.map.0 - gun.0) / span, (a.map.1 - gun.1) / span);
            let at = |x: f64| (gun.0 + unit.0 * x / size_m, gun.1 + unit.1 * x / size_m);
            let end = s.distance_m * 1.06;
            // The heightmap as it is (its trees show where a hint comes from).
            let ground_line: Vec<(f64, f64)> = (0..=160).map(|i| end * f64::from(i) / 160.0).map(|x| (x, full(at(x)) - gun_ground)).collect();
            let projectile = |p: &PlanCharge| -> Option<mtc_ballistics::Projectile> {
                let (own, family) = current.as_ref()?;
                if p.loaded {
                    return Some(own.projectile().clone());
                }
                family.iter().find(|(c, _)| Some(*c) == p.charge).map(|(_, b)| b.projectile().clone())
            };
            let arc_of = |p: &PlanCharge, high: bool, used: bool| -> Option<ProfileArc> {
                let arc = if high { p.high } else { p.low }?;
                let stop = if high { p.high_blocked } else { p.low_blocked };
                // To where the terrain stops it, the arc to fire with to the target, the others on
                // past it; the mark where the terrain stops it, else where something narrower may.
                let to = stop.unwrap_or(if used { s.distance_m } else { end });
                let points = launch.trajectory(&projectile(p)?, arc.elevation, to, 500.0);
                let mark = stop.or(if high { p.high_hint } else { p.low_hint });
                Some(ProfileArc { points, high, charge: p.charge, used, stopped: mark.map(|x| (x, full(at(x)) - gun_ground)) })
            };
            // In the plan panel's order of what it says: the arc to fire with, the loaded charge's,
            // the plan's other arc; the map marks the first that stops or may.
            let mut arcs = vec![arc_of(&plan, plan.high_chosen, true)?];
            if let Some(st) = s.stopped {
                arcs.extend(arc_of(&st, st.high_chosen, false));
            }
            if (if plan.high_chosen { plan.low_blocked.or(plan.low_hint) } else { plan.high_blocked.or(plan.high_hint) }).is_some() {
                arcs.extend(arc_of(&plan, !plan.high_chosen, false));
            }
            let stopped_map = arcs.iter().find_map(|a| a.stopped).map(|(x, _)| at(x));
            // The arc to fire with over the ground as the heightmap has it (the check's rules
            // otherwise): in one recorded replay the arc to objective A, 478 m out, passed 2.42 m
            // under it.
            let skims = (if plan.high_chosen { plan.high_blocked } else { plan.low_blocked }).is_none().then(|| {
                let arc = if plan.high_chosen { plan.high } else { plan.low }?;
                let along = |x: f64| if x < OCCLUSION_NEAR_M { f64::NEG_INFINITY } else { ground(at(x)) - gun_ground };
                let (x, _) = launch.impact(&projectile(&plan)?, arc.elevation, along)?;
                (x < s.distance_m - occlusion_margin(s.distance_m)).then(|| (x, ground(at(x)) - gun_ground))
            }).flatten();
            Some(TerrainProfile { label: a.label.clone(), distance_m: s.distance_m, target_m: s.height_m, ground: ground_line, arcs, stopped_map, skims })
        });
        drop(guard);
        // The selected target's every charge, and the shell's range bands within the gun's limits.
        let target = self.selected().map(|(_, s)| launch.target(s.distance_m, s.height_m));
        let (limit_low, limit_high) = self.shell.as_ref().and_then(|p| mtc_ammo::elevation_limits(&p.weapon)).unwrap_or((-90.0, 90.0));
        (self.charge_options, self.bands) = match &current {
            Some((own, family)) => (
                target.map_or_else(Vec::new, |(d, h)| {
                    family
                        .iter()
                        .map(|(c, b)| {
                            let (low, high) = if b.reaches(d, h) { b.arcs(d, h) } else { (None, None) };
                            ChargeOption { charge: *c, low, high, loaded: Some(*c) == loaded_charge }
                        })
                        .collect()
                }),
                if family.is_empty() {
                    launch.band(own, 0.0, limit_low, limit_high).map(|(min_m, max_m)| ChargeBand { charge: None, min_m, max_m }).into_iter().collect()
                } else {
                    family.iter().filter_map(|(c, b)| launch.band(b, 0.0, limit_low, limit_high).map(|(min_m, max_m)| ChargeBand { charge: Some(*c), min_m, max_m })).collect()
                },
            ),
            None => (Vec::new(), Vec::new()),
        };
        // The selected target's recommendation: only when the loaded charge misses it.
        self.recommendation = self.selected().and_then(|(_, s)| s.plan).filter(|p| !p.loaded).and_then(|p| Some(Recommendation { charge: p.charge?, arc: p.arc }));
        // The range slope on each of the loaded shell's arcs to the selected target: the landing at
        // the target's height (the ground taken there from half way out, so a rising arc is not
        // stopped at the muzzle) a little above and below the arc's elevation.
        self.slopes = match (&current, self.selected()) {
            (Some((own, _)), Some((_, s))) => {
                let (d, h) = (s.distance_m, s.height_m);
                let range = |e: f64| launch.impact(own.projectile(), e, |x| if x < d * 0.5 { f64::NEG_INFINITY } else { h }).map(|(x, _)| x);
                let slope = |arc: Option<Arc>| -> Option<f64> {
                    let e = arc?.elevation;
                    Some(((range(e + SLOPE_STEP_DEG)? - range(e - SLOPE_STEP_DEG)?) / (2.0 * SLOPE_STEP_DEG)).abs()).filter(|m| *m > 0.0)
                };
                (slope(s.low), slope(s.high))
            }
            _ => (None, None),
        };
    }

    /// Prints the solutions when they read differently from the last print.
    fn print(&mut self, map: &MapInfo, gun: (f64, f64), cost_ms: f64) {
        use std::fmt::Write;
        let heights = match &self.heights.lock().expect("heights lock").1 {
            Heights::Ready(Some(_), Some(_)) => "heightmap".to_owned(),
            Heights::Ready(Some(_), None) => "heightmap, no terrain layer: obstructions only warned".to_owned(),
            Heights::Ready(None, _) => "no heightmap for this map: flat ground".to_owned(),
            Heights::Failed(e) => format!("heightmap unavailable ({e}): flat ground"),
            Heights::Loading => unreachable!("computed only once heights are settled"),
        };
        let shell = self.shell.as_ref().map_or("no shell loaded".to_owned(), |p| p.shell.name.clone());
        let w = map.size_m();
        let mut out = format!("Firing solutions ({}, {shell}, {heights}; gun x {:.1} m  y {:.1} m):
", map.name, gun.0 * w, gun.1 * w);
        let arc = |a: Option<Arc>| a.map_or("    —          ".to_owned(), |a| format!("{:6.2}° {:5.1} s", a.elevation, a.time_of_flight));
        for (a, s) in &self.rows {
            let mark = if self.selected.as_deref() == Some(a.label.as_str()) { "▶" } else { " " };
            let name = if a.objective { format!("Obj {}", a.label) } else { a.label.clone() };
            let arcs = if self.shell.is_some() {
                if s.low.is_none() && s.high.is_none() { "  out of range".to_owned() } else { format!("  low {}  high {}", arc(s.low), arc(s.high)) }
            } else {
                String::new()
            };
            let terrain = s.plan.map(|p| {
                let mut notes = Vec::new();
                if !p.loaded && (s.low.is_some() || s.high.is_some()) {
                    notes.push(format!("the loaded charge's arc is blocked by terrain, using {}", p.charge.map_or("—".to_owned(), |c| c.word().to_owned())));
                }
                for (name, hard, hint) in [("low arc", p.low_blocked, p.low_hint), ("high arc", p.high_blocked, p.high_hint)] {
                    if let Some(x) = hint {
                        notes.push(format!("{name} may be blocked {x:.0} m out (trees or houses)"));
                    }
                    if let Some(x) = hard {
                        notes.push(format!("{name} hits terrain {x:.0} m out"));
                    }
                }
                if notes.is_empty() { String::new() } else { format!("  ({}{})", notes.join(", "), if p.high_chosen && p.auto && p.low_blocked.is_some() { ", switched to the high arc" } else { "" }) }
            });
            let _ = writeln!(out, "  {mark} {name:<6} range {:7.1} m  height {:+6.1} m  azimuth {:6.2}°{arcs}{}", s.distance_m, s.height_m, s.azimuth, terrain.unwrap_or_default());
        }
        if let Some(r) = &self.recommendation {
            let _ = writeln!(out, "  suggested charge {} ({:.2}°, {:.1} s)", r.charge.word(), r.arc.elevation, r.arc.time_of_flight);
        }
        if out != self.printed {
            log::info!("{out}  (computed in {cost_ms:.0} ms)");
            self.printed = out;
        }
    }
}

/// Where a map's picture lies on its reference canvas (u0, v0, u1, v1): the columns and rows
/// that are mostly opaque (portrait and landscape maps leave transparent margins; Rohkstov's
/// picture spans u 0.193-0.807).
fn picture_bounds(img: &mtc_maps::RgbaImage) -> [f64; 4] {
    let (w, h) = (img.width as usize, img.height as usize);
    let (mut cols, mut rows) = (vec![0u32; w], vec![0u32; h]);
    for (y, row) in rows.iter_mut().enumerate() {
        for (x, col) in cols.iter_mut().enumerate() {
            if img.data[(y * w + x) * 4 + 3] > 200 {
                *col += 1;
                *row += 1;
            }
        }
    }
    let span = |counts: &[u32], other: usize| {
        let full = |c: &u32| *c as usize * 2 > other;
        let first = counts.iter().position(full).unwrap_or(0);
        let last = counts.iter().rposition(full).map_or(counts.len(), |i| i + 1);
        (first as f64 / counts.len() as f64, last as f64 / counts.len() as f64)
    };
    let ((u0, u1), (v0, v1)) = (span(&cols, h), span(&rows, w));
    [u0, v0, u1, v1]
}
