//! Target memory for the current round.
//!
//! Markers are recorded by their label ("A3") from V1/V2 sightings; each target keeps the
//! position from its highest-resolution sighting (container size × zoom), averaging sightings of
//! equal resolution. V3 and unlabelled sightings only refresh a matching target.
//!
//! Game rules this follows:
//! - Each squad member's markers carry a letter by joining order: the squad's creator (or a
//!   player alone) A, then B, C… Same green diamond, told apart only by the label, so only in
//!   V1/V2. The player's own letter is learned when they place a marker
//!   (`MapFrame::placed`, a right click on V1 away from any marker): the label that appears at the
//!   clicked spot, on a spot that showed no marker before. A until then; kept across rounds (the
//!   squad stays). Learning a new letter drops the targets taken so far (another member's).
//! - At most `MAX_OWN` markers; placing one more removes the one with the smallest number.
//! - A new marker gets the largest number on the map + 1 (A1 on an empty map). So a known label
//!   placed again (seen at a new position) means every own marker numbered n or higher that was
//!   there before it is gone (higher numbers first seen since were placed after it).
//!
//! Removal: deleting in V1 shows "Remove marker" while hovering; the hovered target is removed
//! once it is missing where it should show while the other targets in view are there. A target
//! that should show and stays missing for `MISSING_MS` while other own markers are seen is
//! removed too (the hover can fall between frames). Emptying the map (Clear all, or deleting every
//! marker) is taken from the cursor on Clear all followed by a frame with no own marker where
//! targets should show, or from such frames lasting `EMPTIED_MS`. Targets next to an objective
//! icon are expected to show like any other: one icon never hides both the diamond and the label,
//! and either places the marker.
//!
//! The map is often closed right after a deletion or Clear all, so the minimap (on screen all the
//! time) finishes the job: a target whose minimap spot shows plain map (`MapFrame::spot_clear`:
//! no marker, nothing covering it) and no marker is absent there; a covered spot says nothing.
//! A target absent on `MINIMAP_CONFIRM` minimap frames in a row after it was hovered with "Remove
//! marker", or on `MINIMAP_UNSEEN` without, is gone. After the cursor was on Clear all, as many
//! frames showing no target and an absent one empty the map, covered targets included. Spots near
//! objective icons (`MINIMAP_OBJECTIVE_PX`, the icon is as large as on the big map) are not judged;
//! such a target deleted alone waits for the big map.
//!
//! New labels must hold for `CONFIRM_FRAMES` frames before they count; a candidate is dropped when
//! its spot is in view (V1/V2) and shows no marker. Markers are numbered in placement order, so
//! when a label is confirmed, waiting candidates with smaller numbers are confirmed first (a
//! marker is often seen once while it is placed in a zoomed view, then only after the view has
//! moved on). Labelled sightings are handled before unlabelled ones, in number order. A label
//! whose letter is cut by an icon (`?`) refreshes an own target with that number nearby, or, while
//! no squad marker has been seen, adds the next number; anything else is ignored. A marker that
//! has just appeared (its spot was in view and empty in the previous V1/V2 frame) with its label
//! hidden under an objective icon gets the next number, likewise only without squad markers; a
//! marker that was already there may be older, so it waits for its label. When its real label
//! shows later, the target takes it. A label running past the container's top or right edge is hidden the same
//! way (a marker near the map's edge: the game writes the label up and right of the diamond).
//! Just after a drag, a diamond with its label hidden where no target is, while exactly one own
//! target's spot in view has emptied, is that target moved there (dragged to the map's edge).
//!
//! A known label far from its target: when every known label in view is shifted alike the view
//! is mis-registered and the frame is dropped; a single label that keeps its new position for
//! `RESTART_MS` replaces its target (deleted and placed again unseen).
//!
//! With input (`MapFrame::dragging`, the left button held in V1): the player may be dragging a
//! marker, which the game shows like a deletion — "Remove marker" under the cursor, the marker
//! gone from its spot — followed by its label elsewhere. While dragging, a single known label away
//! from its target moves the target there at once (`TargetEvent::Moved`), and nothing is removed.
//! A hovered marker that goes missing is removed only with a right click seen
//! (`MapFrame::right_click`); the hint itself shows before the click, while hovering.

use crate::map::cues::CLEAR_ALL_BUTTON;
use crate::map::geometry::{MapTransform, MapView};
use crate::map::markers::{CUT_LETTER, MarkerSighting};

/// Most markers a player can have on the map.
pub const MAX_OWN: usize = 6;
/// The player's letter until a placement shows it (alone, or the squad's creator).
const DEFAULT_SERIES: char = 'A';
/// A marker that appears within this long (ms) after the player placed one, at the clicked spot,
/// is theirs.
const PLACE_MS: u64 = 2000;
/// … within this (view px) of the clicked spot (the marked point is the clicked point).
const PLACE_PX: f64 = 8.0;
/// A labelled sighting this far (view px) from its target disagrees with it.
const REUSE_PX: f64 = 8.0;
/// Disagreeing labels shifted alike within this (view px) are one transform error.
const COMMON_SHIFT_PX: f64 = 3.0;
/// A disagreeing label replaces its target after holding its new position this long.
const RESTART_MS: u64 = 1000;
/// Frames without an own marker where targets should show, for this long, mean the map was
/// emptied even when the cursor was not seen on Clear all.
const EMPTIED_MS: u64 = 1000;
/// A target that should show and is missing this long, while other own markers are seen, is gone.
const MISSING_MS: u64 = 1500;
/// Label box of a marker relative to its point (frame px): x from, x to, y from, y to.
const LABEL_BOX: (f64, f64, f64, f64) = (9.0, 36.0, -27.0, -16.0);
/// Half-diagonal of an objective icon (frame px).
const OBJECTIVE_HALF: f64 = 20.5;
/// Frames a new label (or an unlabelled marker with a hidden label) must be seen in.
const CONFIRM_FRAMES: u32 = 2;
/// A diamond with a hidden label is taken for a dragged target this long (ms) after a drag.
const DRAG_HIDDEN_MS: u64 = 3000;
/// Unlabelled sightings match a target within this distance (view px).
const MATCH_PX: f64 = 6.0;
/// Cursor-to-marker distance (frame px) that counts as hovering it.
const HOVER_PX: f64 = 20.0;
/// Targets this close to the container edge (frame px) are not expected to be visible.
const EDGE_PX: f64 = 16.0;
/// A hover or Clear-all click is acted on within this long.
const ACTION_WINDOW_MS: u64 = 5000;
/// Consecutive V1 frames a hovered target must be missing to confirm its deletion.
const MISSING_FRAMES: u32 = 2;
/// Minimap frames in a row a target must be absent from a plain spot: after a hover with "Remove
/// marker" (or Clear all for every target), and without (the HUD reads the minimap every 500 ms).
const MINIMAP_CONFIRM: u32 = 2;
const MINIMAP_UNSEEN: u32 = 5;
/// Minimap spots this close (frame px) to an objective icon's centre are not judged.
const MINIMAP_OBJECTIVE_PX: f64 = 24.0;
/// Sightings within this relative resolution count as equally good.
const SAME_SCALE: f64 = 0.01;
/// A same-resolution sighting this far (view px) from its target, held for `CONFIRM_FRAMES`
/// frames, is the marker placed again close by: it replaces the position instead of averaging.
const MOVED_PX: f64 = 1.5;

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// "A3".
    pub label: String,
    /// Normalised map coordinates.
    pub map: (f64, f64),
    /// Frame px per map unit of the sighting(s) behind `map`.
    pub scale: f64,
    /// Sightings averaged at that resolution.
    pub sightings: u32,
    /// Consecutive V1 frames in which it should have been visible but was not.
    pub missing_frames: u32,
    /// Since when it should have been visible (V1/V2) but was not.
    pub missing_since: Option<u64>,
    /// Numbered by placement order because its label was hidden; replaced by the label once read.
    pub inferred: bool,
    /// A nearby position the marker keeps showing at (placed again close by), and how many frames.
    moved: Option<((f64, f64), u32)>,
    /// When it was first seen.
    first_seen: u64,
    /// Minimap frames in a row its plain spot showed no marker.
    minimap_absent: u32,
}

impl Target {
    fn new(label: String, map: (f64, f64), scale: f64, inferred: bool, first_seen: u64) -> Self {
        Self { label, map, scale, sightings: 1, missing_frames: 0, missing_since: None, inferred, moved: None, first_seen, minimap_absent: 0 }
    }

    fn number(&self) -> u32 {
        number_of(&self.label).unwrap_or(0)
    }
}

fn number_of(label: &str) -> Option<u32> {
    label.get(1..)?.parse().ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetEvent {
    Added(String),
    Removed(String),
    /// Dragged to a new position: same target, same label.
    Moved(String),
    Cleared,
    /// The player's own letter, learned from a marker they placed; the targets of the
    /// letter assumed before are reported removed first.
    Series(char),
}

/// What one frame shows of the map.
pub struct MapFrame<'a> {
    pub t_ms: u64,
    pub transform: MapTransform,
    pub markers: &'a [MarkerSighting],
    /// V1 shows "Remove marker" (the cursor is on a marker).
    pub remove_hint: bool,
    /// The left button is held in V1 (input): markers may be dragged, none is being removed.
    pub dragging: bool,
    /// Whether a right click came recently (input); `None` without input. A hovered marker that
    /// goes missing is a deletion only with one: dragging shows the same hint and gap.
    pub right_click: Option<bool>,
    /// Frame-space cursor position.
    pub cursor: Option<(f64, f64)>,
    pub client_origin: (i32, i32),
    /// Minimap: whether the spot at a map position shows plain map (`markers::minimap_spot_clear`).
    pub spot_clear: Option<&'a dyn Fn((f64, f64)) -> bool>,
    /// The latest right click on V1 away from any marker (the player placing one): when (ms, on
    /// the `t_ms` clock) and where (frame px). `None` without input.
    pub placed: Option<(u64, (f64, f64))>,
}

/// Whose marker a sighting is.
#[derive(Clone, Debug, PartialEq)]
enum Owner {
    /// The player's, with its label.
    Own(String),
    /// Letter cut by an icon; number known.
    Cut(u32),
    /// A squad member's.
    Squad,
    /// No label read.
    Unlabelled,
}

fn owner(m: &MarkerSighting, series: char) -> Owner {
    match m.label.as_deref() {
        None => Owner::Unlabelled,
        Some(l) if l.starts_with(series) => Owner::Own(l.to_owned()),
        Some(l) if l.starts_with(CUT_LETTER) => number_of(l).map_or(Owner::Unlabelled, Owner::Cut),
        Some(_) => Owner::Squad,
    }
}

/// A placement being watched for its marker.
struct Placing {
    /// When the player clicked (ms).
    at: u64,
    /// The clicked spot (normalised map coordinates).
    map: (f64, f64),
    /// Where markers were before the click.
    before: Vec<(f64, f64)>,
}

/// A new label waiting for confirmation.
struct Pending {
    label: String,
    map: (f64, f64),
    /// Resolution of the sighting (0 when its outline was incomplete).
    scale: f64,
    /// Frames it was seen in.
    seen: u32,
    /// When it was first seen.
    first_seen: u64,
}

#[derive(Default)]
pub struct TargetBook {
    targets: Vec<Target>,
    /// Label under the cursor while "Remove marker" showed, and when.
    hovered: Option<(String, u64)>,
    /// Last time the cursor was on "Clear all".
    clear_pressed: Option<u64>,
    /// Labels seen away from their targets: label, since when, where.
    disagreeing: Vec<(String, u64, (f64, f64))>,
    /// Since when no own marker has shown where targets should be.
    empty_since: Option<u64>,
    /// Minimap frames in a row, after Clear all, showing no target and at least one absent.
    minimap_empty: u32,
    /// Objective icon centres of the current map (normalised map coordinates).
    objectives: Vec<(f64, f64)>,
    /// New labels waiting for confirmation.
    pending: Vec<Pending>,
    /// Unlabelled new markers with a hidden label: where, frames seen, first seen.
    unnamed: Vec<((f64, f64), u32, u64)>,
    /// A squad member's marker has been seen this round.
    squad_seen: bool,
    /// The previous V1/V2 frame: its transform and where it showed markers.
    previous: Option<(MapTransform, Vec<(f64, f64)>)>,
    /// When a drag was last seen (ms).
    last_drag: Option<u64>,
    /// The player's letter, once a placement showed it.
    series: Option<char>,
    /// A placement waiting for its marker.
    placing: Option<Placing>,
    /// The click time of the last placement taken up.
    placed_seen: Option<u64>,
}

impl TargetBook {
    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    pub fn target(&self, label: &str) -> Option<&Target> {
        self.targets.iter().find(|t| t.label == label)
    }

    /// The player's letter: learned from a placement, A until then.
    pub fn series(&self) -> char {
        self.series.unwrap_or(DEFAULT_SERIES)
    }

    /// Forget everything (new round or map) but the player's letter: the squad outlasts a round.
    pub fn reset(&mut self) {
        let series = self.series;
        *self = Self::default();
        self.series = series;
    }

    /// Learns the player's letter from the marker that appears where they just placed one; on a
    /// new letter the targets so far (another member's markers) are dropped.
    fn learn_series(&mut self, frame: &MapFrame, events: &mut Vec<TargetEvent>) {
        let t = &frame.transform;
        if t.view == MapView::Mini {
            return;
        }
        if let Some((at, point)) = frame.placed
            && self.placed_seen != Some(at)
        {
            self.placed_seen = Some(at);
            let before = self.previous.as_ref().map(|(_, seen)| seen.clone()).unwrap_or_default();
            self.placing = Some(Placing { at, map: t.to_map(point.0, point.1), before });
        }
        let Some(p) = &self.placing else { return };
        if frame.t_ms.saturating_sub(p.at) > PLACE_MS {
            self.placing = None;
            return;
        }
        let scale = t.scale();
        let px = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() * scale;
        let letter = frame.markers.iter().find_map(|m| {
            let letter = m.label.as_deref()?.chars().next().filter(char::is_ascii_uppercase)?;
            (px(m.map, p.map) <= PLACE_PX && p.before.iter().all(|&b| px(b, m.map) > MATCH_PX)).then_some(letter)
        });
        let Some(letter) = letter else { return };
        self.placing = None;
        if letter != self.series() {
            let mut gone: Vec<String> = self.targets.drain(..).map(|x| x.label).collect();
            gone.sort_by_key(|l| number_of(l));
            events.extend(gone.into_iter().map(TargetEvent::Removed));
            self.pending.clear();
            self.unnamed.clear();
            self.disagreeing.clear();
            self.hovered = None;
            self.clear_pressed = None;
            self.empty_since = None;
            self.minimap_empty = 0;
            events.push(TargetEvent::Series(letter));
        }
        self.series = Some(letter);
    }

    /// Objective icon centres of the current map (`data/objectives.json`), to tell when a new
    /// marker's label is hidden under one.
    pub fn set_objectives(&mut self, objectives: Vec<(f64, f64)>) {
        self.objectives = objectives;
    }

    /// Whether a marker at `map` was not there in the previous V1/V2 frame although its spot was in
    /// view (placed just now).
    fn just_appeared(&self, map: (f64, f64)) -> bool {
        self.previous.as_ref().is_some_and(|(pt, seen)| {
            let (fx, fy) = pt.to_frame(map.0, map.1);
            let inside = fx >= pt.left + EDGE_PX && fy >= pt.top + EDGE_PX && fx < pt.left + pt.size - EDGE_PX && fy < pt.top + pt.size - EDGE_PX;
            inside && seen.iter().all(|&m| ((m.0 - map.0).powi(2) + (m.1 - map.1).powi(2)).sqrt() * pt.scale() > MATCH_PX)
        })
    }

    fn largest_number(&self) -> u32 {
        self.targets.iter().map(Target::number).max().unwrap_or(0)
    }

    /// Whether the label box of a marker at `point` (frame px) is hidden: under an objective icon,
    /// or running past the container's top or right edge.
    fn label_hidden(&self, t: &MapTransform, point: (f64, f64)) -> bool {
        let (bx0, bx1, by0, by1) = LABEL_BOX;
        if point.1 + by0 < t.top || point.0 + bx1 > t.left + t.size {
            return true;
        }
        self.objectives.iter().any(|&(u, v)| {
            let (ox, oy) = t.to_frame(u, v);
            // L1 distance from the icon centre to the nearest point of the label box.
            let dx = (point.0 + bx0 - ox).max(ox - (point.0 + bx1)).max(0.0);
            let dy = (point.1 + by0 - oy).max(oy - (point.1 + by1)).max(0.0);
            dx + dy <= OBJECTIVE_HALF
        })
    }

    /// Adds an own target; the smallest number goes when there are more than `MAX_OWN`.
    /// `placed_at`: the number was given to a marker placed again (a known label seen at a new
    /// position since then), so every target numbered at or above it that was there before is gone
    /// (a new An means the largest number on the map was n − 1). A label seen for the first time may
    /// be an older marker not seen before, so it implies nothing.
    fn add(&mut self, target: Target, placed_at: Option<u64>, events: &mut Vec<TargetEvent>) {
        if let Some(at) = placed_at {
            let n = target.number();
            let older = |x: &Target| x.number() == n || (x.number() > n && x.first_seen < at);
            let mut gone: Vec<&Target> = self.targets.iter().filter(|x| older(x)).collect();
            gone.sort_by_key(|x| x.number());
            events.extend(gone.iter().map(|x| TargetEvent::Removed(x.label.clone())));
            self.targets.retain(|x| !older(x));
        }
        events.push(TargetEvent::Added(target.label.clone()));
        self.targets.push(target);
        while self.targets.len() > MAX_OWN {
            let i = (0..self.targets.len()).min_by_key(|&i| self.targets[i].number()).expect("targets");
            events.push(TargetEvent::Removed(self.targets.remove(i).label));
        }
    }

    pub fn observe(&mut self, frame: &MapFrame) -> Vec<TargetEvent> {
        let mut events = Vec::new();
        let t = &frame.transform;
        let scale = t.scale();
        let px = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() * scale;
        if frame.dragging {
            self.last_drag = Some(frame.t_ms);
        }
        self.learn_series(frame, &mut events);
        let series = self.series();
        // Own targets whose spot is in view with no diamond on it in this frame.
        let vacated: Vec<usize> = if t.view == MapView::Mini {
            Vec::new()
        } else {
            (0..self.targets.len())
                .filter(|&i| {
                    let x = &self.targets[i];
                    let (fx, fy) = t.to_frame(x.map.0, x.map.1);
                    x.label.starts_with(series) && t.contains_frame(fx, fy) && frame.markers.iter().all(|m| px(m.map, x.map) > MATCH_PX)
                })
                .collect()
        };

        // Whose each sighting is; a cut letter is the player's while no squad marker was seen, or
        // when an own target with that number is right there.
        let owners: Vec<Owner> = frame.markers.iter().map(|m| owner(m, series)).collect();
        if t.view != MapView::Mini && owners.contains(&Owner::Squad) {
            self.squad_seen = true;
        }
        let owners: Vec<Owner> = owners
            .into_iter()
            .zip(frame.markers)
            .map(|(o, m)| match o {
                Owner::Cut(n) => {
                    // A cut letter's leftover can read as a digit, so only an own target with that
                    // number right there, or (no squad) exactly the next number, is trusted.
                    let label = format!("{series}{n}");
                    let nearby = self.targets.iter().any(|x| x.label == label && px(x.map, m.map) <= REUSE_PX);
                    let next = !self.squad_seen && n == self.largest_number() + 1;
                    if nearby || next { Owner::Own(label) } else { Owner::Squad }
                }
                o => o,
            })
            .collect();

        // Known labels seen far from their targets.
        let mut away: Vec<(String, (f64, f64), (f64, f64))> = Vec::new();
        for (o, m) in owners.iter().zip(frame.markers) {
            if let Owner::Own(label) = o
                && let Some(x) = self.targets.iter().find(|x| &x.label == label)
                && px(x.map, m.map) > REUSE_PX
            {
                away.push((label.clone(), m.map, ((m.map.0 - x.map.0) * scale, (m.map.1 - x.map.1) * scale)));
            }
        }
        let shifted_alike = away.len() >= 2
            && away.iter().all(|a| ((a.2.0 - away[0].2.0).powi(2) + (a.2.1 - away[0].2.1).powi(2)).sqrt() <= COMMON_SHIFT_PX);
        if shifted_alike {
            return events;
        }
        // Dragging: a single label away from its target is that marker being moved.
        if frame.dragging
            && let [(label, pos, _)] = away.as_slice()
            && let Some(target) = self.targets.iter_mut().find(|x| &x.label == label)
        {
            target.map = *pos;
            target.scale = scale;
            target.sightings = 1;
            target.moved = None;
            self.disagreeing.retain(|d| &d.0 != label);
            events.push(TargetEvent::Moved(label.clone()));
            away.clear();
        }
        self.disagreeing.retain(|d| away.iter().any(|a| a.0 == d.0));
        for (label, pos, _) in &away {
            match self.disagreeing.iter_mut().find(|d| &d.0 == label) {
                Some(d) if px(d.2, *pos) <= REUSE_PX => {
                    if frame.t_ms.saturating_sub(d.1) >= RESTART_MS {
                        // Deleted and placed again: it is new, with what that implies.
                        let (label, since) = (label.clone(), d.1);
                        self.disagreeing.retain(|d| d.0 != label);
                        self.add(Target::new(label, *pos, scale, false, since), Some(since), &mut events);
                    }
                }
                Some(d) => {
                    d.1 = frame.t_ms;
                    d.2 = *pos;
                }
                None => self.disagreeing.push((label.clone(), frame.t_ms, *pos)),
            }
        }
        let held_back: Vec<String> = self.disagreeing.iter().map(|d| d.0.clone()).collect();

        let mut own_seen = false;
        let mut pending_seen = Vec::new();
        let mut unnamed_seen = Vec::new();
        // Labelled sightings first, smallest number first: a confirmation adds the waiting smaller
        // numbers before it, and targets that unlabelled sightings then match.
        let mut order: Vec<usize> = (0..frame.markers.len()).collect();
        order.sort_by_key(|&i| match &owners[i] {
            Owner::Own(label) => (0, number_of(label).unwrap_or(0)),
            Owner::Unlabelled => (2, 0),
            _ => (1, 0),
        });
        for (o, sighting) in order.into_iter().map(|i| (&owners[i], &frame.markers[i])) {
            let index = match o {
                Owner::Own(label) if held_back.contains(label) => continue,
                Owner::Own(label) => self.targets.iter().position(|x| &x.label == label).or_else(|| {
                    // A labelled sighting on an inferred target: the target takes the real label.
                    let i = self.targets.iter().position(|x| x.inferred && px(x.map, sighting.map) <= MATCH_PX)?;
                    self.targets[i].label = label.clone();
                    Some(i)
                }),
                Owner::Unlabelled => self
                    .targets
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| px(x.map, sighting.map) <= MATCH_PX)
                    .min_by(|a, b| px(a.1.map, sighting.map).total_cmp(&px(b.1.map, sighting.map)))
                    .map(|(i, _)| i),
                Owner::Squad | Owner::Cut(_) => continue,
            };
            if let Some(i) = index {
                own_seen = true;
                let target = &mut self.targets[i];
                if matches!(o, Owner::Own(_)) {
                    target.inferred = false;
                }
                if sighting.complete {
                    if scale > target.scale * (1.0 + SAME_SCALE) {
                        target.map = sighting.map;
                        target.scale = scale;
                        target.sightings = 1;
                    } else if scale >= target.scale * (1.0 - SAME_SCALE) {
                        if px(target.map, sighting.map) <= MOVED_PX {
                            let n = f64::from(target.sightings);
                            target.map = ((target.map.0 * n + sighting.map.0) / (n + 1.0), (target.map.1 * n + sighting.map.1) / (n + 1.0));
                            target.sightings += 1;
                            target.moved = None;
                        } else {
                            let seen = match target.moved {
                                Some((at, k)) if px(at, sighting.map) <= MOVED_PX => k + 1,
                                _ => 1,
                            };
                            if seen >= CONFIRM_FRAMES {
                                target.map = sighting.map;
                                target.sightings = 1;
                                target.moved = None;
                            } else {
                                target.moved = Some((sighting.map, seen));
                            }
                        }
                    }
                }
                continue;
            }
            if t.view == MapView::Mini {
                continue;
            }
            match o {
                Owner::Own(label) => {
                    own_seen = true;
                    // A new label: count it once it holds.
                    let resolution = if sighting.complete { scale } else { 0.0 };
                    let (seen, first_seen) = match self.pending.iter_mut().find(|p| &p.label == label && px(p.map, sighting.map) <= REUSE_PX) {
                        Some(p) => {
                            p.seen += 1;
                            (p.seen, p.first_seen)
                        }
                        None => {
                            self.pending.push(Pending { label: label.clone(), map: sighting.map, scale: resolution, seen: 1, first_seen: frame.t_ms });
                            (1, frame.t_ms)
                        }
                    };
                    pending_seen.push(label.clone());
                    if seen >= CONFIRM_FRAMES {
                        // Smaller numbers seen earlier were placed earlier.
                        let n = number_of(label).unwrap_or(0);
                        let mut earlier: Vec<Pending> = Vec::new();
                        let mut i = 0;
                        while i < self.pending.len() {
                            if &self.pending[i].label == label || number_of(&self.pending[i].label).is_some_and(|m| m < n) {
                                let p = self.pending.remove(i);
                                if &p.label != label {
                                    earlier.push(p);
                                }
                            } else {
                                i += 1;
                            }
                        }
                        earlier.sort_by_key(|p| number_of(&p.label));
                        for p in earlier {
                            if self.target(&p.label).is_none() {
                                self.add(Target::new(p.label, p.map, p.scale, false, p.first_seen), None, &mut events);
                            }
                        }
                        self.add(Target::new(label.clone(), sighting.map, resolution, false, first_seen), None, &mut events);
                    }
                }
                Owner::Unlabelled
                    if sighting.complete
                        && vacated.len() == 1
                        && self.last_drag.is_some_and(|d| frame.t_ms.saturating_sub(d) <= DRAG_HIDDEN_MS)
                        && self.label_hidden(t, sighting.frame) =>
                {
                    // Dragged where its label is hidden (to the map's edge): the target moves.
                    let target = &mut self.targets[vacated[0]];
                    target.map = sighting.map;
                    target.scale = scale;
                    target.sightings = 1;
                    target.moved = None;
                    own_seen = true;
                    events.push(TargetEvent::Moved(target.label.clone()));
                }
                Owner::Unlabelled
                    if !self.squad_seen
                        && sighting.complete
                        && self.label_hidden(t, sighting.frame)
                        && (self.unnamed.iter().any(|u| px(u.0, sighting.map) <= MATCH_PX) || self.just_appeared(sighting.map)) =>
                {
                    // Diamond seen, label under an objective icon: the next number once it holds.
                    let (seen, first_seen) = match self.unnamed.iter_mut().find(|u| px(u.0, sighting.map) <= MATCH_PX) {
                        Some(u) => {
                            u.1 += 1;
                            (u.1, u.2)
                        }
                        None => {
                            self.unnamed.push((sighting.map, 1, frame.t_ms));
                            (1, frame.t_ms)
                        }
                    };
                    unnamed_seen.push(sighting.map);
                    if seen >= CONFIRM_FRAMES {
                        self.unnamed.retain(|u| px(u.0, sighting.map) > MATCH_PX);
                        let label = format!("{series}{}", self.largest_number() + 1);
                        self.add(Target::new(label, sighting.map, scale, true, first_seen), None, &mut events);
                    }
                }
                _ => {}
            }
        }
        // Candidates whose spot is in view (V1/V2) but shows no marker start over (a diamond whose
        // label is hidden still counts as there); V3 shows no labels.
        if t.view != MapView::Mini {
            let in_view = |m: (f64, f64)| {
                let (fx, fy) = t.to_frame(m.0, m.1);
                t.contains_frame(fx, fy)
            };
            let diamond_at = |m: (f64, f64)| frame.markers.iter().zip(&owners).any(|(s, o)| *o == Owner::Unlabelled && px(s.map, m) <= MATCH_PX);
            self.pending.retain(|p| pending_seen.contains(&p.label) || diamond_at(p.map) || !in_view(p.map));
            self.unnamed.retain(|u| unnamed_seen.iter().any(|&m| px(u.0, m) <= MATCH_PX) || !in_view(u.0));
            self.previous = Some((*t, frame.markers.iter().map(|m| m.map).collect()));
        }
        // `matched` follows `targets` after additions and removals inside `add`.
        let matched: Vec<bool> = self
            .targets
            .iter()
            .map(|x| frame.markers.iter().zip(&owners).any(|(m, o)| match o {
                Owner::Own(l) => l == &x.label && px(x.map, m.map) <= REUSE_PX,
                Owner::Unlabelled => px(x.map, m.map) <= MATCH_PX,
                _ => false,
            }))
            .collect();
        for (target, &seen) in self.targets.iter_mut().zip(&matched) {
            if seen {
                target.minimap_absent = 0;
            }
        }
        if t.view == MapView::Mini {
            self.track_minimap(frame, &matched, &mut events);
        } else {
            self.track_removals(frame, &matched, own_seen, &mut events);
        }
        events
    }

    /// Deletions and emptying confirmed on the minimap (the map closed right after them).
    fn track_minimap(&mut self, frame: &MapFrame, matched: &[bool], events: &mut Vec<TargetEvent>) {
        let Some(spot_clear) = frame.spot_clear else { return };
        let t = &frame.transform;
        let expected = self.expected(t);
        let judged: Vec<bool> = self
            .targets
            .iter()
            .zip(&expected)
            .map(|(x, &e)| {
                let (fx, fy) = t.to_frame(x.map.0, x.map.1);
                e && !self.objectives.iter().any(|&(u, v)| {
                    let (ox, oy) = t.to_frame(u, v);
                    (ox - fx).hypot(oy - fy) <= MINIMAP_OBJECTIVE_PX
                })
            })
            .collect();
        let absent: Vec<bool> = self.targets.iter().enumerate().map(|(i, x)| judged[i] && !matched[i] && spot_clear(x.map)).collect();
        for (target, &a) in self.targets.iter_mut().zip(&absent) {
            if a {
                target.minimap_absent += 1;
            }
        }
        let recent = |at: u64| frame.t_ms.saturating_sub(at) <= ACTION_WINDOW_MS;
        // Clear all: no target seen anywhere, and a plain spot shows one gone. Without the click a
        // covered target may still be there, so each absent target goes on its own below.
        let emptied = !matched.iter().any(|&m| m) && absent.iter().any(|&a| a);
        self.minimap_empty = if self.clear_pressed.is_some_and(recent) && emptied { self.minimap_empty + 1 } else { 0 };
        if self.minimap_empty >= MINIMAP_CONFIRM {
            self.clear(events);
            return;
        }
        let hovered = self.hovered.clone().filter(|h| recent(h.1)).map(|h| h.0);
        let mut gone: Vec<String> = self
            .targets
            .iter()
            .filter(|x| x.minimap_absent >= if hovered.as_deref() == Some(x.label.as_str()) { MINIMAP_CONFIRM } else { MINIMAP_UNSEEN })
            .map(|x| x.label.clone())
            .collect();
        gone.sort_by_key(|l| number_of(l));
        for label in gone {
            self.targets.retain(|x| x.label != label);
            events.push(TargetEvent::Removed(label));
        }
    }

    /// Targets that should show in the view: inside the container, clear of its edge.
    fn expected(&self, t: &MapTransform) -> Vec<bool> {
        self.targets
            .iter()
            .map(|x| {
                let (fx, fy) = t.to_frame(x.map.0, x.map.1);
                fx >= t.left + EDGE_PX && fy >= t.top + EDGE_PX && fx < t.left + t.size - EDGE_PX && fy < t.top + t.size - EDGE_PX
            })
            .collect()
    }

    fn clear(&mut self, events: &mut Vec<TargetEvent>) {
        self.targets.clear();
        self.hovered = None;
        self.clear_pressed = None;
        self.disagreeing.clear();
        self.empty_since = None;
        self.minimap_empty = 0;
        self.pending.clear();
        self.unnamed.clear();
        events.push(TargetEvent::Cleared);
    }

    fn track_removals(&mut self, frame: &MapFrame, matched: &[bool], own_seen: bool, events: &mut Vec<TargetEvent>) {
        let t = &frame.transform;
        if frame.dragging {
            // A dragged marker is under the cursor or between spots: nothing goes missing.
            for target in &mut self.targets {
                target.missing_frames = 0;
                target.missing_since = None;
            }
            self.hovered = None;
            return;
        }
        let expected = self.expected(t);
        for (i, target) in self.targets.iter_mut().enumerate() {
            let missing = expected[i] && !matched[i];
            if t.view == MapView::Full {
                target.missing_frames = if missing { target.missing_frames + 1 } else { 0 };
            }
            target.missing_since = if missing { target.missing_since.or(Some(frame.t_ms)) } else { None };
        }
        // Operation side (V1).
        if t.view == MapView::Full
            && let Some(cursor) = frame.cursor
        {
            if frame.remove_hint
                && let Some(hover) = frame
                    .markers
                    .iter()
                    .filter(|m| (m.frame.0 - cursor.0).hypot(m.frame.1 - cursor.1) <= HOVER_PX)
                    .min_by(|a, b| (a.frame.0 - cursor.0).hypot(a.frame.1 - cursor.1).total_cmp(&(b.frame.0 - cursor.0).hypot(b.frame.1 - cursor.1)))
            {
                let series = self.series();
                let label = hover.label.clone().filter(|l| l.starts_with(series)).or_else(|| {
                    self.targets
                        .iter()
                        .find(|x| {
                            let (fx, fy) = t.to_frame(x.map.0, x.map.1);
                            (fx - hover.frame.0).hypot(fy - hover.frame.1) <= MATCH_PX
                        })
                        .map(|x| x.label.clone())
                });
                if let Some(label) = label {
                    self.hovered = Some((label, frame.t_ms));
                }
            }
            let (bx, by, bw, bh) = CLEAR_ALL_BUTTON;
            let (bx, by) = (f64::from(bx + frame.client_origin.0), f64::from(by + frame.client_origin.1));
            if cursor.0 >= bx && cursor.1 >= by && cursor.0 < bx + f64::from(bw) && cursor.1 < by + f64::from(bh) && !self.targets.is_empty() {
                self.clear_pressed = Some(frame.t_ms);
            }
        }
        let recent = |at: u64| frame.t_ms.saturating_sub(at) <= ACTION_WINDOW_MS;
        // Emptied map: no own marker shown where targets should be.
        let should_show = self.targets.iter().zip(&expected).any(|(x, &e)| e && (t.view != MapView::Full || x.missing_frames >= MISSING_FRAMES));
        if !own_seen && should_show {
            let since = *self.empty_since.get_or_insert(frame.t_ms);
            if self.clear_pressed.is_some_and(recent) || frame.t_ms.saturating_sub(since) >= EMPTIED_MS {
                self.clear(events);
                return;
            }
        } else {
            self.empty_since = None;
        }
        // Single deletion seen: the hovered target is gone, every other target in view is still there.
        if let Some((label, at)) = self.hovered.clone()
            && recent(at)
            && frame.right_click != Some(false)
            && let Some(i) = self.targets.iter().position(|x| x.label == label)
            && self.targets[i].missing_frames >= MISSING_FRAMES
            && self.targets.iter().enumerate().all(|(j, x)| j == i || !expected[j] || x.missing_frames == 0)
        {
            self.targets.remove(i);
            self.hovered = None;
            events.push(TargetEvent::Removed(label));
            return;
        }
        // Deletion not seen: missing for MISSING_MS while other own markers show.
        if own_seen {
            let mut gone: Vec<String> = self
                .targets
                .iter()
                .filter(|x| x.missing_since.is_some_and(|s| frame.t_ms.saturating_sub(s) >= MISSING_MS))
                .map(|x| x.label.clone())
                .collect();
            gone.sort_by_key(|l| number_of(l));
            for label in gone {
                self.targets.retain(|x| x.label != label);
                events.push(TargetEvent::Removed(label));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sighting(t: &MapTransform, frame: (f64, f64), label: Option<&str>) -> MarkerSighting {
        MarkerSighting { frame, map: t.to_map(frame.0, frame.1), label: label.map(str::to_owned), complete: true, from_label: false }
    }

    fn observe(book: &mut TargetBook, t: MapTransform, t_ms: u64, markers: &[MarkerSighting]) -> Vec<TargetEvent> {
        book.observe(&MapFrame { t_ms, transform: t, markers, remove_hint: false, dragging: false, right_click: None, cursor: None, client_origin: (0, 34), spot_clear: None, placed: None })
    }

    fn view() -> MapTransform {
        MapTransform::fixed(MapView::Full, (0, 34))
    }

    /// Markers at distinct points of the V1 container.
    fn spot(i: usize) -> (f64, f64) {
        (900.0 + 100.0 * i as f64, 400.0 + 60.0 * (i % 3) as f64)
    }

    fn labels(book: &TargetBook) -> Vec<String> {
        let mut l: Vec<String> = book.targets().iter().map(|x| x.label.clone()).collect();
        l.sort_by_key(|l| number_of(l));
        l
    }

    /// Feeds the same markers for two frames (new labels confirm on the second).
    fn show(book: &mut TargetBook, t_ms: &mut u64, markers: &[MarkerSighting]) -> Vec<TargetEvent> {
        let mut events = observe(book, view(), *t_ms, markers);
        *t_ms += 100;
        events.extend(observe(book, view(), *t_ms, markers));
        *t_ms += 100;
        events
    }

    #[test]
    fn a_seventh_marker_pushes_out_the_smallest_number() {
        let t = view();
        let (mut book, mut now) = (TargetBook::default(), 0);
        let mut shown: Vec<MarkerSighting> = (1..=6).map(|n| sighting(&t, spot(n), Some(&format!("A{n}")))).collect();
        show(&mut book, &mut now, &shown);
        assert_eq!(labels(&book), ["A1", "A2", "A3", "A4", "A5", "A6"]);
        shown.remove(0);
        shown.push(sighting(&t, spot(7), Some("A7")));
        let events = show(&mut book, &mut now, &shown);
        assert!(events.contains(&TargetEvent::Removed("A1".into())) && events.contains(&TargetEvent::Added("A7".into())));
        assert_eq!(labels(&book), ["A2", "A3", "A4", "A5", "A6", "A7"]);
    }

    /// A1–A6, A3, A5 and A6 deleted unseen, new marker A5 elsewhere: once A5 holds there, the old
    /// A5 and A6 are implied gone; A3 goes when it stays missing.
    #[test]
    fn a_new_number_implies_the_higher_ones_are_gone() {
        let t = view();
        let (mut book, mut now) = (TargetBook::default(), 0);
        let all: Vec<MarkerSighting> = (1..=6).map(|n| sighting(&t, spot(n), Some(&format!("A{n}")))).collect();
        show(&mut book, &mut now, &all);
        let after = vec![all[0].clone(), all[1].clone(), all[3].clone(), sighting(&t, spot(8), Some("A5"))];
        // A5 elsewhere holds for a second before it counts as placed again.
        let mut events = Vec::new();
        for _ in 0..10 {
            events.extend(show(&mut book, &mut now, &after));
        }
        assert!(events.contains(&TargetEvent::Removed("A6".into())));
        assert!(events.contains(&TargetEvent::Added("A5".into())));
        assert!(events.contains(&TargetEvent::Removed("A3".into())));
        assert_eq!(labels(&book), ["A1", "A2", "A4", "A5"]);
        assert!(book.target("A5").is_some_and(|x| (x.map.0 - t.to_map(spot(8).0, spot(8).1).0).abs() < 1e-9));
    }

    /// A1–A4, A4 deleted and placed again elsewhere, then A5 placed right after: the new A4
    /// replaces the old one and A5, placed after it, stays.
    #[test]
    fn a_marker_placed_after_a_re_placed_number_stays() {
        let t = view();
        let (mut book, mut now) = (TargetBook::default(), 0);
        let first: Vec<MarkerSighting> = (1..=4).map(|n| sighting(&t, spot(n), Some(&format!("A{n}")))).collect();
        show(&mut book, &mut now, &first);
        let after = vec![first[0].clone(), first[1].clone(), first[2].clone(), sighting(&t, spot(8), Some("A4")), sighting(&t, spot(0), Some("A5"))];
        let mut events = Vec::new();
        for _ in 0..10 {
            events.extend(show(&mut book, &mut now, &after));
        }
        assert_eq!(events, [TargetEvent::Added("A5".into()), TargetEvent::Removed("A4".into()), TargetEvent::Added("A4".into())]);
        assert_eq!(labels(&book), ["A1", "A2", "A3", "A4", "A5"]);
    }

    #[test]
    fn squad_markers_are_ignored() {
        let t = view();
        let (mut book, mut now) = (TargetBook::default(), 0);
        let shown = vec![sighting(&t, spot(1), Some("A1")), sighting(&t, spot(2), Some("B7")), sighting(&t, spot(3), Some("B8"))];
        show(&mut book, &mut now, &shown);
        assert_eq!(labels(&book), ["A1"]);
        // With a squad on the map, a cut letter far from own targets is not taken as the player's.
        let cut = vec![shown[0].clone(), shown[1].clone(), sighting(&t, spot(4), Some("?9"))];
        show(&mut book, &mut now, &cut);
        assert_eq!(labels(&book), ["A1"]);
    }

    /// The player joined third (as in a recorded squad game): A's and B's markers are on the map,
    /// taken for the player's while the letter is unknown; the player places a marker and C1
    /// appears at the clicked spot: C is the player's letter, A's markers are dropped, C1 counts.
    #[test]
    fn the_players_letter_is_learned_from_a_placement() {
        let t = view();
        let (mut book, mut now) = (TargetBook::default(), 0);
        let others = vec![sighting(&t, spot(1), Some("A11")), sighting(&t, spot(2), Some("A12")), sighting(&t, spot(3), Some("B21"))];
        show(&mut book, &mut now, &others);
        assert_eq!(labels(&book), ["A11", "A12"]);
        // The click, then the marker a few frames later.
        let click = (now, spot(5));
        let frame = |book: &mut TargetBook, t_ms: u64, markers: &[MarkerSighting]| {
            book.observe(&MapFrame { t_ms, transform: t, markers, remove_hint: false, dragging: false, right_click: Some(true), cursor: Some(spot(5)), client_origin: (0, 34), spot_clear: None, placed: Some(click) })
        };
        let mut events = frame(&mut book, now, &others);
        let mut with_c1 = others.clone();
        with_c1.push(sighting(&t, (spot(5).0 + 1.0, spot(5).1 - 1.0), Some("C1")));
        for k in 1..=3 {
            events.extend(frame(&mut book, now + 100 * k, &with_c1));
        }
        assert_eq!(events, [TargetEvent::Removed("A11".into()), TargetEvent::Removed("A12".into()), TargetEvent::Series('C'), TargetEvent::Added("C1".into())]);
        assert_eq!(book.series(), 'C');
        assert_eq!(labels(&book), ["C1"]);
        // The letter outlasts the round.
        book.reset();
        assert_eq!(book.series(), 'C');
    }

    /// A placement whose marker shows on a spot that already had one (another member's marker
    /// under the click) teaches nothing.
    #[test]
    fn a_marker_already_there_teaches_no_letter() {
        let t = view();
        let (mut book, mut now) = (TargetBook::default(), 0);
        let others = vec![sighting(&t, spot(1), Some("A1")), sighting(&t, spot(5), Some("B3"))];
        show(&mut book, &mut now, &others);
        let click = (now, spot(5));
        for k in 0..3 {
            book.observe(&MapFrame { t_ms: now + 100 * k, transform: t, markers: &others, remove_hint: false, dragging: false, right_click: Some(true), cursor: Some(spot(5)), client_origin: (0, 34), spot_clear: None, placed: Some(click) });
        }
        assert_eq!(book.series(), 'A');
        assert_eq!(labels(&book), ["A1"]);
    }

    #[test]
    fn a_dragged_marker_moves_without_being_removed() {
        let mut book = TargetBook::default();
        let t = MapTransform::fixed(MapView::Full, (0, 34));
        let at = |u: f64, v: f64, label: &str| sighting(&t, t.to_frame(u, v), Some(label));
        for k in 0..3 {
            book.observe(&MapFrame { t_ms: 100 * k, transform: t, markers: &[at(0.3, 0.3, "A1"), at(0.6, 0.6, "A2")], remove_hint: false, dragging: false, right_click: None, cursor: None, client_origin: (0, 34), spot_clear: None, placed: None });
        }
        // The drag: "Remove marker" over A2, A2 gone from its spot, then its label further on.
        let cursor = Some(t.to_frame(0.6, 0.6));
        let e1 = book.observe(&MapFrame { t_ms: 400, transform: t, markers: &[at(0.3, 0.3, "A1")], remove_hint: true, dragging: true, right_click: None, cursor, client_origin: (0, 34), spot_clear: None, placed: None });
        let e2 = book.observe(&MapFrame { t_ms: 500, transform: t, markers: &[at(0.3, 0.3, "A1"), at(0.7, 0.62, "A2")], remove_hint: true, dragging: true, right_click: None, cursor, client_origin: (0, 34), spot_clear: None, placed: None });
        let e3 = book.observe(&MapFrame { t_ms: 3000, transform: t, markers: &[at(0.3, 0.3, "A1"), at(0.7, 0.62, "A2")], remove_hint: false, dragging: false, right_click: None, cursor: None, client_origin: (0, 34), spot_clear: None, placed: None });
        assert!(e1.is_empty(), "{e1:?}");
        assert_eq!(e2, vec![TargetEvent::Moved("A2".into())]);
        assert!(e3.is_empty(), "{e3:?}");
        let a2 = book.target("A2").expect("A2 kept");
        assert!((a2.map.0 - 0.7).abs() < 1e-6 && (a2.map.1 - 0.62).abs() < 1e-6);
    }

    #[test]
    fn a_hovered_marker_is_deleted_with_a_right_click() {
        let mut book = TargetBook::default();
        let t = MapTransform::fixed(MapView::Full, (0, 34));
        let at = |u: f64, v: f64, label: &str| sighting(&t, t.to_frame(u, v), Some(label));
        let cursor = Some(t.to_frame(0.6, 0.6));
        let mut observe = |t_ms: u64, markers: &[MarkerSighting], remove_hint: bool, right_click: bool| {
            book.observe(&MapFrame { t_ms, transform: t, markers, remove_hint, dragging: false, right_click: Some(right_click), cursor, client_origin: (0, 34), spot_clear: None, placed: None })
        };
        let both = [at(0.3, 0.3, "A1"), at(0.6, 0.6, "A2")];
        let one = [at(0.3, 0.3, "A1")];
        for k in 0..3 {
            observe(100 * k, &both, false, false);
        }
        // Hovering shows the hint before the click; after the click the marker is gone and the
        // hint is back to "Add marker".
        observe(400, &both, true, false);
        let mut events = Vec::new();
        for k in 0..3 {
            events.extend(observe(500 + 100 * k, &one, false, true));
        }
        assert_eq!(events, vec![TargetEvent::Removed("A2".into())]);
    }

    /// A1, A2 and A3 each seen once while placed (the zoomed view moving on in between), then A3
    /// again: the earlier labels are confirmed first, in placement order.
    #[test]
    fn labels_seen_once_are_confirmed_in_number_order() {
        let t = view();
        let mut book = TargetBook::default();
        let points: Vec<(f64, f64)> = (1..=3).map(|n| t.to_map(spot(n).0, spot(n).1)).collect();
        // A 5× V1 view centred on a point; the other spots are outside it.
        let close_up = |m: (f64, f64)| MapTransform::zoomed(MapView::Full, (0, 34), 5.0, (m.0 - 0.1, m.1 - 0.1));
        let mut events = Vec::new();
        for (k, &m) in points.iter().chain(&points[2..]).enumerate() {
            let z = close_up(m);
            let s = MarkerSighting { frame: z.to_frame(m.0, m.1), map: m, label: Some(format!("A{}", k.min(2) + 1)), complete: true, from_label: false };
            events.extend(observe(&mut book, z, k as u64 * 1000, &[s]));
        }
        assert_eq!(events, ["A1", "A2", "A3"].map(|l| TargetEvent::Added(l.into())));
    }

    /// A new diamond whose label box lies under an objective icon gets the next number, and takes
    /// its real label when that shows.
    #[test]
    fn hidden_label_gets_the_next_number() {
        let t = view();
        let (mut book, mut now) = (TargetBook::default(), 0);
        book.set_objectives(vec![t.to_map(1300.0, 700.0)]);
        let a2 = sighting(&t, (1000.0, 500.0), Some("A2"));
        assert_eq!(show(&mut book, &mut now, std::slice::from_ref(&a2)), vec![TargetEvent::Added("A2".into())]);
        // Label box (+9..+36, −27..−16) of a marker at (1280, 722) reaches the icon centre.
        let hidden = sighting(&t, (1280.0, 722.0), None);
        assert_eq!(show(&mut book, &mut now, &[a2.clone(), hidden]), vec![TargetEvent::Added("A3".into())]);
        assert!(book.target("A3").is_some_and(|x| x.inferred));
        let named = sighting(&t, (1280.0, 722.0), Some("A3"));
        assert!(show(&mut book, &mut now, &[a2, named]).is_empty());
        assert!(book.target("A3").is_some_and(|x| !x.inferred));
    }

    /// A marker placed near the map's top edge, its label cut off by the container, gets the next
    /// number like one under an objective icon.
    #[test]
    fn label_cut_by_the_edge_gets_the_next_number() {
        let t = view();
        let (mut book, mut now) = (TargetBook::default(), 0);
        let a1 = sighting(&t, (1000.0, 600.0), Some("A1"));
        assert_eq!(show(&mut book, &mut now, std::slice::from_ref(&a1)), vec![TargetEvent::Added("A1".into())]);
        // 20 px below the container's top (260): its label box starts 7 px above it.
        let edge = sighting(&t, (1200.0, t.top + 20.0), None);
        assert_eq!(show(&mut book, &mut now, &[a1, edge]), vec![TargetEvent::Added("A2".into())]);
    }

    /// A marker dragged to the map's edge, where its label is cut off, keeps its number.
    #[test]
    fn marker_dragged_to_the_edge_keeps_its_label() {
        let t = view();
        let (mut book, mut now) = (TargetBook::default(), 0);
        let (a1, a2) = (sighting(&t, (1000.0, 600.0), Some("A1")), sighting(&t, (1300.0, 700.0), Some("A2")));
        show(&mut book, &mut now, &[a1.clone(), a2]);
        let edge = sighting(&t, (1250.0, t.top + 18.0), None);
        let dragged = book.observe(&MapFrame { t_ms: now, transform: t, markers: &[a1.clone(), edge.clone()], remove_hint: false, dragging: true, right_click: None, cursor: None, client_origin: (0, 34), spot_clear: None, placed: None });
        assert_eq!(dragged, vec![TargetEvent::Moved("A2".into())]);
        // Released: it stays, nothing is removed or added.
        for k in 1..30 {
            assert!(observe(&mut book, t, now + 100 * k, &[a1.clone(), edge.clone()]).is_empty());
        }
        assert!(book.target("A2").is_some_and(|x| (x.map.0 - edge.map.0).abs() < 1e-9));
    }

    /// An unlabelled diamond away from objective icons is not numbered.
    #[test]
    fn unlabelled_marker_in_the_open_is_not_numbered() {
        let t = view();
        let (mut book, mut now) = (TargetBook::default(), 0);
        book.set_objectives(vec![t.to_map(1300.0, 700.0)]);
        let open = sighting(&t, (1000.0, 900.0), None);
        for _ in 0..5 {
            assert!(show(&mut book, &mut now, std::slice::from_ref(&open)).is_empty());
        }
        assert!(book.targets().is_empty());
    }

    /// One minimap frame: the given labelled targets' spots show diamonds; `clear` says whether a
    /// spot without one shows plain map.
    fn minimap(book: &mut TargetBook, t_ms: u64, shown: &[&str], clear: bool) -> Vec<TargetEvent> {
        let mini = MapTransform::fixed(MapView::Mini, (0, 34));
        let markers: Vec<MarkerSighting> = book
            .targets()
            .iter()
            .filter(|x| shown.contains(&x.label.as_str()))
            .map(|x| MarkerSighting { frame: mini.to_frame(x.map.0, x.map.1), map: x.map, label: None, complete: true, from_label: false })
            .collect();
        let spot = |_: (f64, f64)| clear;
        book.observe(&MapFrame { t_ms, transform: mini, markers: &markers, remove_hint: false, dragging: false, right_click: None, cursor: None, client_origin: (0, 34), spot_clear: Some(&spot), placed: None })
    }

    fn two_targets() -> (TargetBook, u64) {
        let t = view();
        let (mut book, mut now) = (TargetBook::default(), 0);
        show(&mut book, &mut now, &[sighting(&t, spot(1), Some("A1")), sighting(&t, spot(2), Some("A2"))]);
        (book, now)
    }

    /// A2 hovered with "Remove marker", then the map closed: the minimap confirms the deletion
    /// after two frames.
    #[test]
    fn minimap_confirms_a_hovered_deletion() {
        let (mut book, now) = two_targets();
        let t = view();
        let a = sighting(&t, spot(1), Some("A1"));
        let b = sighting(&t, spot(2), Some("A2"));
        book.observe(&MapFrame { t_ms: now, transform: t, markers: &[a, b], remove_hint: true, dragging: false, right_click: None, cursor: Some(spot(2)), client_origin: (0, 34), spot_clear: None, placed: None });
        assert!(minimap(&mut book, now + 500, &["A1"], true).is_empty());
        assert_eq!(minimap(&mut book, now + 1000, &["A1"], true), vec![TargetEvent::Removed("A2".into())]);
    }

    /// Without a hover the deletion needs five minimap frames.
    #[test]
    fn minimap_removes_an_unseen_deletion_later() {
        let (mut book, now) = two_targets();
        let mut events = Vec::new();
        for k in 1..=4 {
            events.extend(minimap(&mut book, now + 500 * k, &["A1"], true));
        }
        assert!(events.is_empty());
        assert_eq!(minimap(&mut book, now + 2500, &["A1"], true), vec![TargetEvent::Removed("A2".into())]);
    }

    /// Cursor on Clear all, then the map closed: two empty minimap frames clear the targets, the
    /// covered one included.
    #[test]
    fn minimap_confirms_clear_all() {
        let (mut book, now) = two_targets();
        let (bx, by, bw, bh) = CLEAR_ALL_BUTTON;
        let cursor = (f64::from(bx) + f64::from(bw) / 2.0, f64::from(by + 34) + f64::from(bh) / 2.0);
        let t = view();
        let shown = [sighting(&t, spot(1), Some("A1")), sighting(&t, spot(2), Some("A2"))];
        book.observe(&MapFrame { t_ms: now, transform: t, markers: &shown, remove_hint: false, dragging: false, right_click: None, cursor: Some(cursor), client_origin: (0, 34), spot_clear: None, placed: None });
        // A2's spot covered: A1's plain spot shows it gone, nothing is seen.
        let mini = MapTransform::fixed(MapView::Mini, (0, 34));
        let a2 = book.target("A2").expect("A2").map;
        let spot = move |m: (f64, f64)| m != a2;
        for k in 1..=2 {
            let events = book.observe(&MapFrame { t_ms: now + 500 * k, transform: mini, markers: &[], remove_hint: false, dragging: false, right_click: None, cursor: None, client_origin: (0, 34), spot_clear: Some(&spot), placed: None });
            assert_eq!(events, if k == 2 { vec![TargetEvent::Cleared] } else { vec![] });
        }
    }

    /// A covered spot (a vehicle icon over the marker) says nothing: no removal however long; and
    /// without Clear all an empty-looking minimap only removes targets one by one.
    #[test]
    fn covered_minimap_spot_removes_nothing() {
        let (mut book, now) = two_targets();
        for k in 1..=20 {
            assert!(minimap(&mut book, now + 500 * k, &["A1"], false).is_empty());
        }
        assert_eq!(book.targets().len(), 2);
    }
}
