//! Pictures of recent recognition failures, for problem reports: the regions the program had just
//! read when a recognizer failed (the angle readout lost right after it was read, or read but not
//! made out; no view after M or X; a loading whose charge or shell was not read), kept in memory.
//! A report with screenshots ticked carries them with the moment each was taken (seconds since
//! start, as the log counts), so a failure that comes and goes is in the report even when the
//! picture at the moment of the report reads fine.

use std::collections::VecDeque;

use mtc_capture::readback::Region;
use mtc_dataset::image::RgbImage;

/// At most one picture of a kind this often (s).
const EVERY_S: f64 = 1.0;

/// The kinds and how many pictures of each are kept: a dozen of the readout's narrow strip, fewer
/// of the loading area (a few MB a picture on a large screen).
const KINDS: [(&str, usize); 4] = [("readout-lost", 12), ("readout-unread", 8), ("view-none", 8), ("loading-unread", 2)];

#[derive(Default)]
pub struct FailShots {
    /// Per kind (as `KINDS`): when the last picture was taken and the pictures kept, oldest first.
    kept: [(Option<f64>, VecDeque<(f64, Vec<RgbImage>)>); 4],
}

impl FailShots {
    fn slot(kind: &str) -> Option<usize> {
        KINDS.iter().position(|(k, _)| *k == kind)
    }

    /// Whether a picture of `kind` is taken at `now_s` (not one taken in the last `EVERY_S`).
    pub fn wants(&self, kind: &str, now_s: f64) -> bool {
        Self::slot(kind).is_some_and(|i| self.kept[i].0.is_none_or(|t| now_s - t >= EVERY_S))
    }

    /// Keeps the regions read at `now_s` as one picture of `kind` (several regions: the parts of
    /// it), dropping the oldest beyond the kind's count.
    pub fn add(&mut self, kind: &str, now_s: f64, regions: &[&Region]) {
        let Some(i) = Self::slot(kind) else { return };
        let images: Vec<RgbImage> = regions.iter().filter(|r| r.width > 0 && r.height > 0).map(|r| image_of(r, None)).collect();
        if images.is_empty() {
            return;
        }
        let (last, shots) = &mut self.kept[i];
        *last = Some(now_s);
        shots.push_back((now_s, images));
        while shots.len() > KINDS[i].1 {
            shots.pop_front();
        }
    }

    /// Every picture kept: its kind, when, its parts.
    pub fn all(&self) -> impl Iterator<Item = (&'static str, f64, &[RgbImage])> {
        KINDS.iter().zip(&self.kept).flat_map(|((kind, _), (_, shots))| shots.iter().map(move |(t, parts)| (*kind, *t, parts.as_slice())))
    }
}

/// A read-back region (B, G, R, A) as an RGB image, its columns from `mask_from` on blacked out.
pub fn image_of(region: &Region, mask_from: Option<u32>) -> RgbImage {
    let mut image = RgbImage::new(region.width, region.height);
    for py in 0..region.height as usize {
        for px in 0..region.width as usize {
            if mask_from.is_some_and(|m| px as u32 >= m) {
                continue;
            }
            let i = (py * region.width as usize + px) * 4;
            let o = (py * region.width as usize + px) * 3;
            image.data[o] = region.data[i + 2];
            image.data[o + 1] = region.data[i + 1];
            image.data[o + 2] = region.data[i];
        }
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(w: u32, h: u32, bgra: [u8; 4]) -> Region {
        Region { x: 0, y: 0, width: w, height: h, data: bgra.repeat((w * h) as usize) }
    }

    /// One picture a second per kind, the newest few kept, kinds apart; the colours come out RGB.
    #[test]
    fn failures_are_kept_a_few_a_kind() {
        let mut f = FailShots::default();
        let r = region(2, 1, [10, 20, 30, 255]);
        for k in 0..20 {
            let t = f64::from(k) * 0.5;
            if f.wants("readout-lost", t) {
                f.add("readout-lost", t, &[&r]);
            }
        }
        let lost: Vec<f64> = f.all().filter(|(k, ..)| *k == "readout-lost").map(|(_, t, _)| t).collect();
        // Taken at 0, 1, 2 … 9 s (every other try), all 10 kept (12 at most).
        assert_eq!(lost, (0..10).map(f64::from).collect::<Vec<_>>());
        for k in 0..6 {
            f.add("loading-unread", f64::from(k), &[&r]);
        }
        assert_eq!(f.all().filter(|(k, ..)| *k == "loading-unread").count(), 2);
        assert!(!f.wants("no such kind", 0.0));
        let (_, _, parts) = f.all().next().expect("a picture");
        assert_eq!(&parts[0].data[..3], &[30, 20, 10]);
    }
}
