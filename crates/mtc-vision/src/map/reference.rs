//! Official reference map images, resampled to a fixed working resolution.
//!
//! The map service draws the calculator's own grid (9×9 or 8×8 bright 1–2 px lines) into many
//! references; the game's map has no such lines, so they are found on the luma and painted over in
//! every channel at load time.

use crate::map::raster::{Plane, area_resample};

/// Side of the working canvas every reference is resampled to.
pub const WORK_SIZE: usize = 1024;
/// A grid line is a column/row whose median (over opaque pixels) is this much brighter than the
/// pixels two to either side.
const GRID_LINE_MIN_CONTRAST: f32 = 4.0;
const OPAQUE: f32 = 200.0 / 255.0;

pub struct MapReference {
    /// Upstream map id ("fulvia_gap").
    pub id: String,
    /// Canvas side in metres.
    pub size_m: f64,
    /// Luma with the calculator grid painted out, WORK_SIZE².
    pub gray: Plane,
    /// Chroma R−G and B−(R+G)/2, grid painted out, WORK_SIZE².
    pub rg: Plane,
    pub by: Plane,
    /// Opacity 0..1, WORK_SIZE². Transparent parts show the 3D scene in game.
    pub alpha: Plane,
    /// Calculator grid lines painted out (vertical, horizontal).
    pub grid_lines: (usize, usize),
}

impl MapReference {
    /// From the RGBA `user.png` of the map service, any resolution.
    pub fn from_rgba(id: &str, size_m: f64, width: u32, height: u32, rgba: &[u8]) -> Self {
        let w = width as usize;
        let luma = |x: i32, y: i32| {
            let i = (y as usize * w + x as usize) * 4;
            0.299 * f32::from(rgba[i]) + 0.587 * f32::from(rgba[i + 1]) + 0.114 * f32::from(rgba[i + 2])
        };
        let channel = |c: usize| move |x: i32, y: i32| f32::from(rgba[(y as usize * w + x as usize) * 4 + c]);
        let (r, g, b) = (channel(0), channel(1), channel(2));
        let rg = move |x: i32, y: i32| r(x, y) - g(x, y);
        let by = move |x: i32, y: i32| b(x, y) - (r(x, y) + g(x, y)) / 2.0;
        let alpha = |x: i32, y: i32| f32::from(rgba[(y as usize * w + x as usize) * 4 + 3]) / 255.0;
        let size = (f64::from(width), f64::from(height));
        let out = (WORK_SIZE, WORK_SIZE);
        let mut gray = area_resample(luma, (0.0, 0.0), size, out);
        let mut rg = area_resample(rg, (0.0, 0.0), size, out);
        let mut by = area_resample(by, (0.0, 0.0), size, out);
        let alpha = area_resample(alpha, (0.0, 0.0), size, out);
        let vertical = grid_lines(&gray, &alpha, true);
        for plane in [&mut gray, &mut rg, &mut by] {
            paint_out(plane, &vertical, true);
        }
        let horizontal = grid_lines(&gray, &alpha, false);
        for plane in [&mut gray, &mut rg, &mut by] {
            paint_out(plane, &horizontal, false);
        }
        Self {
            id: id.to_owned(),
            size_m,
            gray,
            rg,
            by,
            alpha,
            grid_lines: (vertical.len(), horizontal.len()),
        }
    }
}

/// Positions along one axis whose line response is a local maximum above the contrast threshold.
fn grid_lines(gray: &Plane, alpha: &Plane, vertical: bool) -> Vec<usize> {
    let n = WORK_SIZE;
    let mut profile = vec![0.0f32; n];
    let mut values = Vec::with_capacity(n);
    for (i, p) in profile.iter_mut().enumerate().take(n - 2).skip(2) {
        values.clear();
        for j in 0..n {
            let (x, y) = if vertical { (i, j) } else { (j, i) };
            if alpha.at(x, y) > OPAQUE {
                let (a, b) = if vertical { (gray.at(x - 2, y), gray.at(x + 2, y)) } else { (gray.at(x, y - 2), gray.at(x, y + 2)) };
                values.push(gray.at(x, y) - (a + b) / 2.0);
            }
        }
        if !values.is_empty() {
            let mid = values.len() / 2;
            *p = *values.select_nth_unstable_by(mid, f32::total_cmp).1;
        }
    }
    (3..n - 3)
        .filter(|&i| profile[i] > GRID_LINE_MIN_CONTRAST && profile[i - 2..=i + 2].iter().all(|&v| v <= profile[i]))
        .collect()
}

/// Replace columns/rows i-1..=i+1 of every line by interpolating between i-2 and i+2.
fn paint_out(plane: &mut Plane, lines: &[usize], vertical: bool) {
    for &i in lines {
        for j in 0..WORK_SIZE {
            let at = |k: usize| if vertical { (k, j) } else { (j, k) };
            let (a, b) = (plane.at(at(i - 2).0, at(i - 2).1), plane.at(at(i + 2).0, at(i + 2).1));
            for (step, k) in (i - 1..=i + 1).enumerate() {
                let (x, y) = at(k);
                plane.data[y * WORK_SIZE + x] = a + (b - a) * (step + 1) as f32 / 4.0;
            }
        }
    }
}
