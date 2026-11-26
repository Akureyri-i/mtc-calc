//! Small float-image helpers shared by the map recognizers.

/// Row-major single-channel float image.
#[derive(Clone, Debug)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Plane {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height],
        }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// Value with coordinates clamped to the image.
    #[inline]
    pub fn clamped(&self, x: isize, y: isize) -> f32 {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        self.data[y * self.width + x]
    }

    /// Sobel gradient magnitude, borders replicated.
    pub fn gradient(&self) -> Plane {
        let mut out = Plane::new(self.width, self.height);
        for y in 0..self.height as isize {
            for x in 0..self.width as isize {
                let p = |dx: isize, dy: isize| self.clamped(x + dx, y + dy);
                let gx = p(1, -1) + 2.0 * p(1, 0) + p(1, 1) - p(-1, -1) - 2.0 * p(-1, 0) - p(-1, 1);
                let gy = p(-1, 1) + 2.0 * p(0, 1) + p(1, 1) - p(-1, -1) - 2.0 * p(0, -1) - p(1, -1);
                out.data[y as usize * self.width + x as usize] = (gx * gx + gy * gy).sqrt();
            }
        }
        out
    }

    /// Separable Gaussian blur, borders replicated.
    pub fn gaussian(&self, sigma: f32) -> Plane {
        let radius = (3.0 * sigma).ceil() as isize;
        let kernel: Vec<f32> = (-radius..=radius).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let total: f32 = kernel.iter().sum();
        let kernel: Vec<f32> = kernel.iter().map(|k| k / total).collect();
        let mut tmp = Plane::new(self.width, self.height);
        for y in 0..self.height as isize {
            for x in 0..self.width as isize {
                let v: f32 = kernel.iter().enumerate().map(|(k, w)| w * self.clamped(x + k as isize - radius, y)).sum();
                tmp.data[y as usize * self.width + x as usize] = v;
            }
        }
        let mut out = Plane::new(self.width, self.height);
        for y in 0..self.height as isize {
            for x in 0..self.width as isize {
                let v: f32 = kernel.iter().enumerate().map(|(k, w)| w * tmp.clamped(x, y + k as isize - radius)).sum();
                out.data[y as usize * self.width + x as usize] = v;
            }
        }
        out
    }

    /// Detail above the Gaussian scale `sigma`: the plane minus its blur.
    pub fn high_pass(&self, sigma: f32) -> Plane {
        let blur = self.gaussian(sigma);
        Plane {
            width: self.width,
            height: self.height,
            data: self.data.iter().zip(&blur.data).map(|(a, b)| a - b).collect(),
        }
    }

    /// Area-average down to `width`×`height` (the source spans the whole output).
    pub fn resample(&self, width: usize, height: usize) -> Plane {
        area_resample(
            |x, y| self.at(x as usize, y as usize),
            (0.0, 0.0),
            (self.width as f64, self.height as f64),
            (width, height),
        )
    }
}

/// Source pixels overlapping [start, start + len) with their overlap lengths.
fn spans(start: f64, len: f64) -> Vec<(i32, f32)> {
    let end = start + len;
    let mut out = Vec::new();
    let mut i = start.floor() as i32;
    while f64::from(i) < end {
        let lo = start.max(f64::from(i));
        let hi = end.min(f64::from(i + 1));
        // Floating-point slivers past the window edge are not pixels.
        if hi - lo > 1e-6 {
            out.push((i, (hi - lo) as f32));
        }
        i += 1;
    }
    out
}

/// Area-averages the source rectangle `origin`+`size` (continuous coordinates, any fractional
/// position) into an `out.0`×`out.1` plane. `get(x, y)` reads integer source pixels.
pub fn area_resample(get: impl Fn(i32, i32) -> f32, origin: (f64, f64), size: (f64, f64), out: (usize, usize)) -> Plane {
    let (kx, ky) = (size.0 / out.0 as f64, size.1 / out.1 as f64);
    let cols: Vec<_> = (0..out.0).map(|i| spans(origin.0 + i as f64 * kx, kx)).collect();
    let rows: Vec<_> = (0..out.1).map(|j| spans(origin.1 + j as f64 * ky, ky)).collect();
    let y0 = rows.iter().flatten().map(|&(y, _)| y).min().unwrap_or(0);
    let y1 = rows.iter().flatten().map(|&(y, _)| y + 1).max().unwrap_or(0);
    // Horizontal pass over every source row, then vertical.
    let mut tmp = vec![0.0f32; (y1 - y0) as usize * out.0];
    for y in y0..y1 {
        let row = &mut tmp[(y - y0) as usize * out.0..][..out.0];
        for (i, span) in cols.iter().enumerate() {
            row[i] = span.iter().map(|&(x, w)| w * get(x, y)).sum();
        }
    }
    let norm = (kx * ky) as f32;
    let mut plane = Plane::new(out.0, out.1);
    for (j, span) in rows.iter().enumerate() {
        for i in 0..out.0 {
            let v: f32 = span.iter().map(|&(y, w)| w * tmp[(y - y0) as usize * out.0 + i]).sum();
            plane.data[j * out.0 + i] = v / norm;
        }
    }
    plane
}

/// Normalised cross-correlation of two equally sized planes over the pixels where `mask` is set
/// (all pixels when `mask` is `None`); either plane may hold f32 or bytes.
pub fn masked_ncc<A: Copy + Into<f64>, B: Copy + Into<f64>>(a: &[A], b: &[B], mask: Option<&[bool]>) -> f32 {
    let used = |i: usize| mask.is_none_or(|m| m[i]);
    let (fa, fb) = (|i: usize| -> f64 { a[i].into() }, |i: usize| -> f64 { b[i].into() });
    let (mut n, mut sa, mut sb) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..a.len() {
        if used(i) {
            n += 1.0;
            sa += fa(i);
            sb += fb(i);
        }
    }
    if n < 2.0 {
        return 0.0;
    }
    let (ma, mb) = (sa / n, sb / n);
    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..a.len() {
        if used(i) {
            let (da, db) = (fa(i) - ma, fb(i) - mb);
            ab += da * db;
            aa += da * da;
            bb += db * db;
        }
    }
    if aa <= 0.0 || bb <= 0.0 {
        return 0.0;
    }
    (ab / (aa * bb).sqrt()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_resample_handles_fractional_windows() {
        // 4×1 ramp sampled over [0.5, 3.5) into 3 pixels: each output is one source pixel wide,
        // straddling two source pixels equally.
        let src = [0.0f32, 10.0, 20.0, 30.0];
        let p = area_resample(|x, _| src[x as usize], (0.5, 0.0), (3.0, 1.0), (3, 1));
        assert_eq!(p.data, vec![5.0, 15.0, 25.0]);
    }
}

/// One axis resampled to `n` samples: area average when shrinking, bilinear (half-pixel centres,
/// clamped) when growing, as OpenCV's INTER_AREA and INTER_LINEAR.
fn resample_axis(src: &[f32], n: usize) -> Vec<f32> {
    let m = src.len();
    if n == m {
        return src.to_vec();
    }
    if n < m {
        let k = m as f64 / n as f64;
        (0..n)
            .map(|i| {
                let (start, end) = (i as f64 * k, (i + 1) as f64 * k);
                let mut s = 0.0f64;
                let mut j = start.floor() as usize;
                while (j as f64) < end && j < m {
                    let overlap = end.min(j as f64 + 1.0) - start.max(j as f64);
                    if overlap > 1e-9 {
                        s += overlap * f64::from(src[j]);
                    }
                    j += 1;
                }
                (s / k) as f32
            })
            .collect()
    } else {
        let k = m as f32 / n as f32;
        (0..n)
            .map(|i| {
                let x = ((i as f32 + 0.5) * k - 0.5).max(0.0);
                let x0 = (x.floor() as usize).min(m - 1);
                let x1 = (x0 + 1).min(m - 1);
                let a = x - x0 as f32;
                src[x0] * (1.0 - a) + src[x1] * a
            })
            .collect()
    }
}

/// `p` resized to `w` x `h`, each axis as [`resample_axis`] (OpenCV's INTER_AREA / INTER_LINEAR:
/// what the development scripts that measure text scales use).
pub fn resize(p: &Plane, w: usize, h: usize) -> Plane {
    let mut rows = Plane::new(w, p.height);
    for y in 0..p.height {
        let out = resample_axis(&p.data[y * p.width..(y + 1) * p.width], w);
        rows.data[y * w..(y + 1) * w].copy_from_slice(&out);
    }
    if h == p.height {
        return rows;
    }
    let mut out = Plane::new(w, h);
    for x in 0..w {
        let col: Vec<f32> = (0..p.height).map(|y| rows.data[y * w + x]).collect();
        for (y, v) in resample_axis(&col, h).into_iter().enumerate() {
            out.data[y * w + x] = v;
        }
    }
    out
}
