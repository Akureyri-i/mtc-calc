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
//! sharpest sighting.

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
}

impl ShotLog {
    /// The frame's state at `now_ms`: the readout, a shot the loading pipeline saw, Xs the map
    /// pipeline saw; prints each shot as it is fired and its miss once its X faded.
    pub fn update(&mut self, now_ms: u64, reading: Option<(f64, f64)>, loading: &mut LoadingPipeline, maps: &mut MapPipeline, fire: &FireControl) {
        let map = maps.map();
        if map.map(|m| m.id.clone()) != self.map {
            self.flush(fire);
            let (records, run_n) = (std::mem::take(&mut self.records), self.run_n);
            *self = ShotLog { map: map.map(|m| m.id.clone()), records, run_n, ..ShotLog::default() };
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
                            Some(p) if maps.objective_over(p.map).is_some() => "no_x_under_icon",
                            Some(_) => "no_x",
                            None => "no_prediction",
                        };
                        unseen.push((k, outcome));
                        match shot.predicted {
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

    /// The settled shots' records since the last call.
    pub fn take_records(&mut self) -> Vec<crate::diag::ShotRecord> {
        std::mem::take(&mut self.records)
    }

    /// The diagnostic record of shot `k`, settled as `outcome`.
    fn record(&mut self, k: usize, fire: &FireControl, outcome: &'static str) {
        let shot = &self.shots[k];
        let Some(map) = fire.map() else { return };
        let size_m = map.size_m();
        let m = match (&shot.impact, shot.predicted, shot.gun, shot.reading) {
            (Some(i), Some(p), Some(g), Some((_, azimuth))) => Some(miss(g.map, azimuth, &p, i.map, size_m, i.landed_ms as f64, shot.lands_ms().unwrap_or(0.0))),
            _ => None,
        };
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
            x: shot.impact.as_ref().map(|i| [r5(i.map.0), r5(i.map.1)]),
            x_px_per_m: shot.impact.as_ref().map(|i| r3(i.scale / size_m)),
            along_m: m.map(|m| (m.along_m * 10.0).round() / 10.0),
            across_m: m.map(|m| (m.across_m * 10.0).round() / 10.0),
            late_s: m.map(|m| (m.late_s * 100.0).round() / 100.0),
            recorded: if shot.backfilled { "backfill" } else { "screen" },
            outcome,
            n: self.run_n,
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
        self.shots.push(shot);
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
        let shell = pick.map(|p| p.shell.name.clone());
        let weapon = pick.map(|p| p.weapon.clone());
        let charge = pick.and_then(|p| p.shell.charge()).map(|c| c.word());
        Shot { n: 0, lit_ms, dark_ms, reading, unsure, shell, weapon, charge, backfilled: false, gun, predicted, impact: None, done: false }
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
                self.shots.push(shot);
            }
            None => log::info!("Impact #{}: no shot matches (none predicted to land near it about then)", impact.id),
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
}
