//! The shot log (single-shot guns): each shot with the readout and the landing predicted for it,
//! joined to the X it makes, and the miss along and across the line of fire.
//!
//! A shot is the gunner's bar going dark (the loading pipeline's fired check); it fell between the
//! last read with the slot lit and the first without, 200 ms apart, and the gun was laid at the
//! last lit read. The landing is predicted from the arrow at that moment, not the solution's gun
//! position (that one moves only on UPDATE GUN): the miss measures the ballistics, not a stale gun.
//! Mounted or not: an emplaced gun's gunner shows the on-foot arrow (the D-30 and the 60 mm
//! mortar), and a shot means the player is at the gun. A coarser sighting within its own
//! precision of a finer one is the same spot, so the finer one is kept (V3 between V1 looks).
//! An X joins the shot whose predicted landing is nearest in time, when it lands near the
//! predicted point about when predicted. The miss is printed once the X has faded, from its
//! sharpest sighting. An X no prediction explains that lands near a shot's line of fire after it
//! was fired is kept as that shot's landing (off the prediction: a hit on something on the way,
//! or a shell that flew otherwise than the readout says).
//!
//! Each record also says what surrounded the shot: how the readout moved just before it, the seat
//! changes and UPDATE GUN presses since the previous shot, how far the arrow moved, and where the
//! solution's gun and target stood (its gun moves only on UPDATE GUN or a finer sighting, so a
//! player aiming with a stale gun shows here, not in the miss).

use std::collections::VecDeque;

use mtc_vision::map::MapView;
use mtc_vision::map::impacts::Impact;

use crate::fire::{FireControl, Prediction};
use crate::loading::LoadingPipeline;
use crate::maps::{ExpectedImpact, GunSighting, MapPipeline};
use crate::marks;

/// Readings kept to look back over a shot's window (ms).
const READINGS_KEPT_MS: u64 = 15_000;
/// The readout at a shot is sure when the window between the last lit and the first dark read is
/// no wider than this and a reading came this soon before the last lit read, or the readings
/// covered a wider window with no gap wider than this and did not change (ms; reads come every
/// 33 ms, the bar every 200).
const SURE_WITHIN_MS: u64 = 450;
/// An X joins a shot landing within this of the predicted time (ms; at least, and as a share of
/// the flight) and this near the predicted point (m; at least, and as a share of the distance).
const JOIN_TIME_MS: f64 = 1_500.0;
const JOIN_TIME_SHARE: f64 = 0.1;
const JOIN_DISTANCE_M: f64 = 150.0;
const JOIN_DISTANCE_SHARE: f64 = 0.15;
/// A shot whose X has not come this long (ms) after its predicted landing missed being seen.
const UNSEEN_AFTER_MS: f64 = 4_000.0;
/// The X shows this long (ms) after the landing: it fades out in ~2 s (`impacts::FADE_PER_S`).
const X_SHOWS_MS: f64 = 2_000.0;
/// How the readout moved before a shot is taken over this long (ms).
const STEADY_MS: u64 = 2_000;
/// An X no prediction explains is a shot's off-prediction landing when it lands within this long
/// after the shot (ms, a long flight) and this near its line of fire (m; at least, and as a share
/// of the distance), at least `FAR_MIN_M` out.
const FAR_WITHIN_MS: f64 = 60_000.0;
const FAR_ACROSS_M: f64 = 60.0;
const FAR_ACROSS_SHARE: f64 = 0.04;
const FAR_MIN_M: f64 = 30.0;

/// What surrounded a shot, for its record.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShotContext {
    /// How far the elevation and azimuth readout moved (largest minus smallest, degrees) over
    /// `STEADY_MS` before the shot.
    pub elevation_moved: Option<f64>,
    pub azimuth_moved: Option<f64>,
    /// Since the previous shot: changes of the player's seat, entries into the driver's seat and
    /// UPDATE GUN presses.
    pub seat_changes: u32,
    pub driver_entries: u32,
    pub gun_updates: u32,
    /// How far the arrow moved since the previous shot (m).
    pub gun_moved_m: Option<f64>,
    /// How far the solution's gun position was from the arrow at the shot (m), and how many shots
    /// were fired before this one since it last changed.
    pub solution_gun_off_m: Option<f64>,
    pub gun_age: Option<u32>,
    /// The selected target (normalised map coordinates) and the elevation and azimuth the
    /// solution gave for it.
    pub target: Option<(f64, f64)>,
    pub aim: Option<(f64, f64)>,
}

/// How far a series of readings moved over the `STEADY_MS` up to `at_ms`: elevation and azimuth
/// (largest minus smallest, degrees; azimuths taken around the first, so 359° then 1° is 2°);
/// `None` with fewer than two readings.
fn moved(readings: &VecDeque<(u64, (f64, f64))>, at_ms: u64) -> (Option<f64>, Option<f64>) {
    let window: Vec<(f64, f64)> = readings.iter().filter(|&&(t, _)| t <= at_ms && at_ms - t <= STEADY_MS).map(|&(_, r)| r).collect();
    if window.len() < 2 {
        return (None, None);
    }
    let range = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max) - v.iter().copied().fold(f64::INFINITY, f64::min);
    let a0 = window[0].1;
    let elevations: Vec<f64> = window.iter().map(|r| r.0).collect();
    let azimuths: Vec<f64> = window.iter().map(|r| a0 + ((r.1 - a0 + 540.0) % 360.0 - 180.0)).collect();
    (Some(range(&elevations)), Some(range(&azimuths)))
}

/// One shot.
#[derive(Clone, Debug)]
pub struct Shot {
    /// Numbered from 1 on the map.
    pub n: u32,
    /// The last read with the slot lit and the first without (ms).
    pub lit_ms: u64,
    pub dark_ms: u64,
    /// Elevation and azimuth at the last lit read.
    pub reading: Option<(f64, f64)>,
    /// Why that reading may not be the one the shot was fired at (`None`: it is).
    pub unsure: Option<String>,
    pub shell: Option<String>,
    pub weapon: Option<String>,
    pub charge: Option<&'static str>,
    /// A press the screen did not confirm, joined to its X afterwards.
    pub backfilled: bool,
    /// The arrow it was fired from (the last one seen in a vehicle).
    pub gun: Option<GunSighting>,
    pub predicted: Option<Prediction>,
    /// The X it made, as last seen.
    pub impact: Option<Impact>,
    /// An X off its prediction near its line of fire, kept as its landing when no X matched.
    pub far: Option<(Impact, Miss)>,
    pub context: ShotContext,
    /// Printed in full (the X faded) or given up on.
    pub done: bool,
}

impl Shot {
    /// When it was fired (ms): between the two reads.
    fn fired_ms(&self) -> f64 {
        (self.lit_ms + self.dark_ms) as f64 / 2.0
    }

    /// When its predicted landing is (ms).
    fn lands_ms(&self) -> Option<f64> {
        self.predicted.map(|p| self.fired_ms() + p.time_s * 1000.0)
    }


    /// How far from the predicted landing (ms) its X may land: the model's error grows with the
    /// flight, and the shot is known only between two reads without the fire key.
    fn join_window_ms(&self, p: &Prediction) -> f64 {
        JOIN_TIME_MS.max(JOIN_TIME_SHARE * p.time_s * 1000.0) + (self.dark_ms - self.lit_ms) as f64 / 2.0
    }

    /// How far from the predicted point (m) its X may be.
    fn join_radius_m(p: &Prediction) -> f64 {
        JOIN_DISTANCE_M.max(JOIN_DISTANCE_SHARE * p.distance_m)
    }
}

/// The miss of one shot from the arrow it was fired from: along the line of fire (beyond the
/// predicted landing positive) and across it (right positive), in metres; where the X is from
/// the gun; and how much later than predicted it landed (s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Miss {
    pub along_m: f64,
    pub across_m: f64,
    pub distance_m: f64,
    pub azimuth: f64,
    pub late_s: f64,
}

/// The miss of a shot fired from `gun` at `azimuth` whose predicted landing is `predicted` and
/// whose X is at `impact` (normalised map coordinates, a map `size_m` across), landed at
/// `landed_ms` against a predicted `lands_ms`.
pub fn miss(gun: (f64, f64), azimuth: f64, predicted: &Prediction, impact: (f64, f64), size_m: f64, landed_ms: f64, lands_ms: f64) -> Miss {
    let (dx, dy) = ((impact.0 - gun.0) * size_m, (impact.1 - gun.1) * size_m);
    let distance_m = dx.hypot(dy);
    // Azimuth 0° is up the map (−v), 90° to the right (+u).
    let bearing = (dx.atan2(-dy).to_degrees() + 360.0) % 360.0;
    let off = (bearing - azimuth).to_radians();
    Miss { along_m: distance_m * off.cos() - predicted.distance_m, across_m: distance_m * off.sin(), distance_m, azimuth: bearing, late_s: (landed_ms - lands_ms) / 1000.0 }
}

#[derive(Default)]
pub struct ShotLog {
    readings: VecDeque<(u64, (f64, f64))>,
    /// The player's arrow: the latest, or a finer one the latest agrees with.
    arrow: Option<GunSighting>,
    shots: Vec<Shot>,
    /// F presses the screen did not confirm with the gun loaded, worked out as shots: one becomes
    /// a shot when an X lands where and when it would (the X is only ever the player's own; seen
    /// with AVRE shots fired on V1 or driven away from).
    candidates: Vec<Shot>,
    map: Option<String>,
    /// Settled shots for the diagnostic data (`take_records`).
    records: Vec<crate::diag::ShotRecord>,
    /// Shots numbered over the run (`Shot::n` restarts on each map).
    run_n: u32,
    /// Seat changes, entries into the driver's seat and UPDATE GUN presses so far
    /// (`note_counters`), and their values at the last shot.
    counters: (u32, u32, u32),
    counters_at_shot: (u32, u32, u32),
    /// The arrow at the last shot, the solution's gun-change count then and the shots fired since
    /// that changed.
    arrow_at_shot: Option<(f64, f64)>,
    gun_changes_at_shot: Option<u32>,
    shots_since_gun_change: u32,
}

impl ShotLog {
    /// The frame's state at `now_ms`: the readout, a shot the loading pipeline saw, Xs the map
    /// pipeline saw; prints each shot as it is fired and its miss once its X faded.
    pub fn update(&mut self, now_ms: u64, reading: Option<(f64, f64)>, loading: &mut LoadingPipeline, maps: &mut MapPipeline, fire: &FireControl) {
        let map = maps.map();
        if map.map(|m| m.id.clone()) != self.map {
            self.flush(fire);
            let (records, run_n, counters) = (std::mem::take(&mut self.records), self.run_n, self.counters);
            *self = ShotLog { map: map.map(|m| m.id.clone()), records, run_n, counters, counters_at_shot: counters, ..ShotLog::default() };
        }
        if let Some(r) = reading {
            self.readings.push_back((now_ms, r));
        }
        while self.readings.front().is_some_and(|&(t, _)| now_ms.saturating_sub(t) > READINGS_KEPT_MS) {
            self.readings.pop_front();
        }
        if let (Some(arrow), Some(m)) = (maps.arrow(), map) {
            let size_m = m.size_m();
            let same_finer = self.arrow.is_some_and(|g| {
                g.scale > arrow.scale
                    && (g.map.0 - arrow.map.0).hypot(g.map.1 - arrow.map.1) * size_m <= marks::gun_precision_m(size_m, arrow.scale, arrow.clipped)
            });
            if !same_finer {
                self.arrow = Some(arrow);
            }
        }
        if let Some((lit_ms, dark_ms)) = loading.take_shot() {
            self.fired(lit_ms, dark_ms, fire);
        }
        for key in loading.take_unconfirmed() {
            let shot = self.shot_at(key, key, fire);
            if shot.predicted.is_some() {
                self.candidates.push(shot);
            }
        }
        // A press whose X would have landed and faded by now was no shot (or its X went unseen).
        self.candidates.retain(|c| c.predicted.zip(c.lands_ms()).is_some_and(|(p, lands)| now_ms as f64 <= lands + c.join_window_ms(&p) + X_SHOWS_MS));
        for impact in maps.take_new_impacts() {
            self.join(impact, fire);
        }
        // Where and when the shots not landed yet will: read the maps more often then, and look
        // under the objective icons there.
        let expected = self
            .shots
            .iter()
            .chain(&self.candidates)
            .filter(|s| !s.done && s.impact.is_none())
            .filter_map(|s| {
                let (p, lands) = (s.predicted?, s.lands_ms()?);
                let window = s.join_window_ms(&p);
                Some(ExpectedImpact { from_ms: (lands - window).max(0.0) as u64, until_ms: (lands + window + X_SHOWS_MS) as u64, map: p.map, radius_m: Shot::join_radius_m(&p) })
            })
            .collect();
        maps.expect_impacts(expected);
        // Follow each joined X to its sharpest sighting; print the miss once it is gone.
        let active = maps.impacts();
        let mut finished = Vec::new();
        let mut unseen = Vec::new();
        for (k, shot) in self.shots.iter_mut().enumerate().filter(|(_, s)| !s.done) {
            match &shot.impact {
                Some(i) => match active.iter().find(|a| a.id == i.id) {
                    Some(a) => shot.impact = Some(a.clone()),
                    None => finished.push(k),
                },
                None => {
                    let late = shot.lands_ms().map_or(now_ms as f64 - shot.fired_ms() > 60_000.0, |t| now_ms as f64 - t > UNSEEN_AFTER_MS);
                    if late {
                        shot.done = true;
                        let outcome = match shot.predicted {
                            _ if shot.far.is_some() => "x_far",
                            Some(p) if maps.objective_over(p.map).is_some() => "no_x_under_icon",
                            Some(_) => "no_x",
                            None => "no_prediction",
                        };
                        unseen.push((k, outcome));
                        match shot.predicted {
                            _ if shot.far.is_some() => {}
                            Some(p) => match maps.objective_over(p.map) {
                                Some(letter) => log::info!("Shot {}: due to land after {:.1} s, {:.0} m out, under objective {letter}'s icon; its X was not seen (hidden by the icon)", shot.n, p.time_s, p.distance_m),
                                None => log::info!("Shot {}: due to land after {:.1} s, {:.0} m out; its X was not seen", shot.n, p.time_s, p.distance_m),
                            },
                            None => log::info!("Shot {}: no prediction (shell, readout or gun missing) and no X matched", shot.n),
                        }
                    }
                }
            }
        }
        for k in finished {
            self.print_miss(k, fire);
            self.shots[k].done = true;
            self.record(k, fire, "x");
        }
        for (k, outcome) in unseen {
            self.record(k, fire, outcome);
        }
    }

    /// The run's totals so far of seat changes, entries into the driver's seat and UPDATE GUN
    /// presses.
    pub fn note_counters(&mut self, seat_changes: u32, driver_entries: u32, gun_updates: u32) {
        self.counters = (seat_changes, driver_entries, gun_updates);
    }

    /// The settled shots' records since the last call.
    pub fn take_records(&mut self) -> Vec<crate::diag::ShotRecord> {
        std::mem::take(&mut self.records)
    }

    /// The diagnostic record of shot `k`, settled as `outcome`.
    fn record(&mut self, k: usize, fire: &FireControl, outcome: &'static str) {
        let shot = &self.shots[k];
        let Some(map) = fire.map() else { return };
        let size_m = map.size_m();
        // Its own X, else an X off the prediction near its line of fire.
        let landing = shot.impact.as_ref().or(shot.far.as_ref().map(|(i, _)| i));
        let m = match (landing, shot.predicted, shot.gun, shot.reading) {
            (Some(i), Some(p), Some(g), Some((_, azimuth))) => Some(miss(g.map, azimuth, &p, i.map, size_m, i.landed_ms as f64, shot.lands_ms().unwrap_or(0.0))),
            _ => None,
        };
        let c = shot.context;
        let r2 = |x: f64| (x * 100.0).round() / 100.0;
        let r1 = |x: f64| (x * 10.0).round() / 10.0;
        let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
        let r5 = |x: f64| (x * 100_000.0).round() / 100_000.0;
        self.run_n += 1;
        self.records.push(crate::diag::ShotRecord {
            kind: "shot",
            v: crate::diag::RECORD_VERSION,
            run: String::new(),
            app: env!("CARGO_PKG_VERSION"),
            day: String::new(),
            map: map.id.clone(),
            weapon: shot.weapon.clone(),
            shell: shot.shell.clone(),
            charge: shot.charge.map(str::to_owned),
            launch: shot.weapon.as_deref().map(|w| match mtc_ammo::launch_point(w).1 {
                Some(from) => format!("borrowed {from}"),
                None => "measured".to_owned(),
            }),
            elevation: shot.reading.map(|r| r.0),
            azimuth: shot.reading.map(|r| r.1),
            sure: shot.unsure.is_none(),
            gun: shot.gun.map(|g| [r5(g.map.0), r5(g.map.1)]),
            gun_view: shot.gun.map(|g| match g.view {
                MapView::Full => "V1",
                MapView::Scope => "V2",
                MapView::Mini => "V3",
            }),
            gun_px_per_m: shot.gun.map(|g| r3(g.scale / size_m)),
            predicted: shot.predicted.map(|p| [r5(p.map.0), r5(p.map.1)]),
            predicted_m: shot.predicted.map(|p| (p.distance_m * 10.0).round() / 10.0),
            predicted_s: shot.predicted.map(|p| (p.time_s * 100.0).round() / 100.0),
            x: landing.map(|i| [r5(i.map.0), r5(i.map.1)]),
            x_px_per_m: landing.map(|i| r3(i.scale / size_m)),
            along_m: m.map(|m| (m.along_m * 10.0).round() / 10.0),
            across_m: m.map(|m| (m.across_m * 10.0).round() / 10.0),
            late_s: m.map(|m| (m.late_s * 100.0).round() / 100.0),
            recorded: if shot.backfilled { "backfill" } else { "screen" },
            outcome,
            n: self.run_n,
            elevation_moved: c.elevation_moved.map(r2),
            azimuth_moved: c.azimuth_moved.map(r2),
            seat_changes: c.seat_changes,
            driver_entries: c.driver_entries,
            gun_updates: c.gun_updates,
            gun_moved_m: c.gun_moved_m.map(r1),
            solution_gun_off_m: c.solution_gun_off_m.map(r1),
            gun_age: c.gun_age,
            target: c.target.map(|t| [r5(t.0), r5(t.1)]),
            aim_elevation: c.aim.map(|a| r2(a.0)),
            aim_azimuth: c.aim.map(|a| r2(a.1)),
        });
    }

    /// Prints the misses of shots whose X is still showing (the end of a replay).
    pub fn flush(&mut self, fire: &FireControl) {
        for k in 0..self.shots.len() {
            if !self.shots[k].done && self.shots[k].impact.is_some() {
                self.print_miss(k, fire);
                self.shots[k].done = true;
            }
        }
    }

    fn fired(&mut self, lit_ms: u64, dark_ms: u64, fire: &FireControl) {
        let mut shot = self.shot_at(lit_ms, dark_ms, fire);
        shot.n = self.shots.len() as u32 + 1;
        self.print_fired(&shot, fire, "");
        self.commit(&shot, fire);
        self.shots.push(shot);
    }

    /// A shot is on the record: what "since the previous shot" counts from.
    fn commit(&mut self, shot: &Shot, fire: &FireControl) {
        self.counters_at_shot = self.counters;
        self.arrow_at_shot = shot.gun.map(|g| g.map);
        let changes = fire.gun_changes();
        self.shots_since_gun_change = if self.gun_changes_at_shot == Some(changes) { self.shots_since_gun_change + 1 } else { 1 };
        self.gun_changes_at_shot = Some(changes);
    }

    /// What surrounds a shot laid at `lit_ms` from `gun`.
    fn context(&self, lit_ms: u64, gun: Option<GunSighting>, fire: &FireControl) -> ShotContext {
        let (elevation_moved, azimuth_moved) = moved(&self.readings, lit_ms);
        let size_m = fire.map().map(|m| m.size_m());
        let dist = |a: (f64, f64), b: (f64, f64)| size_m.map(|s| (a.0 - b.0).hypot(a.1 - b.1) * s);
        let changes = fire.gun_changes();
        ShotContext {
            elevation_moved,
            azimuth_moved,
            seat_changes: self.counters.0.saturating_sub(self.counters_at_shot.0),
            driver_entries: self.counters.1.saturating_sub(self.counters_at_shot.1),
            gun_updates: self.counters.2.saturating_sub(self.counters_at_shot.2),
            gun_moved_m: gun.zip(self.arrow_at_shot).and_then(|(g, a)| dist(g.map, a)),
            solution_gun_off_m: gun.zip(fire.gun()).and_then(|(g, s)| dist(g.map, s.map)),
            gun_age: fire.gun().map(|_| if self.gun_changes_at_shot == Some(changes) { self.shots_since_gun_change } else { 0 }),
            target: fire.selected().map(|(a, _)| a.map),
            aim: fire.aim(),
        }
    }

    /// A shot fired between the reads at `lit_ms` and `dark_ms`: the readout then, the arrow, the
    /// predicted landing (not numbered).
    fn shot_at(&self, lit_ms: u64, dark_ms: u64, fire: &FireControl) -> Shot {
        // The readout at the last lit read (the gun laid), else the nearest one.
        let at_lit = self.readings.iter().rev().find(|&&(t, _)| t <= lit_ms).or_else(|| self.readings.iter().min_by_key(|&&(t, _)| t.abs_diff(lit_ms))).copied();
        let reading = at_lit.map(|(_, r)| r);
        let unsure = match at_lit {
            None => Some("no readout".to_owned()),
            Some((t, _)) if lit_ms.saturating_sub(t) > SURE_WITHIN_MS => Some(format!("no readout for {:.1} s before the shot", (dark_ms - t) as f64 / 1000.0)),
            Some((t, r)) if dark_ms - lit_ms > SURE_WITHIN_MS => {
                // A wide window: sure only if the readout showed the whole time and held still.
                let mut times = vec![t];
                let mut same = true;
                for &(u, q) in self.readings.iter().filter(|&&(u, _)| u > lit_ms && u <= dark_ms) {
                    times.push(u);
                    same &= q == r;
                }
                times.push(dark_ms);
                let gaps = times.windows(2).all(|w| w[1] - w[0] <= SURE_WITHIN_MS);
                (!(gaps && same)).then(|| format!("the shot is known only within a {:.1} s window, during which {}", (dark_ms - lit_ms) as f64 / 1000.0, if gaps { "the readout changed" } else { "the readout was missing for a while" }))
            }
            Some(_) => None,
        };
        let gun = self.arrow;
        let predicted = gun.zip(reading).and_then(|(g, r)| fire.landing_from(g.map, r));
        let pick = fire.shell();
        // With the charge unknown, the shell without it.
        let shell = pick.map(|p| if p.charge_known { p.shell.name.clone() } else { p.shell.base_name() });
        let weapon = pick.map(|p| p.weapon.clone());
        let charge = pick.and_then(|p| p.loaded_charge()).map(|c| c.word());
        let context = self.context(lit_ms, gun, fire);
        Shot { n: 0, lit_ms, dark_ms, reading, unsure, shell, weapon, charge, backfilled: false, gun, predicted, impact: None, far: None, context, done: false }
    }

    fn print_fired(&self, shot: &Shot, fire: &FireControl, note: &str) {
        let (lit_ms, dark_ms, gun, predicted) = (shot.lit_ms, shot.dark_ms, shot.gun, shot.predicted);
        let read = shot.reading.map_or("no readout".to_owned(), |(e, a)| format!("elevation {e:+.2}° azimuth {a:06.2}°"));
        let from = match (gun, fire.map()) {
            (Some(g), Some(m)) => format!(", gun from the {}", arrow_source(&g, m.size_m())),
            _ => ", no arrow seen".to_owned(),
        };
        let landing = predicted.map_or(", no prediction".to_owned(), |p| format!(", predicted {:.1} m out, landing after {:.1} s", p.distance_m, p.time_s));
        let unsure = shot.unsure.as_ref().map_or(String::new(), |u| format!(" (the readout may not be the one fired at: {u})"));
        log::info!("Shot {} ({}){note}: fired between {:.1} and {:.1} s, {read}{from}{landing}{unsure}", shot.n, shot.shell.as_deref().unwrap_or("shell unknown"), lit_ms as f64 / 1000.0, dark_ms as f64 / 1000.0);
    }

    /// Joins an X to the shot it is: landed about when predicted, near the predicted point; of
    /// several, the nearest in time.
    fn join(&mut self, impact: Impact, fire: &FireControl) {
        let Some(size_m) = fire.map().map(|m| m.size_m()) else { return };
        let landed = impact.landed_ms as f64;
        let best = self
            .shots
            .iter_mut()
            .filter(|s| s.impact.is_none() && !s.done)
            .filter_map(|s| {
                let p = s.predicted?;
                let dt = (landed - s.lands_ms()?).abs();
                let dm = (impact.map.0 - p.map.0).hypot(impact.map.1 - p.map.1) * size_m;
                (dt <= s.join_window_ms(&p) && dm <= Shot::join_radius_m(&p)).then_some((dt, s))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, shot)) = best {
            shot.impact = Some(impact);
            return;
        }
        // A press the screen did not confirm, whose X this is: a shot after all.
        let candidate = self
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(k, s)| {
                let p = s.predicted?;
                let dt = (landed - s.lands_ms()?).abs();
                let dm = (impact.map.0 - p.map.0).hypot(impact.map.1 - p.map.1) * size_m;
                (dt <= s.join_window_ms(&p) && dm <= Shot::join_radius_m(&p)).then_some((dt, k))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        match candidate {
            Some((_, k)) => {
                let mut shot = self.candidates.remove(k);
                shot.n = self.shots.len() as u32 + 1;
                shot.backfilled = true;
                self.print_fired(&shot, fire, " (not confirmed on screen at F; its X matched the prediction, recorded afterwards)");
                shot.impact = Some(impact);
                self.commit(&shot, fire);
                self.shots.push(shot);
            }
            None => match far_shot(&self.shots, &impact, size_m) {
                Some((k, m)) => {
                    log::info!(
                        "Impact #{}: off shot {}'s prediction, {:.0} m out at azimuth {:.2}° ({:+.1} m along, {:+.1} m across, landed {:+.1} s against the prediction); kept as its landing",
                        impact.id,
                        self.shots[k].n,
                        m.distance_m,
                        m.azimuth,
                        m.along_m,
                        m.across_m,
                        m.late_s
                    );
                    self.shots[k].far = Some((impact, m));
                }
                None => log::info!("Impact #{}: no shot matches (none predicted to land near it about then)", impact.id),
            },
        }
    }

    fn print_miss(&self, k: usize, fire: &FireControl) {
        let shot = &self.shots[k];
        let (Some(impact), Some(p), Some(gun), Some((_, azimuth)), Some(map)) = (&shot.impact, shot.predicted, shot.gun, shot.reading, fire.map()) else { return };
        let m = miss(gun.map, azimuth, &p, impact.map, map.size_m(), impact.landed_ms as f64, shot.lands_ms().unwrap_or(0.0));
        let when = if m.late_s >= 0.0 { format!("{:.1} s late", m.late_s) } else { format!("{:.1} s early", -m.late_s) };
        let caveat = shot.unsure.as_ref().map_or(String::new(), |u| format!("; this shot's miss is not reliable ({u})"));
        log::info!(
            "Shot {} impact #{}: {:.1} m out at azimuth {:.2}°; against the prediction {:+.1} m along, {:+.1} m across (right positive), landed {when} (X seen on a map at {:.1} m per pixel){caveat}",
            shot.n,
            impact.id,
            m.distance_m,
            m.azimuth,
            m.along_m,
            m.across_m,
            map.size_m() / impact.scale
        );
    }
}

/// The shot an X no prediction explains most likely is: fired before it landed (within
/// `FAR_WITHIN_MS`), with no X of its own yet, the X near its line of fire and at least
/// `FAR_MIN_M` out; of several, the one whose line passes nearest. With the X's miss from it.
fn far_shot(shots: &[Shot], impact: &Impact, size_m: f64) -> Option<(usize, Miss)> {
    let landed = impact.landed_ms as f64;
    shots
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.done && s.impact.is_none() && s.far.is_none())
        .filter_map(|(k, s)| {
            let (p, g, (_, azimuth)) = (s.predicted?, s.gun?, s.reading?);
            if !(0.0..=FAR_WITHIN_MS).contains(&(landed - s.fired_ms())) {
                return None;
            }
            let m = miss(g.map, azimuth, &p, impact.map, size_m, landed, s.lands_ms()?);
            (m.distance_m >= FAR_MIN_M && m.across_m.abs() <= FAR_ACROSS_M.max(FAR_ACROSS_SHARE * m.distance_m)).then_some((k, m))
        })
        .min_by(|a, b| a.1.across_m.abs().total_cmp(&b.1.across_m.abs()))
}

/// "V1 arrow ±0.9 m": where a gun position came from and how sure it is.
fn arrow_source(g: &GunSighting, size_m: f64) -> String {
    let view = match g.view {
        MapView::Full => "V1",
        MapView::Scope => "V2",
        MapView::Mini => "V3",
    };
    format!("{view}{} arrow ±{:.1} m", if g.clipped { " clipped" } else { "" }, marks::gun_precision_m(size_m, g.scale, g.clipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shot east (azimuth 90°) predicted to land 500 m out: an X 540 m east and 10 m south is
    /// 40 m long and 10 m right; landed 0.3 s after the predicted time.
    #[test]
    fn a_miss_splits_along_and_across_the_line_of_fire() {
        let size = 1000.0;
        let gun = (0.2, 0.5);
        let p = Prediction { map: (0.7, 0.5), distance_m: 500.0, time_s: 3.0 };
        let m = miss(gun, 90.0, &p, (0.74, 0.51), size, 10_300.0, 10_000.0);
        assert!((m.along_m - 40.0).abs() < 0.2, "{m:?}");
        assert!((m.across_m - 10.0).abs() < 0.2, "{m:?}");
        assert!((m.late_s - 0.3).abs() < 1e-9);
        // North (0°), the X left of the line: across negative.
        let p = Prediction { map: (0.2, 0.3), distance_m: 200.0, time_s: 2.0 };
        let m = miss(gun, 0.0, &p, (0.19, 0.3), size, 0.0, 0.0);
        assert!((m.across_m + 10.0).abs() < 0.1 && m.along_m.abs() < 0.5, "{m:?}");
    }

    /// The readout's movement before a shot: over the last 2 s only, azimuth across north counted
    /// the short way.
    #[test]
    fn readout_movement_counts_the_last_two_seconds() {
        let readings: VecDeque<(u64, (f64, f64))> = [(1_000, (9.0, 10.0)), (8_100, (5.90, 359.5)), (9_000, (5.93, 0.4)), (10_000, (5.93, 0.6))].into_iter().collect();
        let (el, az) = moved(&readings, 10_000);
        assert!((el.unwrap() - 0.03).abs() < 1e-9 && (az.unwrap() - 1.1).abs() < 1e-9, "{el:?} {az:?}");
        assert_eq!(moved(&readings, 1_500), (None, None));
    }

    /// An X no prediction explains: kept for the shot whose line of fire it lies on (one landed
    /// 240 m beyond its prediction), not for a shot fired after it landed nor one off the line.
    #[test]
    fn an_x_off_the_prediction_on_the_line_is_the_shots() {
        let size = 3000.0;
        let gun = GunSighting { map: (0.1, 0.5), mounted: true, scale: 1000.0, clipped: false, view: MapView::Full };
        let shot = |n: u32, lit_ms: u64, azimuth: f64| Shot {
            n,
            lit_ms,
            dark_ms: lit_ms + 100,
            reading: Some((5.9, azimuth)),
            unsure: None,
            shell: None,
            weapon: None,
            charge: None,
            backfilled: false,
            gun: Some(gun),
            // 2700 m east of the gun, landing after 7.4 s.
            predicted: Some(Prediction { map: (1.0, 0.5), distance_m: 2700.0, time_s: 7.4 }),
            impact: None,
            far: None,
            context: ShotContext::default(),
            done: false,
        };
        let x = |landed_ms: u64, map: (f64, f64)| Impact { id: 1, map, scale: 1000.0, first_ms: landed_ms, last_ms: landed_ms, landed_ms, first_alpha: 1.2, last_alpha: 1.2, sightings: 1 };
        // Shot 1 east, shot 2 north-east; an X 2940 m east, 8.5 s after shot 1.
        let shots = vec![shot(1, 10_000, 90.0), shot(2, 11_000, 45.0)];
        let (k, m) = far_shot(&shots, &x(18_550, (1.08, 0.5)), size).expect("shot 1");
        assert_eq!(k, 0);
        assert!((m.along_m - 240.0).abs() < 1.0 && m.across_m.abs() < 1.0 && (m.late_s - 1.1).abs() < 0.1, "{m:?}");
        // Fired after it landed: not that shot.
        assert!(far_shot(&shots[1..], &x(10_500, (1.08, 0.5)), size).is_none());
        // 300 m off the line (beyond max(60 m, 4 % of 2940 m)): no shot.
        assert!(far_shot(&shots[..1], &x(18_550, (1.08, 0.6)), size).is_none());
    }
}
