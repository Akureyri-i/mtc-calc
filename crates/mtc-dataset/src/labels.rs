//! Ground-truth labels. Every field is optional: `None` means the property was not labelled for
//! that frame, while an explicit empty list means it was checked and nothing is there (for
//! example `markers: Some(vec![])` = no markers visible). Positions are frame pixels.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FrameLabel {
    /// Matches `FrameMeta::file`.
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<Scene>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<AimReadout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ammo: Option<AmmoUi>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<MapState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markers: Option<Vec<Marker>>,
    /// Pivot of the player's own arrow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<Point>,
    /// Yellow X impact markers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impacts: Option<Vec<ImpactLabel>>,
    /// Loading UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loading: Option<LoadingLabel>,
    /// The seat menu in the bottom-left corner, top to bottom; empty when not shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seats: Option<Vec<SeatRowLabel>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One seat of the seat menu. A passenger group ("three seat icons and a count") is not a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatRowLabel {
    pub kind: SeatKind,
    /// A player sits there (a name, not "Empty").
    pub occupied: bool,
    /// The recording player sits there.
    #[serde(default)]
    pub me: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatKind {
    Driver,
    Gunner,
    Commander,
    Loader,
    Passenger,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadingLabel {
    pub state: LoadingState,
    /// With `state: menu`: which layout the cards are in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu_seat: Option<Seat>,
    /// With `state: menu`: each card's charge, left to right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cards: Option<Vec<CardChargeLabel>>,
    /// With `state: menu`: segments of each card's charge bag capsule (0 without one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bags: Option<Vec<u32>>,
    /// Highlighted shell slot (1-based) in the gunner's bar. Unlike elsewhere, absent here means
    /// checked and none shows (the whole `loading` record is labelled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar_slot: Option<u32>,
    /// Segments of the charge bag capsule on the highlighted slot of the gunner's bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar_bags: Option<u32>,
    /// Shell type icon on the highlighted slot of the gunner's bar: the icon library's file name
    /// ("he", "time-fuze", ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar_icon: Option<String>,
    /// The gunner's bar collapsed under the full map, with its highlighted slot (0: none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_bar_slot: Option<u32>,
    /// The loader's simplified bar after loading; absent when it does not show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader_bar: Option<LoaderBarLabel>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderBarLabel {
    pub slots: u32,
    /// Framed (loaded) slot, 1-based; absent when none is framed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadingState {
    /// No loading UI (the ammo bar may still show elsewhere, e.g. collapsed under a map).
    None,
    /// Ammo cards open.
    Menu,
    /// Loading countdown bar shown.
    Countdown,
    /// Only the ammo bar after loading (gunner's bar or the loader's simplified one).
    Bar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardChargeLabel {
    /// The shell has no charges (no charge panel).
    None,
    Low,
    Medium,
    High,
    /// Under the hover detail box, or not decidable (mid-click).
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MapView {
    /// Full map opened with M, not scoped (zoomable, markers editable).
    V1,
    /// Map opened with M while scoped.
    V2,
    /// Resident minimap, top right.
    V3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Seat {
    Gunner,
    Loader,
    /// Any other crew seat (driver, commander, ...).
    Other,
    Dismounted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Charge {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// Map view shown, `None` when no map is visible (for example a menu covers it).
    pub map_view: Option<MapView>,
    pub scoped: bool,
    pub seat: Option<Seat>,
}

/// Angle readout text exactly as displayed, without the degree sign ("+0.01", "032.73").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AimReadout {
    pub elevation: String,
    pub azimuth: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AmmoUi {
    /// Weapon label text ("155 mm M284").
    pub weapon: Option<String>,
    /// Ammo slots left to right as shown on screen.
    pub slots: Vec<AmmoSlot>,
    /// 1-based slot highlighted in the bottom bar.
    pub highlighted_slot: Option<u32>,
    /// Countdown text while loading ("17.1").
    pub countdown: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AmmoSlot {
    /// 1-based position.
    pub index: u32,
    /// Shell name as in the ammo data, without the charge suffix ("M107").
    pub shell: Option<String>,
    /// Selected charge, `None` for shells without charges.
    pub charge: Option<Charge>,
    pub remaining: Option<u32>,
    pub capacity: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapState {
    /// Map key in the calculator's map configuration.
    pub id: String,
    pub view: MapView,
    /// V1 content zoom relative to the unzoomed view.
    pub zoom: Option<f64>,
    /// V1 only: normalised map coordinate drawn at the container's top-left corner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// "A1".."A9…", `None` where the view shows no label (V3).
    pub id: Option<String>,
    pub x: f64,
    pub y: f64,
}

/// One yellow X, where it is drawn. Icons drawn over part of it are noted: `under`, the
/// centre of the objective icon over it (an X beside or mostly under one, often only the tips of
/// its arms in view); `covered`, vehicle icons or a death X over part of its arms.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImpactLabel {
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<Point>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub covered: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
