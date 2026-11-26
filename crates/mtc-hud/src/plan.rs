//! The V1 plan panel's contents, from the fire control, the map and the loading
//! pipelines: live, and in replays that render it.

use mtc_overlay::{PanelLoad, PlanArc, PlanChargeBox, PlanDetail, PlanNext, PlanRow, PlanState, PlanTerrain, ProfileArc, TerrainNote};

use crate::fire::FireControl;
use crate::loading::LoadingPipeline;
use crate::maps::MapPipeline;
use crate::marks;

/// What the plan panel is made from.
pub struct PlanSources<'a> {
    pub maps: &'a MapPipeline,
    pub fire: &'a FireControl,
    pub loading: &'a LoadingPipeline,
    /// The lowest and highest elevation the readout has shown for this weapon.
    pub elevation_seen: Option<&'a (String, f64, f64)>,
    /// The target under the pointer (map or list).
    pub hover: Option<&'a str>,
    /// The terrain profile is open.
    pub terrain_open: bool,
}

/// Whether `elevation` is outside what the current gun reaches: its range checked against the
/// readout (`data/elevation_limits.json`), widened by what the readout has shown this session
/// (slopes, built-up ground). Unknown ranges mark nothing.
pub fn beyond_reach(loading: &LoadingPipeline, elevation_seen: Option<&(String, f64, f64)>, elevation: f64) -> bool {
    const MARGIN: f64 = 0.05;
    let Some((lo, hi)) = loading.elevation_range() else { return false };
    let (lo, hi) = match (elevation_seen, loading.weapon()) {
        (Some((w, seen_lo, seen_hi)), Some(weapon)) if w == weapon => (lo.min(*seen_lo), hi.max(*seen_hi)),
        _ => (lo, hi),
    };
    elevation > hi + MARGIN || elevation < lo - MARGIN
}

/// The V1 plan panel at `at` (screen px of its top-left corner): every target with the charge it
/// needs and its flight time; the selected one's arcs, charges and what to load next (loading
/// needs V3); the gun position.
pub fn plan_state(at: (i32, i32), sources: &PlanSources) -> PlanState {
    use marks::{charge_index, charge_name};
    let fire = sources.fire;
    let shell = fire.shell();
    let map = sources.maps.map();
    let precision = |label: &str| {
        let map = map?;
        let target = sources.maps.targets().iter().find(|t| t.label == label)?;
        let metres = marks::precision_m(map.size_m(), target.scale);
        Some((metres as f32, metres > marks::COARSE_PRECISION_M))
    };
    let named = |c: mtc_ammo::ChargeLevel| (charge_name(c), charge_index(c));
    let rows = fire
        .aimpoints()
        .iter()
        .map(|a| {
            let solution = fire.rows.iter().find(|(r, _)| r.label == a.label).map(|(_, s)| s);
            let plan = solution.and_then(|s| s.plan);
            PlanRow {
                label: a.label.clone(),
                objective: a.objective,
                distance_m: solution.map(|s| s.distance_m),
                charge: plan.and_then(|p| p.charge).map(named),
                time_s: plan.map(|p| p.arc.time_of_flight),
                high_arc: plan.is_some_and(|p| p.high_chosen),
                reachable: shell.is_none() || solution.is_none() || plan.is_some(),
                precision: if a.objective { None } else { precision(&a.label) },
            }
        })
        .collect();
    let selected = fire.selected();
    let plan = selected.and_then(|(_, s)| s.plan);
    // The loaded charge does for the target once it is in (or going in): fired, the same charge
    // is loaded again, or for a rocket pod resupplied first.
    let in_gun = sources.loading.is_loaded() || sources.loading.is_loading();
    let next = match (shell, plan) {
        (None, _) => PlanNext::LoadShell,
        (Some(s), _) if sources.loading.is_fired() && s.shell.salvo() => PlanNext::Resupply,
        (Some(_), None) => PlanNext::Nothing,
        (Some(_), Some(p)) if p.loaded && in_gun => PlanNext::Ready,
        (Some(_), Some(p)) => p.charge.map_or(PlanNext::LoadShell, |c| PlanNext::Load(charge_name(c), charge_index(c))),
    };
    let detail = selected.map(|(a, s)| {
        let arc = |a: Option<mtc_ballistics::Arc>, blocked: Option<f64>, hint: Option<f64>| {
            a.map(|a| PlanArc {
                elevation: a.elevation,
                time_s: a.time_of_flight,
                beyond_reach: beyond_reach(sources.loading, sources.elevation_seen, a.elevation),
                blocked_m: blocked.map(|m| m as f32),
                hint_m: hint.map(|m| m as f32),
            })
        };
        let high = plan.is_some_and(|p| p.high_chosen);
        let charges = fire
            .charge_options()
            .iter()
            .map(|o| {
                let band = fire.bands().iter().find(|b| b.charge == Some(o.charge));
                // On the target's arc, or the other one where that one does not reach (as the
                // aim falls back).
                let arc = if high { o.high.or(o.low) } else { o.low.or(o.high) };
                PlanChargeBox {
                    name: charge_name(o.charge),
                    index: charge_index(o.charge),
                    band_km: band.map(|b| ((b.min_m / 1000.0) as f32, (b.max_m / 1000.0) as f32)),
                    time_s: arc.map(|a| a.time_of_flight),
                    loaded: o.loaded,
                    planned: plan.is_some_and(|p| p.charge == Some(o.charge)),
                }
            })
            .collect();
        PlanDetail {
            label: a.label.clone(),
            height_m: s.height_m,
            azimuth: s.azimuth,
            precision: if a.objective { None } else { precision(&a.label) },
            blast_m: shell.and_then(|p| p.shell.explosive.as_ref()).map(|e| (e.radius as f32, e.kill_radius as f32)),
            low: arc(plan.and_then(|p| p.low), plan.and_then(|p| p.low_blocked), plan.and_then(|p| p.low_hint)),
            high: arc(plan.and_then(|p| p.high), plan.and_then(|p| p.high_blocked), plan.and_then(|p| p.high_hint)),
            high_chosen: high,
            auto: plan.is_none_or(|p| p.auto),
            charges,
            terrain: plan.zip(fire.terrain().filter(|t| t.label == a.label)).map(|(p, t)| terrain(&p, s.stopped, t)),
        }
    });
    let gun_source = fire.gun().zip(map).map(|(g, map)| {
        let view = match g.view {
            mtc_vision::map::MapView::Full => "V1",
            mtc_vision::map::MapView::Scope => "V2",
            mtc_vision::map::MapView::Mini => "V3",
        };
        let edge = if g.clipped { " edge" } else { "" };
        format!("{view}{edge} arrow · ±{:.1} m", marks::gun_precision_m(map.size_m(), g.scale, g.clipped))
    });
    let notice = if fire.map().is_none() {
        Some("Identifying the map…")
    } else if !fire.gun_set() {
        Some("Gun position unknown: zoom in on your arrow")
    } else if !fire.heights_settled() {
        Some("Loading heightmap…")
    } else {
        None
    };
    PlanState {
        at,
        rows,
        selected: fire.selected_label().map(str::to_owned),
        hover: sources.hover.map(str::to_owned),
        shell: shell.map(|p| p.shell.base_name()),
        loaded_charge: shell.and_then(|p| p.shell.charge()).map(named),
        load: if sources.loading.is_loading() {
            PanelLoad::Loading
        } else if sources.loading.is_loaded() {
            PanelLoad::Loaded
        } else if sources.loading.is_fired() {
            PanelLoad::Fired
        } else {
            PanelLoad::Empty
        },
        next,
        detail,
        gun_set: fire.gun_set(),
        gun_source,
        notice: notice.map(str::to_owned),
        terrain_open: sources.terrain_open,
        settings_open: false,
        scale: 1.0,
        targets_open: true,
    }
}

/// The plan panel's terrain section for the selected target: the profile, and what the
/// ground does to the arcs, the arc to fire with first.
fn terrain(plan: &crate::fire::PlanCharge, stopped: Option<crate::fire::PlanCharge>, t: &crate::fire::TerrainProfile) -> PlanTerrain {
    use marks::{charge_index, charge_name};
    let blocked = |p: &crate::fire::PlanCharge, high: bool| if high { p.high_blocked } else { p.low_blocked };
    let hinted = |p: &crate::fire::PlanCharge, high: bool| if high { p.high_hint } else { p.low_hint };
    let loaded = stopped.and_then(|st| Some((st.charge?, blocked(&st, st.high_chosen)?)));
    let note = if let Some(at) = blocked(plan, plan.high_chosen) {
        TerrainNote::Used { high: plan.high_chosen, at_m: at as f32, auto: plan.auto, hard: true }
    } else if let Some(at) = hinted(plan, plan.high_chosen) {
        TerrainNote::Used { high: plan.high_chosen, at_m: at as f32, auto: false, hard: false }
    } else if let Some((at, _)) = t.skims {
        TerrainNote::Skims { high: plan.high_chosen, at_m: at as f32 }
    } else if let Some((charge, at)) = loaded {
        TerrainNote::LoadedCharge { name: charge_name(charge), index: charge_index(charge), at_m: at as f32 }
    } else if let Some(at) = blocked(plan, !plan.high_chosen) {
        TerrainNote::OtherArc { high: !plan.high_chosen, at_m: at as f32, auto: plan.auto, hard: true }
    } else if let Some(at) = hinted(plan, !plan.high_chosen) {
        TerrainNote::OtherArc { high: !plan.high_chosen, at_m: at as f32, auto: false, hard: false }
    } else {
        TerrainNote::Clear
    };
    let pair = |(x, h): (f64, f64)| (x as f32, h as f32);
    PlanTerrain {
        distance_m: t.distance_m as f32,
        target_m: t.target_m as f32,
        ground: t.ground.iter().copied().map(pair).collect(),
        arcs: t
            .arcs
            .iter()
            .map(|a| ProfileArc { points: a.points.iter().copied().map(pair).collect(), high: a.high, charge: a.charge.map(charge_index), used: a.used, stopped: a.stopped.map(pair) })
            .collect(),
        skims: t.skims.map(pair),
        note,
    }
}

