//! Accuracy bookkeeping for recognizer tests over labelled frames.
//!
//! ```ignore
//! let mut tally = Tally::new("aim readout");
//! for frame in labelled_frames()? {
//!     let Some(truth) = &frame.label.aim else { continue };
//!     let read = read_aim(&frame.image()?);
//!     tally.check(&frame.id(), read.as_ref() == Some(truth), || format!("{read:?} vs {truth:?}"));
//! }
//! tally.require(1.0);
//! ```

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::image::RgbImage;
use crate::{FrameLabel, FrameMeta, labels, sessions};

/// A labelled frame with its recording metadata.
pub struct LabelledFrame {
    pub session: String,
    pub path: PathBuf,
    pub meta: FrameMeta,
    pub label: FrameLabel,
}

impl LabelledFrame {
    pub fn id(&self) -> String {
        format!("{}/{}", self.session, self.meta.file)
    }

    pub fn image(&self) -> io::Result<RgbImage> {
        RgbImage::load_png(&self.path)
    }
}

/// Every labelled frame across all local sessions.
pub fn labelled_frames() -> io::Result<Vec<LabelledFrame>> {
    let mut out = Vec::new();
    for session in sessions()? {
        for label in labels(&session.info.id)? {
            let Some(meta) = session.frames.iter().find(|m| m.file == label.file) else {
                return Err(io::Error::other(format!(
                    "label {} has no frame in session {}",
                    label.file, session.info.id
                )));
            };
            out.push(LabelledFrame {
                session: session.info.id.clone(),
                path: session.frame_path(meta),
                meta: meta.clone(),
                label,
            });
        }
    }
    Ok(out)
}

/// Pass/fail counts plus the failing cases, printed as a report.
pub struct Tally {
    name: String,
    total: usize,
    passed: usize,
    failures: Vec<String>,
    elapsed: Duration,
}

impl Tally {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            total: 0,
            passed: 0,
            failures: Vec::new(),
            elapsed: Duration::ZERO,
        }
    }

    pub fn check(&mut self, case: &str, ok: bool, detail: impl FnOnce() -> String) {
        self.total += 1;
        if ok {
            self.passed += 1;
        } else {
            self.failures.push(format!("{case}: {}", detail()));
        }
    }

    /// Runs `work`, adding its duration to the timing summary.
    pub fn time<T>(&mut self, work: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let value = work();
        self.elapsed += started.elapsed();
        value
    }

    pub fn rate(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.passed as f64 / self.total as f64
        }
    }

    pub fn report(&self) -> String {
        let mut text = format!(
            "{}: {}/{} ({:.2}%)",
            self.name,
            self.passed,
            self.total,
            self.rate() * 100.0
        );
        if self.total > 0 && !self.elapsed.is_zero() {
            text += &format!(
                ", {:.3} ms/case",
                self.elapsed.as_secs_f64() * 1000.0 / self.total as f64
            );
        }
        for failure in self.failures.iter().take(20) {
            text += &format!("\n  FAIL {failure}");
        }
        if self.failures.len() > 20 {
            text += &format!("\n  … {} more", self.failures.len() - 20);
        }
        text
    }

    /// Prints the report and panics when the pass rate is below `minimum`.
    pub fn require(&self, minimum: f64) {
        println!("{}", self.report());
        assert!(
            self.rate() >= minimum,
            "{} below required {:.2}%",
            self.name,
            minimum * 100.0
        );
    }
}
