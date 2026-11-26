//! What the player set on the HUD, kept between runs in `%APPDATA%\MTC Calculator\prefs.json`:
//! the panel's lock and where it was dragged to, per seat; whether the plan panel's terrain
//! profile is open; and the settings card: the landing prediction on or off, the aim tolerance,
//! the charge rule, what V1's small labels show, the drag follow's fine-tuning. Positions are the
//! panel's bottom-right corner in client pixels, so they hold wherever the game window is.

use std::path::PathBuf;

use mtc_overlay::{ChargeRule, LabelFields, Setting, SettingsState, Stepper, ToleranceMode};
use serde::{Deserialize, Serialize};

/// The angle tolerance's range and step (°), and its value when the player switches to it.
const ANGLE: (f64, f64, f64) = (0.01, 1.0, 0.01);
const DEFAULT_ANGLE: f64 = 0.05;
/// The tolerated miss on the ground: range and step (m), and the default (shells show no spread,
/// the calibrated model misses by 1–4 m, the measured shells kill within 7.5–25.6 m; ±0.05° put
/// flat full-charge shots 44–93 m off).
const IMPACT: (f64, f64, f64) = (1.0, 50.0, 1.0);
const DEFAULT_IMPACT: f64 = 5.0;
/// Half a step of the angle readouts (°, they show hundredths): the nearest reading is at most this
/// far from any solution, so a tolerance below it might never be met.
pub const READOUT_HALF_STEP: f64 = 0.005;
/// Which defaults a prefs file has taken: 1 since the tolerance by impact became the default.
const DEFAULTS_VERSION: u32 = 1;
/// The drag follow's fine-tuning: range and step (ms), as `--follow-tune`'s [ and ] keys.
const DRAG: (i32, i32, i32) = (-60, 60, 2);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub panel_locked: bool,
    /// Where the dragged panel is in the gunner seat (third person and scoped): its offset from
    /// where it sits by default (beside the angle readout), divided by the panel's scale, so it
    /// keeps its place by the game's HUD at every window size and scaling (a place in client px
    /// had to be dragged again after each change); `None`: there.
    pub panel_gunner_offset: Option<(f32, f32)>,
    /// The same in the loader seat (by default left of the cards).
    pub panel_loader_offset: Option<(f32, f32)>,
    /// The dragged panel's place in client px as older versions saved it, each dragged at the
    /// reference configuration: taken as the offsets on the first run there, then cleared.
    pub panel_gunner: Option<(i32, i32)>,
    pub panel_loader: Option<(i32, i32)>,
    pub plan_terrain_open: bool,
    /// The plan panel lists every target (else only the selected one).
    pub plan_targets_open: bool,
    /// The landing prediction shows.
    pub show_pred: bool,
    pub tolerance_mode: ToleranceMode,
    /// Elevation and azimuth tolerance by angle (°).
    pub tolerance_angle: (f64, f64),
    /// Tolerated miss along and across the line of fire (m).
    pub tolerance_impact: (f64, f64),
    pub charge_rule: ChargeRule,
    pub map_labels: LabelFields,
    /// The drag follow's fine-tuning (ms, positive: later); 0 leaves it automatic.
    pub drag_offset_ms: i32,
    /// The player agreed to send statistics, for this record version
    /// (`diag::RECORD_VERSION`: asked again when what is collected grows), or asked not to be
    /// asked again.
    pub share_stats: bool,
    pub consent_version: u32,
    pub consent_never: bool,
    /// A problem report gets screenshots of what the HUD reads.
    pub report_screenshots: bool,
    /// `DEFAULTS_VERSION` when the file was written (0: before it existed, so a file without it
    /// reads 0, not the new default's version).
    #[serde(default)]
    pub defaults_version: u32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            panel_locked: true,
            share_stats: false,
            consent_version: 0,
            consent_never: false,
            report_screenshots: false,
            panel_gunner_offset: None,
            panel_loader_offset: None,
            panel_gunner: None,
            panel_loader: None,
            plan_terrain_open: false,
            plan_targets_open: true,
            show_pred: true,
            tolerance_mode: ToleranceMode::Impact,
            tolerance_angle: (DEFAULT_ANGLE, DEFAULT_ANGLE),
            tolerance_impact: (DEFAULT_IMPACT, DEFAULT_IMPACT),
            charge_rule: ChargeRule::NearestCharge,
            map_labels: LabelFields::default(),
            drag_offset_ms: 0,
            defaults_version: DEFAULTS_VERSION,
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("MTC Calculator").join("prefs.json"))
}

/// `value` moved `steps` steps of `step` within `min`–`max`, on the step's grid.
fn stepped(value: f64, steps: i32, (min, max, step): (f64, f64, f64)) -> f64 {
    (((value / step).round() + f64::from(steps)) * step).clamp(min, max)
}

impl Prefs {
    pub fn load() -> Self {
        path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|json| serde_json::from_str::<Self>(&json).ok()).map_or_else(Self::default, Self::upgraded)
    }

    /// A file written before the tolerance by impact became the default takes it once; a choice
    /// made after stays.
    fn upgraded(mut self) -> Self {
        if self.defaults_version < 1 {
            self.tolerance_mode = ToleranceMode::Impact;
            self.tolerance_impact = (DEFAULT_IMPACT, DEFAULT_IMPACT);
        }
        self.defaults_version = DEFAULTS_VERSION;
        self
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, serde_json::to_string_pretty(self).expect("prefs to json")));
        if let Err(e) = written {
            log::warn!("Settings could not be written to {}: {e}", path.display());
        }
    }

    /// Takes a change from the settings card.
    pub fn apply(&mut self, setting: Setting) {
        match setting {
            Setting::Pred(on) => self.show_pred = on,
            Setting::Mode(mode) => self.tolerance_mode = mode,
            Setting::Rule(rule) => self.charge_rule = rule,
            Setting::Label(field, on) => self.map_labels.set(field, on),
            Setting::Share(on) => {
                self.share_stats = on;
                if on {
                    self.consent_version = crate::diag::RECORD_VERSION;
                }
            }
            Setting::ReportShots(on) => self.report_screenshots = on,
            Setting::Step(stepper, steps) => match (stepper, self.tolerance_mode) {
                (Stepper::Elevation, ToleranceMode::Angle) => self.tolerance_angle.0 = stepped(self.tolerance_angle.0, steps, ANGLE),
                (Stepper::Azimuth, ToleranceMode::Angle) => self.tolerance_angle.1 = stepped(self.tolerance_angle.1, steps, ANGLE),
                (Stepper::Elevation, ToleranceMode::Impact) => self.tolerance_impact.0 = stepped(self.tolerance_impact.0, steps, IMPACT),
                (Stepper::Azimuth, ToleranceMode::Impact) => self.tolerance_impact.1 = stepped(self.tolerance_impact.1, steps, IMPACT),
                (Stepper::DragOffset, _) => self.drag_offset_ms = (self.drag_offset_ms + steps * DRAG.2).clamp(DRAG.0, DRAG.1),
            },
        }
    }

    /// Statistics are sent: agreed to for the record version now.
    pub fn sharing(&self) -> bool {
        self.share_stats && self.consent_version == crate::diag::RECORD_VERSION
    }

    /// The startup question shows: not sharing, and not asked to stop asking (unless what is
    /// collected grew since the player agreed).
    pub fn ask_consent(&self) -> bool {
        !self.sharing() && (!self.consent_never || self.share_stats)
    }

    /// The settings for the run record.
    pub fn diag_settings(&self) -> std::collections::BTreeMap<&'static str, String> {
        let mut m = std::collections::BTreeMap::new();
        m.insert("tolerance_mode", format!("{:?}", self.tolerance_mode));
        m.insert("tolerance_impact_m", format!("{:.0}/{:.0}", self.tolerance_impact.0, self.tolerance_impact.1));
        m.insert("tolerance_angle_deg", format!("{:.2}/{:.2}", self.tolerance_angle.0, self.tolerance_angle.1));
        m.insert("charge_rule", format!("{:?}", self.charge_rule));
        m.insert("show_prediction", self.show_pred.to_string());
        m.insert("map_labels", format!("{:?}", self.map_labels));
        m.insert("drag_offset_ms", self.drag_offset_ms.to_string());
        m.insert("panel_moved", (self.panel_gunner_offset.is_some() || self.panel_loader_offset.is_some()).to_string());
        m
    }

    /// The settings window showing these values, centred on `centre` (screen px), at `scale`.
    pub fn settings_state(&self, centre: (i32, i32), scale: f32, report_note: Option<String>) -> SettingsState {
        let ends = |v: f64, (min, max, step): (f64, f64, f64)| (v <= min + step / 2.0, v >= max - step / 2.0);
        let (elevation, azimuth) = match self.tolerance_mode {
            ToleranceMode::Angle => (ends(self.tolerance_angle.0, ANGLE), ends(self.tolerance_angle.1, ANGLE)),
            ToleranceMode::Impact => (ends(self.tolerance_impact.0, IMPACT), ends(self.tolerance_impact.1, IMPACT)),
        };
        SettingsState {
            centre,
            pred: self.show_pred,
            mode: self.tolerance_mode,
            angle: self.tolerance_angle,
            impact: self.tolerance_impact,
            rule: self.charge_rule,
            labels: self.map_labels,
            drag_offset_ms: self.drag_offset_ms,
            limits: [elevation, azimuth, (self.drag_offset_ms <= DRAG.0, self.drag_offset_ms >= DRAG.1)],
            share: self.sharing(),
            report_shots: self.report_screenshots,
            report_note,
            scale,
        }
    }
}

/// The angle tolerances (elevation, azimuth; °) a tolerated miss on the ground (`along`, `across`;
/// m) makes for a target `distance_m` out whose range changes `slope` m per degree of elevation;
/// `angle` where either is unknown. Never finer than half a readout step.
pub fn tolerance_from_impact((along, across): (f64, f64), slope: Option<f64>, distance_m: Option<f64>, angle: (f64, f64)) -> (f64, f64) {
    let elevation = slope.filter(|s| *s > 0.0).map_or(angle.0, |s| along / s);
    let azimuth = distance_m.filter(|d| *d > 0.0).map_or(angle.1, |d| (across / d).to_degrees());
    (elevation.max(READOUT_HALF_STEP), azimuth.max(READOUT_HALF_STEP))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ±5 m on a Low arc (117 m/°) is ±0.043°; on a flat full-charge shot (1850 m/°) it would be
    /// 0.0027°, below half a readout step, so it is held there; across, 5 m at 2 km is 0.143°.
    #[test]
    fn a_miss_on_the_ground_becomes_an_angle_no_finer_than_the_readout() {
        let (e, a) = tolerance_from_impact((5.0, 5.0), Some(117.0), Some(2000.0), (0.05, 0.05));
        assert!((e - 0.0427).abs() < 1e-3 && (a - 0.1432).abs() < 1e-3, "{e} {a}");
        let (e, _) = tolerance_from_impact((5.0, 5.0), Some(1850.0), Some(2000.0), (0.05, 0.05));
        assert_eq!(e, READOUT_HALF_STEP);
        assert_eq!(tolerance_from_impact((5.0, 5.0), None, None, (0.05, 0.06)), (0.05, 0.06));
    }

    #[test]
    fn settings_step_on_their_grid_within_their_range() {
        let mut p = Prefs::default();
        p.apply(Setting::Mode(ToleranceMode::Angle));
        p.apply(Setting::Step(Stepper::Elevation, 1));
        assert!((p.tolerance_angle.0 - 0.06).abs() < 1e-9);
        for _ in 0..200 {
            p.apply(Setting::Step(Stepper::Azimuth, -1));
        }
        assert!((p.tolerance_angle.1 - ANGLE.0).abs() < 1e-9);
        assert_eq!(p.settings_state((0, 0), 1.0, None).limits[1], (true, false));
        p.apply(Setting::Mode(ToleranceMode::Impact));
        p.apply(Setting::Step(Stepper::Elevation, 3));
        assert!((p.tolerance_impact.0 - 8.0).abs() < 1e-9 && (p.tolerance_angle.0 - 0.06).abs() < 1e-9);
        for _ in 0..40 {
            p.apply(Setting::Step(Stepper::DragOffset, 1));
        }
        assert_eq!(p.drag_offset_ms, DRAG.1);
    }

    #[test]
    fn an_old_prefs_file_keeps_its_values_and_takes_the_new_defaults() {
        let p: Prefs = serde_json::from_str(r#"{"panel_locked": false, "plan_terrain_open": true}"#).expect("parse");
        assert!(!p.panel_locked && p.plan_terrain_open);
        assert!(p.show_pred && !p.map_labels.any() && p.tolerance_mode == ToleranceMode::Impact);
    }

    /// A file from before the tolerance by impact was the default (angle mode written in it) takes
    /// it once; one written after keeps the angle mode chosen.
    #[test]
    fn the_tolerance_default_moves_old_files_once() {
        let old: Prefs = serde_json::from_str(r#"{"tolerance_mode": "Angle", "tolerance_angle": [0.08, 0.08]}"#).expect("parse");
        let old = old.upgraded();
        assert_eq!((old.tolerance_mode, old.tolerance_impact, old.tolerance_angle), (ToleranceMode::Impact, (5.0, 5.0), (0.08, 0.08)));
        let chosen: Prefs = serde_json::from_str(r#"{"tolerance_mode": "Angle", "defaults_version": 1}"#).expect("parse");
        assert_eq!(chosen.upgraded().tolerance_mode, ToleranceMode::Angle);
    }
}
