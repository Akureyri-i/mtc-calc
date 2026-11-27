//! What the map marks show: every remembered marker and the selected target on
//! the map view on screen, in screen pixels, with each marker's location precision.

use mtc_ammo::ChargeLevel;
use mtc_overlay::{Blast, Hover, MapMark, MapMarks, Planning, RangeRing, RingKind, TargetLabel};
use mtc_vision::layout::Placement;
use mtc_vision::map::{MapTransform, MapView};

use crate::fire::FireControl;
use crate::maps::MapPipeline;

/// A marker's location precision (m) is the single-sighting scatter of a map icon, ~0.6 px,
/// at the resolution of its best sighting.
const ICON_SCATTER_PX: f64 = 0.6;
/// Precision worse than this (m) asks to zoom in on V1. Provisional: a 155 mm blast is ~6 m
/// across.
pub const COARSE_PRECISION_M: f64 = 2.0;
/// The game draws a marker's label centred ~(+20.5, −21.5) px from the marker point, ~23 × 11 px;
/// objective icons (41 px diamonds) cover it when their centre is this close to the label's, or
/// cover the diamond when this close to the marker point.
const LABEL_OFFSET: (f32, f32) = (20.5, -21.5);
const COVERS_LABEL_PX: f32 = 24.0;
const COVERS_MARKER_PX: f32 = 22.0;

/// The player's arrow is placed within ~1 px (320 of 320 labelled arrows), an arrow cut by
/// the map's edge within ~3 px (`examples/arrow_clip_eval.rs`).
const ARROW_SCATTER_PX: f64 = 1.0;
const CLIPPED_ARROW_SCATTER_PX: f64 = 3.0;

/// Location precision (m) of a marker last seen at `scale` frame px per map unit.
pub fn precision_m(map_size_m: f64, scale: f64) -> f64 {
    map_size_m / scale * ICON_SCATTER_PX
}

/// Location precision (m) of the gun position (an arrow seen at `scale` frame px per map unit).
pub fn gun_precision_m(map_size_m: f64, scale: f64, clipped: bool) -> f64 {
    map_size_m / scale * if clipped { CLIPPED_ARROW_SCATTER_PX } else { ARROW_SCATTER_PX }
}

/// A charge's colour index (`mtc_overlay::CHARGE_COLORS`), Low to High.
pub fn charge_index(charge: ChargeLevel) -> u8 {
    ChargeLevel::ALL.iter().position(|&c| c == charge).unwrap_or(0) as u8
}

/// "Medium".
pub fn charge_name(charge: ChargeLevel) -> String {
    charge.word().trim_end_matches(" Charge").to_owned()
}

/// The marks for the map on screen; `offset` turns frame into screen pixels. On V1 they carry the
/// planning layer: range rings, a label per target and `hover`'s firing data. Off the
/// reference the pipeline's transforms are on the views mapped to the reference containers: drawn
/// where the containers are here (`Placement::map_transform_to_frame`), at the HUD scale.
pub fn map_marks(maps: &MapPipeline, fire: &FireControl, placement: &Placement, offset: (i32, i32), hover: Option<&str>) -> Option<MapMarks> {
    let origin = placement.bottom_origin();
    let t = placement.map_transform_to_frame(maps.shown_transform(origin)?);
    let map = maps.map()?;
    let to_screen = |u: f64, v: f64| {
        let (x, y) = t.to_frame(u, v);
        (x as f32 + offset.0 as f32, y as f32 + offset.1 as f32)
    };
    let objectives: Vec<(f32, f32)> = mtc_maps::objectives(&map.id).iter().map(|o| to_screen(o.u, o.v)).collect();
    let selected = fire.selected_label();
    // A marker in view on V1 zoomed in further than its best sighting is about to be seen at this
    // scale (the next registered frame, 0.9–1.7 s later on Rohkstov): the precision shown, and the
    // prompt to zoom in, go by the view's scale at once (otherwise the prompt lingers).
    let shown = placement.map_transform_to_frame(MapTransform::fixed(t.view, origin));
    let in_view = |(x, y): (f64, f64)| x >= shown.left && x <= shown.left + shown.size && y >= shown.top && y <= shown.top + shown.size;
    let view_scale = (t.view == MapView::Full && maps.scale_known()).then(|| t.scale());
    let mut marks: Vec<MapMark> = maps
        .targets()
        .iter()
        .map(|target| {
            let (x, y) = to_screen(target.map.0, target.map.1);
            // The game's label and icons, at its HUD scale.
            let k = placement.map_marks_scale(t.view);
            let label = (x + LABEL_OFFSET.0 * k, y + LABEL_OFFSET.1 * k);
            let covered = objectives.iter().any(|&(ox, oy)| (ox - label.0).hypot(oy - label.1) < COVERS_LABEL_PX * k || (ox - x).hypot(oy - y) < COVERS_MARKER_PX * k);
            MapMark {
                label: target.label.clone(),
                x,
                y,
                selected: selected == Some(target.label.as_str()),
                objective: false,
                precision_m: Some({
                    let best = precision_m(map.size_m(), target.scale);
                    view_scale.filter(|_| in_view(t.to_frame(target.map.0, target.map.1))).map_or(best, |s| best.min(precision_m(map.size_m(), s))) as f32
                }),
                covered,
            }
        })
        .collect();
    if let Some(objective) = fire.aimpoints().iter().find(|a| a.objective && selected == Some(a.label.as_str())) {
        let (x, y) = to_screen(objective.map.0, objective.map.1);
        marks.push(MapMark { label: objective.label.clone(), x, y, selected: true, objective: true, precision_m: None, covered: false });
    }
    // The selected target's blast at map scale: damage reach and kill radius of the current
    // shell, and for a cluster shell the area its submunitions cover on the aimed arc.
    let px_per_m = (t.scale() / map.size_m()) as f32;
    // Not while V1's zoom animates without two diamonds to measure the scale by.
    let blast = fire.shell().filter(|_| maps.scale_known() && marks.iter().any(|m| m.selected)).and_then(|pick| {
        let explosive = pick.shell.explosive.as_ref()?;
        let spread_px = pick.shell.cluster.as_ref().and_then(|_| {
            let (_, solution) = fire.selected()?;
            let (elevation, _) = fire.aim()?;
            let launch = mtc_ammo::launch_point(&pick.weapon).0;
            let diameter = mtc_ballistics::submunition_blast_diameter(&pick.shell.projectile(), elevation, solution.height_m - launch.height_m);
            diameter.is_finite().then(|| mtc_ballistics::studs_to_meters(diameter) as f32 / 2.0 * px_per_m)
        });
        Some(Blast { reach_px: explosive.radius as f32 * px_per_m, kill_px: explosive.kill_radius as f32 * px_per_m, reach_m: explosive.radius as f32, spread_px })
    });
    let planning = (t.view == MapView::Full).then(|| {
        // Every objective is labelled too (the selected one is already in).
        for a in fire.aimpoints().iter().filter(|a| a.objective && selected != Some(a.label.as_str())) {
            let (x, y) = to_screen(a.map.0, a.map.1);
            marks.push(MapMark { label: a.label.clone(), x, y, selected: false, objective: true, precision_m: None, covered: false });
        }
        planning(fire, &to_screen, px_per_m, maps.scale_known(), hover)
    });
    // The window covers the container itself, not moved by the nudge onto the diamonds (which
    // moves only what is drawn in it).
    let container = placement.map_transform_to_frame(MapTransform::fixed(t.view, origin));
    Some(MapMarks {
        left: container.left.floor() as i32 + offset.0,
        top: container.top.floor() as i32 + offset.1,
        size: container.size.ceil() as i32,
        small: t.view == MapView::Mini,
        marks,
        coarse_m: COARSE_PRECISION_M as f32,
        blast,
        planning,
        follow: None,
        pred: fire.prediction().map(|p| to_screen(p.map.0, p.map.1)),
        scale: placement.map_marks_scale(t.view),
    })
}

/// V1's planning layer: the current shell's charges' range rings around the gun, by emphasis (the
/// loaded charge, the one the selected target needs, the others), clipped to the map's picture;
/// each target's flight time with the charge it needs; the hovered target's firing data.
fn planning(fire: &FireControl, to_screen: &dyn Fn(f64, f64) -> (f32, f32), px_per_m: f32, scale_known: bool, hover: Option<&str>) -> Planning {
    let gun = fire.gun().map(|g| to_screen(g.map.0, g.map.1));
    let picture = fire.picture().map_or((f32::MIN, f32::MIN, f32::MAX, f32::MAX), |[u0, v0, u1, v1]| {
        let (left, top) = to_screen(u0, v0);
        let (right, bottom) = to_screen(u1, v1);
        (left, top, right, bottom)
    });
    let loaded = fire.shell().and_then(|p| p.loaded_charge());
    let focus = fire.selected().and_then(|(_, s)| s.plan?.charge);
    // Not while V1's zoom animates without two diamonds to measure the scale by (as the blast).
    let rings = if gun.is_some() && scale_known {
        fire.bands()
            .iter()
            .flat_map(|b| {
                let kind = if b.charge == loaded {
                    RingKind::Loaded
                } else if b.charge.is_some() && b.charge == focus {
                    RingKind::Focus
                } else {
                    RingKind::Other
                };
                let charge = b.charge.map(charge_index);
                [(b.max_m, false), (b.min_m, true)].map(|(m, min)| RangeRing { radius_px: m as f32 * px_per_m, charge, kind, min })
            })
            .collect()
    } else {
        Vec::new()
    };
    let labels = fire
        .rows
        .iter()
        .map(|(a, s)| TargetLabel { label: a.label.clone(), flight_s: s.plan.map(|p| p.arc.time_of_flight as f32), charge: s.plan.and_then(|p| p.charge).map(charge_index), elevation: s.plan.map(|p| p.arc.elevation as f32), distance_m: s.distance_m as f32 })
        .collect();
    let hover = hover.and_then(|label| {
        let (_, s) = fire.rows.iter().find(|(a, _)| a.label == label)?;
        let arc = |a: Option<mtc_ballistics::Arc>| a.map(|a| (a.elevation as f32, a.time_of_flight as f32));
        Some(Hover {
            label: label.to_owned(),
            charge: s.plan.and_then(|p| p.charge).map(|c| (charge_name(c), charge_index(c))),
            distance_m: s.distance_m as f32,
            low: arc(s.plan.and_then(|p| p.low)),
            high: arc(s.plan.and_then(|p| p.high)),
            high_chosen: s.plan.is_some_and(|p| p.high_chosen),
        })
    });
    let stopped = fire.terrain().and_then(|t| t.stopped_map).map(|(u, v)| to_screen(u, v));
    // Without a gun position the rings' centre is nowhere (and keeps no label away).
    Planning { gun: gun.unwrap_or(mtc_overlay::NO_GUN), picture, rings, labels, fields: Default::default(), hover, stopped }
}
