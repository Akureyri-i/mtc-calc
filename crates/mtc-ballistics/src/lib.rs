//! Artillery ballistics, ported from the upstream calculator (`math.ts` of
//! grand-hawk/artillery-calculator at 8c83e02) and checked against its output on every projectile
//! of the gun data (`fixtures/ballistics/golden.json`).
//!
//! The flight is stepped like the game's: 1/60 s frames, each split into sub-steps of at most
//! about `SUBSTEP_STUDS`, gravity then quadratic drag then position. One trajectory per degree
//! from −90° to 90° is traced once per projectile (`Ballistics::new`); an arc is found where the
//! height miss at the target distance changes sign between two of them and refined by bisection
//! on freshly shot trajectories.
//!
//! Distances are in metres, heights relative to the gun, angles in degrees, times in seconds.
//! Arithmetic follows the original operation by operation, `hypot` included (V8's: normalised by
//! the largest value, Kahan-summed), so results match it to rounding.

use serde::Deserialize;

/// In-game gravity (m/s²), the default.
const G: f64 = 9.81;
/// Air density (kg/m³) of the blast formula.
const AIR_DENSITY: f64 = 1.2;
/// The game's frame step: the trajectory depends on it.
const FRAME_STEP: f64 = 1.0 / 60.0;
const SUBSTEP_STUDS: f64 = 20.0;
/// Traced trajectories keep every this many frames.
const FRAMES_PER_SAMPLE: u32 = 3;
const MIN_ANGLE: i32 = -90;
const MAX_ANGLE: i32 = 90;
/// Height (m) below the gun at which a trajectory stops being traced.
const MIN_HEIGHT: f64 = -3000.0;
const REFINE_ITERATIONS: u32 = 20;
/// Largest height miss (m) for a refined angle to count as a solution.
const REFINE_TOLERANCE: f64 = 1.0;

/// A projectile's physics, as in the gun data (missing values take the game's defaults).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Projectile {
    /// Muzzle velocity, m/s.
    pub velocity: f64,
    /// Gravitational acceleration, m/s² (default `G`).
    pub gravity: Option<f64>,
    /// Quadratic drag constant, 1/m (default 0).
    pub drag_constant: Option<f64>,
    /// Seconds before the projectile despawns (default: never).
    pub lifetime: Option<f64>,
    pub submunitions: Option<f64>,
    /// Full spread angle of the submunitions, degrees.
    pub submunition_angular_dispersion: Option<f64>,
    /// Share of the apex time at which the submunitions deploy.
    pub submunition_deploy_factor: Option<f64>,
    /// kg.
    pub explosive_mass: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc {
    /// Elevation, degrees.
    pub elevation: f64,
    pub time_of_flight: f64,
}

/// Studs to metres (5 studs = 1.8 m).
pub fn studs_to_meters(s: f64) -> f64 {
    s / (5.0 / 1.8)
}

pub fn meters_to_studs(m: f64) -> f64 {
    m * (5.0 / 1.8)
}

/// `Math.hypot` as V8 computes it: values normalised by the largest, squares Kahan-summed.
fn hypot(values: &[f64]) -> f64 {
    if values.iter().any(|v| v.is_nan()) {
        return if values.iter().any(|v| v.is_infinite()) { f64::INFINITY } else { f64::NAN };
    }
    let max = values.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if max == f64::INFINITY {
        return f64::INFINITY;
    }
    if max == 0.0 {
        return 0.0;
    }
    let (mut sum, mut compensation) = (0.0f64, 0.0f64);
    for v in values {
        let n = v.abs() / max;
        let summand = n * n - compensation;
        let preliminary = sum + summand;
        compensation = (preliminary - sum) - summand;
        sum = preliminary;
    }
    sum.sqrt() * max
}

#[derive(Clone, Copy, Debug)]
struct Flight {
    x: f64,
    y: f64,
    z: f64,
    vx: f64,
    vy: f64,
    vz: f64,
    time: f64,
}

fn launch(v: f64, elevation: f64) -> Flight {
    let radians = (elevation * std::f64::consts::PI) / 180.0;
    Flight { x: 0.0, y: 0.0, z: 0.0, vx: v * radians.cos(), vy: v * radians.sin(), vz: 0.0, time: 0.0 }
}

fn in_flight(f: &Flight, p: &Projectile) -> bool {
    f.time <= p.lifetime.unwrap_or(f64::INFINITY) && f.y > MIN_HEIGHT && f.vx > 0.0
}

fn step_frame(f: &mut Flight, p: &Projectile) {
    let gravity = p.gravity.unwrap_or(G);
    let drag = p.drag_constant.unwrap_or(0.0);
    let speed = hypot(&[f.vx, f.vy, f.vz]);
    let substeps = ((meters_to_studs(speed) * FRAME_STEP) / SUBSTEP_STUDS).max(1.0).round();
    let dt = FRAME_STEP / substeps;
    for _ in 0..substeps as u32 {
        f.vy -= gravity * dt;
        let scale = (-drag * hypot(&[f.vx, f.vy, f.vz]) * dt).exp();
        f.vx *= scale;
        f.vy *= scale;
        f.vz *= scale;
        f.x += f.vx * dt;
        f.y += f.vy * dt;
        f.z += f.vz * dt;
    }
    f.time += FRAME_STEP;
}

fn has_landed(f: &Flight, h: f64) -> bool {
    f.vy < 0.0 && f.y < h
}

/// Height miss at distance `d` for a target at height `h` (−∞ when the projectile falls short,
/// NaN when it despawns above the target), and the time of flight.
fn shoot(p: &Projectile, elevation: f64, d: f64, h: f64) -> (f64, f64) {
    let mut f = launch(p.velocity, elevation);
    while in_flight(&f, p) {
        let (x, y, time) = (f.x, f.y, f.time);
        step_frame(&mut f, p);
        if f.x >= d {
            let fraction = (d - x) / (f.x - x);
            return (y + (f.y - y) * fraction - h, time + FRAME_STEP * fraction);
        }
    }
    (if f.y < h { f64::NEG_INFINITY } else { f64::NAN }, f64::NAN)
}

struct Trajectory {
    xs: Vec<f64>,
    ys: Vec<f64>,
}

impl Trajectory {
    /// Horizontal distance (m) where it comes down through height `h`, interpolated between
    /// samples; `None` when it ends above `h`.
    fn landing(&self, h: f64) -> Option<f64> {
        let last = self.xs.len() - 1;
        if self.ys[last] >= h {
            return None;
        }
        let mut i = last;
        while i > 0 && self.ys[i - 1] < h {
            i -= 1;
        }
        if i == 0 {
            return None;
        }
        let fraction = (self.ys[i - 1] - h) / (self.ys[i - 1] - self.ys[i]);
        Some(self.xs[i - 1] + (self.xs[i] - self.xs[i - 1]) * fraction)
    }
}

fn miss_on(t: &Trajectory, d: f64, h: f64) -> f64 {
    let last = t.xs.len() - 1;
    if t.xs[last] < d {
        return if t.ys[last] < h { f64::NEG_INFINITY } else { f64::NAN };
    }
    let (mut low, mut high) = (0usize, last);
    while high - low > 1 {
        let middle = (low + high) / 2;
        if t.xs[middle] < d {
            low = middle;
        } else {
            high = middle;
        }
    }
    let fraction = (d - t.xs[low]) / (t.xs[high] - t.xs[low]);
    t.ys[low] + (t.ys[high] - t.ys[low]) * fraction - h
}

fn refine_arc(p: &Projectile, short: f64, long: f64, d: f64, h: f64) -> Option<Arc> {
    let (mut short, mut long) = (short, long);
    for _ in 0..REFINE_ITERATIONS {
        let middle = (short + long) / 2.0;
        if shoot(p, middle, d, h).0 < 0.0 {
            short = middle;
        } else {
            long = middle;
        }
    }
    let elevation = (short + long) / 2.0;
    let (miss, time_of_flight) = shoot(p, elevation, d, h);
    (miss.abs() < REFINE_TOLERANCE).then_some(Arc { elevation, time_of_flight })
}

/// One projectile's traced trajectories (one per degree), for arcs and maximum range.
pub struct Ballistics {
    projectile: Projectile,
    trajectories: Vec<Trajectory>,
}

impl Ballistics {
    pub fn new(projectile: Projectile) -> Self {
        let trajectories = (MIN_ANGLE..=MAX_ANGLE)
            .map(|elevation| {
                let mut f = launch(projectile.velocity, f64::from(elevation));
                let (mut xs, mut ys) = (vec![0.0], vec![0.0]);
                let mut frame = 1u32;
                while in_flight(&f, &projectile) {
                    step_frame(&mut f, &projectile);
                    if frame % FRAMES_PER_SAMPLE == 0 {
                        xs.push(f.x);
                        ys.push(f.y);
                    }
                    frame += 1;
                }
                Trajectory { xs, ys }
            })
            .collect();
        Self { projectile, trajectories }
    }

    pub fn projectile(&self) -> &Projectile {
        &self.projectile
    }

    /// Low and high arcs to a target `d` m away at height `h` (relative to the gun); `None` where
    /// the projectile cannot reach it on that arc.
    pub fn arcs(&self, d: f64, h: f64) -> (Option<Arc>, Option<Arc>) {
        if !(d > 0.0) {
            return (None, None);
        }
        let misses: Vec<f64> = self.trajectories.iter().map(|t| miss_on(t, d, h)).collect();
        let (mut low, mut high) = (None, None);
        for i in 1..misses.len() {
            let elevation = f64::from(MIN_ANGLE + i as i32);
            if low.is_none() && misses[i - 1] < 0.0 && misses[i] >= 0.0 {
                low = refine_arc(&self.projectile, elevation - 1.0, elevation, d, h);
            }
            if misses[i - 1] >= 0.0 && misses[i] < 0.0 {
                high = refine_arc(&self.projectile, elevation, elevation - 1.0, d, h).or(high);
            }
        }
        (low, high)
    }

    /// Whether some arc reaches a target `d` m away at height `h`: the traced trajectories cross
    /// it, as `arcs` finds them before refining (a fraction of its cost).
    pub fn reaches(&self, d: f64, h: f64) -> bool {
        if !(d > 0.0) {
            return false;
        }
        let misses: Vec<f64> = self.trajectories.iter().map(|t| miss_on(t, d, h)).collect();
        misses.windows(2).any(|w| (w[0] < 0.0 && w[1] >= 0.0) || (w[0] >= 0.0 && w[1] < 0.0))
    }

    /// The greatest range (m) at height `h` over the traced trajectories (one per degree), and
    /// the elevation that gives it: what the trajectories alone say, as `reaches` does.
    fn traced_max_range(&self, h: f64) -> (f64, i32) {
        let mut max_range = 0.0f64;
        let mut max_elevation = MIN_ANGLE;
        for (index, t) in self.trajectories.iter().enumerate() {
            if let Some(range) = t.landing(h)
                && range > max_range
            {
                max_range = range;
                max_elevation = MIN_ANGLE + index as i32;
            }
        }
        (max_range, max_elevation)
    }

    /// Shortest and longest range (m) on ground at height `h` for elevations `min_e..=max_e` (the
    /// gun's limits; whole degrees, the traced trajectories): range rises with elevation up to
    /// about 45° and falls after, so the gun reaches every range between the two by some
    /// elevation. `None` when no elevation in the limits lands.
    pub fn band(&self, h: f64, min_e: f64, max_e: f64) -> Option<(f64, f64)> {
        let first = (min_e.ceil() as i32).max(MIN_ANGLE);
        let last = (max_e.floor() as i32).min(MAX_ANGLE);
        let at = |e: i32| self.trajectories.get((e - MIN_ANGLE) as usize).and_then(|t| t.landing(h));
        let longest = (first..=last).filter_map(at).fold(None, |m: Option<f64>, r| Some(m.map_or(r, |m| m.max(r))))?;
        let shortest = [at(first), at(last)].into_iter().flatten().fold(longest, f64::min);
        Some((shortest, longest))
    }

    /// Maximum range (m) to a target at height `h`.
    pub fn max_range(&self, h: f64) -> f64 {
        let (mut max_range, max_elevation) = self.traced_max_range(h);
        let mut landed = f64::from(max_elevation);
        let mut despawned = f64::from(max_elevation + 1);
        if !landing_range(&self.projectile, despawned, h).is_nan() {
            return max_range;
        }
        for _ in 0..REFINE_ITERATIONS {
            let middle = (landed + despawned) / 2.0;
            let range = landing_range(&self.projectile, middle, h);
            if range.is_nan() {
                despawned = middle;
            } else {
                landed = middle;
                max_range = max_range.max(range);
            }
        }
        max_range
    }
}

/// Whether a projectile reaches a target, kept as its maximum range over a ladder of heights: a
/// few hundred bytes instead of the traced trajectories (for shells merely on offer, such as the
/// loading menu's other cards). Some arc reaches a target `d` m away at height `h` exactly when
/// `d` is within the maximum range at `h`; between rungs the range is interpolated.
#[derive(Clone, Debug, PartialEq)]
pub struct ReachTable {
    lowest: f64,
    step: f64,
    ranges: Vec<f64>,
}

impl ReachTable {
    /// Heights the table covers relative to the gun (m) and its step.
    pub const LOWEST_M: f64 = -400.0;
    pub const HIGHEST_M: f64 = 400.0;
    pub const STEP_M: f64 = 10.0;

    /// From the traced trajectories, as `Ballistics::reaches` judges (no refining: a table costs
    /// well under a millisecond).
    pub fn new(b: &Ballistics) -> Self {
        let rungs = ((Self::HIGHEST_M - Self::LOWEST_M) / Self::STEP_M).round() as usize + 1;
        let ranges = (0..rungs).map(|k| b.traced_max_range(Self::LOWEST_M + Self::STEP_M * k as f64).0).collect();
        Self { lowest: Self::LOWEST_M, step: Self::STEP_M, ranges }
    }

    /// Whether some arc reaches `d` m away at height `h` (outside the table's heights: no).
    pub fn reaches(&self, d: f64, h: f64) -> bool {
        let at = (h - self.lowest) / self.step;
        if !(d > 0.0) || at < 0.0 || at > (self.ranges.len() - 1) as f64 {
            return false;
        }
        let k = (at.floor() as usize).min(self.ranges.len() - 2);
        let f = at - k as f64;
        d <= self.ranges[k] + (self.ranges[k + 1] - self.ranges[k]) * f
    }
}

/// Horizontal distance (m) at which a shot at `elevation` comes down to height `h`; NaN when it
/// despawns first.
pub fn landing_range(p: &Projectile, elevation: f64, h: f64) -> f64 {
    let mut f = launch(p.velocity, elevation);
    while in_flight(&f, p) {
        step_frame(&mut f, p);
        if has_landed(&f, h) {
            return f.x;
        }
    }
    f64::NAN
}

/// Ground this close to the gun (m) is not hit: the heightmap's steps and the vehicle itself.
const MUZZLE_CLEAR_M: f64 = 30.0;

/// The ground is looked at this often (m) along each frame's flight: a flat full-charge shot covers
/// 11 m a frame, and a crest a heightmap cell or two wide between two frames' points is still in
/// its way (the game tests the whole segment; in one recorded case, with objective A 478 m out,
/// a crest at 359 m lay between frame points).
const GROUND_SAMPLE_M: f64 = 2.0;

/// Where a shot at `elevation` hits the ground, `ground(x)` being the ground's height (m, relative
/// to the gun) at horizontal distance `x` (m) along the shot: the distance and the time of flight,
/// where it first passes below the ground, on the way up too (a hill in front stops a low arc; the
/// upstream solutions know only the target's height, so an arc they give over a hill shows here
/// where the hill takes it). Each frame's flight is taken as straight and the ground looked at
/// every `GROUND_SAMPLE_M` along it. Where the ground in between stays below the arc, an arc to a
/// target comes down on it. `None` when it despawns first.
pub fn impact(p: &Projectile, elevation: f64, ground: impl Fn(f64) -> f64) -> Option<(f64, f64)> {
    let mut f = launch(p.velocity, elevation);
    // The last point looked at: distance, height above the ground, time.
    let mut last: (f64, f64, f64) = (f.x, 0.0, f.time);
    while in_flight(&f, p) {
        let (x0, y0, t0) = (f.x, f.y, f.time);
        step_frame(&mut f, p);
        let samples = ((f.x - x0).abs() / GROUND_SAMPLE_M).ceil().max(1.0);
        for j in 1..=samples as u32 {
            let u = f64::from(j) / samples;
            let (x, t) = (x0 + (f.x - x0) * u, t0 + (f.time - t0) * u);
            let above = y0 + (f.y - y0) * u - ground(x);
            if x >= MUZZLE_CLEAR_M && above < 0.0 {
                // Between the last point and this one; at this one where the last had no ground
                // under it (`ground` infinitely low there: a caller's unchecked stretch).
                let k = match last.1 {
                    h if h.is_infinite() => 1.0,
                    h if h > 0.0 => h / (h - above),
                    _ => 0.0,
                };
                return Some((last.0 + (x - last.0) * k, last.2 + (t - last.2) * k));
            }
            last = (x, above, t);
        }
    }
    None
}

/// The path of a shot at `elevation` (horizontal m, height m, one point a frame) up to `max_x` m out
/// or until it comes down `below` m under the launch height, for drawing a terrain profile.
pub fn trajectory(p: &Projectile, elevation: f64, max_x: f64, below: f64) -> Vec<(f64, f64)> {
    let mut f = launch(p.velocity, elevation);
    let mut points = vec![(0.0, 0.0)];
    while in_flight(&f, p) && f.x < max_x && !(f.vy < 0.0 && f.y < -below) {
        step_frame(&mut f, p);
        points.push((f.x, f.y));
    }
    points
}

/// Where the shell leaves the gun, relative to the player's arrow on the map: `height_m` above the
/// ground under the arrow and `ahead_m` in front of it along the line of fire. The functions above
/// launch from the ground at the arrow, as the upstream calculator does (`LaunchPoint::default()`
/// is that); measured shots land as if launched from here (on the debug map and Fulvia Gap: per
/// gun 1.3–2.75 m up and 13.5–23 m ahead, residuals 1.1–3.8 m RMS). Distances in
/// and out are from the arrow, heights relative to the ground under it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LaunchPoint {
    pub height_m: f64,
    pub ahead_m: f64,
}

impl LaunchPoint {
    /// A target `d` m out at height `h` as the launch point sees it, for `Ballistics::arcs`,
    /// `reaches` and `ReachTable::reaches`.
    pub fn target(&self, d: f64, h: f64) -> (f64, f64) {
        (d - self.ahead_m, h - self.height_m)
    }

    /// `impact` from the launch point: `ground(x)` is the ground's height at `x` m from the arrow.
    pub fn impact(&self, p: &Projectile, elevation: f64, ground: impl Fn(f64) -> f64) -> Option<(f64, f64)> {
        impact(p, elevation, |x| ground(x + self.ahead_m) - self.height_m).map(|(x, t)| (x + self.ahead_m, t))
    }

    /// `landing_range` from the launch point down to ground at height `h`.
    pub fn landing_range(&self, p: &Projectile, elevation: f64, h: f64) -> f64 {
        landing_range(p, elevation, h - self.height_m) + self.ahead_m
    }

    /// `trajectory` from the launch point, up to `max_x` m from the arrow or `below` m under the
    /// arrow's ground.
    pub fn trajectory(&self, p: &Projectile, elevation: f64, max_x: f64, below: f64) -> Vec<(f64, f64)> {
        trajectory(p, elevation, max_x - self.ahead_m, below + self.height_m).into_iter().map(|(x, y)| (x + self.ahead_m, y + self.height_m)).collect()
    }

    /// `Ballistics::band` on ground at height `h`.
    pub fn band(&self, b: &Ballistics, h: f64, min_e: f64, max_e: f64) -> Option<(f64, f64)> {
        b.band(h - self.height_m, min_e, max_e).map(|(near, far)| (near + self.ahead_m, far + self.ahead_m))
    }
}

/// Azimuth (0–360°) from position 1 to position 2 (map coordinates, y down; 0° is up).
pub fn azimuth(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let radians = (y2 - y1).atan2(x2 - x1);
    ((90.0 + (radians * 180.0) / std::f64::consts::PI + 360.0) % 360.0).abs()
}

pub fn distance(x1: f64, y1: f64, x2: f64, y2: f64, z1: f64, z2: f64) -> f64 {
    let (dx, dy, dz) = (x2 - x1, y2 - y1, z2 - z1);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Map size in studs from its grid size in metres.
pub fn map_size(grid_m: f64, grids: f64) -> f64 {
    meters_to_studs(grid_m) * grids
}

/// Blast diameter (studs) of `mass` kg of explosive: cap multiplier `c`, blast multiplier `b`,
/// explosive constant `a` (the game uses 1, 1, 0.07 with air density 1.2).
pub fn blast_diameter(mass: f64, c: f64, b: f64, a: f64) -> f64 {
    let range = (1.0 / a) * ((mass / AIR_DENSITY).powf(1.0 / 3.0) / 0.1);
    range.min(500.0 * c) * b
}

/// Estimated blast diameter (studs) of a cluster shell's submunitions fired at `elevation` onto
/// height `h`; NaN when they do not deploy before landing.
pub fn submunition_blast_diameter(p: &Projectile, elevation: f64, h: f64) -> f64 {
    let dispersion = p.submunition_angular_dispersion.unwrap_or(0.0);
    let deploy = p.submunition_deploy_factor.unwrap_or(0.5);
    let mut f = launch(p.velocity, elevation);
    let deployment_time = ((f.vy * 2.0) / G) * deploy;
    while f.time < deployment_time {
        if !in_flight(&f, p) || has_landed(&f, h) {
            return f64::NAN;
        }
        step_frame(&mut f, p);
    }
    let half_angle = (dispersion * std::f64::consts::PI) / 360.0;
    f.vz = hypot(&[f.vx, f.vy]) * half_angle.sin();
    f.vx *= half_angle.cos();
    f.vy *= half_angle.cos();
    while !has_landed(&f, h) {
        if !in_flight(&f, p) {
            return f64::NAN;
        }
        step_frame(&mut f, p);
    }
    2.0 * meters_to_studs(f.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reach_tables_agree_with_the_trajectories() {
        // M107 at Low and High charge: away from the range limit the table and the traced
        // trajectories agree; at the limit they differ by under a metre of range.
        for velocity in [207.0, 684.0] {
            let b = Ballistics::new(Projectile {
                velocity,
                gravity: Some(9.81),
                drag_constant: Some(7.5e-5),
                lifetime: Some(60.0),
                ..Projectile::default()
            });
            let table = ReachTable::new(&b);
            for h in [-250.0, -37.0, 0.0, 12.5, 60.0, 143.0] {
                let limit = b.traced_max_range(h).0;
                for d in (1..=400).map(|k| f64::from(k) * 50.0) {
                    if (d - limit).abs() > 1.0 {
                        assert_eq!(table.reaches(d, h), b.reaches(d, h), "v {velocity} d {d} h {h} limit {limit}");
                    }
                }
            }
        }
    }

    #[test]
    fn band_spans_what_the_limits_reach() {
        // M107 High: a howitzer's limits (-3..75 deg) reach from next to the gun to the maximum
        // range (shots above ~40 deg outlive the 60 s lifetime and do not land).
        let b = Ballistics::new(Projectile { velocity: 684.0, gravity: Some(9.81), drag_constant: Some(7.5e-5), lifetime: Some(60.0), ..Projectile::default() });
        let (short, long) = b.band(0.0, -3.0, 75.0).expect("lands");
        assert!(short < 50.0, "{short}");
        assert!((long - b.traced_max_range(0.0).0).abs() < 1e-9, "{long}");
        // M107 Low within a mortar's limits (40..85 deg): from the range of the highest elevation.
        let b = Ballistics::new(Projectile { velocity: 207.0, gravity: Some(9.81), drag_constant: Some(7.5e-5), lifetime: Some(60.0), ..Projectile::default() });
        let (short, long) = b.band(0.0, 40.0, 85.0).expect("lands");
        assert!(short > 300.0 && short < long, "{short} {long}");
        for d in [short + 10.0, (short + long) / 2.0, long - 10.0] {
            assert!(b.reaches(d, 0.0), "{d}");
        }
    }

    #[test]
    fn reaches_agrees_with_arcs() {
        // M107 at Low and High charge (wiki data), targets from 100 m to past the maximum range,
        // level and on a 60 m rise.
        for velocity in [207.0, 684.0] {
            let b = Ballistics::new(Projectile {
                velocity,
                gravity: Some(9.81),
                drag_constant: Some(7.5e-5),
                lifetime: Some(60.0),
                ..Projectile::default()
            });
            for h in [0.0, 60.0] {
                for d in (1..=200).map(|k| f64::from(k) * 100.0) {
                    let (low, high) = b.arcs(d, h);
                    assert_eq!(b.reaches(d, h), low.is_some() || high.is_some(), "v {velocity} d {d} h {h}");
                }
            }
        }
    }

    #[test]
    fn a_shot_at_an_arc_comes_down_on_its_target() {
        // M107 Low and High: fired at an arc to a target with the ground in between below the arc
        // (level; a valley from the muzzle on; a rise halfway under a high arc), it comes down
        // within a metre of the target, at the arc's time.
        for velocity in [207.0, 684.0] {
            let p = Projectile { velocity, gravity: Some(9.81), drag_constant: Some(7.5e-5), lifetime: Some(60.0), ..Projectile::default() };
            let b = Ballistics::new(p.clone());
            for (d, h, high_only) in [(800.0, 0.0, false), (2400.0, 0.0, false), (3000.0, -60.0, false), (1500.0, 45.0, true)] {
                let (low, high) = b.arcs(d, h);
                for arc in [if high_only { None } else { low }, high].into_iter().flatten() {
                    let step = if h > 0.0 { d / 2.0 } else { MUZZLE_CLEAR_M };
                    let ground = |x: f64| if x > step { h } else { 0.0 };
                    let (x, t) = impact(&p, arc.elevation, ground).expect("lands");
                    assert!((x - d).abs() < 1.0 + d * 1e-3, "v {velocity} d {d} h {h} e {}: {x}", arc.elevation);
                    assert!((t - arc.time_of_flight).abs() < 0.05, "v {velocity} d {d} h {h}: {t} vs {}", arc.time_of_flight);
                }
            }
        }
        let p = Projectile { velocity: 684.0, gravity: Some(9.81), drag_constant: Some(7.5e-5), lifetime: Some(60.0), ..Projectile::default() };
        // A low arc to 2400 m on level ground with a 100 m rise from 1000 m on: it hits the rise,
        // within a ground sample of its face.
        let (low, _) = Ballistics::new(p.clone()).arcs(2400.0, 0.0);
        let (x, _) = impact(&p, low.expect("reaches").elevation, |x| if x >= 1000.0 { 100.0 } else { 0.0 }).expect("hits");
        assert!((x - 1000.0).abs() <= GROUND_SAMPLE_M, "{x}");
        // A crest 3 m wide between two frames' points (a flat full-charge shot covers 11 m a
        // frame) still stops it.
        let (low, _) = Ballistics::new(p.clone()).arcs(800.0, 0.0);
        let e = low.expect("reaches").elevation;
        let crest = |x: f64| if (500.0..503.0).contains(&x) { 40.0 } else { 0.0 };
        let (x, _) = impact(&p, e, crest).expect("hits the crest");
        assert!((497.0..503.0).contains(&x), "{x}");
        // Ground left unchecked (infinitely low) up to 100 m, a crest rising from there: met at
        // the crest, not lost to an infinite interpolation.
        let near = |x: f64| if x < 100.0 { f64::NEG_INFINITY } else if x < 110.0 { 30.0 } else { 0.0 };
        let (x, _) = impact(&p, e, near).expect("hits the crest past the unchecked stretch");
        assert!((100.0..=100.0 + GROUND_SAMPLE_M).contains(&x), "{x}");
        // Despawned in the air (a 60 s lifetime at 80 degrees and full charge): nowhere.
        assert!(impact(&p, 80.0, |_| 0.0).is_none());
    }

    /// The launch point at the arrow is the upstream model; a raised one lands farther on a flat
    /// shot, and the arc found for a target through `target` lands on it.
    #[test]
    fn a_launch_point_moves_the_shot_and_the_arcs_agree() {
        let p = Projectile { velocity: 350.0, gravity: Some(9.81), drag_constant: Some(9.652183153767759e-05), lifetime: Some(50.0), ..Projectile::default() };
        let at_arrow = LaunchPoint::default();
        assert_eq!(at_arrow.landing_range(&p, 2.5, 0.0), landing_range(&p, 2.5, 0.0));
        let d30 = LaunchPoint { height_m: 1.3, ahead_m: 22.8 };
        // On the debug map, Medium at 2.50 degrees landed 1061.4 m out (the model from the ground: 1014.8).
        let r = d30.landing_range(&p, 2.5, 0.0);
        assert!((r - 1061.4).abs() < 8.0, "{r}");
        // The arc solved for the target as the launch point sees it passes it within the solver's
        // tolerance (a height miss of `REFINE_TOLERANCE`).
        let (d, h) = d30.target(1500.0, 5.0);
        let (low, _) = Ballistics::new(p.clone()).arcs(d, h);
        let (miss, _) = shoot(&p, low.unwrap().elevation, d, h);
        assert!(miss.abs() <= REFINE_TOLERANCE, "{miss}");
        // Over flat ground `impact` agrees with `landing_range` (which stops at the frame after the
        // landing, up to a frame's flight on); the path starts at the launch point.
        let (x, _) = d30.impact(&p, 2.5, |_| 0.0).unwrap();
        assert!(x <= r && r - x < p.velocity / 60.0, "{x} {r}");
        let path = d30.trajectory(&p, 2.5, 500.0, 10.0);
        assert_eq!(path[0], (22.8, 1.3));
    }
}
