//! Angle readout accuracy on every labelled frame.
//! A wrong value fails the test; an unreadable frame only counts against availability.

use mtc_dataset::eval::{Tally, labelled_frames};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};

#[test]
fn aim_readout_matches_labels() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let frames = labelled_frames().expect("dataset");
    let mut reader = AimReader::new();
    let mut correct = Tally::new("aim readout: no wrong value");
    let mut available = Tally::new("aim readout: read (not skipped)");
    for frame in frames.iter().filter(|f| f.label.aim.is_some()) {
        let truth = frame.label.aim.as_ref().expect("filtered");
        let image = frame.image().expect("frame image");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let origin = frame.meta.geometry.client_in_frame();
        let result = available.time(|| reader.read(&view, (origin.x, origin.y)));
        match &result {
            AimResult::Read(r) => {
                let ok = r.elevation_text == truth.elevation && r.azimuth_text == truth.azimuth;
                correct.check(&frame.id(), ok, || format!("read {} {} vs {} {}", r.elevation_text, r.azimuth_text, truth.elevation, truth.azimuth));
                available.check(&frame.id(), true, String::new);
            }
            other => {
                correct.check(&frame.id(), true, String::new);
                available.check(&frame.id(), false, || format!("{other:?}"));
            }
        }
    }
    available.require(AVAILABLE_MIN);
    correct.require(1.0);
}

/// Measured on the labels: 175/180. Not read: two snow frames whose label is found only on the
/// shadow-corrected plane or not at all, one whose shadow-corrected digits lose the decimal point,
/// and two others. Raise as the reader improves.
const AVAILABLE_MIN: f64 = 0.97;
